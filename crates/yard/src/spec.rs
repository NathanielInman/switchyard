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
    /// Program and flag that run a command string, e.g. ["bash", "-c"].
    #[serde(default)]
    pub shell: Vec<String>,
}

impl RunSpec {
    pub fn shell_command(&self, cmd: &str) -> std::process::Command {
        let shell = if self.shell.is_empty() { crate::supervise::default_shell() } else { self.shell.clone() };
        let mut c = std::process::Command::new(&shell[0]);
        c.args(&shell[1..]);
        #[cfg(windows)]
        {
            // cmd.exe parses its own command line; hand it over untouched.
            use std::os::windows::process::CommandExt;
            let prog = std::path::Path::new(&shell[0]).file_stem().map(|s| s.to_string_lossy().to_lowercase());
            if prog.as_deref() == Some("cmd") {
                c.raw_arg(cmd);
                return c;
            }
        }
        c.arg(cmd);
        c
    }
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
