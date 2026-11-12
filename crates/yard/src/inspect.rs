//! Reads repos for the Inspection tab: cheap status on a timer, history when
//! HEAD moves, and the two clean-up actions. Everything here runs on worker
//! threads; the event loop only receives the results.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use yard_core::inspect::{self, ActionKind, InspectionConfig, Output, Signals, Status, Worktree};

/// A git command that cannot run code from the repo's config and takes no
/// optional locks, so a scan never gets in the way of the user's own git.
fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false", "-c", "color.ui=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    c
}

fn out(mut c: Command) -> Option<String> {
    let o = c.output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
}

fn ok(mut c: Command) -> bool {
    c.stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn times(text: &str) -> Vec<f64> {
    text.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// Lowers this thread's priority, so git and the size walk only get CPU
/// nobody else wants. Children inherit it. Linux treats nice as per thread;
/// elsewhere this would lower the whole daemon, so it is Linux only.
pub fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// The repo rows: every repo under `roots`, with worktrees folded into their
/// repo. A worktree whose repo is not under `roots` is a row of its own.
pub fn repo_dirs(cfg: &InspectionConfig) -> Vec<PathBuf> {
    let found = cfg.candidate_dirs();
    // Git records a worktree's repo by its real path (macOS /private/var,
    // Windows long names), so compare real paths on both sides.
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let set: HashSet<PathBuf> = found.iter().map(|p| real(p)).collect();
    found
        .into_iter()
        .filter(|d| {
            let g = d.join(".git");
            if g.is_dir() {
                return true;
            }
            // A worktree's .git is a file: "gitdir: <repo>/.git/worktrees/<name>".
            let main = std::fs::read_to_string(&g).ok().and_then(|t| {
                let p = PathBuf::from(t.trim().strip_prefix("gitdir:")?.trim());
                let p = if p.is_relative() { d.join(p) } else { p };
                p.parent()?.parent()?.parent().map(Path::to_path_buf)
            });
            !main.is_some_and(|m| set.contains(&real(&m)))
        })
        .collect()
}

fn default_branch(dir: &Path) -> Option<String> {
    let mut c = git(dir);
    c.args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]);
    if let Some(r) = out(c).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Some(r);
    }
    for r in ["refs/remotes/origin/main", "refs/remotes/origin/master", "refs/heads/main", "refs/heads/master"] {
        let mut c = git(dir);
        c.args(["rev-parse", "--verify", "--quiet", r]);
        if ok(c) {
            return Some(r.trim_start_matches("refs/remotes/").trim_start_matches("refs/heads/").to_string());
        }
    }
    None
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    let mut c = git(dir);
    c.args(["merge-base", "--is-ancestor", a, b]);
    ok(c)
}

fn dirty_count(dir: &Path) -> u32 {
    let mut c = git(dir);
    c.args(["status", "--porcelain=v2", "--untracked-files=normal"]);
    out(c).map(|t| t.lines().filter(|l| !l.starts_with('#')).count() as u32).unwrap_or(0)
}

fn last_commit(dir: &Path) -> Option<f64> {
    let mut c = git(dir);
    c.args(["log", "-1", "--format=%ct", "HEAD"]);
    out(c).and_then(|t| t.trim().parse().ok())
}

