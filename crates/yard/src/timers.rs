//! Watched systemd timers: read-only tiles for timers that other repos and
//! the distro installed. Switchyard reads their state through `systemctl show`
//! and their output from the journal. It never edits a unit; the only thing it
//! sends is "start now" and "stop" for user units, when asked.
//!
//! Linux only: elsewhere [`read`] finds nothing and the config gets a problem.

// Off Linux only the stubs below are called; the parsing stays for its tests.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::BTreeMap;
use yard_core::model::Outcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}

/// One watched timer and the service it starts, as last read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimerState {
    pub id: String,
    /// `None` when neither manager knows the unit.
    pub scope: Option<Scope>,
    pub description: String,
    /// `foo.service`.
    pub service: String,
    pub next: Option<f64>,
    /// The service is running right now.
    pub active: bool,
    pub started: Option<f64>,
    pub exited: Option<f64>,
    /// systemd's `Result`: `success`, `exit-code`, `signal`, `timeout`, ...
    pub result: String,
    pub status: i32,
}

impl TimerState {
    /// How the last finished run went, when one finished since boot.
    pub fn outcome(&self) -> Option<(Outcome, String)> {
        self.exited?;
        if self.result == "success" {
            return Some((Outcome::Ok, String::new()));
        }
        let msg = match self.result.as_str() {
            "exit-code" => format!("exit {}", self.status),
            "signal" | "core-dump" => format!("killed by signal {}", self.status),
            "timeout" => "timed out".into(),
            "oom-kill" => "killed: out of memory".into(),
            "" => "failed".into(),
            other => other.replace('-', " "),
        };
        Some((Outcome::Failed, msg))
    }
}

const TIMER_PROPS: &str = "Id,LoadState,Description,Unit,NextElapseUSecRealtime";
const SERVICE_PROPS: &str = "Id,ActiveState,SubState,Result,ExecMainStatus,ExecMainStartTimestamp,ExecMainExitTimestamp";

/// `systemctl show` output: blocks of `Key=value` lines, one per unit, by `Id`.
pub fn parse_show(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for block in text.split("\n\n") {
        let props: BTreeMap<String, String> =
            block.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(id) = props.get("Id") {
            out.insert(id.clone(), props);
        }
    }
    out
}

/// `@1795428760` (from `--timestamp=unix`); empty means never.
fn ts(v: Option<&String>) -> Option<f64> {
    v?.strip_prefix('@')?.parse().ok()
}

/// Builds a timer's state from its `show` blocks.
pub fn state(id: &str, scope: Scope, timer: &BTreeMap<String, String>, service: Option<&BTreeMap<String, String>>) -> TimerState {
    let get = |m: Option<&BTreeMap<String, String>>, k: &str| m.and_then(|m| m.get(k)).cloned().unwrap_or_default();
    let active_state = get(service, "ActiveState");
    let sub = get(service, "SubState");
    // A oneshot with RemainAfterExit stays "active (exited)": that is done.
    let active = matches!(active_state.as_str(), "active" | "activating" | "reloading" | "deactivating") && sub != "exited";
    TimerState {
        id: id.to_string(),
        scope: Some(scope),
        description: timer.get("Description").cloned().unwrap_or_default(),
        service: timer.get("Unit").cloned().unwrap_or_default(),
        next: ts(timer.get("NextElapseUSecRealtime")),
        active,
        started: ts(service.and_then(|s| s.get("ExecMainStartTimestamp"))),
        exited: if active { None } else { ts(service.and_then(|s| s.get("ExecMainExitTimestamp"))) },
        result: get(service, "Result"),
        status: get(service, "ExecMainStatus").parse().unwrap_or(0),
    }
}

#[cfg(target_os = "linux")]
fn systemctl(scope: Scope) -> std::process::Command {
    let mut c = std::process::Command::new("systemctl");
    if scope == Scope::User {
        c.arg("--user");
    }
    c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c
}

