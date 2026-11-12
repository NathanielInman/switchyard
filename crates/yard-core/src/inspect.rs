//! Inspection: the optional tab with one row per repo under
//! `inspection.roots`. This module holds the config, the wire types, the
//! parsers for git's output and the rules that turn a repo's state into
//! findings. Running git is the daemon's job.

use crate::paths;
use crate::size::{human, parse_size};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ config

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Never shown.
    Off,
    /// Shown in the details; never lights a row.
    #[default]
    Info,
    /// Clean-up worth doing: the caution lamp.
    Tidy,
    /// Work at risk or a rule broken: the stop lamp.
    Act,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Off => "off",
            Severity::Info => "info",
            Severity::Tidy => "tidy",
            Severity::Act => "act",
        }
    }
}

/// Every rule with its default severity.
pub const RULES: &[(&str, Severity)] = &[
    ("unpushed", Severity::Act),
    ("stale_dirty", Severity::Act),
    ("dirty", Severity::Info),
    ("foreign_author", Severity::Tidy),
    ("merged_worktree", Severity::Tidy),
    ("merged_branches", Severity::Tidy),
    ("behind", Severity::Tidy),
    ("old_stash", Severity::Tidy),
    ("big_output", Severity::Tidy),
    ("quiet", Severity::Info),
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    #[default]
    Owner,
    Host,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct InspectionConfig {
    /// Folders to look in. A trailing `/*` means each child.
    pub roots: Vec<String>,
    /// Remote owners that are yours: tagged `mine`, and checked for foreign
    /// authors.
    pub mine: Vec<String>,
    /// Author names or emails that are you (after `.mailmap`).
    pub me: Vec<String>,
    pub group_by: GroupBy,
    /// How much history to read for hotspots, fixes and firefighting.
    pub window: String,
    /// Globs left out of hotspots. One without a `/` matches the file name.
    pub ignore: Vec<String>,
    /// No commits this long: the row dims and sinks.
    pub quiet_after: String,
    /// Build output larger than this in an idle repo is a finding.
    pub big_output: String,
    /// Renames for tags taken from remotes.
    pub tags: BTreeMap<String, String>,
    /// Severity per rule name.
    pub severity: BTreeMap<String, Severity>,
}

impl Default for InspectionConfig {
    fn default() -> Self {
        InspectionConfig {
            roots: vec![],
            mine: vec![],
            me: vec![],
            group_by: GroupBy::Owner,
            window: "1y".into(),
            ignore: ["*.lock", "package-lock.json", "pnpm-lock.yaml", "CHANGELOG.md"].iter().map(|s| s.to_string()).collect(),
            quiet_after: "90d".into(),
            big_output: "5G".into(),
            tags: BTreeMap::new(),
            severity: BTreeMap::new(),
        }
    }
}

const DAY: f64 = 86400.0;

/// "90d", "2w", "6m", "1y", "36h" as seconds.
pub fn parse_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (n, unit) = s.split_at(split);
    let n: f64 = n.parse().ok()?;
    let mult = match unit.trim() {
        "h" => 3600.0,
        "d" => DAY,
        "w" => 7.0 * DAY,
        "m" => 30.0 * DAY,
        "y" => 365.0 * DAY,
        _ => return None,
    };
    Some(n * mult)
}

impl InspectionConfig {
    pub fn window_secs(&self) -> f64 {
        parse_duration(&self.window).unwrap_or(365.0 * DAY)
    }
    pub fn quiet_secs(&self) -> f64 {
        parse_duration(&self.quiet_after).unwrap_or(90.0 * DAY)
    }
    pub fn big_output_bytes(&self) -> u64 {
        parse_size(&self.big_output).unwrap_or(5 << 30)
    }
    pub fn severity(&self, rule: &str) -> Severity {
        self.severity
            .get(rule)
            .copied()
            .unwrap_or_else(|| RULES.iter().find(|(r, _)| *r == rule).map(|(_, s)| *s).unwrap_or(Severity::Info))
    }

    /// Mistakes worth showing on the board.
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        for k in self.severity.keys() {
            if !RULES.iter().any(|(r, _)| r == k) {
                let names: Vec<&str> = RULES.iter().map(|(r, _)| *r).collect();
                out.push(format!("inspection.severity: unknown rule \"{k}\" (rules: {})", names.join(", ")));
            }
        }
        for (key, v) in [("window", &self.window), ("quiet_after", &self.quiet_after)] {
            if parse_duration(v).is_none() {
                out.push(format!("inspection.{key}: \"{v}\" is not a duration like 90d, 6m or 1y"));
            }
        }
        if parse_size(&self.big_output).is_none() {
            out.push(format!("inspection.big_output: \"{}\" is not a size like 5G", self.big_output));
        }
        out
    }

    /// The directories under `roots` that hold a repo or a worktree.
    pub fn candidate_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![];
        for pat in &self.roots {
            let p = paths::expand(pat);
            let s = p.to_string_lossy().to_string();
            if s.contains(['*', '?', '[']) {
                // Hidden folders are not projects.
                let opts = glob::MatchOptions { require_literal_leading_dot: true, ..Default::default() };
                if let Ok(entries) = glob::glob_with(&s, opts) {
                    out.extend(entries.flatten().filter(|e| e.join(".git").exists()));
                }
            } else if p.join(".git").exists() {
                out.push(p);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn is_me(&self, name: &str, email: &str) -> bool {
        self.me.iter().any(|m| m.eq_ignore_ascii_case(name) || (!email.is_empty() && m.eq_ignore_ascii_case(email)))
    }

    fn tag(&self, t: &str) -> String {
        self.tags.get(t).cloned().unwrap_or_else(|| t.to_string())
    }
}

// ------------------------------------------------------------------ remotes

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub host: String,
    pub owner: String,
}

