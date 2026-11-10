//! What the daemon tells its clients: the snapshot of every task.

use crate::config::{BoardConfig, TabConfig};
use crate::errors::ErrorHit;
use crate::manifest::Confirm;
use crate::Problem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Idle,
    Queued,
    Waiting,
    Building,
    Running,
    Ok,
    Failed,
    Seen,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Queued => "queued",
            Phase::Waiting => "waiting",
            Phase::Building => "building",
            Phase::Running => "running",
            Phase::Ok => "ok",
            Phase::Failed => "failed",
            Phase::Seen => "seen",
        }
    }

    pub fn active(self) -> bool {
        matches!(self, Phase::Queued | Phase::Waiting | Phase::Building | Phase::Running)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepInfo {
    /// 1-based.
    pub index: usize,
    pub total: usize,
    pub name: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastRun {
    pub outcome: Outcome,
    /// Unix seconds when it ended.
    pub at: f64,
    /// Seconds the build phase took (or the whole run for run-only tasks).
    pub duration: f64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamUse {
    pub value: String,
    pub at: f64,
    pub starred: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamView {
    pub name: String,
    pub default: String,
    pub choices: Vec<String>,
    pub history: Vec<ParamUse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariantView {
    pub name: String,
    pub label: String,
    pub confirm: Confirm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GitInfo {
    pub branch: String,
    pub dirty: u32,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub tags: Vec<String>,
    pub pin: Option<i64>,
    pub heavy: bool,
    pub confirm: Confirm,
    /// On the Shelf (manifest `shelf = true` or moved there by hand).
    pub shelf: bool,
    /// Unpinned and unused for `shelf.stale_days`.
    pub stale: bool,
    pub phase: Phase,
    /// Unix seconds when the current phase began.
    pub since: f64,
    pub step: Option<StepInfo>,
    pub message: Option<String>,
    pub error: Option<ErrorHit>,
    pub mem: Option<u64>,
    pub cpu: Option<f32>,
    pub mem_history: Vec<u64>,
    pub last: Option<LastRun>,
    /// Median of recent successful build durations, in seconds.
    pub typical: Option<f64>,
    pub params: Vec<ParamView>,
    pub variants: Vec<VariantView>,
    pub has_clean: bool,
    pub has_run: bool,
    pub log: String,
    pub dir: String,
    pub git: Option<GitInfo>,
    pub last_used: Option<f64>,
    pub frecency: f64,
    pub queue_pos: Option<usize>,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MainlineView {
    pub current: Option<String>,
    pub queue: Vec<String>,
    pub waiting_memory: bool,
    pub mem_available: u64,
    pub mem_total: u64,
    pub min_free: u64,
    pub cap: u64,
    /// Memory used by the current heavy build's unit.
    pub current_mem: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Aggregate {
    pub phase: Phase,
    pub failed: Vec<String>,
    pub building: Vec<String>,
    pub queued: Vec<String>,
    pub running: Vec<String>,
    pub ok: Vec<String>,
}

impl Aggregate {
    /// The most urgent state wins: failed, building, queued, running, ok, idle.
    pub fn from_tasks(tasks: &[TaskView]) -> Aggregate {
        let mut a = Aggregate::default();
        for t in tasks {
            match t.phase {
                Phase::Failed => a.failed.push(t.label.clone()),
                Phase::Building => a.building.push(t.label.clone()),
                Phase::Queued | Phase::Waiting => a.queued.push(t.label.clone()),
                Phase::Running => a.running.push(t.label.clone()),
                Phase::Ok => a.ok.push(t.label.clone()),
                _ => {}
            }
        }
        a.phase = if !a.failed.is_empty() {
            Phase::Failed
        } else if !a.building.is_empty() {
            Phase::Building
        } else if !a.queued.is_empty() {
            Phase::Queued
        } else if !a.running.is_empty() {
            Phase::Running
        } else if !a.ok.is_empty() {
            Phase::Ok
        } else {
            Phase::Idle
        };
        a
    }

    pub fn tooltip(&self) -> String {
        let mut lines = vec![];
        let mut add = |label: &str, v: &Vec<String>| {
            if !v.is_empty() {
                lines.push(format!("{label}: {}", v.join(", ")));
            }
        };
        add("failed", &self.failed);
        add("building", &self.building);
        add("queued", &self.queued);
        add("running", &self.running);
        add("ok", &self.ok);
        if lines.is_empty() {
            "Switchyard: all quiet".into()
        } else {
            lines.join("\n")
        }
    }

    pub fn count(&self) -> usize {
        match self.phase {
            Phase::Failed => self.failed.len(),
            Phase::Building => self.building.len(),
            Phase::Queued => self.queued.len(),
            Phase::Running => self.running.len(),
            Phase::Ok => self.ok.len(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub tasks: Vec<TaskView>,
    pub mainline: MainlineView,
    pub aggregate: Aggregate,
    pub tabs: Vec<TabConfig>,
    pub problems: Vec<Problem>,
    pub board: BoardConfig,
    pub has_editor: bool,
    pub has_terminal: bool,
    /// Bumps whenever a run finishes, so a board knows to refresh history.
    #[serde(default)]
    pub history_rev: u64,
    /// The machine as a running Roundhouse sees it; `None` when Roundhouse
    /// is not running.
    #[serde(default)]
    pub machine: Option<MachineView>,
    /// Unix seconds on the daemon's clock, so clients can correct for skew.
    pub now: f64,
}

/// Roundhouse's lamp: `calm`, `heads-up` or `act`, with its reasons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MachineView {
    pub level: String,
    pub reasons: Vec<MachineReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MachineReason {
    pub level: String,
    pub text: String,
}

impl MachineView {
    /// The first reason Roundhouse says needs acting on.
    pub fn act_reason(&self) -> Option<&str> {
        if self.level != "act" {
            return None;
        }
        self.reasons.iter().find(|r| r.level == "act").map(|r| r.text.as_str()).or(Some("the machine needs attention"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShelfItem {
    pub name: String,
    pub desc: String,
    pub cmd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ShelfData {
    pub tools: Vec<ShelfItem>,
    pub binds: Vec<ShelfItem>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

/// Where the board should open, in monitor-local logical pixels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Anchor {
    pub monitor: Option<String>,
    pub edge: Edge,
    /// Click position relative to the monitor's top-left corner.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Space other surfaces reserve: left, top, right, bottom.
    pub reserved: [f64; 4],
    pub center: bool,
    /// Coordinates are global physical pixels (tray icons) rather than
    /// monitor-local logical ones (Hyprland).
    #[serde(default)]
    pub physical: bool,
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// "4:12", "1:02:09".
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "just now", "6m ago", "2h ago", "yesterday", "3d ago".
pub fn ago(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 45.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{}m ago", (s / 60.0).round().max(1.0) as u64)
    } else if s < 86400.0 {
        format!("{}h ago", (s / 3600.0).round() as u64)
    } else if s < 2.0 * 86400.0 {
        "yesterday".into()
    } else {
        format!("{}d ago", (s / 86400.0) as u64)
    }
}

/// "1m 52s", "48s", "1h 3m".
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s / 60) % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(clock(252.0), "4:12");
        assert_eq!(clock(3729.0), "1:02:09");
        assert_eq!(ago(10.0), "just now");
        assert_eq!(ago(360.0), "6m ago");
        assert_eq!(ago(7200.0), "2h ago");
        assert_eq!(ago(90000.0), "yesterday");
        assert_eq!(ago(3.0 * 86400.0), "3d ago");
        assert_eq!(duration(112.0), "1m 52s");
        assert_eq!(duration(48.0), "48s");
    }
}
