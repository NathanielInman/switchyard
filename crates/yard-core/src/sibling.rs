//! Siblings: Switchyard and Roundhouse know about each other when both run,
//! and neither needs the other. A sibling that is missing, stopped or too old
//! to answer unlocks nothing and costs at most a 300 ms connect deadline.
//!
//! Switchyard reads Roundhouse's lamp from its socket, picking only the fields
//! it uses, so either can be upgraded first. It never links Roundhouse's
//! crates.

use crate::model::{Anchor, MachineReason, MachineView};
use serde_json::Value;

/// Pulls the lamp out of a Roundhouse snapshot reply.
pub fn parse_roundhouse(reply: &str) -> Option<MachineView> {
    let v: Value = serde_json::from_str(reply).ok()?;
    if v.get("ok").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let lamp = v.get("snapshot")?.get("lamp")?;
    let level = lamp.get("level").and_then(Value::as_str).unwrap_or("calm").to_string();
    let reasons = lamp
        .get("reasons")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(MachineReason { level: r.get("level")?.as_str()?.to_string(), text: r.get("text")?.as_str()?.to_string() })
                })
                .collect()
        })
        .unwrap_or_default();
    let sends_hold = lamp.get("hold").is_some();
    let hold = lamp.get("hold").and_then(Value::as_str).map(String::from);
    Some(MachineView { level, reasons, hold, sends_hold })
}

/// Sends one request line to Roundhouse and returns its reply line, or
/// `None` when Roundhouse is not running or does not answer in 300 ms.
#[cfg(unix)]
fn ask_roundhouse(line: &str) -> Option<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;
    let path = crate::paths::roundhouse_socket();
    if !path.exists() {
        return None;
    }
    let mut s = std::os::unix::net::UnixStream::connect(&path).ok()?;
    let t = Some(Duration::from_millis(300));
    s.set_read_timeout(t).ok()?;
    s.set_write_timeout(t).ok()?;
    s.write_all(line.as_bytes()).ok()?;
    s.write_all(b"\n").ok()?;
    let mut out = String::new();
    BufReader::new(s).read_line(&mut out).ok()?;
    Some(out)
}

/// Roundhouse does not run on macOS or Windows.
#[cfg(not(unix))]
fn ask_roundhouse(_line: &str) -> Option<String> {
    None
}

/// Roundhouse's view of the machine, or `None` when it is not running.
pub fn roundhouse() -> Option<MachineView> {
    parse_roundhouse(&ask_roundhouse(r#"{"cmd":"snapshot"}"#)?)
}

/// Asks Roundhouse to close its drawer, so only one panel is open at a time.
/// Does nothing when Roundhouse is not running.
pub fn roundhouse_hide() {
    let _ = ask_roundhouse(r#"{"cmd":"hide"}"#);
}

/// Asks Roundhouse to open its drawer from the same icon the board opened
/// from. Returns false when Roundhouse is not there to ask.
pub fn roundhouse_show(anchor: Option<&Anchor>) -> bool {
    let req = serde_json::json!({ "cmd": "show", "anchor": anchor });
    ask_roundhouse(&req.to_string()).is_some_and(|r| r.contains("\"ok\":true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lamp() {
        let reply = r#"{"ok":true,"snapshot":{"host":{"name":"x"},"lamp":{"level":"act","reasons":[
            {"level":"heads-up","text":"CPU is at 88°C","tab":"machine"},
            {"level":"act","text":"/data is 96% full","tab":"storage","future":1}]},"now":1}}"#;
        let m = parse_roundhouse(reply).unwrap();
        assert_eq!(m.level, "act");
        assert_eq!(m.reasons.len(), 2);
        assert_eq!(m.hold_reason(), Some("/data is 96% full"));
    }

    #[test]
    fn follows_the_hold_roundhouse_sends() {
        // A CPU at its limit is act, but heat is not a reason to hold.
        let reply = r#"{"ok":true,"snapshot":{"lamp":{"level":"act","reasons":[
            {"level":"act","text":"CPU is slowing itself down for heat","tab":"machine","key":"cpu.throttle"}],"hold":null}}}"#;
        assert_eq!(parse_roundhouse(reply).unwrap().hold_reason(), None);
        // A recording session holds while the lamp is calm.
        let reply = r#"{"ok":true,"snapshot":{"lamp":{"level":"calm","reasons":[],"hold":"a recording session is on"}}}"#;
        assert_eq!(parse_roundhouse(reply).unwrap().hold_reason(), Some("a recording session is on"));
    }

    #[test]
    fn calm_and_broken_replies() {
        let m = parse_roundhouse(r#"{"ok":true,"snapshot":{"lamp":{"level":"calm","reasons":[]}}}"#).unwrap();
        assert_eq!(m.hold_reason(), None);
        assert!(parse_roundhouse(r#"{"ok":false,"error":"x"}"#).is_none());
        assert!(parse_roundhouse(r#"{"ok":true,"snapshot":{}}"#).is_none());
        assert!(parse_roundhouse("not json").is_none());
    }
}
