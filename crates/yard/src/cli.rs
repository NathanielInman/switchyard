//! The command-line side of `yard`.

use crate::sys;
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use yard_core::config::Config;
use yard_core::model::{ago, clock, duration, Phase, Snapshot, TaskView};
use yard_core::paths;
use yard_core::protocol::{request, AckPhase, BarLine, Reply, Request, SubKind};

pub enum BoardCmd {
    Toggle,
    Show,
    Hide,
}

/// Starts the daemon if nothing answers on the socket.
fn ensure_daemon() -> Result<()> {
    if yard_core::protocol::connect().is_ok() {
        return Ok(());
    }
    // An isolated setup (SWITCHYARD_RUNTIME_DIR) never starts the installed service.
    let isolated = std::env::var_os("SWITCHYARD_RUNTIME_DIR").is_some();
    if isolated || !crate::setup::start_service() {
        let me = std::env::current_exe()?;
        let mut c = Command::new(me);
        c.arg("daemon");
        let _ = std::fs::create_dir_all(paths::state_dir());
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(paths::state_dir().join("daemon.log")) {
            if let Ok(f2) = f.try_clone() {
                c.stdout(f).stderr(f2);
            }
        }
        sys::detach(&mut c);
        sys::spawn_detached_keep_output(c)?;
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        if yard_core::protocol::connect().is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    bail!("the daemon did not start; try `yard daemon` in a terminal to see why")
}

fn send(req: Request) -> Result<Reply> {
    ensure_daemon()?;
    let r = request(&req)?;
    if !r.ok {
        bail!("{}", r.error.unwrap_or_else(|| "request failed".into()));
    }
    Ok(r)
}

fn print_reply(r: &Reply) {
    if let Some(m) = &r.message {
        println!("{m}");
    }
}

pub fn simple(req: Request) -> Result<i32> {
    let r = send(req)?;
    print_reply(&r);
    Ok(0)
}

pub fn board(cmd: BoardCmd, rest: &[String]) -> Result<i32> {
    let center = rest.iter().any(|a| a == "--center" || a == "-c");
    let req = match cmd {
        BoardCmd::Hide => Request::Hide,
        _ => {
            let (cfg, _) = Config::load();
            let anchor = yard_core::hypr::anchor(&cfg.board.bar_namespace, center).or_else(|| {
                center.then(|| yard_core::model::Anchor { center: true, ..Default::default() })
            });
            match cmd {
                BoardCmd::Toggle => Request::Toggle { anchor },
                _ => Request::Show { anchor },
            }
        }
    };
    send(req)?;
    Ok(0)
}

fn snapshot() -> Result<Snapshot> {
    let r = send(Request::Snapshot)?;
    r.snapshot.map(|s| *s).ok_or_else(|| anyhow!("the daemon sent no snapshot"))
}

struct Paint {
    on: bool,
}

impl Paint {
    fn new() -> Paint {
        Paint { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }
    fn c(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn phase(&self, p: Phase) -> String {
        let code = match p {
            Phase::Failed | Phase::Seen => "31",
            Phase::Building | Phase::Queued | Phase::Waiting => "33",
            Phase::Running => "36",
            Phase::Ok => "32",
            Phase::Idle => "90",
        };
        self.c(code, "●")
    }
}

fn state_text(t: &TaskView, now: f64) -> String {
    match t.phase {
        Phase::Building => {
            let step = t.step.as_ref().filter(|s| s.total > 0).map(|s| format!(" {}/{} {}", s.index, s.total, s.name)).unwrap_or_default();
            format!("building{step} · {}", clock(now - t.since))
        }
        Phase::Running => format!("running · {}", clock(now - t.since)),
        Phase::Queued => format!("queued #{}", t.queue_pos.map(|p| p + 1).unwrap_or(0)),
        Phase::Waiting => t.message.clone().unwrap_or_else(|| "waiting".into()),
        Phase::Failed | Phase::Seen => format!("failed · {}", t.message.clone().unwrap_or_default()),
        Phase::Ok => "ok".into(),
        Phase::Idle => match &t.last {
            Some(l) => format!("{} {}", format!("{:?}", l.outcome).to_lowercase(), ago(now - l.at)),
            None => "never run".into(),
        },
    }
}

pub fn ls(rest: &[String]) -> Result<i32> {
    let snap = snapshot()?;
    if rest.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&snap.tasks)?);
        return Ok(0);
    }
    let p = Paint::new();
    let w = snap.tasks.iter().map(|t| t.id.len()).max().unwrap_or(4).max(4);
    let mut tasks: Vec<&TaskView> = snap.tasks.iter().collect();
    tasks.sort_by(|a, b| (a.shelf, a.pin.unwrap_or(i64::MAX), &a.id).cmp(&(b.shelf, b.pin.unwrap_or(i64::MAX), &b.id)));
    for t in tasks {
        let tags = if t.tags.is_empty() { String::new() } else { p.c("90", &format!(" [{}]", t.tags.join(" "))) };
        let heavy = if t.heavy { p.c("33", " heavy") } else { String::new() };
        println!("{} {:w$}  {}{}{}", p.phase(t.phase), t.id, state_text(t, snap.now), heavy, tags, w = w);
    }
    for pr in &snap.problems {
        eprintln!("{} {}: {}", p.c("33", "!"), pr.file, pr.message);
    }
    if snap.tasks.is_empty() {
        eprintln!("no tasks yet: set `scan` in {} or add files to {}", paths::tilde(&paths::config_file()), paths::tilde(&paths::overlay_dir()));
    }
    Ok(0)
}

