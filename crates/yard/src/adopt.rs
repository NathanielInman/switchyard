//! `yard adopt cron`: turns crontab lines into a `tasks.d` overlay, one
//! scheduled task per job. It never edits the crontab; it says which lines to
//! remove once the tasks have run.

use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;
use std::process::Command;
use yard_core::paths;
use yard_core::protocol::{request, Request};
use yard_core::schedule::Schedule;

const USAGE: &str = "usage: yard adopt cron [--from <file>] [--print]";

/// Cron variables that mean nothing to a task.
const CRON_ONLY: [&str; 6] = ["MAILTO", "MAILFROM", "SHELL", "CRON_TZ", "TZ", "RANDOM_DELAY"];

/// One crontab job.
#[derive(Debug, PartialEq)]
pub struct Job {
    /// The line as written, to name it when it should be removed.
    pub line: String,
    pub schedule: String,
    pub cmd: String,
    /// The comment lines right above it.
    pub note: Option<String>,
    /// Why it cannot become a task.
    pub problem: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Crontab {
    pub jobs: Vec<Job>,
    /// `NAME=value` lines a task should still see.
    pub env: Vec<(String, String)>,
    /// `MAILTO` and friends, left out.
    pub ignored_env: Vec<String>,
}

pub fn parse_crontab(text: &str) -> Crontab {
    let mut out = Crontab::default();
    let mut note: Vec<String> = vec![];
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            note.clear();
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            note.push(c.trim().to_string());
            continue;
        }
        if let Some((k, v)) = env_assignment(line) {
            if CRON_ONLY.contains(&k.as_str()) {
                out.ignored_env.push(k);
            } else {
                out.env.push((k, v));
            }
            note.clear();
            continue;
        }
        let split = if let Some(l) = line.strip_prefix('@') {
            let (alias, cmd) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            Some((alias.to_string(), cmd.trim().to_string()))
        } else {
            split_fields(line, 5).map(|(f, c)| (f.join(" "), c.to_string()))
        };
        let note_text = (!note.is_empty()).then(|| note.join(" "));
        note.clear();
        let Some((schedule, cmd)) = split else {
            out.jobs.push(Job {
                line: line.into(),
                schedule: String::new(),
                cmd: String::new(),
                note: note_text,
                problem: Some("not a cron line".into()),
            });
            continue;
        };
        let problem = if cmd.is_empty() {
            Some("no command".to_string())
        } else if has_bare_percent(&cmd) {
            Some("uses %, which cron turns into a newline and stdin; move the command into a script".into())
        } else {
            Schedule::parse(&schedule).err()
        };
        out.jobs.push(Job { line: line.into(), schedule, cmd: cmd.replace("\\%", "%"), note: note_text, problem });
    }
    out
}

/// `NAME=value` or `NAME = "value"`, as long as the line is not a job.
fn env_assignment(line: &str) -> Option<(String, String)> {
    if line.starts_with(|c: char| c.is_ascii_digit() || c == '*' || c == '@') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    let k = k.trim();
    let mut chars = k.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let v = v.trim();
    let v = v
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(v);
    Some((k.to_string(), v.to_string()))
}

fn split_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut rest = line;
    let mut fields = vec![];
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    Some((fields, rest.trim()))
}

fn has_bare_percent(cmd: &str) -> bool {
    let b = cmd.as_bytes();
    b.iter().enumerate().any(|(i, c)| *c == b'%' && (i == 0 || b[i - 1] != b'\\'))
}