fn mtime(p: &Path) -> Option<f64> {
    let m = std::fs::symlink_metadata(p).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

/// Reads a repo's state. `None` when git cannot read it at all.
pub fn status(dir: &Path) -> Option<Status> {
    let mut c = git(dir);
    c.args(["status", "--porcelain=v2", "--branch", "--untracked-files=normal"]);
    let lines = inspect::parse_status(&out(c)?);
    let dirty_oldest = lines
        .paths
        .iter()
        .take(400)
        .filter_map(|p| mtime(&dir.join(p.trim_end_matches('/'))))
        .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.min(t))));

    let mut c = git(dir);
    c.args(["config", "--get", "remote.origin.url"]);
    let remote_url = out(c).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut c = git(dir);
    c.arg("remote");
    let has_remotes = out(c).is_some_and(|s| !s.trim().is_empty());

    let mut c = git(dir);
    c.args(["stash", "list", "--format=%ct"]);
    let stashes = out(c).map(|t| times(&t)).unwrap_or_default();

    let (mut unpushed, mut unpushed_oldest) = (0, None);
    if has_remotes {
        let mut c = git(dir);
        c.args(["log", "--branches", "--not", "--remotes", "--format=%ct"]);
        let t = out(c).map(|t| times(&t)).unwrap_or_default();
        unpushed = t.len() as u32;
        unpushed_oldest = t.iter().copied().fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.min(t))));
    }

    let default = default_branch(dir);
    let mut worktrees = vec![];
    let mut c = git(dir);
    c.args(["worktree", "list", "--porcelain"]);
    let canon = dir.canonicalize().unwrap_or(dir.to_path_buf());
    for w in out(c).map(|t| inspect::parse_worktrees(&t)).unwrap_or_default() {
        let p = PathBuf::from(&w.path);
        if w.bare || p.canonicalize().unwrap_or(p.clone()) == canon || !p.exists() {
            continue;
        }
        let merged = match (&w.branch, &default) {
            (Some(b), Some(d)) => is_ancestor(dir, &format!("refs/heads/{b}"), d),
            _ => false,
        };
        worktrees.push(Worktree {
            path: w.path.clone(),
            branch: w.branch.clone(),
            dirty: dirty_count(&p),
            merged,
            last_commit: last_commit(&p),
        });
    }

    let mut merged_branches = vec![];
    if let Some(d) = &default {
        let short = d.rsplit('/').next().unwrap_or(d).to_string();
        let mut c = git(dir);
        c.args(["for-each-ref", "--merged", d, "--format=%(refname:short)%09%(worktreepath)", "refs/heads"]);
        for l in out(c).unwrap_or_default().lines() {
            let mut f = l.splitn(2, '\t');
            let (Some(name), wt) = (f.next(), f.next().unwrap_or("")) else { continue };
            if name.is_empty() || name == short || !wt.trim().is_empty() || name == lines.branch {
                continue;
            }
            merged_branches.push(name.to_string());
        }
    }

    Some(Status {
        head: lines.head,
        branch: lines.branch,
        upstream: lines.upstream,
        ahead: lines.ahead,
        behind: lines.behind,
        dirty: lines.paths.len() as u32,
        dirty_oldest,
        stashes,
        remote_url,
        has_remotes,
        unpushed,
        unpushed_oldest,
        last_commit: last_commit(dir),
        default_branch: default,
        worktrees,
        merged_branches,
    })
}

/// Disk use of a tree, without following links.
fn tree_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    total += m.blocks() * 512;
                }
                #[cfg(not(unix))]
                {
                    total += m.len();
                }
            }
        }
    }
    total
}

const OUTPUT_DIRS: &[&str] = &["target", "node_modules", ".godot"];

fn outputs(dir: &Path, worktrees: &[Worktree]) -> Vec<Output> {
    let mut out = vec![];
    let mut roots = vec![(dir.to_path_buf(), String::new())];
    for w in worktrees {
        let p = PathBuf::from(&w.path);
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        roots.push((p, format!("{name}/")));
    }
    for (root, prefix) in roots {
        for d in OUTPUT_DIRS {
            let p = root.join(d);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
                let bytes = tree_size(&p);
                if bytes > 0 {
                    out.push(Output { dir: format!("{prefix}{d}"), bytes });
                }
            }
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.bytes));
    out
}