pub fn run(rest: &[String]) -> Result<i32> {
    let mut id = None;
    let mut params = BTreeMap::new();
    let mut variant = None;
    let mut clean = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" | "--param" => {
                let kv = it.next().ok_or_else(|| anyhow!("-p needs key=value"))?;
                let (k, v) = kv.split_once('=').ok_or_else(|| anyhow!("-p needs key=value, got \"{kv}\""))?;
                params.insert(k.to_string(), v.to_string());
            }
            "--variant" | "-v" => variant = Some(it.next().ok_or_else(|| anyhow!("--variant needs a name"))?.clone()),
            "--clean" => clean = true,
            s if id.is_none() => id = Some(s.to_string()),
            s => bail!("unexpected argument \"{s}\""),
        }
    }
    let id = id.ok_or_else(|| anyhow!("usage: yard run <id> [-p k=v] [--variant name] [--clean]"))?;
    simple(Request::Run { id, params, variant, clean })
}

pub fn stop(rest: &[String]) -> Result<i32> {
    let id = rest.first().ok_or_else(|| anyhow!("usage: yard stop <id>"))?;
    simple(Request::Stop { id: id.clone() })
}

pub fn ack(rest: &[String]) -> Result<i32> {
    simple(Request::Ack { phase: AckPhase::All, id: rest.first().cloned() })
}

pub fn status() -> Result<i32> {
    let snap = snapshot()?;
    let a = &snap.aggregate;
    println!("{}: {}", a.phase.as_str(), a.tooltip().replace('\n', "; "));
    Ok(0)
}

pub fn log(rest: &[String]) -> Result<i32> {
    let mut id = None;
    let mut follow = false;
    let mut n: Option<usize> = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-f" | "--follow" => follow = true,
            "-n" => n = it.next().and_then(|v| v.parse().ok()),
            s => id = Some(s.to_string()),
        }
    }
    let id = id.ok_or_else(|| anyhow!("usage: yard log <id> [-f] [-n N]"))?;
    let snap = snapshot()?;
    let t = snap.tasks.iter().find(|t| t.id == id).ok_or_else(|| anyhow!("no task named \"{id}\""))?;
    let path = PathBuf::from(&t.log);
    let mut f = std::fs::File::open(&path).map_err(|_| anyhow!("{} has no log yet ({})", t.label, paths::tilde(&path)))?;
    let mut text = String::new();
    f.read_to_string(&mut text)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = n.map(|n| lines.len().saturating_sub(n)).unwrap_or(0);
    let mut out = std::io::stdout().lock();
    for l in &lines[start..] {
        writeln!(out, "{l}")?;
    }
    if !follow {
        return Ok(0);
    }
    let mut pos = f.stream_position()?;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len < pos {
            // A new run rotated the log.
            f = std::fs::File::open(&path)?;
            pos = 0;
            writeln!(out, "----- new run -----")?;
        }
        f.seek(SeekFrom::Start(pos))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        pos += buf.len() as u64;
        out.write_all(&buf)?;
        out.flush()?;
    }
}

