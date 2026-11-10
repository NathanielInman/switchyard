//! What Switchyard remembers between runs: results, durations, parameter
//! history and your Shelf choices. Stored as JSON in the state directory.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yard_core::model::{LastRun, ParamUse};
use yard_core::paths;

const KEEP_RUNS: usize = 50;
const KEEP_DURATIONS: usize = 10;
const HALF_LIFE_DAYS: f64 = 7.0;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskHist {
    #[serde(default)]
    pub runs: Vec<f64>,
    #[serde(default)]
    pub durations: Vec<f64>,
    #[serde(default)]
    pub last: Option<LastRun>,
    #[serde(default)]
    pub params: BTreeMap<String, Vec<ParamUse>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct History {
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskHist>,
    /// Explicit Shelf placement chosen on the board; overrides the manifest.
    #[serde(default)]
    pub shelved: BTreeMap<String, bool>,
}

impl History {
    pub fn load() -> History {
        std::fs::read_to_string(paths::state_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = paths::state_file();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn task(&mut self, id: &str) -> &mut TaskHist {
        self.tasks.entry(id.to_string()).or_default()
    }

    pub fn record_start(&mut self, id: &str, at: f64) {
        let t = self.task(id);
        t.runs.push(at);
        if t.runs.len() > KEEP_RUNS {
            let drop = t.runs.len() - KEEP_RUNS;
            t.runs.drain(..drop);
        }
    }

    pub fn record_duration(&mut self, id: &str, secs: f64) {
        let t = self.task(id);
        t.durations.push(secs);
        if t.durations.len() > KEEP_DURATIONS {
            let drop = t.durations.len() - KEEP_DURATIONS;
            t.durations.drain(..drop);
        }
    }

    /// Adds a value to a parameter's history, newest first, keeping starred
    /// values no matter how old they are.
    pub fn record_param(&mut self, id: &str, param: &str, value: &str, at: f64, keep: usize) {
        let list = self.task(id).params.entry(param.to_string()).or_default();
        let starred = list.iter().any(|u| u.value == value && u.starred);
        list.retain(|u| u.value != value);
        list.insert(0, ParamUse { value: value.to_string(), at, starred });
        let mut unstarred_seen = 0;
        list.retain(|u| {
            if u.starred {
                true
            } else {
                unstarred_seen += 1;
                unstarred_seen <= keep.max(1)
            }
        });
    }

    pub fn star(&mut self, id: &str, param: &str, value: &str, starred: bool, at: f64) {
        let list = self.task(id).params.entry(param.to_string()).or_default();
        match list.iter_mut().find(|u| u.value == value) {
            Some(u) => u.starred = starred,
            None => list.insert(0, ParamUse { value: value.to_string(), at, starred }),
        }
    }

    pub fn frecency(&self, id: &str, now: f64) -> f64 {
        self.tasks
            .get(id)
            .map(|t| {
                t.runs
                    .iter()
                    .map(|r| 0.5f64.powf(((now - r) / 86400.0).max(0.0) / HALF_LIFE_DAYS))
                    .sum()
            })
            .unwrap_or(0.0)
    }

    pub fn last_used(&self, id: &str) -> Option<f64> {
        self.tasks.get(id).and_then(|t| t.runs.last().copied())
    }

    pub fn typical(&self, id: &str) -> Option<f64> {
        let t = self.tasks.get(id)?;
        if t.durations.is_empty() {
            return None;
        }
        let mut d = t.durations.clone();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(d[d.len() / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_keep_stars() {
        let mut h = History::default();
        for i in 0..5 {
            h.record_param("t", "seed", &i.to_string(), i as f64, 3);
        }
        h.star("t", "seed", "3", true, 9.0);
        h.record_param("t", "seed", "9", 10.0, 2);
        let vals: Vec<_> = h.tasks["t"].params["seed"].iter().map(|u| u.value.clone()).collect();
        assert_eq!(vals, vec!["9", "4", "3"]);
        h.record_param("t", "seed", "3", 11.0, 2);
        assert!(h.tasks["t"].params["seed"][0].starred);
    }

    #[test]
    fn frecency_decays() {
        let mut h = History::default();
        let now = 100.0 * 86400.0;
        h.record_start("a", now);
        h.record_start("b", now - 7.0 * 86400.0);
        assert!((h.frecency("a", now) - 1.0).abs() < 1e-9);
        assert!((h.frecency("b", now) - 0.5).abs() < 1e-9);
        assert_eq!(h.frecency("none", now), 0.0);
    }

    #[test]
    fn typical_is_median() {
        let mut h = History::default();
        for d in [10.0, 50.0, 20.0] {
            h.record_duration("a", d);
        }
        assert_eq!(h.typical("a"), Some(20.0));
    }
}
