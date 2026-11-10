//! `yard setup`: starts Switchyard at login and writes a starter config.
//!
//! - Linux: systemd user units for the daemon and the board.
//! - macOS: LaunchAgents for the daemon and the menu bar app.
//! - Windows: a Run key that starts the tray app, which starts the daemon.

use anyhow::Result;
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::Command;
use yard_core::paths;

const STARTER: &str = r#"# Switchyard config. Reference:
# https://github.com/NathanielInman/switchyard/blob/main/docs/design.md#config-reference

# Folders whose children may hold a .yard.toml. A trailing /* means each child.
scan = ["~/code/*"]

# Run `yard add` inside a repo to scaffold its tasks, or put task files for
# repos you do not own in tasks.d/ next to this file.

[mainline]
min_free = "8G"   # free memory needed before a heavy build starts
mem = "12G"       # memory cap for each heavy build
"#;

fn bin_dir() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| paths::home().join(".local/bin"));
    stable_dir(&dir)
}

/// Package managers install into versioned directories that the next upgrade
/// deletes. Login items must point at the stable link instead:
/// Homebrew's `<prefix>/bin`, Scoop's `apps/switchyard/current`.
fn stable_dir(dir: &Path) -> PathBuf {
    let s = dir.to_string_lossy().to_string();
    if let Some(i) = s.find("/Cellar/") {
        return PathBuf::from(format!("{}/bin", &s[..i]));
    }
    let lower = s.to_lowercase().replace('/', "\\");
    if let Some(i) = lower.find("\\apps\\switchyard\\") {
        let base = &s[..i + "\\apps\\switchyard\\".len()];
        return PathBuf::from(format!("{base}current"));
    }
    dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_dirs() {
        assert_eq!(stable_dir(Path::new("/opt/homebrew/Cellar/switchyard/0.1.0/bin")), PathBuf::from("/opt/homebrew/bin"));
        assert_eq!(
            stable_dir(Path::new("/home/linuxbrew/.linuxbrew/Cellar/switchyard/0.2.0/bin")),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin")
        );
        assert_eq!(
            stable_dir(Path::new(r"C:\Users\n\scoop\apps\switchyard\0.1.0")),
            PathBuf::from(r"C:\Users\n\scoop\apps\switchyard\current")
        );
        assert_eq!(stable_dir(Path::new("/home/n/.local/bin")), PathBuf::from("/home/n/.local/bin"));
    }
}

fn bin(name: &str) -> PathBuf {
    bin_dir().join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

pub fn run(args: &[String]) -> Result<i32> {
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let no_start = args.iter().any(|a| a == "--no-start");
    if uninstall {
        remove()?;
        println!("removed the login items; your config and history are untouched");
        return Ok(0);
    }
    let cfg = paths::config_file();
    if !cfg.exists() {
        std::fs::create_dir_all(paths::config_dir())?;
        std::fs::create_dir_all(paths::overlay_dir())?;
        std::fs::write(&cfg, STARTER)?;
        println!("wrote a starter config: {}", paths::tilde(&cfg));
    } else {
        println!("config: {}", paths::tilde(&cfg));
    }
    install(no_start)?;
    hints();
    Ok(0)
}

// ------------------------------------------------------------------ Linux

#[cfg(target_os = "linux")]
fn unit_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home().join(".config"))
        .join("systemd/user")
}

#[cfg(target_os = "linux")]
fn install(no_start: bool) -> Result<()> {
    if !crate::supervise::which("systemctl").is_some_and(|_| crate::sys::systemctl_user(&["show-environment"]).is_some_and(|t| !t.is_empty())) {
        println!("no systemd user manager: start `{} daemon` from your session's autostart", bin("yard").display());
        return Ok(());
    }
    let dir = unit_dir();
    std::fs::create_dir_all(&dir)?;
    let daemon = format!(
        "[Unit]\nDescription=Switchyard daemon: runs and watches your tasks\nDocumentation=https://github.com/NathanielInman/switchyard\nPartOf=graphical-session.target\nAfter=graphical-session.target\n\n[Service]\nExecStart={} daemon\nRestart=on-failure\nRestartSec=2\n# Tasks run in their own units; only the daemon stops here.\nKillMode=process\n\n[Install]\nWantedBy=graphical-session.target\n",
        bin("yard").display()
    );
    std::fs::write(dir.join("switchyard.service"), daemon)?;
    let mut units = vec!["switchyard.service"];
    if bin("yard-board").exists() {
        let board = format!(
            "[Unit]\nDescription=Switchyard board: the panel your bar opens\nDocumentation=https://github.com/NathanielInman/switchyard\nPartOf=graphical-session.target\nAfter=graphical-session.target switchyard.service\nWants=switchyard.service\nRequisite=graphical-session.target\n\n[Service]\nExecStart={}\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=graphical-session.target\n",
            bin("yard-board").display()
        );
        std::fs::write(dir.join("switchyard-board.service"), board)?;
        units.push("switchyard-board.service");
    }
    let sc = |a: &[&str]| Command::new("systemctl").arg("--user").args(a).status();
    sc(&["daemon-reload"])?;
    let mut en = vec!["enable"];
    en.extend(&units);
    sc(&en)?;
    if !no_start {
        for u in &units {
            let _ = sc(&["restart", u]);
        }
    }
    println!("enabled {} (systemd user units in {})", units.join(" and "), paths::tilde(&dir));
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove() -> Result<()> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "switchyard.service", "switchyard-board.service"]).status();
    for u in ["switchyard.service", "switchyard-board.service"] {
        let _ = std::fs::remove_file(unit_dir().join(u));
    }
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    Ok(())
}