/// The waybar module. Reconnects forever, so the bar never shows a dead module.
pub fn bar() -> Result<i32> {
    let (cfg, _) = Config::load();
    let offline = serde_json::to_string(&BarLine {
        text: cfg.bar.icon.clone(),
        class: "offline".into(),
        tooltip: "Switchyard daemon is not running".into(),
        alt: "offline".into(),
    })?;
    let mut out = std::io::stdout().lock();
    let mut last = String::new();
    let mut tried_start = false;
    loop {
        match yard_core::protocol::subscribe_raw(SubKind::Bar) {
            Ok(lines) => {
                for line in lines {
                    let Ok(line) = line else { break };
                    if line != last {
                        writeln!(out, "{line}")?;
                        out.flush()?;
                        last = line;
                    }
                }
            }
            Err(_) => {
                if !tried_start {
                    tried_start = true;
                    let _ = ensure_daemon();
                    continue;
                }
            }
        }
        if last != offline {
            writeln!(out, "{offline}")?;
            out.flush()?;
            last = offline.clone();
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn check(ok: bool, what: &str, hint: &str) -> bool {
    let p = Paint::new();
    if ok {
        println!("{} {what}", p.c("32", "ok  "));
    } else {
        println!("{} {what}: {hint}", p.c("31", "fail"));
    }
    ok
}

pub fn doctor() -> Result<i32> {
    let mut all = true;
    let (cfg, problems) = Config::load();
    all &= check(problems.is_empty(), &format!("config {}", paths::tilde(&paths::config_file())), &problems.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("; "));
    let which = |b: &str| crate::supervise::which(b).is_some();
    let backend = crate::supervise::pick();
    check(true, &format!("task backend: {}", backend.name()), "");
    #[cfg(target_os = "linux")]
    if backend.name() != "systemd" {
        println!("     (no systemd user manager: memory caps are enforced by the daemon, not the kernel)");
    }
    let shell = if cfg.shell.is_empty() { crate::supervise::default_shell() } else { cfg.shell.clone() };
    check(which(&shell[0]), &format!("shell: {}", shell.join(" ")), "set `shell` in the config");
    #[cfg(target_os = "linux")]
    check(which("notify-send"), "notify-send", "install libnotify for failure notifications");
    check(which("git"), "git", "repo badges need git");
    let board_name = if cfg!(target_os = "linux") { "yard-board" } else { "yard-tray" };
    let board = sys::sibling_binary(board_name);
    all &= check(board.exists() || which(board_name), board_name, "install the board next to yard");
    let hypr = yard_core::hypr::socket().is_some();
    if cfg!(target_os = "linux") {
        check(hypr, "Hyprland IPC", "not under Hyprland: the board opens centered");
    }
    if hypr {
        let a = yard_core::hypr::anchor(&cfg.board.bar_namespace, false);
        let edge = a.as_ref().map(|a| format!("{:?}", a.edge).to_lowercase()).unwrap_or_default();
        check(a.is_some(), &format!("bar \"{}\" found, {edge} edge", cfg.board.bar_namespace), "set board.bar_namespace to your bar's layer namespace");
    }
    let daemon = request(&Request::Ping);
    all &= check(daemon.is_ok(), "daemon", "run `yard setup`, or `yard daemon` in a terminal");
    if daemon.is_ok() {
        let snap = snapshot()?;
        check(!snap.tasks.is_empty(), &format!("{} tasks", snap.tasks.len()), "add `scan` paths or manifests");
        for p in &snap.problems {
            check(false, &p.file, &p.message);
        }
    }
    let scan = cfg.scan_dirs();
    println!("     scanning {} directories, {} script dirs", scan.len(), cfg.scripts.len());
    Ok(if all { 0 } else { 1 })
}

/// Writes a starting `.yard.toml` from what the directory contains.
pub fn add(rest: &[String]) -> Result<i32> {
    let dir = match rest.first() {
        Some(d) => paths::expand(d),
        None => std::env::current_dir()?,
    };
    let dir = dir.canonicalize()?;
    let target = dir.join(".yard.toml");
    if target.exists() {
        bail!("{} already exists", paths::tilde(&target));
    }
    let id: String = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "task".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_".contains(c) { c } else { '-' })
        .collect();
    let text = scaffold(&dir, &id);
    std::fs::write(&target, &text)?;
    println!("wrote {}\n", paths::tilde(&target));
    print!("{text}");
    let (cfg, _) = Config::load();
    if !cfg.scan_dirs().iter().any(|d| d == &dir) {
        println!(
            "\nnote: {} is not covered by `scan` in {}, so the daemon will not see this file yet.",
            paths::tilde(&dir),
            paths::tilde(&paths::config_file())
        );
    }
    let _ = request(&Request::Reload);
    Ok(0)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

pub fn scaffold(dir: &Path, id: &str) -> String {
    let mut out = String::from("# Switchyard tasks for this repo. Reference: https://github.com/NathanielInman/switchyard/blob/main/docs/design.md\n\n");
    let godot = dir.join("project.godot");
    if godot.exists() {
        let proj = read(&godot);
        let scene = proj
            .lines()
            .find_map(|l| l.trim().strip_prefix("run/main_scene="))
            .map(|s| s.trim_matches('"').to_string())
            .unwrap_or_else(|| "res://Main.tscn".into());
        let csharp = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "csproj")))
            .unwrap_or(false);
        let mut steps = vec![];
        if csharp {
            steps.push("\"dotnet build\"".to_string());
        }
        steps.push("{ name = \"import\", cmd = \"godot --headless --import\" }".into());
        out.push_str(&format!(
            "[task.{id}]\ntags = [\"games\"]\nheavy = true\nsteps = [{}]\nrun = \"godot --path . {scene}\"\nerrors = [{}\"godot\"]\n\n[task.{id}.retry]\nstep = {}\non = \"signal\"\nattempts = 3\n\n[[task.{id}.watch]]\nmatch = '^(USER )?(SCRIPT )?ERROR:'\ntitle = \"runtime error\"\nurgency = \"critical\"\n",
            steps.join(", "),
            if csharp { "\"csharp\", " } else { "" },
            steps.len()
        ));
        return out;
    }
    if dir.join("Cargo.toml").exists() {
        let bin = read(&dir.join("Cargo.toml")).contains("[[bin]]") || dir.join("src/main.rs").exists();
        out.push_str(&format!("[task.{id}]\nheavy = true\nsteps = [\"cargo build\"]\n"));
        if bin {
            out.push_str("run = \"cargo run -q\"\n");
        }
        out.push_str(&format!("errors = [\"rust\"]\n\n[task.{id}-test]\nlabel = \"{id} tests\"\nheavy = true\nsteps = [\"cargo test\"]\nerrors = [\"rust\"]\n"));
        return out;
    }
    let pkg = dir.join("package.json");
    if pkg.exists() {
        let v: serde_json::Value = serde_json::from_str(&read(&pkg)).unwrap_or_default();
        let runner = if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
            "bun run"
        } else if dir.join("pnpm-lock.yaml").exists() {
            "pnpm run"
        } else if dir.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm run"
        };
        let scripts: Vec<String> = v.get("scripts").and_then(|s| s.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        if scripts.is_empty() {
            out.push_str(&format!("[task.{id}]\nsteps = [\"{runner} build\"]\n"));
        }
        for s in scripts.iter().take(8) {
            let safe: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
            let long = matches!(s.as_str(), "dev" | "start" | "serve" | "watch" | "preview");
            let key = if long { "run" } else { "steps" };
            let val = if long { format!("\"{runner} {s}\"") } else { format!("[\"{runner} {s}\"]") };
            out.push_str(&format!("[task.{id}-{safe}]\nlabel = \"{id} {s}\"\n{key} = {val}\nerrors = [\"typescript\", \"generic\"]\n\n"));
        }
        return out;
    }
    if dir.join("Makefile").exists() || dir.join("makefile").exists() {
        let mk = read(&dir.join("Makefile"));
        let targets: Vec<&str> = mk
            .lines()
            .filter_map(|l| {
                let (t, _) = l.split_once(':')?;
                (!t.is_empty() && !t.starts_with(['.', '\t', '#', ' ']) && !t.contains(['$', '=', '%', ' '])).then_some(t)
            })
            .take(6)
            .collect();
        for t in if targets.is_empty() { vec!["all"] } else { targets } {
            out.push_str(&format!("[task.{id}-{t}]\nlabel = \"{id} {t}\"\nsteps = [\"make {t}\"]\n\n"));
        }
        return out;
    }
    out.push_str(&format!("[task.{id}]\nsteps = [\"./build.sh\"]\n# run = \"./run.sh\"\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolds_godot_and_cargo() {
        let d = std::env::temp_dir().join(format!("yard-scaffold-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        std::fs::write(d.join("project.godot"), "[application]\nrun/main_scene=\"res://Boot.tscn\"\n").unwrap();
        std::fs::write(d.join("Game.csproj"), "").unwrap();
        let s = scaffold(&d, "game");
        assert!(s.contains("res://Boot.tscn"));
        assert!(s.contains("dotnet build"));
        let mut p = vec![];
        let tasks = yard_core::manifest::parse_manifest(&s, &d.join(".yard.toml"), yard_core::manifest::Origin::Repo, &d, &mut p);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(tasks[0].retry.as_ref().unwrap().step, Some(2));
        let _ = std::fs::remove_dir_all(&d);
        let d = std::env::temp_dir().join(format!("yard-scaffold-c-{}", std::process::id()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        std::fs::write(d.join("src/main.rs"), "").unwrap();
        let s = scaffold(&d, "x");
        let tasks = yard_core::manifest::parse_manifest(&s, &d.join(".yard.toml"), yard_core::manifest::Origin::Repo, &d, &mut p);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(tasks.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn durations_render() {
        assert_eq!(duration(65.0), "1m 5s");
    }
}

/// `yard history [id] [--json]`: the episode feed in the terminal.
pub fn history(rest: &[String]) -> Result<i32> {
    let json = rest.iter().any(|a| a == "--json");
    let id = rest.iter().find(|a| !a.starts_with("--")).cloned();
    let r = send(Request::History { id: id.clone(), limit: None })?;
    let h = r.history.ok_or_else(|| anyhow!("the daemon sent no history"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&h)?);
        return Ok(0);
    }
    let p = Paint::new();
    if h.records.is_empty() {
        println!("no runs recorded yet{}", id.map(|i| format!(" for {i}")).unwrap_or_default());
        return Ok(0);
    }
    use yard_core::history::Lamp;
    for day in h.days.iter().take(7) {
        println!("{}", p.c("1", &day.label.to_lowercase()));
        for e in &day.episodes {
            let lamp = match e.lamp {
                Lamp::Go => p.c("32", "●"),
                Lamp::Stop => p.c("31", "●"),
                Lamp::Fix => format!("{}{}", p.c("31", "●"), p.c("32", "●")),
                Lamp::Caution => p.c("33", "●"),
                Lamp::Frost => p.c("36", "●"),
            };
            let chip = e.chip.as_ref().map(|(t, k)| {
                let code = match k.as_str() {
                    "bad" => "31",
                    "ok" => "32",
                    _ => "33",
                };
                format!(" {}", p.c(code, &format!("({t})")))
            });
            let who = if id.is_some() { String::new() } else { format!("{:14} ", e.who) };
            println!("  {lamp:<2} {who}{}{} {}", e.what, chip.unwrap_or_default(), p.c("90", &format!("· {} · {}", e.sub, e.when)));
        }
    }
    let w = &h.week;
    let mut parts = vec![format!("{} building", duration(w.building_secs)), format!("{} on the main line", duration(w.waiting_secs))];
    if let Some((t, n)) = &w.flakiest {
        parts.push(format!("flakiest: {t} ({n} retries)"));
    }
    if let Some((t, g)) = &w.slower {
        parts.push(format!("{t} +{:.0}% slower", g * 100.0));
    }
    println!("\n{} {}", p.c("1", "this week"), parts.join(" · "));
    Ok(0)
}