/// Reads the history signals for a repo at `head`.
pub fn history(dir: &Path, head: &str, status: &Status, cfg: &InspectionConfig, now: f64) -> Signals {
    // Fourteen months for the rhythm, or the window if it is longer.
    let since = now - cfg.window_secs().max(430.0 * 86400.0);
    let mut c = git(dir);
    c.args([
        "log",
        "--no-merges",
        &format!("--since=@{}", since as i64),
        &format!("--format={}", inspect::LOG_FORMAT),
        "--name-only",
        "HEAD",
    ]);
    let log = out(c).map(|t| inspect::parse_log(&t)).unwrap_or_default();
    let mut c = git(dir);
    c.args(["log", "--no-merges", &format!("--format={}", inspect::PEOPLE_FORMAT), "HEAD"]);
    let people = out(c).map(|t| inspect::parse_people(&t)).unwrap_or_default();
    let mut s = inspect::signals(head, &log, &people, cfg, now);
    s.output = outputs(dir, &status.worktrees);
    s
}

/// Runs a clean-up action after checking again that it is still safe.
pub fn act(dir: &Path, kind: ActionKind) -> Result<String, String> {
    let s = status(dir).ok_or_else(|| format!("git cannot read {}", dir.display()))?;
    match kind {
        ActionKind::RemoveWorktrees => {
            let targets: Vec<&Worktree> = s.worktrees.iter().filter(|w| w.merged && w.dirty == 0).collect();
            if targets.is_empty() {
                return Err("No merged, clean worktree left to remove".into());
            }
            let mut done = 0;
            let mut failed = vec![];
            for w in targets {
                let mut c = git(dir);
                c.args(["worktree", "remove", &w.path]);
                if ok(c) {
                    done += 1;
                } else {
                    failed.push(Path::new(&w.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                }
            }
            let msg = format!("Removed {done} worktree{}", if done == 1 { "" } else { "s" });
            if failed.is_empty() {
                Ok(msg)
            } else {
                Err(format!("{msg}; git refused {}", failed.join(", ")))
            }
        }
        ActionKind::DeleteBranches => {
            if s.merged_branches.is_empty() {
                return Err("No merged branch left to delete".into());
            }
            let mut done = 0;
            let mut failed = vec![];
            for b in &s.merged_branches {
                let mut c = git(dir);
                // -d, never -D: git itself refuses a branch that is not merged.
                c.args(["branch", "-d", b]);
                if ok(c) {
                    done += 1;
                } else {
                    failed.push(b.clone());
                }
            }
            let msg = format!("Deleted {done} merged branch{}", if done == 1 { "" } else { "es" });
            if failed.is_empty() {
                Ok(msg)
            } else {
                Err(format!("{msg}; git refused {}", failed.join(", ")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let s = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(s.success(), "git {args:?}");
    }

    #[test]
    fn reads_worktrees_and_cleans_up() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("yard-inspect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "feat: start"]);
        run(&repo, &["branch", "done"]);
        run(&repo, &["branch", "wt-branch"]);
        let wt = root.join("repo-wt");
        run(&repo, &["worktree", "add", "-q", wt.to_str().unwrap(), "wt-branch"]);
        std::fs::write(repo.join("b.txt"), "b").unwrap();

        let cfg = InspectionConfig { roots: vec![format!("{}/*", root.to_string_lossy())], ..Default::default() };
        let dirs = repo_dirs(&cfg);
        assert_eq!(dirs.len(), 1, "the worktree folds into its repo: {dirs:?}");

        let s = status(&repo).unwrap();
        assert_eq!(s.branch, "main");
        assert_eq!(s.dirty, 1);
        assert!(!s.has_remotes);
        assert_eq!(s.default_branch.as_deref(), Some("main"));
        assert_eq!(s.worktrees.len(), 1);
        assert!(s.worktrees[0].merged);
        assert_eq!(s.merged_branches, vec!["done"]);

        let sig = history(&repo, s.head.as_deref().unwrap(), &s, &cfg, yard_core::model::now());
        assert_eq!(sig.commits, 1);
        assert_eq!(sig.authors[0].name, "Test");

        assert_eq!(act(&repo, ActionKind::RemoveWorktrees).unwrap(), "Removed 1 worktree");
        assert!(!wt.exists());
        assert_eq!(act(&repo, ActionKind::DeleteBranches).unwrap(), "Deleted 2 merged branches");
        let s = status(&repo).unwrap();
        assert!(s.worktrees.is_empty() && s.merged_branches.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