#[cfg(target_os = "linux")]
fn show(scope: Scope, props: &str, units: &[String]) -> BTreeMap<String, BTreeMap<String, String>> {
    if units.is_empty() {
        return BTreeMap::new();
    }
    let out = systemctl(scope).args(["show", "--timestamp=unix", "-p", props]).args(units).output();
    out.map(|o| parse_show(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// Reads every watched timer, `(id, timer unit name)`, from the user manager
/// first and the system manager for the rest. Two or four `systemctl`
/// processes per read, however many timers there are.
#[cfg(target_os = "linux")]
pub fn read(watch: &[(String, String)]) -> Vec<TimerState> {
    let unit = |name: &str| format!("{name}.timer");
    let mut out = vec![];
    let mut rest: Vec<(String, String)> = watch.to_vec();
    for scope in [Scope::User, Scope::System] {
        let names: Vec<String> = rest.iter().map(|(_, n)| unit(n)).collect();
        let timers = show(scope, TIMER_PROPS, &names);
        let found: Vec<(String, String)> = rest
            .iter()
            .filter(|(_, n)| timers.get(&unit(n)).is_some_and(|t| t.get("LoadState").is_some_and(|l| l == "loaded")))
            .cloned()
            .collect();
        let services: Vec<String> =
            found.iter().filter_map(|(_, n)| timers.get(&unit(n))?.get("Unit").cloned()).filter(|s| !s.is_empty()).collect();
        let svc = show(scope, SERVICE_PROPS, &services);
        for (id, n) in &found {
            let t = &timers[&unit(n)];
            out.push(state(id, scope, t, t.get("Unit").and_then(|u| svc.get(u))));
        }
        rest.retain(|r| !found.contains(r));
    }
    out.extend(rest.into_iter().map(|(id, _)| TimerState { id, ..Default::default() }));
    out
}

#[cfg(not(target_os = "linux"))]
pub fn read(watch: &[(String, String)]) -> Vec<TimerState> {
    watch.iter().map(|(id, _)| TimerState { id: id.clone(), ..Default::default() }).collect()
}

/// The service's journal between two times, as plain lines.
#[cfg(target_os = "linux")]
pub fn journal(scope: Scope, service: &str, since: f64, until: f64) -> String {
    let mut c = std::process::Command::new("journalctl");
    if scope == Scope::User {
        c.arg("--user");
    }
    c.args(["-u", service, "--no-pager", "-o", "cat"])
        .arg(format!("--since=@{}", since.floor() as i64))
        .arg(format!("--until=@{}", until.ceil() as i64 + 1))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
pub fn journal(_: Scope, _: &str, _: f64, _: f64) -> String {
    String::new()
}

/// `systemctl --user start|stop --no-block <service>`. System units need root,
/// so they only say how.
#[cfg(target_os = "linux")]
pub fn control(scope: Scope, verb: &str, service: &str) -> Result<(), String> {
    if scope == Scope::System {
        return Err(format!("{service} is a system unit: sudo systemctl {verb} {service}"));
    }
    let ok = systemctl(scope).args([verb, "--no-block", service]).status().is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err(format!("systemctl --user {verb} {service} failed"))
    }
}

#[cfg(not(target_os = "linux"))]
pub fn control(_: Scope, _: &str, service: &str) -> Result<(), String> {
    Err(format!("{service}: timers are watched on Linux only"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = "Id=plos.timer\nDescription=copy backups daily\nLoadState=loaded\nUnit=plos.service\nNextElapseUSecRealtime=@1795514819\n\nId=gone.timer\nDescription=gone.timer\nLoadState=not-found\nUnit=\nNextElapseUSecRealtime=\n";

    #[test]
    fn reads_show_blocks() {
        let t = parse_show(SHOW);
        assert_eq!(t.len(), 2);
        assert_eq!(t["gone.timer"]["LoadState"], "not-found");
        let svc = parse_show("Id=plos.service\nActiveState=inactive\nSubState=dead\nResult=exit-code\nExecMainStatus=2\nExecMainStartTimestamp=@1795428760\nExecMainExitTimestamp=@1795428762\n");
        let s = state("plos", Scope::User, &t["plos.timer"], svc.get("plos.service"));
        assert_eq!(s.service, "plos.service");
        assert_eq!(s.next, Some(1795514819.0));
        assert!(!s.active);
        assert_eq!(s.exited, Some(1795428762.0));
        assert_eq!(s.outcome(), Some((Outcome::Failed, "exit 2".into())));
    }

    #[test]
    fn running_and_never_run() {
        let t = parse_show(SHOW);
        let running = parse_show("Id=plos.service\nActiveState=activating\nSubState=start\nResult=success\nExecMainStartTimestamp=@1795428760\nExecMainExitTimestamp=@1795000000\n");
        let s = state("plos", Scope::User, &t["plos.timer"], running.get("plos.service"));
        assert!(s.active);
        assert_eq!(s.exited, None, "a running service has not finished");
        // Since boot the scrub has not run: no timestamps, no outcome.
        let fresh = parse_show(
            "Id=plos.service\nActiveState=inactive\nSubState=dead\nResult=success\nExecMainStartTimestamp=\nExecMainExitTimestamp=\n",
        );
        let s = state("plos", Scope::System, &t["plos.timer"], fresh.get("plos.service"));
        assert_eq!(s.outcome(), None);
        // RemainAfterExit: active (exited) is finished.
        let kept = parse_show("Id=plos.service\nActiveState=active\nSubState=exited\nResult=success\nExecMainExitTimestamp=@1795428762\n");
        let s = state("plos", Scope::User, &t["plos.timer"], kept.get("plos.service"));
        assert!(!s.active);
        assert_eq!(s.outcome(), Some((Outcome::Ok, String::new())));
    }

    #[test]
    fn ids_from_units() {
        let c = |unit: &str| yard_core::config::TimerConfig { unit: unit.into(), id: None, label: None, description: None, tags: vec![] };
        assert_eq!(c("btrfs-scrub@-.timer").id(), "btrfs-scrub");
        assert_eq!(c("btrfs-scrub@data").id(), "btrfs-scrub-data");
        assert_eq!(c("plos-origins-pull-backups").name(), "plos-origins-pull-backups");
    }
}
