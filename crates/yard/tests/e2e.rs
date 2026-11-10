//! End to end: a real daemon in a throwaway directory, driven through the
//! CLI. Runs on Linux, macOS and Windows with each platform's native shell.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const YARD: &str = env!("CARGO_BIN_EXE_yard");

struct Sandbox {
    root: PathBuf,
    daemon: Option<Child>,
}

#[cfg(windows)]
mod cmds {
    pub const SHELL: &str = r#"["cmd", "/C"]"#;
    pub const OK: &str = "echo hello";
    pub const FAIL: &str = "echo error: the build broke & exit /b 3";
    pub fn sleep(s: u32) -> String {
        format!("ping -n {} 127.0.0.1 > nul", s + 1)
    }
}

#[cfg(not(windows))]
mod cmds {
    pub const SHELL: &str = r#"["sh", "-c"]"#;
    pub const OK: &str = "echo hello";
    pub const FAIL: &str = "echo 'error: the build broke'; exit 3";
    pub fn sleep(s: u32) -> String {
        format!("sleep {s}")
    }
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("yard-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["config/tasks.d", "state", "run", "work"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let toml_path = |p: &Path| p.to_string_lossy().replace('\\', "/");
        std::fs::write(
            root.join("config/config.toml"),
            format!(
                "shell = {}\nlog_dir = \"{}\"\nok_flash = 1\n[mainline]\nmin_free = \"1M\"\nmem = \"2G\"\n[shelf]\nbinds = false\n",
                cmds::SHELL,
                toml_path(&root.join("state/logs"))
            ),
        )
        .unwrap();
        let work = toml_path(&root.join("work"));
        std::fs::write(
            root.join("config/tasks.d/e2e.toml"),
            format!(
                r#"
[defaults]
dir = "{work}"

[task.good]
steps = ["{ok}"]

[task.bad]
steps = ["{fail}"]
errors = ["generic"]
notify = "never"

[task.heavy-a]
heavy = true
steps = ["{s2}"]

[task.heavy-b]
heavy = true
steps = ["{s2}"]

[task.server]
run = "{s60}"
"#,
                ok = cmds::OK,
                fail = cmds::FAIL.replace('"', "\\\""),
                s2 = cmds::sleep(2),
                s60 = cmds::sleep(60),
            ),
        )
        .unwrap();
        let mut sb = Sandbox { root, daemon: None };
        sb.start();
        sb
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(YARD);
        c.env("SWITCHYARD_CONFIG_DIR", self.root.join("config"))
            .env("SWITCHYARD_STATE_DIR", self.root.join("state"))
            .env("SWITCHYARD_RUNTIME_DIR", self.root.join("run"));
        c
    }

    fn start(&mut self) {
        let child = self.cmd().arg("daemon").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        self.daemon = Some(child);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if self.cmd().arg("status").output().is_ok_and(|o| o.status.success()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("daemon did not come up");
    }

    fn stop_daemon(&mut self) {
        let _ = self.request(r#"{"cmd":"shutdown"}"#);
        if let Some(mut d) = self.daemon.take() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if d.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = d.kill();
            let _ = d.wait();
        }
    }

    fn request(&self, json: &str) -> Option<String> {
        let o = self.cmd().args(["__request", json]).output().ok()?;
        Some(String::from_utf8_lossy(&o.stdout).to_string())
    }

    fn run(&self, id: &str) -> String {
        let o = self.cmd().args(["run", id]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    fn task(&self, id: &str) -> Value {
        let o = self.cmd().args(["ls", "--json"]).output().unwrap();
        let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
        v.as_array()
            .and_then(|a| a.iter().find(|t| t["id"] == id).cloned())
            .unwrap_or(Value::Null)
    }

    fn wait_for(&self, id: &str, phases: &[&str], secs: u64) -> Value {
        let start = Instant::now();
        loop {
            let t = self.task(id);
            if phases.iter().any(|p| t["phase"] == *p) {
                return t;
            }
            if start.elapsed() > Duration::from_secs(secs) {
                panic!("{id} never reached {phases:?}: {t}");
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.cmd().args(["stop", "server"]).output();
        self.stop_daemon();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn tasks_run_fail_queue_and_survive_a_restart() {
    let mut sb = Sandbox::new("main");

    // A passing task flashes ok, then goes idle with an ok result.
    sb.run("good");
    sb.wait_for("good", &["ok", "idle"], 30);
    let t = sb.wait_for("good", &["idle"], 30);
    assert_eq!(t["last"]["outcome"], "ok");

    // A failing task finds its error line.
    sb.run("bad");
    let t = sb.wait_for("bad", &["failed"], 30);
    assert!(t["error"]["text"].as_str().unwrap_or("").contains("the build broke"), "{t}");
    assert!(t["message"].as_str().unwrap_or("").contains("exit 3"), "{t}");

    // Two heavy tasks share the main line: the second waits for the first.
    sb.run("heavy-a");
    let out = sb.run("heavy-b");
    assert!(out.contains("queued"), "{out}");
    assert_eq!(sb.task("heavy-b")["phase"], "queued");
    sb.wait_for("heavy-b", &["building"], 30);
    assert_ne!(sb.task("heavy-a")["phase"], "building");
    sb.wait_for("heavy-b", &["ok", "idle"], 30);

    // A long run survives a daemon restart and can still be stopped.
    sb.run("server");
    sb.wait_for("server", &["running"], 30);
    sb.stop_daemon();
    sb.start();
    sb.wait_for("server", &["running"], 15);
    sb.cmd().args(["stop", "server"]).output().unwrap();
    let t = sb.wait_for("server", &["idle"], 30);
    assert_eq!(t["last"]["outcome"], "stopped");

    // Every finished run is in the history, with its own log kept.
    let o = sb.cmd().args(["history", "--json"]).output().unwrap();
    let h: Value = serde_json::from_slice(&o.stdout).unwrap();
    let recs = h["records"].as_array().unwrap();
    assert!(recs.len() >= 5, "{h}");
    let bad = recs.iter().find(|r| r["task"] == "bad").unwrap();
    assert_eq!(bad["outcome"], "failed");
    assert!(bad["error"]["text"].as_str().unwrap_or("").contains("the build broke"));
    assert!(std::path::Path::new(bad["log"].as_str().unwrap()).exists());
    assert!(!h["days"].as_array().unwrap().is_empty());
}