/// The host and owner of a remote URL: `https://host/owner/repo`,
/// `git@host:owner/repo.git`, `ssh://git@host:22/owner/repo`.
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let rest = if let Some((_, r)) = url.split_once("://") {
        r.to_string()
    } else if let Some((h, p)) = url.split_once(':') {
        // scp-like: user@host:owner/repo
        if h.contains('/') {
            return None;
        }
        format!("{h}/{p}")
    } else {
        return None;
    };
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let host = parts.next()?;
    let host = host.rsplit('@').next()?.split(':').next()?.to_ascii_lowercase();
    let owner = parts.next()?.to_ascii_lowercase();
    parts.next()?;
    if host.is_empty() || owner.is_empty() {
        return None;
    }
    Some(Remote { host, owner })
}

/// `github.com` becomes `github`; an unknown host stays as it is.
pub fn host_tag(host: &str) -> String {
    for (h, t) in [("github.com", "github"), ("gitlab.com", "gitlab"), ("bitbucket.org", "bitbucket"), ("codeberg.org", "codeberg")] {
        if host == h {
            return t.into();
        }
    }
    host.to_string()
}

// ------------------------------------------------------------------ state

/// A linked worktree (not the repo's own working tree).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Worktree {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub dirty: u32,
    /// The branch is an ancestor of the default branch.
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub last_commit: Option<f64>,
}

