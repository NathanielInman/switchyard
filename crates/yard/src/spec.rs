//! The file the daemon hands to the runner inside a unit.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use yard_core::manifest::{Retry, Step, Watch};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpec {
    pub run: u64,
    pub task: String,
    pub label: String,
    pub steps: Vec<Step>,
    pub run_cmd: Option<String>,
    pub log: PathBuf,
    pub retry: Option<Retry>,
    pub watch: Vec<Watch>,
    pub scratch: PathBuf,
}

impl RunSpec {
    pub fn exit_file(&self, phase: &str) -> PathBuf {
        exit_file(self.run, phase)
    }
}

pub fn exit_file(run: u64, phase: &str) -> PathBuf {
    yard_core::paths::runs_dir().join(format!("{run}.{phase}.exit"))
}

pub fn spec_file(run: u64) -> PathBuf {
    yard_core::paths::runs_dir().join(format!("{run}.json"))
}
