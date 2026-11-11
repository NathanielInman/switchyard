//! The daemon socket: newline-delimited JSON.

use crate::model::{Anchor, ShelfData, Snapshot};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SubKind {
    Board,
    Bar,
    Snapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AckPhase {
    /// The board opened: failed becomes seen.
    Open,
    /// The board closed: seen becomes idle.
    Close,
    /// Clear everything at once (bar right-click, `yard ack`).
    All,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpenWhat {
    Editor,
    Terminal,
    Log,
    Tool,
    Man,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Snapshot,
    Subscribe {
        kind: SubKind,
    },
    Run {
        id: String,
        #[serde(default)]
        params: BTreeMap<String, String>,
        #[serde(default)]
        variant: Option<String>,
        #[serde(default)]
        clean: bool,
    },
    Stop {
        id: String,
    },
    Ack {
        phase: AckPhase,
        #[serde(default)]
        id: Option<String>,
    },
    Toggle {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Show {
        #[serde(default)]
        anchor: Option<Anchor>,
    },
    Hide,
    BoardVisible {
        visible: bool,
    },
    Shelf,
    /// Run history: records, the day-by-day episode feed and the week.
    History {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Reload,
    Star {
        id: String,
        param: String,
        value: String,
        starred: bool,
    },
    Shelve {
        id: String,
        shelved: bool,
    },
    Touch {
        id: String,
    },
    /// Opens something for the user: an editor, a terminal, a log or a tool.
    Open {
        what: OpenWhat,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        line: Option<u32>,
        #[serde(default)]
        tool: Option<String>,
    },
    /// Sent by the runner inside a task unit.
    Progress {
        run: u64,
        index: usize,
        total: usize,
        name: String,
        attempt: u32,
    },
    /// Sent by the runner when a phase ends.
    Finished {
        run: u64,
        phase: String,
        code: i32,
        #[serde(default)]
        oom: bool,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Box<Snapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelf: Option<ShelfData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Box<HistoryData>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HistoryData {
    /// The daemon's offset from UTC in seconds, so clients agree on "today".
    #[serde(default)]
    pub offset: i64,
    pub records: Vec<crate::history::RunRecord>,
    pub days: Vec<crate::history::Day>,
    pub week: crate::history::WeekStats,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply { ok: true, ..Default::default() }
    }
    pub fn msg(m: impl Into<String>) -> Reply {
        Reply { ok: true, message: Some(m.into()), ..Default::default() }
    }
    pub fn err(e: impl Into<String>) -> Reply {
        Reply { ok: false, error: Some(e.into()), ..Default::default() }
    }
}

/// The waybar module line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BarLine {
    pub text: String,
    pub class: String,
    pub tooltip: String,
    pub alt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Snapshot(Box<Snapshot>),
    Show { anchor: Option<Anchor> },
    Hide,
    Toggle { anchor: Option<Anchor> },
    Bar(BarLine),
}

pub use crate::transport::{connect, Stream};

/// Sends one request and waits for its reply.
pub fn request(req: &Request) -> Result<Reply> {
    let mut s = connect()?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut r = BufReader::new(s);
    let mut buf = String::new();
    r.read_line(&mut buf)?;
    if buf.is_empty() {
        return Err(anyhow!("the daemon closed the connection without replying"));
    }
    Ok(serde_json::from_str(&buf)?)
}

/// Opens a subscription and returns a reader of events.
pub fn subscribe(kind: SubKind) -> Result<impl Iterator<Item = Result<Event>>> {
    let mut s = connect()?;
    let mut line = serde_json::to_string(&Request::Subscribe { kind })?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let r = BufReader::new(s);
    Ok(r.lines().map(|l| {
        let l = l?;
        Ok(serde_json::from_str::<Event>(&l)?)
    }))
}

/// Opens a subscription and returns the raw lines (the bar stream is plain
/// waybar JSON, not tagged events).
pub fn subscribe_raw(kind: SubKind) -> Result<impl Iterator<Item = std::io::Result<String>>> {
    let mut s = connect()?;
    let mut line = serde_json::to_string(&Request::Subscribe { kind })?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    Ok(BufReader::new(s).lines())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shapes() {
        let r: Request = serde_json::from_str(r#"{"cmd":"run","id":"x","params":{"seed":"1"}}"#).unwrap();
        assert_eq!(r, Request::Run { id: "x".into(), params: [("seed".into(), "1".into())].into(), variant: None, clean: false });
        let s = serde_json::to_string(&Request::Ack { phase: AckPhase::Open, id: None }).unwrap();
        assert_eq!(s, r#"{"cmd":"ack","phase":"open","id":null}"#);
        let e = serde_json::to_string(&Event::Hide).unwrap();
        assert_eq!(e, r#"{"event":"hide"}"#);
        let b = Event::Bar(BarLine { text: "●".into(), class: "idle".into(), tooltip: "t".into(), alt: "idle".into() });
        let back: Event = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back, b);
    }
}
