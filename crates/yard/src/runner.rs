//! Runs inside a task's systemd unit: `yard __build <spec>` executes the
//! steps, `yard __launch <spec>` the long-running command.

use crate::spec::RunSpec;
use anyhow::{Context, Result};
use regex::Regex;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{ExitStatus, Stdio};
use std::time::Instant;
use yard_core::manifest::RetryOn;
use yard_core::model::duration;
use yard_core::protocol::{request, Request};

fn load(path: &str) -> Result<RunSpec> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    Ok(serde_json::from_str(&text)?)
}

fn open_log(spec: &RunSpec) -> Result<File> {
    if let Some(d) = spec.log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    Ok(OpenOptions::new().create(true).append(true).open(&spec.log)?)
}

#[cfg(unix)]
fn code_of(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(c) => c,
        None => 128 + status.signal().unwrap_or(0),
    }
}

#[cfg(not(unix))]
fn code_of(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

/// True when this unit's cgroup has seen an out-of-memory kill.
#[cfg(target_os = "linux")]
fn oom_killed() -> bool {
    let Ok(cg) = std::fs::read_to_string("/proc/self/cgroup") else { return false };
    let Some(path) = cg.lines().find_map(|l| l.strip_prefix("0::")) else { return false };
    let events = format!("/sys/fs/cgroup{}/memory.events", path.trim());
    std::fs::read_to_string(events)
        .map(|t| t.lines().any(|l| l.strip_prefix("oom_kill ").and_then(|n| n.trim().parse::<u64>().ok()).is_some_and(|n| n > 0)))
        .unwrap_or(false)
}

/// Elsewhere the supervisor knows (it enforced the cap itself).
#[cfg(not(target_os = "linux"))]
fn oom_killed() -> bool {
    false
}

fn finish(spec: &RunSpec, phase: &str, code: i32, oom: bool) {
    let _ = std::fs::write(spec.exit_file(phase), format!("{code} {}\n", oom as u8));
    let _ = request(&Request::Finished { run: spec.run, phase: phase.into(), code, oom });
}

pub fn build(path: &str) -> Result<i32> {
    let spec = load(path)?;
    let mut log = open_log(&spec)?;
    let started = Instant::now();
    let total = spec.steps.len();
    for (i, step) in spec.steps.iter().enumerate() {
        let n = i + 1;
        let attempts = match &spec.retry {
            Some(r) if r.step.is_none() || r.step == Some(n) => r.attempts.max(1),
            _ => 1,
        };
        let mut code = 0;
        for attempt in 1..=attempts {
            let suffix = if attempt > 1 { format!(" (try {attempt} of {attempts})") } else { String::new() };
            writeln!(log, "===== step {n}/{total}: {}{suffix} =====", step.name)?;
            log.flush()?;
            let _ = request(&Request::Progress { run: spec.run, index: n, total, name: step.name.clone(), attempt });
            let out = log.try_clone()?;
            let err = log.try_clone()?;
            let status = spec.shell_command(&step.cmd).stdin(Stdio::null()).stdout(out).stderr(err).status();
            code = match status {
                Ok(s) => code_of(s),
                Err(e) => {
                    writeln!(log, "switchyard: could not start the shell: {e}")?;
                    127
                }
            };
            if code == 0 {
                break;
            }
            let retryable = match &spec.retry {
                Some(r) => r.on == RetryOn::Failure || code >= 128,
                None => false,
            };
            if attempt < attempts && retryable {
                writeln!(log, "===== step {n}/{total} crashed (exit {code}), trying again =====")?;
                continue;
            }
            break;
        }
        if code != 0 {
            let oom = oom_killed();
            let why = if oom { " · out of memory" } else { "" };
            writeln!(log, "===== exit {code} · step {n} of {total} · stopped{why} =====")?;
            log.flush()?;
            finish(&spec, "build", code, oom);
            return Ok(code);
        }
    }
    writeln!(log, "===== build ok · {} =====", duration(started.elapsed().as_secs_f64()))?;
    log.flush()?;
    finish(&spec, "build", 0, false);
    Ok(0)
}

struct WatchRule {
    re: Regex,
    title: String,
    urgency: String,
    throttle: u64,
    last: Option<Instant>,
}

pub fn launch(path: &str) -> Result<i32> {
    let spec = load(path)?;
    let mut log = open_log(&spec)?;
    let cmd = spec.run_cmd.clone().unwrap_or_default();
    writeln!(log, "===== run: {cmd} =====")?;
    log.flush()?;
    let mut rules: Vec<WatchRule> = spec
        .watch
        .iter()
        .filter_map(|w| {
            Some(WatchRule {
                re: Regex::new(&w.pattern).ok()?,
                title: w.title.clone(),
                urgency: w.urgency.clone(),
                throttle: w.throttle,
                last: None,
            })
        })
        .collect();
    let mut child = spec
        .shell_command(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting the shell")?;
    // Merge stdout and stderr line by line, the way a terminal shows them.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let pump = |r: Box<dyn std::io::Read + Send>, tx: std::sync::mpsc::Sender<Vec<u8>>| {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(r);
            loop {
                let mut buf = Vec::new();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(buf).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    };
    let out = pump(Box::new(child.stdout.take().expect("piped stdout")), tx.clone());
    let err = pump(Box::new(child.stderr.take().expect("piped stderr")), tx);
    for buf in rx {
        log.write_all(&buf)?;
        if !buf.ends_with(b"\n") {
            log.write_all(b"\n")?;
        }
        log.flush()?;
        if rules.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = yard_core::errors::strip_ansi(line.trim_end());
        for r in rules.iter_mut() {
            if !r.re.is_match(&line) {
                continue;
            }
            if r.last.is_some_and(|t| t.elapsed().as_secs() < r.throttle) {
                continue;
            }
            r.last = Some(Instant::now());
            crate::sys::notify(&format!("{} {}", spec.label, r.title), &line, &r.urgency);
        }
    }
    let _ = out.join();
    let _ = err.join();
    let code = match child.wait() {
        Ok(s) => code_of(s),
        Err(_) => 1,
    };
    writeln!(log, "===== run ended · exit {code} =====")?;
    log.flush()?;
    let oom = code != 0 && oom_killed();
    finish(&spec, "launch", code, oom);
    Ok(code)
}