/// Starts the installed service, if there is one. False when the caller
/// should start the daemon itself.
#[cfg(target_os = "linux")]
pub fn start_service() -> bool {
    if !unit_dir().join("switchyard.service").exists() {
        return false;
    }
    Command::new("systemctl").args(["--user", "start", "switchyard.service"]).status().is_ok_and(|s| s.success())
}

#[cfg(target_os = "linux")]
fn hints() {
    println!(
        "\nnext:\n  1. waybar: add \"custom/switchyard\" to a modules list and define it as\n       \"custom/switchyard\": {{ \"exec\": \"{0} bar\", \"return-type\": \"json\", \"on-click\": \"{0} toggle\", \"on-click-right\": \"{0} ack\" }}\n  2. Hyprland: hl.bind(\"SUPER + Y\", hl.dsp.exec_cmd(\"{0} toggle --center\"))\n  3. in a repo: yard add",
        bin("yard").display()
    );
}

// ------------------------------------------------------------------ macOS

#[cfg(target_os = "macos")]
const LABEL: &str = "io.github.nathanielinman.switchyard";

#[cfg(target_os = "macos")]
fn agent(name: &str) -> PathBuf {
    paths::home().join("Library/LaunchAgents").join(format!("{name}.plist"))
}

#[cfg(target_os = "macos")]
fn uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(target_os = "macos")]
fn plist(label: &str, args: &[String], keep_alive: bool) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let args: String = args.iter().map(|a| format!("    <string>{}</string>\n", esc(a))).collect();
    let log = paths::state_dir().join(format!("{label}.log"));
    // launchd starts agents with a bare PATH; keep the one setup ran with.
    let path = std::env::var("PATH").unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <{keep}/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        keep = if keep_alive { "true" } else { "false" },
        path = esc(&path),
        log = esc(&log.to_string_lossy()),
    )
}

#[cfg(target_os = "macos")]
fn install(no_start: bool) -> Result<()> {
    std::fs::create_dir_all(paths::home().join("Library/LaunchAgents"))?;
    std::fs::create_dir_all(paths::state_dir())?;
    let daemon = format!("{LABEL}");
    let tray = format!("{LABEL}.tray");
    std::fs::write(agent(&daemon), plist(&daemon, &[bin("yard").to_string_lossy().into(), "daemon".into()], true))?;
    let mut labels = vec![daemon];
    if bin("yard-tray").exists() {
        std::fs::write(agent(&tray), plist(&tray, &[bin("yard-tray").to_string_lossy().into()], true))?;
        labels.push(tray);
    }
    if !no_start {
        for l in &labels {
            let domain = format!("gui/{}", uid());
            let _ = Command::new("launchctl").args(["bootout", &domain, &agent(l).to_string_lossy()]).status();
            let _ = Command::new("launchctl").args(["bootstrap", &domain, &agent(l).to_string_lossy()]).status();
        }
    }
    println!("installed LaunchAgents: {}", labels.join(", "));
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove() -> Result<()> {
    for l in [LABEL.to_string(), format!("{LABEL}.tray")] {
        let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}", uid()), &agent(&l).to_string_lossy()]).status();
        let _ = std::fs::remove_file(agent(&l));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn start_service() -> bool {
    if !agent(LABEL).exists() {
        return false;
    }
    Command::new("launchctl")
        .args(["kickstart", &format!("gui/{}/{LABEL}", uid())])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(target_os = "macos")]
fn hints() {
    println!("\nnext:\n  1. the signal is in your menu bar; click it to open the board\n  2. in a repo: yard add");
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
fn install(no_start: bool) -> Result<()> {
    let tray = bin("yard-tray");
    let target = if tray.exists() { format!("\"{}\"", tray.display()) } else { format!("\"{}\" daemon", bin("yard").display()) };
    let status = Command::new("reg").args(["add", RUN_KEY, "/v", "Switchyard", "/t", "REG_SZ", "/d", &target, "/f"]).status();
    match status {
        Ok(s) if s.success() => println!("Switchyard will start at login ({RUN_KEY}\\Switchyard)"),
        _ => println!("could not write the Run key; start {target} yourself at login"),
    }
    if !no_start {
        let mut c = if tray.exists() {
            Command::new(&tray)
        } else {
            let mut c = Command::new(bin("yard"));
            c.arg("daemon");
            c
        };
        crate::sys::detach(&mut c);
        let _ = crate::sys::spawn_detached(c);
    }
    Ok(())
}

#[cfg(windows)]
fn remove() -> Result<()> {
    let _ = Command::new("reg").args(["delete", RUN_KEY, "/v", "Switchyard", "/f"]).status();
    let _ = yard_core::protocol::request(&yard_core::protocol::Request::Shutdown);
    Ok(())
}

#[cfg(windows)]
pub fn start_service() -> bool {
    false
}

#[cfg(windows)]
fn hints() {
    println!("\nnext:\n  1. the signal is in your system tray; click it to open the board\n  2. in a repo: yard add");
}
