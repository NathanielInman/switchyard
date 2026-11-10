//! The run history on disk: one JSON line per finished run, plus a copy of
//! each run's log so any entry can still open its own output.

use std::io::Write;
use std::path::{Path, PathBuf};
use yard_core::history::RunRecord;
use yard_core::paths;

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const LOGS_PER_TASK: usize = 20;
const LOG_COPY_BYTES: u64 = 2 * 1024 * 1024;

pub fn dir() -> PathBuf {
    paths::state_dir().join("history")
}

fn file() -> PathBuf {
    dir().join("runs.jsonl")
}

pub fn append(rec: &RunRecord) {
    let _ = std::fs::create_dir_all(dir());
    let path = file();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(rec) {
            let _ = writeln!(f, "{line}");
        }
    }
    // Keep the file bounded: past the limit, drop the older half.
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let lines: Vec<&str> = text.lines().collect();
            let keep = &lines[lines.len() / 2..];
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, keep.join("\n") + "\n").is_ok() {
                let _ = std::fs::rename(tmp, &path);
            }
        }
    }
}

/// The newest `limit` records, oldest first, optionally for one task.
pub fn read(task: Option<&str>, limit: usize) -> Vec<RunRecord> {
    let Ok(text) = std::fs::read_to_string(file()) else { return vec![] };
    let mut out: Vec<RunRecord> = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<RunRecord>(l).ok())
        .filter(|r| task.is_none_or(|t| r.task == t))
        .take(limit)
        .collect();
    out.reverse();
    out
}

/// Copies the end of a task's log next to the history and returns its path.
/// Keeps the newest few per task.
pub fn archive_log(log: &Path, task: &str, run: u64) -> Option<String> {
    let src = std::fs::File::open(log).ok()?;
    let dir = dir().join("logs").join(yard_core::manifest::unit_safe(task));
    std::fs::create_dir_all(&dir).ok()?;
    let dst = dir.join(format!("{run}.log"));
    let len = src.metadata().ok()?.len();
    let mut src = src;
    if len > LOG_COPY_BYTES {
        use std::io::{Seek, SeekFrom};
        src.seek(SeekFrom::Start(len - LOG_COPY_BYTES)).ok()?;
    }
    let mut out = std::fs::File::create(&dst).ok()?;
    std::io::copy(&mut src, &mut out).ok()?;
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).collect();
    logs.sort_by_key(|p| p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    while logs.len() > LOGS_PER_TASK {
        let _ = std::fs::remove_file(logs.remove(0));
    }
    Some(dst.to_string_lossy().to_string())
}