/// A task id from the command: the script's name without its extension or a
/// `cron-` prefix, so `.../cron-sites-nightly-backup.sh` is `sites-nightly-backup`.
fn job_id(cmd: &str) -> String {
    let word = cmd.split_whitespace().find(|w| !w.contains('=')).unwrap_or("").trim_matches(|c| c == '"' || c == '\'');
    let mut base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    for ext in [".sh", ".bash", ".zsh", ".py", ".pl", ".rb", ".js", ".ps1"] {
        base = base.strip_suffix(ext).unwrap_or(base);
    }
    base = base.strip_prefix("cron-").unwrap_or(base);
    let id: String = base.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "cron-job".into()
    } else {
        id
    }
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The overlay file for a crontab.
pub fn overlay(tab: &Crontab, date: &str) -> String {
    let mut out = format!(
        "# Adopted from your crontab by `yard adopt cron` on {date}.\n\
         # Each task keeps its cron line's schedule. Remove those lines from your\n\
         # crontab (crontab -e) once the first runs show up on the board, or cron\n\
         # and Switchyard will both run them.\n\n\
         [defaults]\n\
         tags = [\"cron\"]\n\
         dir = \"~\"\n\
         notify = \"failure\"\n\
         # A run missed while the daemon was down or the machine asleep happens\n\
         # once it is back. Cron skips those; set this to false to do the same.\n\
         catch_up = true\n"
    );
    if !tab.env.is_empty() {
        let pairs: Vec<String> = tab.env.iter().map(|(k, v)| format!("{k} = {}", quote(v))).collect();
        out.push_str(&format!("env = {{ {} }}\n", pairs.join(", ")));
    }
    if !tab.ignored_env.is_empty() {
        out.push_str(&format!("# Left out, cron only: {}\n", tab.ignored_env.join(", ")));
    }
    let mut used: Vec<String> = vec![];
    for j in &tab.jobs {
        out.push('\n');
        if let Some(p) = &j.problem {
            out.push_str(&format!("# Not adopted: {p}\n#   {}\n", j.line));
            continue;
        }
        let base = job_id(&j.cmd);
        let mut id = base.clone();
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        used.push(id.clone());
        let description = j.note.clone().unwrap_or_else(|| format!("cron: {}", j.schedule));
        out.push_str(&format!(
            "[task.{id}]\nlabel = {}\ndescription = {}\nschedule = {}\nsteps = [{}]\n",
            quote(&id.replace(['-', '_'], " ")),
            quote(&description),
            quote(&j.schedule),
            quote(&j.cmd)
        ));
    }
    out
}