/// What `git status` and friends say right now. Cheap to read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Status {
    #[serde(default)]
    pub head: Option<String>,
    pub branch: String,
    #[serde(default)]
    pub upstream: bool,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(default)]
    pub dirty: u32,
    /// The oldest modification time among uncommitted files.
    #[serde(default)]
    pub dirty_oldest: Option<f64>,
    /// Stash times, newest first.
    #[serde(default)]
    pub stashes: Vec<f64>,
    #[serde(default)]
    pub remote_url: Option<String>,
    #[serde(default)]
    pub has_remotes: bool,
    /// Commits on local branches that no remote has.
    #[serde(default)]
    pub unpushed: u32,
    #[serde(default)]
    pub unpushed_oldest: Option<f64>,
    #[serde(default)]
    pub last_commit: Option<f64>,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
    /// Local branches merged into the default branch and checked out nowhere.
    #[serde(default)]
    pub merged_branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Hotspot {
    pub file: String,
    pub changes: u32,
    pub fixes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Month {
    /// "2026-10".
    pub label: String,
    pub commits: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Author {
    pub name: String,
    pub commits: u32,
    /// Commits in the last six months.
    pub recent: u32,
    /// Commits in the last 30 days.
    pub month: u32,
    pub last: f64,
    pub bot: bool,
    pub me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Output {
    /// Relative to the repo, like `target` or `../storefront-fix/node_modules`.
    pub dir: String,
    pub bytes: u64,
}

/// What a read of the history says. Costlier; cached by HEAD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Signals {
    pub head: String,
    pub computed_at: f64,
    /// Commits in the window.
    pub commits: u32,
    /// Share of the window's subjects that are conventional commits.
    pub conventional: f64,
    pub hotspots: Vec<Hotspot>,
    /// The last 14 months, oldest first.
    pub months: Vec<Month>,
    /// Every author over all history, most commits first, `me` folded into one.
    pub authors: Vec<Author>,
    /// Subjects in the window that mention a revert or hotfix, newest first.
    pub firefights: Vec<String>,
    pub output: Vec<Output>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    RemoveWorktrees,
    DeleteBranches,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoAction {
    pub kind: ActionKind,
    pub key: char,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Lamp {
    Act,
    Tidy,
    Clear,
    Quiet,
}

impl Lamp {
    pub fn as_str(self) -> &'static str {
        match self {
            Lamp::Act => "act",
            Lamp::Tidy => "tidy",
            Lamp::Clear => "clear",
            Lamp::Quiet => "quiet",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Repo {
    pub dir: String,
    pub name: String,
    pub tags: Vec<String>,
    pub group: String,
    pub mine: bool,
    pub lamp: Lamp,
    pub headline: String,
    pub sub: String,
    pub status: Status,
    #[serde(default)]
    pub signals: Option<Signals>,
    pub findings: Vec<Finding>,
    pub actions: Vec<RepoAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct InspectionData {
    /// Ordered: by group, then lamp, then name.
    pub repos: Vec<Repo>,
    /// Every tag with how many repos carry it, most first.
    pub tags: Vec<(String, usize)>,
    /// A scan is under way; rows may still change.
    pub scanning: bool,
    pub scanned_at: f64,
}

/// What the snapshot carries for the tab label.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct InspectionCounts {
    pub repos: usize,
    pub act: usize,
    pub tidy: usize,
    /// Bumps whenever any row changes.
    pub rev: u64,
}

impl InspectionData {
    pub fn counts(&self, rev: u64) -> InspectionCounts {
        InspectionCounts {
            repos: self.repos.len(),
            act: self.repos.iter().filter(|r| r.lamp == Lamp::Act).count(),
            tidy: self.repos.iter().filter(|r| r.lamp == Lamp::Tidy).count(),
            rev,
        }
    }
}

/// True when a repo matches a filter: words match the name, `#tag` terms
/// must all be among its tags.
pub fn matches(repo: &Repo, filter: &str) -> bool {
    filter.split_whitespace().all(|term| match term.strip_prefix('#') {
        Some(t) if !t.is_empty() => repo.tags.iter().any(|g| g.eq_ignore_ascii_case(t)),
        Some(_) => true,
        None => repo.name.to_ascii_lowercase().contains(&term.to_ascii_lowercase()),
    })
}

// ------------------------------------------------------------------ parsing

/// The parts of `git status --porcelain=v2 --branch` Inspection uses: the
/// branch, upstream, ahead and behind, and the changed paths.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusLines {
    pub head: Option<String>,
    pub branch: String,
    pub upstream: bool,
    pub ahead: u32,
    pub behind: u32,
    pub paths: Vec<String>,
}

pub fn parse_status(text: &str) -> StatusLines {
    let mut s = StatusLines::default();
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("# branch.oid ") {
            s.head = (v != "(initial)").then(|| v.to_string());
        } else if let Some(v) = l.strip_prefix("# branch.head ") {
            s.branch = if v == "(detached)" { "detached".into() } else { v.to_string() };
        } else if l.starts_with("# branch.upstream ") {
            s.upstream = true;
        } else if let Some(ab) = l.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    s.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    s.behind = b.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = l.strip_prefix("1 ") {
            if let Some(p) = rest.splitn(8, ' ').nth(7) {
                s.paths.push(p.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("2 ") {
            if let Some(p) = rest.splitn(9, ' ').nth(8) {
                s.paths.push(p.split('\t').next().unwrap_or(p).to_string());
            }
        } else if let Some(rest) = l.strip_prefix("u ") {
            if let Some(p) = rest.splitn(10, ' ').nth(9) {
                s.paths.push(p.to_string());
            }
        } else if let Some(p) = l.strip_prefix("? ") {
            s.paths.push(p.to_string());
        }
    }
    s
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorktreeLine {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
}

pub fn parse_worktrees(text: &str) -> Vec<WorktreeLine> {
    let mut out = vec![];
    let mut cur: Option<WorktreeLine> = None;
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            if let Some(c) = cur.take() {
                out.push(c);
            }
            cur = Some(WorktreeLine { path: p.to_string(), ..Default::default() });
        } else if let Some(c) = cur.as_mut() {
            if let Some(h) = l.strip_prefix("HEAD ") {
                c.head = Some(h.to_string());
            } else if let Some(b) = l.strip_prefix("branch ") {
                c.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if l == "bare" {
                c.bare = true;
            }
        }
    }
    out.extend(cur);
    out
}

/// The format Inspection reads history with: a record separator, then the
/// commit's time, author name, author email and subject, then its files.
pub const LOG_FORMAT: &str = "%x1e%ct%x09%aN%x09%aE%x09%s";
/// One line per commit over all history, for people.
pub const PEOPLE_FORMAT: &str = "%ct%x09%aN%x09%aE";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Commit {
    pub time: f64,
    pub name: String,
    pub email: String,
    pub subject: String,
    pub files: Vec<String>,
}

pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\x1e')
        .filter_map(|rec| {
            let mut lines = rec.lines();
            let head = lines.next()?;
            let mut f = head.splitn(4, '\t');
            let time = f.next()?.trim().parse::<f64>().ok()?;
            let name = f.next()?.to_string();
            let email = f.next()?.to_string();
            let subject = f.next().unwrap_or("").to_string();
            let files = lines.map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            Some(Commit { time, name, email, subject, files })
        })
        .collect()
}

pub fn parse_people(text: &str) -> Vec<(f64, String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some((f.next()?.trim().parse().ok()?, f.next()?.to_string(), f.next().unwrap_or("").to_string()))
        })
        .collect()
}

// ------------------------------------------------------------------ signals

/// `feat: x`, `fix(core)!: y`.
pub fn is_conventional(subject: &str) -> bool {
    let Some((head, _)) = subject.split_once(':') else { return false };
    let ty = head.split('(').next().unwrap_or(head).trim_end_matches('!');
    !ty.is_empty() && ty.len() <= 12 && ty.chars().all(|c| c.is_ascii_lowercase()) && !head.contains(' ')
}

/// A fix commit: the conventional `fix` type, or in a repo that mostly does
/// not use conventional commits, the words fix, bug or broken.
pub fn is_fix(subject: &str, conventional_repo: bool) -> bool {
    if conventional_repo {
        let head = subject.split(':').next().unwrap_or("");
        return is_conventional(subject) && head.split('(').next().unwrap_or("").trim_end_matches('!') == "fix";
    }
    let s = subject.to_ascii_lowercase();
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| matches!(w, "fix" | "fixes" | "fixed" | "bug" | "bugfix" | "broken"))
}

pub fn is_firefight(subject: &str) -> bool {
    let s = subject.to_ascii_lowercase();
    ["revert", "hotfix", "emergency", "rollback"].iter().any(|w| s.contains(w))
}

/// `github-actions[bot]`, `dependabot[bot]`, `semantic-release-bot`.
pub fn is_bot(name: &str, email: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("[bot]") || n.ends_with("-bot") || n == "bot" || email.to_ascii_lowercase().contains("[bot]@")
}

fn ignored(path: &str, patterns: &[glob::Pattern]) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    patterns.iter().any(|p| if p.as_str().contains('/') { p.matches(path) } else { p.matches(base) })
}

/// Turns a window of commits and the all-time author lines into signals.
pub fn signals(head: &str, window: &[Commit], people: &[(f64, String, String)], cfg: &InspectionConfig, now: f64) -> Signals {
    let since = now - cfg.window_secs();
    let in_window: Vec<&Commit> = window.iter().filter(|c| c.time >= since).collect();
    let conv = if in_window.is_empty() {
        0.0
    } else {
        in_window.iter().filter(|c| is_conventional(&c.subject)).count() as f64 / in_window.len() as f64
    };
    let conventional_repo = conv >= 0.5;
    let patterns: Vec<glob::Pattern> = cfg.ignore.iter().filter_map(|p| glob::Pattern::new(p).ok()).collect();
    let mut files: HashMap<&str, (u32, u32)> = HashMap::new();
    let mut firefights = vec![];
    for c in &in_window {
        let fix = is_fix(&c.subject, conventional_repo);
        for f in &c.files {
            if ignored(f, &patterns) {
                continue;
            }
            let e = files.entry(f.as_str()).or_default();
            e.0 += 1;
            if fix {
                e.1 += 1;
            }
        }
        if is_firefight(&c.subject) {
            firefights.push((c.time, c.subject.clone()));
        }
    }
    firefights.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut hotspots: Vec<Hotspot> = files.into_iter().map(|(f, (c, x))| Hotspot { file: f.to_string(), changes: c, fixes: x }).collect();
    hotspots.sort_by(|a, b| b.changes.cmp(&a.changes).then(b.fixes.cmp(&a.fixes)).then(a.file.cmp(&b.file)));
    hotspots.truncate(8);

    // Fourteen local months ending with this one.
    let (mut y, mut m) = crate::history::year_month(now);
    let mut labels = vec![];
    for _ in 0..14 {
        labels.push(format!("{y}-{m:02}"));
        if m == 1 {
            y -= 1;
            m = 12;
        } else {
            m -= 1;
        }
    }
    labels.reverse();
    let mut counts: HashMap<String, u32> = HashMap::new();
    for c in window {
        let (y, m) = crate::history::year_month(c.time);
        *counts.entry(format!("{y}-{m:02}")).or_default() += 1;
    }
    let months = labels.into_iter().map(|l| Month { commits: counts.get(&l).copied().unwrap_or(0), label: l }).collect();

    let mut by: BTreeMap<String, Author> = BTreeMap::new();
    for (t, name, email) in people {
        let me = cfg.is_me(name, email);
        let key = if me { "\u{0}me".to_string() } else { name.clone() };
        let a = by.entry(key).or_insert_with(|| Author { name: name.clone(), bot: is_bot(name, email), me, ..Default::default() });
        a.commits += 1;
        if *t >= now - 182.0 * DAY {
            a.recent += 1;
        }
        if *t >= now - 30.0 * DAY {
            a.month += 1;
        }
        if *t > a.last {
            a.last = *t;
            if me {
                a.name = name.clone();
            }
        }
    }
    let mut authors: Vec<Author> = by.into_values().collect();
    authors.sort_by(|a, b| b.commits.cmp(&a.commits).then(a.name.cmp(&b.name)));

    Signals {
        head: head.to_string(),
        computed_at: now,
        commits: in_window.len() as u32,
        conventional: conv,
        hotspots,
        months,
        authors,
        firefights: firefights.into_iter().map(|(_, s)| s).take(20).collect(),
        output: vec![],
    }
}

// ------------------------------------------------------------------ findings

/// "3d", "5w", "4mo", "2y".
pub fn span(secs: f64) -> String {
    let d = (secs / DAY).max(0.0);
    if secs < 3600.0 {
        "now".into()
    } else if d < 1.0 {
        format!("{}h", (secs / 3600.0) as u64)
    } else if d < 21.0 {
        format!("{}d", d as u64)
    } else if d < 90.0 {
        format!("{}w", (d / 7.0) as u64)
    } else if d < 730.0 {
        format!("{}mo", (d / 30.0) as u64)
    } else {
        format!("{}y", (d / 365.0) as u64)
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Tags, group and whether the repo is yours, from its remote.
pub fn classify(status: &Status, cfg: &InspectionConfig) -> (Vec<String>, String, bool) {
    let remote = status.remote_url.as_deref().and_then(parse_remote);
    let mut tags = vec![];
    let (owner, host) = match &remote {
        Some(r) => (Some(cfg.tag(&r.owner)), Some(cfg.tag(&host_tag(&r.host)))),
        None => (None, None),
    };
    let mine = remote.as_ref().is_some_and(|r| cfg.mine.iter().any(|m| m.eq_ignore_ascii_case(&r.owner)));
    tags.extend(owner.clone());
    tags.extend(host.clone());
    if mine {
        tags.push("mine".into());
    }
    if remote.is_none() {
        tags.push("local".into());
    }
    tags.dedup();
    let group = match cfg.group_by {
        GroupBy::Owner => owner.unwrap_or_else(|| "local".into()),
        GroupBy::Host => host.unwrap_or_else(|| "local".into()),
        GroupBy::None => String::new(),
    };
    (tags, group, mine)
}

/// Applies the rules to a repo's state and fills in its findings, actions,
/// lamp and the two lines a row shows.
pub fn evaluate(dir: &Path, status: Status, signals: Option<Signals>, cfg: &InspectionConfig, now: f64) -> Repo {
    let (tags, group, mine) = classify(&status, cfg);
    let mut findings: Vec<Finding> = vec![];
    let mut add = |rule: &str, text: String| {
        let severity = cfg.severity(rule);
        if severity != Severity::Off {
            findings.push(Finding { rule: rule.into(), severity, text });
        }
    };
    let s = &status;

    if s.has_remotes && s.unpushed > 0 {
        if let Some(t) = s.unpushed_oldest.filter(|t| now - t > 3.0 * DAY) {
            add("unpushed", format!("{} on no remote, oldest {}", plural(s.unpushed, "commit", "commits"), span(now - t)));
        }
    }
    let stale_dirty = s.dirty > 0 && s.dirty_oldest.is_some_and(|t| now - t > 7.0 * DAY);
    if stale_dirty {
        add(
            "stale_dirty",
            format!("{} uncommitted, oldest {}", plural(s.dirty, "file", "files"), span(now - s.dirty_oldest.unwrap_or(now))),
        );
    } else if s.dirty > 0 {
        add("dirty", format!("{} uncommitted", plural(s.dirty, "file", "files")));
    }
    if mine && !cfg.me.is_empty() {
        if let Some(sig) = &signals {
            let foreign: Vec<&Author> = sig.authors.iter().filter(|a| !a.me && a.month > 0).collect();
            if let Some(top) = foreign.first() {
                let more = if foreign.len() > 1 { format!(" and {} more", foreign.len() - 1) } else { String::new() };
                add("foreign_author", format!("{} wrote {} this month{more}", top.name, plural(top.month, "commit", "commits")));
            }
        }
    }
    let merged_wt: Vec<&Worktree> = s.worktrees.iter().filter(|w| w.merged && w.dirty == 0).collect();
    if !merged_wt.is_empty() {
        let text = if merged_wt.len() == 1 {
            let w = merged_wt[0];
            let name = Path::new(&w.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match w.last_commit.filter(|t| now - t >= DAY) {
                Some(t) => format!("worktree {name} merged and clean, idle {}", span(now - t)),
                None => format!("worktree {name} merged and clean"),
            }
        } else {
            format!("{} worktrees merged and clean", merged_wt.len())
        };
        add("merged_worktree", text);
    }
    if !s.merged_branches.is_empty() {
        add("merged_branches", plural(s.merged_branches.len() as u32, "merged branch", "merged branches"));
    }
    if s.behind > 0 {
        add("behind", format!("{} behind its upstream", s.behind));
    }
    if let Some(t) = s.stashes.iter().copied().fold(None::<f64>, |a, t| Some(a.map_or(t, |a| a.min(t)))) {
        if now - t > 14.0 * DAY {
            add("old_stash", format!("{}, oldest {}", plural(s.stashes.len() as u32, "stash", "stashes"), span(now - t)));
        }
    }
    let quiet = s.last_commit.is_some_and(|t| now - t > cfg.quiet_secs());
    if let Some(sig) = &signals {
        let total: u64 = sig.output.iter().map(|o| o.bytes).sum();
        let idle = s.last_commit.is_some_and(|t| now - t > 30.0 * DAY);
        if total > cfg.big_output_bytes() && idle {
            add("big_output", format!("{} of build output, idle {}", human(total), span(now - s.last_commit.unwrap_or(now))));
        }
    }
    if quiet {
        add("quiet", format!("quiet for {}", span(now - s.last_commit.unwrap_or(now))));
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));

    let worst = findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Info);
    let lamp = match worst {
        Severity::Act => Lamp::Act,
        Severity::Tidy => Lamp::Tidy,
        _ if quiet => Lamp::Quiet,
        _ => Lamp::Clear,
    };
    let headline = match findings.iter().find(|f| f.severity >= Severity::Tidy) {
        Some(f) => f.text.clone(),
        None if quiet => format!("quiet for {}", span(now - s.last_commit.unwrap_or(now))),
        None => match &signals {
            Some(sig) => {
                let recent = sig.months.last().map(|m| m.commits).unwrap_or(0);
                if recent > 0 {
                    format!("{} this month", plural(recent, "commit", "commits"))
                } else {
                    "clean and pushed".to_string()
                }
            }
            None => "reading history…".into(),
        },
    };
    let mut sub = vec![s.branch.clone()];
    if s.dirty == 0 {
        sub.push("clean".into());
    } else if !stale_dirty {
        sub.push(format!("{} uncommitted", s.dirty));
    }
    if s.has_remotes && s.unpushed == 0 && s.ahead == 0 {
        sub.push("pushed".into());
    }
    if !s.has_remotes {
        sub.push("no remote".into());
    }
    let others: Vec<&Finding> = findings.iter().filter(|f| f.severity >= Severity::Tidy).skip(1).collect();
    for f in others.iter().take(2) {
        sub.push(f.text.clone());
    }
    if let Some(sig) = &signals {
        let total: u64 = sig.output.iter().map(|o| o.bytes).sum();
        if total >= 1 << 30 && !findings.iter().any(|f| f.rule == "big_output") {
            sub.push(format!("{} build output", human(total)));
        }
    }

    let mut actions = vec![];
    if !merged_wt.is_empty() {
        let label = if merged_wt.len() == 1 {
            "remove the merged worktree".to_string()
        } else {
            format!("remove {} merged worktrees", merged_wt.len())
        };
        actions.push(RepoAction { kind: ActionKind::RemoveWorktrees, key: 'w', label });
    }
    if !s.merged_branches.is_empty() {
        actions.push(RepoAction {
            kind: ActionKind::DeleteBranches,
            key: 'b',
            label: format!("delete {}", plural(s.merged_branches.len() as u32, "merged branch", "merged branches")),
        });
    }

    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.to_string_lossy().to_string());
    Repo {
        dir: dir.to_string_lossy().to_string(),
        name,
        tags,
        group,
        mine,
        lamp,
        headline,
        sub: sub.join(" · "),
        status,
        signals,
        findings,
        actions,
    }
}

/// Orders rows (group, lamp, name) and counts tags.
pub fn assemble(mut repos: Vec<Repo>, scanning: bool, scanned_at: f64) -> InspectionData {
    // Groups that hold your repos first, then the rest, then repos without a
    // remote; each set alphabetical.
    let mut rank: HashMap<String, u8> = HashMap::new();
    for r in &repos {
        let k = if r.group == "local" {
            2
        } else if r.mine {
            0
        } else {
            1
        };
        let e = rank.entry(r.group.clone()).or_insert(k);
        *e = (*e).min(k);
    }
    repos.sort_by(|a, b| {
        let ga = (rank.get(&a.group).copied().unwrap_or(1), a.group.as_str());
        let gb = (rank.get(&b.group).copied().unwrap_or(1), b.group.as_str());
        ga.cmp(&gb).then(a.lamp.cmp(&b.lamp)).then(a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for r in &repos {
        for t in &r.tags {
            *tags.entry(t.clone()).or_default() += 1;
        }
    }
    let mut tags: Vec<(String, usize)> = tags.into_iter().collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    InspectionData { repos, tags, scanning, scanned_at }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> InspectionConfig {
        InspectionConfig {
            roots: vec!["~/Sites/*".into()],
            mine: vec!["NathanielInman".into(), "theoestudio".into()],
            me: vec!["Nathaniel Inman".into(), "NBInman".into()],
            ..Default::default()
        }
    }

    #[test]
    fn parses_config() {
        let c: InspectionConfig = toml::from_str(
            r#"
            roots = ["~/Sites/*"]
            mine = ["nathanielinman"]
            group_by = "host"
            [tags]
            "twinspear-games" = "twinspear"
            [severity]
            foreign_author = "act"
            "#,
        )
        .unwrap();
        assert_eq!(c.group_by, GroupBy::Host);
        assert_eq!(c.severity("foreign_author"), Severity::Act);
        assert_eq!(c.severity("merged_branches"), Severity::Tidy);
        assert_eq!(c.window_secs(), 365.0 * DAY);
        assert!(c.problems().is_empty());
        let bad: InspectionConfig = toml::from_str("[severity]\nfoo = \"act\"").unwrap();
        assert_eq!(bad.problems().len(), 1);
        assert!(toml::from_str::<InspectionConfig>("rootz = []").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90d"), Some(90.0 * DAY));
        assert_eq!(parse_duration("6m"), Some(180.0 * DAY));
        assert_eq!(parse_duration("1y"), Some(365.0 * DAY));
        assert_eq!(parse_duration("2w"), Some(14.0 * DAY));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("5"), None);
    }

    #[test]
    fn remotes() {
        let r = parse_remote("https://github.com/NathanielInman/switchyard.git").unwrap();
        assert_eq!((r.host.as_str(), r.owner.as_str()), ("github.com", "nathanielinman"));
        let r = parse_remote("git@gitlab.com:digitalturbine/ODS/ci/ci-common.git").unwrap();
        assert_eq!((r.host.as_str(), r.owner.as_str()), ("gitlab.com", "digitalturbine"));
        let r = parse_remote("ssh://git@git.example.com:2222/team/app").unwrap();
        assert_eq!((r.host.as_str(), r.owner.as_str()), ("git.example.com", "team"));
        assert_eq!(parse_remote("/srv/git/thing.git"), None);
        assert_eq!(parse_remote("https://github.com/onlyowner"), None);
        assert_eq!(host_tag("gitlab.com"), "gitlab");
        assert_eq!(host_tag("git.example.com"), "git.example.com");
    }

    #[test]
    fn status_v2() {
        let text = "# branch.oid 4b31f2d0\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n\
                    1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\n\
                    2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\told.rs\n\
                    ? notes.txt\n";
        let s = parse_status(text);
        assert_eq!(s.head.as_deref(), Some("4b31f2d0"));
        assert_eq!(s.branch, "main");
        assert!(s.upstream);
        assert_eq!((s.ahead, s.behind), (2, 3));
        assert_eq!(s.paths, vec!["src/a b.rs", "new.rs", "notes.txt"]);
        let d = parse_status("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!(d.head, None);
        assert_eq!(d.branch, "detached");
    }

    #[test]
    fn worktrees() {
        let text = "worktree /s/repo\nHEAD aaa\nbranch refs/heads/main\n\nworktree /s/repo-fix\nHEAD bbb\nbranch refs/heads/fix/x\n\nworktree /s/repo-demo\nHEAD ccc\ndetached\n";
        let w = parse_worktrees(text);
        assert_eq!(w.len(), 3);
        assert_eq!(w[1].branch.as_deref(), Some("fix/x"));
        assert_eq!(w[2].branch, None);
    }

    #[test]
    fn subjects() {
        assert!(is_conventional("feat(board): add a tab"));
        assert!(is_conventional("fix!: drop it"));
        assert!(!is_conventional("Merge branch 'x' into main"));
        assert!(!is_conventional("WIP: stuff"));
        assert!(is_fix("fix(core): it", true));
        assert!(!is_fix("feat: prefix fix", true));
        assert!(is_fix("Fixed the login bug", false));
        assert!(!is_fix("add prefix and debug log", false));
        assert!(is_firefight("Revert \"feat: x\""));
        assert!(is_bot("github-actions[bot]", ""));
        assert!(is_bot("semantic-release-bot", "semantic-release-bot@martynus.net"));
        assert!(!is_bot("Robert", "bob@x.com"));
    }

    #[test]
    fn log_and_signals() {
        let now = 1_794_500_000.0;
        let text = format!(
            "\x1e{}\tNathaniel Inman\tn@x\tfix(world): load order\n\ncrates/world/src/load.rs\nCargo.lock\n\x1e{}\tNBInman\tn@y\tfeat: add\n\ncrates/world/src/load.rs\nREADME.md\n",
            now - DAY,
            now - 2.0 * DAY
        );
        let log = parse_log(&text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].files, vec!["crates/world/src/load.rs", "Cargo.lock"]);
        let people = vec![
            (now - DAY, "Nathaniel Inman".to_string(), "n@x".to_string()),
            (now - 2.0 * DAY, "NBInman".to_string(), "n@y".to_string()),
            (now - 3.0 * DAY, "github-actions[bot]".to_string(), "b@x".to_string()),
            (now - 400.0 * DAY, "Kyle".to_string(), "k@x".to_string()),
        ];
        let s = signals("abc", &log, &people, &cfg(), now);
        assert_eq!(s.commits, 2);
        assert_eq!(s.hotspots[0], Hotspot { file: "crates/world/src/load.rs".into(), changes: 2, fixes: 1 });
        assert!(s.hotspots.iter().all(|h| h.file != "Cargo.lock"));
        assert_eq!(s.months.len(), 14);
        assert_eq!(s.months.iter().map(|m| m.commits).sum::<u32>(), 2);
        assert_eq!(s.authors[0].commits, 2);
        assert!(s.authors[0].me);
        let bot = s.authors.iter().find(|a| a.bot).unwrap();
        assert_eq!((bot.month, bot.recent), (1, 1));
        let kyle = s.authors.iter().find(|a| a.name == "Kyle").unwrap();
        assert_eq!((kyle.recent, kyle.month), (0, 0));
    }

    #[test]
    fn rules_and_lamps() {
        let now = 1_794_500_000.0;
        let c = cfg();
        let base = Status {
            branch: "main".into(),
            remote_url: Some("https://github.com/TwinSpear-Games/det33-godot.git".into()),
            has_remotes: true,
            last_commit: Some(now - 3.0 * DAY),
            ..Default::default()
        };
        let clean = evaluate(Path::new("/s/det33-godot"), base.clone(), None, &c, now);
        assert_eq!(clean.lamp, Lamp::Clear);
        assert_eq!(clean.tags, vec!["twinspear-games", "github"]);
        assert_eq!(clean.group, "twinspear-games");
        assert!(!clean.mine);

        let mut s = base.clone();
        s.remote_url = Some("https://github.com/NathanielInman/switchyard.git".into());
        let sig = Signals {
            authors: vec![
                Author { name: "Nathaniel Inman".into(), commits: 50, month: 10, me: true, ..Default::default() },
                Author { name: "github-actions[bot]".into(), commits: 14, month: 14, bot: true, ..Default::default() },
            ],
            ..Default::default()
        };
        let r = evaluate(Path::new("/s/switchyard"), s.clone(), Some(sig.clone()), &c, now);
        assert!(r.mine);
        assert_eq!(r.lamp, Lamp::Tidy);
        assert_eq!(r.headline, "github-actions[bot] wrote 14 commits this month");
        let mut act = c.clone();
        act.severity.insert("foreign_author".into(), Severity::Act);
        assert_eq!(evaluate(Path::new("/s/switchyard"), s.clone(), Some(sig.clone()), &act, now).lamp, Lamp::Act);
        let mut off = c.clone();
        off.severity.insert("foreign_author".into(), Severity::Off);
        assert_eq!(evaluate(Path::new("/s/switchyard"), s, Some(sig), &off, now).lamp, Lamp::Clear);

        let mut s = base.clone();
        s.worktrees = vec![
            Worktree {
                path: "/s/x-dtcd703".into(),
                branch: Some("DTCD-703".into()),
                dirty: 0,
                merged: true,
                last_commit: Some(now - 22.0 * DAY),
            },
            Worktree { path: "/s/x-dtcd706".into(), branch: Some("DTCD-706".into()), dirty: 1, merged: true, last_commit: None },
        ];
        s.merged_branches = vec!["a".into(), "b".into()];
        let r = evaluate(Path::new("/s/x"), s, None, &c, now);
        assert_eq!(r.lamp, Lamp::Tidy);
        assert_eq!(r.headline, "worktree x-dtcd703 merged and clean, idle 3w");
        assert_eq!(r.actions.len(), 2);
        assert_eq!(r.actions[0].kind, ActionKind::RemoveWorktrees);
        assert_eq!(r.actions[1].label, "delete 2 merged branches");

        let mut s = base.clone();
        s.unpushed = 2;
        s.unpushed_oldest = Some(now - 5.0 * DAY);
        s.dirty = 3;
        s.dirty_oldest = Some(now - DAY);
        let r = evaluate(Path::new("/s/x"), s, None, &c, now);
        assert_eq!(r.lamp, Lamp::Act);
        assert_eq!(r.headline, "2 commits on no remote, oldest 5d");
        assert!(r.findings.iter().any(|f| f.rule == "dirty" && f.severity == Severity::Info));

        let mut s = base.clone();
        s.unpushed = 9;
        s.unpushed_oldest = Some(now - 50.0 * DAY);
        s.has_remotes = false;
        s.remote_url = None;
        s.last_commit = Some(now - 300.0 * DAY);
        let r = evaluate(Path::new("/s/old"), s, None, &c, now);
        assert_eq!(r.lamp, Lamp::Quiet);
        assert_eq!(r.tags, vec!["local"]);
        assert_eq!(r.headline, "quiet for 10mo");
    }

    #[test]
    fn filtering_and_order() {
        let now = 1_794_500_000.0;
        let c = cfg();
        let mk = |dir: &str, url: &str, dirty_days: Option<f64>| {
            let s = Status {
                branch: "main".into(),
                remote_url: Some(url.into()),
                has_remotes: true,
                last_commit: Some(now - DAY),
                dirty: dirty_days.map(|_| 1).unwrap_or(0),
                dirty_oldest: dirty_days.map(|d| now - d * DAY),
                ..Default::default()
            };
            evaluate(Path::new(dir), s, None, &c, now)
        };
        let d = assemble(
            vec![
                mk("/s/zeta", "https://gitlab.com/digitalturbine/a/zeta", None),
                mk("/s/alpha", "https://gitlab.com/digitalturbine/a/alpha", Some(10.0)),
                mk("/s/bulwark", "https://github.com/NathanielInman/bulwark", None),
            ],
            false,
            now,
        );
        let names: Vec<&str> = d.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["bulwark", "alpha", "zeta"]);
        assert_eq!(d.tags[0], ("digitalturbine".to_string(), 2));
        assert!(matches(&d.repos[1], "#digitalturbine"));
        assert!(matches(&d.repos[1], "al #gitlab"));
        assert!(!matches(&d.repos[0], "#digitalturbine"));
        assert!(matches(&d.repos[0], "#mine"));
        assert_eq!(d.counts(4), InspectionCounts { repos: 3, act: 1, tidy: 0, rev: 4 });
    }
}
