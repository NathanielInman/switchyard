//! Small things the daemon asks of the system: memory, notifications, git,
//! systemd and the installed-tools list.

#[allow(unused_imports)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use yard_core::model::{GitInfo, ShelfItem};

#[derive(Debug, Clone, Copy, Default)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
}

#[cfg(target_os = "linux")]
pub fn meminfo() -> MemInfo {
    let mut m = MemInfo::default();
    if let Ok(t) = std::fs::read_to_string("/proc/meminfo") {
        for l in t.lines() {
            let mut it = l.split_whitespace();
            let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
            let v: u64 = v.parse().unwrap_or(0) * 1024;
            match k {
                "MemTotal:" => m.total = v,
                "MemAvailable:" => m.available = v,
                _ => {}
            }
        }
    }
    m
}

#[cfg(not(target_os = "linux"))]
pub fn meminfo() -> MemInfo {
    let Ok(mut s) = sysinfo::System::new() else { return MemInfo::default() };
    s.refresh_memory();
    MemInfo { total: s.total_memory(), available: s.available_memory() }
}

/// Runs a command without waiting for it, and reaps it on a thread so it does
/// not linger as a zombie.
pub fn spawn_detached(mut cmd: Command) -> std::io::Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Makes a child outlive us: its own session on Unix, a detached process
/// group on Windows.
pub fn detach(c: &mut Command) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        c.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
}

/// Like [`spawn_detached`] but keeps whatever stdout and stderr the command
/// was given.
pub fn spawn_detached_keep_output(mut cmd: Command) -> std::io::Result<()> {
    cmd.stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn notify(title: &str, body: &str, urgency: &str) {
    let mut c = Command::new("notify-send");
    c.args(["-a", "Switchyard", "-u", urgency]);
    let icon = match urgency {
        "critical" => "dialog-error",
        _ => "dialog-information",
    };
    c.args(["-i", icon, title, &escape_markup(body)]);
    let _ = spawn_detached(c);
}

#[cfg(target_os = "macos")]
pub fn notify(title: &str, body: &str, _urgency: &str) {
    // osascript needs no app bundle and no Objective-C build step.
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"Switchyard\" subtitle \"{}\"", q(body), q(title));
    let mut c = Command::new("osascript");
    c.args(["-e", &script]);
    let _ = spawn_detached(c);
}

#[cfg(windows)]
pub fn notify(title: &str, body: &str, _urgency: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().appname("Switchyard").summary(&title).body(&body).show();
    });
}

/// Notification bodies are Pango markup; a stray `&` or `<` breaks them.
pub fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn git_info(dir: &Path) -> Option<GitInfo> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v2", "--branch", "--untracked-files=normal"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut g = GitInfo::default();
    for l in text.lines() {
        if let Some(b) = l.strip_prefix("# branch.head ") {
            g.branch = b.to_string();
        } else if let Some(ab) = l.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    g.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    g.behind = b.parse().unwrap_or(0);
                }
            }
        } else if !l.starts_with('#') && !l.is_empty() {
            g.dirty += 1;
        }
    }
    Some(g)
}

/// Explicitly installed packages that ship a program in /usr/bin, with their
/// descriptions. Two pacman calls in total.
#[cfg(target_os = "linux")]
pub fn pacman_tools() -> Vec<ShelfItem> {
    let mut bins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(out) = Command::new("pacman").args(["-Qel"]).stderr(Stdio::null()).output() else {
        return vec![];
    };
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((pkg, path)) = l.split_once(' ') else { continue };
        if let Some(bin) = path.strip_prefix("/usr/bin/") {
            if !bin.is_empty() && !bin.contains('/') {
                bins.entry(pkg.to_string()).or_default().push(bin.to_string());
            }
        }
    }
    let Ok(info) = Command::new("pacman").args(["-Qei"]).stderr(Stdio::null()).output() else {
        return vec![];
    };
    let mut descs: BTreeMap<String, String> = BTreeMap::new();
    let mut name = String::new();
    for l in String::from_utf8_lossy(&info.stdout).lines() {
        if let Some(v) = l.strip_prefix("Name") {
            name = v.trim_start_matches([' ', ':']).trim().to_string();
        } else if let Some(v) = l.strip_prefix("Description") {
            descs.insert(name.clone(), v.trim_start_matches([' ', ':']).trim().to_string());
        }
    }
    let mut out = vec![];
    for (pkg, list) in bins {
        let desc = descs.get(&pkg).cloned().unwrap_or_default();
        // Prefer the binary named like the package; otherwise list them all.
        if list.contains(&pkg) {
            out.push(ShelfItem { name: pkg.clone(), desc, cmd: None });
        } else {
            for b in list.into_iter().take(4) {
                out.push(ShelfItem { name: b, desc: desc.clone(), cmd: None });
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
pub fn systemctl_user(args: &[&str]) -> Option<String> {
    let out = Command::new("systemctl").arg("--user").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
pub fn unit_properties(unit: &str, props: &[&str]) -> BTreeMap<String, String> {
    let mut args = vec!["show", unit];
    let joined = props.join(",");
    let p = format!("--property={joined}");
    args.push(&p);
    let text = systemctl_user(&args).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[cfg(target_os = "linux")]
pub fn cgroup_path(unit: &str) -> Option<PathBuf> {
    let props = unit_properties(unit, &["ControlGroup"]);
    let cg = props.get("ControlGroup")?;
    if cg.is_empty() {
        return None;
    }
    Some(PathBuf::from(format!("/sys/fs/cgroup{cg}")))
}

#[cfg(target_os = "linux")]
pub fn read_u64(p: &Path) -> Option<u64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
pub fn cpu_usage_usec(cg: &Path) -> Option<u64> {
    let t = std::fs::read_to_string(cg.join("cpu.stat")).ok()?;
    t.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// True when the cgroup still has processes in it.
#[cfg(target_os = "linux")]
pub fn cgroup_alive(cg: &Path) -> bool {
    match std::fs::read_to_string(cg.join("cgroup.procs")) {
        Ok(t) => !t.trim().is_empty(),
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn pacman_tools() -> Vec<ShelfItem> {
    vec![]
}

/// Finds a binary next to the running executable, then on PATH.
pub fn sibling_binary(name: &str) -> PathBuf {
    let name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(&name);
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup() {
        assert_eq!(escape_markup("a & <b>"), "a &amp; &lt;b&gt;");
    }

    #[test]
    fn meminfo_reads() {
        let m = meminfo();
        assert!(m.total > 0);
        assert!(m.available <= m.total);
    }
}