fn read_crontab() -> Result<String> {
    let out =
        Command::new("crontab").arg("-l").output().map_err(|_| anyhow!("there is no `crontab` command here; pass a file with --from"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.to_lowercase().contains("no crontab") {
            return Ok(String::new());
        }
        bail!("crontab -l: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn run(rest: &[String]) -> Result<i32> {
    let mut it = rest.iter();
    if it.next().map(String::as_str) != Some("cron") {
        bail!("{USAGE}");
    }
    let mut from: Option<PathBuf> = None;
    let mut print = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--print" => print = true,
            "--from" => from = Some(paths::expand(it.next().ok_or_else(|| anyhow!("--from needs a file"))?)),
            s => bail!("unexpected argument \"{s}\"\n{USAGE}"),
        }
    }
    let text = match &from {
        Some(f) => std::fs::read_to_string(f).map_err(|e| anyhow!("{}: {e}", paths::tilde(f)))?,
        None => read_crontab()?,
    };
    let tab = parse_crontab(&text);
    if tab.jobs.is_empty() {
        println!("no cron jobs to adopt");
        return Ok(0);
    }
    let text = overlay(&tab, &chrono::Local::now().format("%Y-%m-%d").to_string());
    if print {
        print!("{text}");
        return Ok(0);
    }
    let target = paths::overlay_dir().join("cron.toml");
    if target.exists() {
        bail!("{} already exists; `yard adopt cron --print` shows what would go in it", paths::tilde(&target));
    }
    std::fs::create_dir_all(paths::overlay_dir())?;
    std::fs::write(&target, &text)?;
    let adopted: Vec<&Job> = tab.jobs.iter().filter(|j| j.problem.is_none()).collect();
    println!("wrote {} with {} task{}", paths::tilde(&target), adopted.len(), if adopted.len() == 1 { "" } else { "s" });
    for j in tab.jobs.iter().filter(|j| j.problem.is_some()) {
        println!("left out: {} ({})", j.line, j.problem.as_deref().unwrap_or_default());
    }
    if !adopted.is_empty() {
        println!("\nRemove these lines from your crontab (crontab -e) once their first runs show up on the board.\nUntil then cron and Switchyard both run them:\n");
        for j in adopted {
            println!("  {}", j.line);
        }
    }
    let _ = request(&Request::Reload);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: &str = r#"
MAILTO=""
PATH=/usr/bin:/bin
@hourly /home/u/Sites/dot-files/scripts/cron-git-notes-auto-update.sh
# nightly safety net: back up any dirty ~/Sites repo to its remote
0 3 * * * /home/u/Sites/dot-files/scripts/cron-sites-nightly-backup.sh
# refresh pkglist/ so the 3am backup commits any package changes
55 2 * * * /home/u/Sites/dot-files/scripts/pkglist-refresh.sh

30 3 * * * $HOME/Sites/dot-files/scripts/cron-dotfiles-drift.sh --quiet
@reboot /usr/bin/thing
0 4 * * * date +\%F > /tmp/d
0 5 * * * date +%F > /tmp/d
0 25 * * * /bin/true
"#;

    #[test]
    fn reads_a_crontab() {
        let t = parse_crontab(TAB);
        assert_eq!(t.env, vec![("PATH".to_string(), "/usr/bin:/bin".to_string())]);
        assert_eq!(t.ignored_env, vec!["MAILTO"]);
        assert_eq!(t.jobs.len(), 8);
        assert_eq!(t.jobs[0].schedule, "hourly");
        assert_eq!(t.jobs[0].note, None);
        assert_eq!(t.jobs[1].schedule, "0 3 * * *");
        assert_eq!(t.jobs[1].note.as_deref(), Some("nightly safety net: back up any dirty ~/Sites repo to its remote"));
        assert_eq!(t.jobs[3].cmd, "$HOME/Sites/dot-files/scripts/cron-dotfiles-drift.sh --quiet");
        assert_eq!(t.jobs[3].note, None, "a blank line ends a comment");
        assert!(t.jobs[4].problem.as_deref().unwrap().contains("autostart"));
        assert_eq!(t.jobs[5].problem, None);
        assert_eq!(t.jobs[5].cmd, "date +%F > /tmp/d");
        assert!(t.jobs[6].problem.as_deref().unwrap().contains('%'));
        assert!(t.jobs[7].problem.is_some());
    }

    #[test]
    fn writes_an_overlay_that_loads() {
        let text = overlay(&parse_crontab(TAB), "2026-11-23");
        let mut problems = vec![];
        let tasks = yard_core::manifest::parse_manifest(
            &text,
            std::path::Path::new("/c/tasks.d/cron.toml"),
            yard_core::manifest::Origin::Overlay,
            std::path::Path::new("/logs"),
            &mut problems,
        );
        assert!(problems.is_empty(), "{problems:?}\n{text}");
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["date", "dotfiles-drift", "git-notes-auto-update", "pkglist-refresh", "sites-nightly-backup"]);
        let b = tasks.iter().find(|t| t.id == "sites-nightly-backup").unwrap();
        assert_eq!(b.schedule.as_ref().unwrap().text(), "0 3 * * *");
        assert_eq!(b.label, "sites nightly backup");
        assert!(b.catch_up);
        assert_eq!(b.tags, vec!["cron"]);
        assert_eq!(b.env["PATH"], "/usr/bin:/bin");
        assert!(text.contains("# Not adopted: @reboot"), "{text}");
        assert!(text.contains("#   @reboot /usr/bin/thing"), "{text}");
    }

    #[test]
    fn ids_come_from_the_script() {
        assert_eq!(job_id("/a/b/cron-sites-nightly-backup.sh"), "sites-nightly-backup");
        assert_eq!(job_id("FOO=1 \"$HOME/bin/Sync Notes.py\" --x"), "sync");
        assert_eq!(job_id("$HOME/x/run.sh"), "run");
        assert_eq!(job_id(""), "cron-job");
    }
}
