//! Siblings: Switchyard and Roundhouse know about each other when both run,
//! and neither needs the other. A sibling that is missing, stopped or too old
//! to answer unlocks nothing and costs at most a 300 ms connect deadline.
//!
//! Switchyard reads Roundhouse's lamp from its socket, picking only the fields
//! it uses, so either can be upgraded first. It never links Roundhouse's
//! crates.

use crate::model::{MachineReason, MachineView};
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
                    Some(MachineReason {
                        level: r.get("level")?.as_str()?.to_string(),
                        text: r.get("text")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(MachineView { level, reasons })
}

/// Roundhouse's view of the machine, or `None` when it is not running.
#[cfg(unix)]
pub fn roundhouse() -> Option<MachineView> {
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
    s.write_all(b"{\"cmd\":\"snapshot\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    parse_roundhouse(&line)
}

/// Roundhouse does not run on Windows yet.
#[cfg(not(unix))]
pub fn roundhouse() -> Option<MachineView> {
    None
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
        assert_eq!(m.act_reason(), Some("/data is 96% full"));
    }

    #[test]
    fn calm_and_broken_replies() {
        let m = parse_roundhouse(r#"{"ok":true,"snapshot":{"lamp":{"level":"calm","reasons":[]}}}"#).unwrap();
        assert_eq!(m.act_reason(), None);
        assert!(parse_roundhouse(r#"{"ok":false,"error":"x"}"#).is_none());
        assert!(parse_roundhouse(r#"{"ok":true,"snapshot":{}}"#).is_none());
        assert!(parse_roundhouse("not json").is_none());
    }
}
