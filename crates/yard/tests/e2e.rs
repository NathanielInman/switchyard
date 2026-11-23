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
        v.as_array().and_then(|a| a.iter().find(|t| t["id"] == id).cloned()).unwrap_or(Value::Null)
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

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn inspection_reads_repos_and_clears_merged_branches() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let sb = Sandbox::new("inspect");
    let repo = sb.root.join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(git(&repo, &["init", "-q"]));
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    assert!(git(&repo, &["add", "."]));
    assert!(git(&repo, &["commit", "-q", "-m", "fix: start"]));
    assert!(git(&repo, &["branch", "done"]));

    // Off until configured.
    let off = sb.cmd().args(["inspect"]).output().unwrap();
    assert!(!off.status.success());
    assert!(String::from_utf8_lossy(&off.stderr).contains("Inspection is off"));

    let cfg = sb.root.join("config/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!("[inspection]\nroots = [\"{}/*\"]\n", sb.root.join("repos").to_string_lossy().replace('\\', "/")));
    std::fs::write(&cfg, text).unwrap();
    sb.cmd().arg("reload").output().unwrap();

    let read = || -> Value {
        let o = sb.cmd().args(["inspect", "--json"]).output().unwrap();
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    };
    let start = Instant::now();
    let app = loop {
        let v = read();
        let app = v["repos"].as_array().and_then(|a| a.iter().find(|r| r["name"] == "app").cloned());
        if let Some(app) = app.filter(|a| !a["signals"].is_null()) {
            break app;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "app never appeared: {v}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(app["tags"], serde_json::json!(["local"]));
    assert_eq!(app["status"]["merged_branches"], serde_json::json!(["done"]));
    assert_eq!(app["lamp"], "tidy");
    assert_eq!(app["signals"]["hotspots"][0]["file"], "a.txt");
    assert_eq!(app["signals"]["hotspots"][0]["fixes"], 1);

    let o = sb.cmd().args(["inspect", "app", "--delete-branches"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("Deleted 1 merged branch"), "{o:?}");
    let start = Instant::now();
    loop {
        let v = read();
        let app = v["repos"].as_array().and_then(|a| a.iter().find(|r| r["name"] == "app").cloned()).unwrap_or(Value::Null);
        if app["status"]["merged_branches"] == serde_json::json!([]) {
            assert_eq!(app["lamp"], "clear");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "the branch stayed: {app}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn schedules_catch_up_and_adopt_cron() {
    let mut sb = Sandbox::new("schedule");
    // Both tasks last ran two hours ago, so a slot came round since. The
    // strict one's time is an hour ago, so it cannot come round mid-test.
    sb.stop_daemon();
    let past = chrono::Local::now() - chrono::TimeDelta::hours(1);
    let strict = format!("{} {} * * *", chrono::Timelike::minute(&past), chrono::Timelike::hour(&past));
    let work = sb.root.join("work").to_string_lossy().replace('\\', "/");
    std::fs::write(
        sb.root.join("config/tasks.d/sched.toml"),
        format!(
            r#"
[defaults]
dir = "{work}"
notify = "never"

[task.nightly]
schedule = "hourly"
catch_up = true
steps = ["{ok}"]

[task.strict]
schedule = "{strict}"
steps = ["{ok}"]
"#,
            ok = cmds::OK
        ),
    )
    .unwrap();
    let then = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() - 7200.0;
    std::fs::write(
        sb.root.join("state/state.json"),
        format!(r#"{{"tasks":{{"nightly":{{"runs":[{then}]}},"strict":{{"runs":[{then}]}}}}}}"#),
    )
    .unwrap();
    sb.start();

    // The catch_up task runs by itself; the other waits for its next slot.
    let start = Instant::now();
    let t = loop {
        let t = sb.task("nightly");
        if t["last"]["outcome"] == "ok" {
            break t;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "nightly never caught up: {t}");
        std::thread::sleep(Duration::from_millis(150));
    };
    assert_eq!(t["schedule"], "hourly");
    let strict = sb.task("strict");
    assert!(strict["last"].is_null(), "{strict}");
    assert!(strict["next_run"].as_f64().unwrap() > then + 7200.0, "{strict}");
    assert!(strict["next_label"].as_str().unwrap().contains(':'), "{strict}");

    let o = sb.cmd().args(["history", "nightly", "--json"]).output().unwrap();
    let h: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(h["records"][0]["trigger"], "schedule", "{h}");

    // A crontab becomes an overlay of scheduled tasks; cron itself is not touched.
    let tab = sb.root.join("crontab.txt");
    std::fs::write(
        &tab,
        "MAILTO=\"\"\n# sync the notes repo\n@hourly /home/u/bin/cron-notes-sync.sh\n0 3 * * * $HOME/bin/backup.sh --all\n@reboot /usr/bin/thing\n",
    )
    .unwrap();
    let o = sb.cmd().args(["adopt", "cron", "--from"]).arg(&tab).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{o:?}");
    assert!(out.contains("with 2 tasks"), "{out}");
    assert!(out.contains("left out: @reboot"), "{out}");
    assert!(out.contains("  0 3 * * * $HOME/bin/backup.sh --all"), "{out}");
    sb.cmd().arg("reload").output().unwrap();
    let t = sb.task("notes-sync");
    assert_eq!(t["schedule"], "hourly", "{t}");
    assert_eq!(t["description"], "sync the notes repo", "{t}");
    assert_eq!(sb.task("backup")["schedule"], "0 3 * * *");

    // A second adopt never overwrites the first.
    let o = sb.cmd().args(["adopt", "cron", "--from"]).arg(&tab).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("already exists"));
}

#[test]
fn one_off_runs_fire_survive_restarts_and_cancel() {
    let mut sb = Sandbox::new("once");
    let at = |id: &str, when: &str| {
        let o = sb.cmd().args(["run", id, "--at", when]).output().unwrap();
        assert!(o.status.success(), "{o:?}");
        String::from_utf8_lossy(&o.stdout).to_string()
    };

    // A run booked two seconds out happens by itself and counts as scheduled.
    let out = at("good", "in 2s");
    assert!(out.contains("runs once at"), "{out}");
    assert!(sb.task("good")["next_label"].as_str().unwrap_or("").ends_with(" once"), "{}", sb.task("good"));
    let start = Instant::now();
    loop {
        let t = sb.task("good");
        if t["last"]["outcome"] == "ok" {
            assert!(t["next_run"].is_null(), "{t}");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "the one-off never ran: {t}");
        std::thread::sleep(Duration::from_millis(150));
    }
    let o = sb.cmd().args(["history", "good", "--json"]).output().unwrap();
    let h: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(h["records"][0]["trigger"], "schedule", "{h}");

    // A booking survives a restart, and stop cancels it.
    at("bad", "in 1h");
    sb.stop_daemon();
    sb.start();
    assert!(sb.task("bad")["next_label"].as_str().unwrap_or("").ends_with(" once"), "{}", sb.task("bad"));
    let o = sb.cmd().args(["stop", "bad"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("cancelled"), "{o:?}");
    assert!(sb.task("bad")["next_run"].is_null());
    assert!(sb.task("bad")["last"].is_null());

    // Nonsense and the past are refused before they reach the daemon.
    assert!(!sb.cmd().args(["run", "good", "--at", "whenever"]).output().unwrap().status.success());
    assert!(!sb.cmd().args(["run", "good", "--at", "2001-01-01 00:00"]).output().unwrap().status.success());
}

#[test]
fn watched_timers_name_what_they_cannot_find() {
    let sb = Sandbox::new("timers");
    let cfg = sb.root.join("config/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str("[[timer]]\nunit = \"yard-e2e-no-such-unit.timer\"\nlabel = \"missing\"\n");
    std::fs::write(&cfg, text).unwrap();
    sb.cmd().arg("reload").output().unwrap();
    let want = if cfg!(target_os = "linux") { "no such timer" } else { "Linux only" };
    let start = Instant::now();
    loop {
        let o = sb.cmd().arg("ls").output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        if err.contains(want) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no problem about the timer: {err}");
        std::thread::sleep(Duration::from_millis(200));
    }
    if cfg!(target_os = "linux") {
        let t = sb.task("yard-e2e-no-such-unit");
        assert_eq!(t["watched"], true, "{t}");
        assert_eq!(t["label"], "missing");
        assert_eq!(t["tags"], serde_json::json!(["scheduled"]));
    }
}

#[test]
fn the_finish_hook_hears_every_finished_run() {
    let sb = Sandbox::new("hooks");
    let out = sb.root.join("hook.txt");
    let out_s = out.to_string_lossy().replace('\\', "/");
    let hook = if cfg!(windows) {
        // A digit right before >> would be read as a handle number: keep a space.
        format!("echo %YARD_TASK% %YARD_OUTCOME% %YARD_TRIGGER% %YARD_CODE% >> \"{out_s}\"")
    } else {
        format!("echo \"$YARD_TASK $YARD_OUTCOME $YARD_TRIGGER $YARD_CODE $YARD_ERROR\" >> '{out_s}'")
    };
    let cfg = sb.root.join("config/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!("[hooks]\non_finish = {}\n", toml_str(&hook)));
    std::fs::write(&cfg, text).unwrap();
    sb.cmd().arg("reload").output().unwrap();

    sb.run("good");
    sb.wait_for("good", &["ok", "idle"], 30);
    sb.run("bad");
    sb.wait_for("bad", &["failed"], 30);
    let start = Instant::now();
    let lines = loop {
        let t = std::fs::read_to_string(&out).unwrap_or_default();
        if t.lines().count() >= 2 {
            break t;
        }
        let log = std::fs::read_to_string(sb.root.join("state/hooks.log")).unwrap_or_default();
        assert!(start.elapsed() < Duration::from_secs(20), "the hook did not run twice: {t:?}\nhooks.log:\n{log}");
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(lines.lines().any(|l| l.trim() == "good ok manual 0"), "{lines}");
    let bad = lines.lines().find(|l| l.starts_with("bad ")).unwrap_or_default();
    assert!(bad.starts_with("bad failed manual 3"), "{lines}");
    if !cfg!(windows) {
        assert!(bad.contains("the build broke"), "{lines}");
    }

    // A failing hook is a problem on the board, not a silent one.
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text = text.replace(&format!("on_finish = {}", toml_str(&hook)), "on_finish = \"exit 7\"");
    std::fs::write(&cfg, text).unwrap();
    sb.cmd().arg("reload").output().unwrap();
    sb.run("good");
    let start = Instant::now();
    loop {
        let o = sb.cmd().arg("ls").output().unwrap();
        if String::from_utf8_lossy(&o.stderr).contains("hooks.on_finish failed") {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no hook problem: {o:?}");
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn toml_str(s: &str) -> String {
    format!("'''{s}'''")
}
