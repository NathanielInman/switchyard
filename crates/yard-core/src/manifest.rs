//! Task manifests: `.yard.toml` files, `tasks.d` overlays and `@yard` script
//! headers, resolved into [`TaskDef`]s.

use crate::config::Config;
use crate::paths;
use crate::size::parse_size;
use crate::Problem;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const MANIFEST_NAME: &str = ".yard.toml";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Confirm {
    #[default]
    None,
    Hold,
    Type,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NotifyMode {
    /// Failures always; success when the task had no run phase and took 20 s or more.
    #[default]
    Auto,
    Always,
    Failure,
    Never,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RetryOn {
    /// Exit status 128 or higher: the step was killed by a signal or crashed.
    #[default]
    Signal,
    /// Any non-zero exit.
    Failure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Rerun {
    /// Clicking a running task stops it and starts a fresh run.
    #[default]
    Restart,
    /// Clicking a running task does nothing but say so.
    Ignore,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Repo,
    Overlay,
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub name: String,
    pub cmd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Retry {
    /// 1-based step index; `None` means every step.
    pub step: Option<usize>,
    pub on: RetryOn,
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamDef {
    pub name: String,
    pub default: String,
    pub history: usize,
    pub capture: Option<String>,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Variant {
    pub name: String,
    pub label: String,
    pub params: BTreeMap<String, String>,
    pub steps: Option<Vec<Step>>,
    pub run: Option<String>,
    pub confirm: Option<Confirm>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Watch {
    pub pattern: String,
    pub title: String,
    pub urgency: String,
    pub throttle: u64,
}

/// A fully resolved task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDef {
    pub id: String,
    pub label: String,
    pub description: String,
    pub dir: PathBuf,
    pub tags: Vec<String>,
    pub pin: Option<i64>,
    pub heavy: bool,
    pub mem: Option<u64>,
    pub idle_cpu: Option<bool>,
    pub env: BTreeMap<String, String>,
    pub steps: Vec<Step>,
    pub run: Option<String>,
    pub log: PathBuf,
    pub clean: Option<String>,
    pub check: Option<String>,
    pub errors: Vec<String>,
    pub error_regex: Option<String>,
    pub confirm: Confirm,
    pub notify: NotifyMode,
    pub notify_tail: usize,
    pub shelf: bool,
    pub retry: Option<Retry>,
    pub params: Vec<ParamDef>,
    pub variants: Vec<Variant>,
    pub watch: Vec<Watch>,
    pub rerun: Rerun,
    pub source: PathBuf,
    pub origin: Origin,
}

impl TaskDef {
    pub fn variant(&self, name: &str) -> Option<&Variant> {
        self.variants.iter().find(|v| v.name == name)
    }

    /// A shell rendering of what the task runs, for "copy command".
    pub fn command_preview(&self) -> String {
        let mut parts: Vec<String> = vec![format!("cd {}", shell_quote(&self.dir.to_string_lossy()))];
        for s in &self.steps {
            parts.push(s.cmd.clone());
        }
        if let Some(c) = &self.check {
            parts.push(c.clone());
        }
        if let Some(r) = &self.run {
            parts.push(r.clone());
        }
        parts.join(" && ")
    }
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Characters systemd accepts in a unit name, everything else becomes `_`.
pub fn unit_safe(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

// ---------------------------------------------------------------------------
// Raw TOML shapes

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => s.split_whitespace().map(String::from).collect(),
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawStep {
    Cmd(String),
    Named { name: Option<String>, cmd: String },
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRetry {
    step: Option<usize>,
    on: Option<RetryOn>,
    attempts: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawDefault {
    Str(String),
    Int(i64),
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawParam {
    default: Option<RawDefault>,
    history: Option<usize>,
    capture: Option<String>,
    choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVariant {
    name: String,
    label: Option<String>,
    #[serde(default)]
    params: BTreeMap<String, toml::Value>,
    steps: Option<Vec<RawStep>>,
    run: Option<String>,
    confirm: Option<Confirm>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWatch {
    #[serde(rename = "match")]
    pattern: String,
    title: Option<String>,
    urgency: Option<String>,
    throttle: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTask {
    label: Option<String>,
    description: Option<String>,
    dir: Option<String>,
    tags: Option<OneOrMany>,
    pin: Option<i64>,
    heavy: Option<bool>,
    mem: Option<String>,
    idle_cpu: Option<bool>,
    env: Option<BTreeMap<String, String>>,
    steps: Option<Vec<RawStep>>,
    run: Option<String>,
    log: Option<String>,
    clean: Option<String>,
    check: Option<String>,
    errors: Option<OneOrMany>,
    error_regex: Option<String>,
    confirm: Option<Confirm>,
    notify: Option<NotifyMode>,
    notify_tail: Option<usize>,
    shelf: Option<bool>,
    retry: Option<RawRetry>,
    params: Option<BTreeMap<String, RawParam>>,
    variants: Option<Vec<RawVariant>>,
    watch: Option<Vec<RawWatch>>,
    rerun: Option<Rerun>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    defaults: Option<RawTask>,
    #[serde(default)]
    task: BTreeMap<String, RawTask>,
}

fn toml_value_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn steps_from(raw: Vec<RawStep>) -> Vec<Step> {
    raw.into_iter()
        .enumerate()
        .map(|(i, s)| match s {
            RawStep::Cmd(cmd) => Step { name: step_name(&cmd, i), cmd },
            RawStep::Named { name, cmd } => Step {
                name: name.unwrap_or_else(|| step_name(&cmd, i)),
                cmd,
            },
        })
        .collect()
}

/// A short name for an unnamed step: the first two words of the command.
fn step_name(cmd: &str, i: usize) -> String {
    let words: Vec<&str> = cmd
        .split_whitespace()
        .filter(|w| !w.contains('=') || w.starts_with('-'))
        .take(2)
        .collect();
    if words.is_empty() {
        format!("step {}", i + 1)
    } else {
        words
            .iter()
            .map(|w| w.rsplit('/').next().unwrap_or(w))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Merges a task over the file's `[defaults]`.
fn merge(t: RawTask, d: &RawTask) -> RawTask {
    let mut env = d.env.clone().unwrap_or_default();
    if let Some(e) = t.env {
        env.extend(e);
    }
    RawTask {
        label: t.label.or_else(|| d.label.clone()),
        description: t.description.or_else(|| d.description.clone()),
        dir: t.dir.or_else(|| d.dir.clone()),
        tags: t.tags.or_else(|| d.tags.clone()),
        pin: t.pin,
        heavy: t.heavy.or(d.heavy),
        mem: t.mem.or_else(|| d.mem.clone()),
        idle_cpu: t.idle_cpu.or(d.idle_cpu),
        env: Some(env),
        steps: t.steps.or_else(|| d.steps.clone()),
        run: t.run.or_else(|| d.run.clone()),
        log: t.log.or_else(|| d.log.clone()),
        clean: t.clean.or_else(|| d.clean.clone()),
        check: t.check.or_else(|| d.check.clone()),
        errors: t.errors.or_else(|| d.errors.clone()),
        error_regex: t.error_regex.or_else(|| d.error_regex.clone()),
        confirm: t.confirm.or(d.confirm),
        notify: t.notify.or(d.notify),
        notify_tail: t.notify_tail.or(d.notify_tail),
        shelf: t.shelf.or(d.shelf),
        retry: t.retry.or_else(|| d.retry.clone()),
        params: match (t.params, &d.params) {
            (Some(mut p), Some(dp)) => {
                for (k, v) in dp {
                    p.entry(k.clone()).or_insert_with(|| v.clone());
                }
                Some(p)
            }
            (p, dp) => p.or_else(|| dp.clone()),
        },
        variants: t.variants.or_else(|| d.variants.clone()),
        watch: t.watch.or_else(|| d.watch.clone()),
        rerun: t.rerun.or(d.rerun),
    }
}

struct Ctx<'a> {
    base: &'a Path,
    source: &'a Path,
    origin: Origin,
    log_dir: &'a Path,
}

fn resolve(id: &str, t: RawTask, ctx: &Ctx, problems: &mut Vec<Problem>) -> Option<TaskDef> {
    let file = paths::tilde(ctx.source);
    let problem = |problems: &mut Vec<Problem>, m: String| {
        problems.push(Problem { file: file.clone(), message: m });
    };
    if !valid_id(id) {
        problem(problems, format!("task id \"{id}\" may only use letters, digits, '-', '_' and '.'"));
        return None;
    }
    let steps = steps_from(t.steps.unwrap_or_default());
    if steps.is_empty() && t.run.is_none() {
        problem(problems, format!("task \"{id}\" has neither steps nor run"));
        return None;
    }
    let dir = match &t.dir {
        Some(d) => paths::expand_in(d, ctx.base),
        None => ctx.base.to_path_buf(),
    };
    let mem = match &t.mem {
        Some(m) => match parse_size(m) {
            Some(b) => Some(b),
            None => {
                problem(problems, format!("task \"{id}\": mem \"{m}\" is not a size like \"12G\""));
                None
            }
        },
        None => None,
    };
    let errors = t.errors.map(|e| e.into_vec()).unwrap_or_default();
    for e in &errors {
        if !crate::errors::MATCHERS.contains(&e.as_str()) {
            problem(
                problems,
                format!("task \"{id}\": unknown error matcher \"{e}\" (known: {})", crate::errors::MATCHERS.join(", ")),
            );
        }
    }
    if let Some(r) = &t.error_regex {
        if let Err(e) = regex::Regex::new(r) {
            problem(problems, format!("task \"{id}\": error_regex: {e}"));
        }
    }
    let mut params: Vec<ParamDef> = vec![];
    for (name, p) in t.params.unwrap_or_default() {
        if let Some(c) = &p.capture {
            if let Err(e) = regex::Regex::new(c) {
                problem(problems, format!("task \"{id}\": params.{name}.capture: {e}"));
            }
        }
        let default = match p.default {
            Some(RawDefault::Str(s)) => s,
            Some(RawDefault::Int(i)) => i.to_string(),
            None => p.choices.as_ref().and_then(|c| c.first().cloned()).unwrap_or_default(),
        };
        params.push(ParamDef {
            name,
            default,
            history: p.history.unwrap_or(10),
            capture: p.capture,
            choices: p.choices.unwrap_or_default(),
        });
    }
    let variants = t
        .variants
        .unwrap_or_default()
        .into_iter()
        .map(|v| Variant {
            label: v.label.unwrap_or_else(|| v.name.clone()),
            name: v.name,
            params: v.params.iter().map(|(k, v)| (k.clone(), toml_value_string(v))).collect(),
            steps: v.steps.map(steps_from),
            run: v.run,
            confirm: v.confirm,
        })
        .collect();
    let mut watch = vec![];
    for w in t.watch.unwrap_or_default() {
        if let Err(e) = regex::Regex::new(&w.pattern) {
            problem(problems, format!("task \"{id}\": watch match: {e}"));
            continue;
        }
        watch.push(Watch {
            title: w.title.unwrap_or_else(|| "output".into()),
            urgency: w.urgency.unwrap_or_else(|| "normal".into()),
            throttle: w.throttle.unwrap_or(15),
            pattern: w.pattern,
        });
    }
    let retry = t.retry.map(|r| Retry {
        step: r.step,
        on: r.on.unwrap_or_default(),
        attempts: r.attempts.unwrap_or(3).max(1),
    });
    let log = match &t.log {
        Some(l) => paths::expand_in(l, ctx.base),
        None => ctx.log_dir.join(format!("{}.log", unit_safe(id))),
    };
    let description = t.description.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    });
    Some(TaskDef {
        id: id.to_string(),
        label: t.label.unwrap_or_else(|| id.to_string()),
        description,
        dir,
        tags: t.tags.map(|t| t.into_vec()).unwrap_or_default(),
        pin: t.pin,
        heavy: t.heavy.unwrap_or(false),
        mem,
        idle_cpu: t.idle_cpu,
        env: t.env.unwrap_or_default(),
        steps,
        run: t.run,
        log,
        clean: t.clean,
        check: t.check,
        errors,
        error_regex: t.error_regex,
        confirm: t.confirm.unwrap_or_default(),
        notify: t.notify.unwrap_or_default(),
        notify_tail: t.notify_tail.unwrap_or(0),
        shelf: t.shelf.unwrap_or(false),
        retry,
        params,
        variants,
        watch,
        rerun: t.rerun.unwrap_or_default(),
        source: ctx.source.to_path_buf(),
        origin: ctx.origin,
    })
}

/// Parses one manifest file's text.
pub fn parse_manifest(
    text: &str,
    source: &Path,
    origin: Origin,
    log_dir: &Path,
    problems: &mut Vec<Problem>,
) -> Vec<TaskDef> {
    let raw: RawManifest = match toml::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            problems.push(Problem {
                file: paths::tilde(source),
                message: e.to_string().trim().to_string(),
            });
            return vec![];
        }
    };
    let base = source.parent().unwrap_or(Path::new("/"));
    let ctx = Ctx { base, source, origin, log_dir };
    let defaults = raw.defaults.unwrap_or_default();
    raw.task
        .into_iter()
        .filter_map(|(id, t)| resolve(&id, merge(t, &defaults), &ctx, problems))
        .collect()
}

/// Reads `@yard.key value` header comments from a script.
pub fn parse_script(text: &str, path: &Path, log_dir: &Path, problems: &mut Vec<Problem>) -> Option<TaskDef> {
    let mut keys: Vec<(String, String)> = vec![];
    for line in text.lines().take(60) {
        let l = line.trim_start();
        let Some(idx) = l.find("@yard.") else { continue };
        if !(l.starts_with('#') || l.starts_with("//") || l.starts_with("--")) {
            continue;
        }
        let rest = &l[idx + 6..];
        let (k, v) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        keys.push((k.to_string(), v.to_string()));
    }
    if keys.is_empty() {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let get = |k: &str| keys.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let id = get("id").unwrap_or_else(|| {
        stem.chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' })
            .collect()
    });
    let cmd = shell_quote(&path.to_string_lossy());
    let variants = keys
        .iter()
        .filter(|(k, _)| k == "alt")
        .enumerate()
        .map(|(i, (_, v))| {
            let (cmd_part, label) = match v.find(" # ") {
                Some(p) => (v[..p].trim().to_string(), v[p + 3..].trim().to_string()),
                None => (v.clone(), "alternate".to_string()),
            };
            RawVariant {
                name: if i == 0 { "alt".into() } else { format!("alt{}", i + 1) },
                label: Some(label),
                params: BTreeMap::new(),
                steps: Some(vec![RawStep::Cmd(cmd_part)]),
                run: None,
                confirm: None,
            }
        })
        .collect::<Vec<_>>();
    let raw = RawTask {
        label: get("label"),
        description: get("desc").or_else(|| Some(paths::tilde(path))),
        dir: Some(paths::home().to_string_lossy().to_string()),
        tags: get("tags").map(OneOrMany::One),
        pin: get("pin").and_then(|p| p.parse().ok()),
        heavy: get("heavy").map(|v| v.is_empty() || v == "true"),
        confirm: get("confirm").and_then(|v| match v.as_str() {
            "hold" => Some(Confirm::Hold),
            "type" => Some(Confirm::Type),
            _ => None,
        }),
        shelf: get("shelf").map(|v| v.is_empty() || v == "true"),
        steps: Some(vec![RawStep::Named { name: Some(stem.clone()), cmd }]),
        variants: if variants.is_empty() { None } else { Some(variants) },
        notify: Some(NotifyMode::Failure),
        ..Default::default()
    };
    let base = path.parent().unwrap_or(Path::new("/"));
    let ctx = Ctx { base, source: path, origin: Origin::Script, log_dir };
    resolve(&id, raw, &ctx, problems)
}

/// Everything discovery looked at, with modification times. Comparing two of
/// these is how the daemon decides to reload without reading every file.
pub type Fingerprint = Vec<(PathBuf, Option<SystemTime>)>;

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn script_files(cfg: &Config) -> Vec<PathBuf> {
    let mut out = vec![];
    for d in &cfg.scripts {
        let dir = paths::expand(d);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // Follow symlinks: ~/.local/bin is often full of them.
            if let Ok(m) = std::fs::metadata(&p) {
                if m.is_file() && is_executable(&p, &m) && m.len() < 512 * 1024 {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
fn is_executable(_p: &Path, m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

/// Windows has no execute bit: scripts are known by their extension.
#[cfg(not(unix))]
fn is_executable(p: &Path, _m: &std::fs::Metadata) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "ps1" | "cmd" | "bat" | "sh" | "py" | "exe")
}

fn overlay_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(paths::overlay_dir())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub fn fingerprint(cfg: &Config) -> Fingerprint {
    let mut fp = vec![];
    let cf = paths::config_file();
    fp.push((cf.clone(), mtime(&cf)));
    let od = paths::overlay_dir();
    fp.push((od.clone(), mtime(&od)));
    for f in overlay_files() {
        let m = mtime(&f);
        fp.push((f, m));
    }
    for d in cfg.scan_dirs() {
        let f = d.join(MANIFEST_NAME);
        let m = mtime(&f);
        fp.push((f, m));
    }
    for d in &cfg.scripts {
        let dir = paths::expand(d);
        let m = mtime(&dir);
        fp.push((dir, m));
    }
    for f in script_files(cfg) {
        let m = mtime(&f);
        fp.push((f, m));
    }
    fp
}

pub struct Discovery {
    pub tasks: Vec<TaskDef>,
    pub problems: Vec<Problem>,
}

/// Loads every task. Repo manifests first, then scripts, then overlays; a
/// later definition of the same id replaces the earlier one with a note.
pub fn discover(cfg: &Config) -> Discovery {
    let log_dir = cfg.log_dir();
    let mut problems = vec![];
    let mut tasks: Vec<TaskDef> = vec![];
    let push = |t: TaskDef, tasks: &mut Vec<TaskDef>, problems: &mut Vec<Problem>| {
        if let Some(i) = tasks.iter().position(|x| x.id == t.id) {
            problems.push(Problem {
                file: paths::tilde(&t.source),
                message: format!(
                    "task \"{}\" replaces the one in {}",
                    t.id,
                    paths::tilde(&tasks[i].source)
                ),
            });
            tasks[i] = t;
        } else {
            tasks.push(t);
        }
    };
    for d in cfg.scan_dirs() {
        let f = d.join(MANIFEST_NAME);
        if let Ok(text) = std::fs::read_to_string(&f) {
            for t in parse_manifest(&text, &f, Origin::Repo, &log_dir, &mut problems) {
                push(t, &mut tasks, &mut problems);
            }
        }
    }
    for f in script_files(cfg) {
        let Ok(bytes) = std::fs::read(&f) else { continue };
        let head = &bytes[..bytes.len().min(8192)];
        if !head.windows(6).any(|w| w == b"@yard.") {
            continue;
        }
        let text = String::from_utf8_lossy(head);
        if let Some(t) = parse_script(&text, &f, &log_dir, &mut problems) {
            push(t, &mut tasks, &mut problems);
        }
    }
    for f in overlay_files() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            for t in parse_manifest(&text, &f, Origin::Overlay, &log_dir, &mut problems) {
                push(t, &mut tasks, &mut problems);
            }
        }
    }
    Discovery { tasks, problems }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (Vec<TaskDef>, Vec<Problem>) {
        let mut p = vec![];
        let t = parse_manifest(text, Path::new("/code/game/.yard.toml"), Origin::Repo, Path::new("/logs"), &mut p);
        (t, p)
    }

    #[test]
    fn resolves_defaults_and_steps() {
        let (t, p) = parse(
            r#"
            [defaults]
            tags = ["games"]
            heavy = true
            env = { A = "1" }
            steps = ["dotnet build", { name = "import", cmd = "godot --headless --import" }]
            [defaults.retry]
            step = 2

            [task.lanternfall]
            run = "godot --path . res://Boot.tscn"
            pin = 1
            env = { B = "2" }

            [task.proving]
            label = "proving grounds"
            heavy = false
            run = "godot res://Workbench.tscn"
            "#,
        );
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(t.len(), 2);
        let l = t.iter().find(|t| t.id == "lanternfall").unwrap();
        assert!(l.heavy);
        assert_eq!(l.tags, vec!["games"]);
        assert_eq!(l.steps[0].name, "dotnet build");
        assert_eq!(l.steps[1].name, "import");
        assert_eq!(l.env.get("A").unwrap(), "1");
        assert_eq!(l.env.get("B").unwrap(), "2");
        assert_eq!(l.dir, PathBuf::from("/code/game"));
        assert_eq!(l.log, PathBuf::from("/logs/lanternfall.log"));
        assert_eq!(l.retry.as_ref().unwrap().step, Some(2));
        assert_eq!(l.retry.as_ref().unwrap().attempts, 3);
        assert_eq!(l.description, "game");
        let pg = t.iter().find(|t| t.id == "proving").unwrap();
        assert!(!pg.heavy);
        assert_eq!(pg.label, "proving grounds");
    }

    #[test]
    fn params_variants_watch() {
        let (t, p) = parse(
            r#"
            [task.hollowdeep]
            steps = ["make build"]
            run = "make run SEED={seed}"
            errors = "csharp godot"
            confirm = "hold"
            [task.hollowdeep.params.seed]
            default = "random:u30"
            capture = 'seed (\d+)'
            [task.hollowdeep.params.size]
            choices = ["small", "large"]
            [[task.hollowdeep.variants]]
            name = "small"
            params = { size = "small", depth = 3 }
            [[task.hollowdeep.watch]]
            match = '^ERROR:'
            throttle = 5
            "#,
        );
        assert!(p.is_empty(), "{p:?}");
        let h = &t[0];
        assert_eq!(h.errors, vec!["csharp", "godot"]);
        assert_eq!(h.confirm, Confirm::Hold);
        let seed = h.params.iter().find(|p| p.name == "seed").unwrap();
        assert_eq!(seed.default, "random:u30");
        let size = h.params.iter().find(|p| p.name == "size").unwrap();
        assert_eq!(size.default, "small");
        assert_eq!(h.variants[0].params["depth"], "3");
        assert_eq!(h.watch[0].throttle, 5);
        assert_eq!(h.watch[0].urgency, "normal");
    }

    #[test]
    fn reports_problems() {
        let (t, p) = parse(
            r#"
            [task."bad id"]
            run = "x"
            [task.empty]
            label = "nothing"
            [task.badmem]
            run = "x"
            mem = "lots"
            errors = ["klingon"]
            "#,
        );
        assert_eq!(t.len(), 1);
        assert_eq!(p.len(), 4, "{p:?}");
        let (t, p) = parse("[task.x]\nrun = 1");
        assert!(t.is_empty());
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn script_headers() {
        let text = "#!/bin/bash\n# @yard.label  rotate wallpaper\n# @yard.tags   desk misc\n# @yard.alt    pkill -USR2 -f wallpaper.sh # ban current\npkill -USR1 -f wallpaper.sh\n";
        let mut p = vec![];
        let t = parse_script(text, Path::new("/home/u/.local/bin/wallpaper-next"), Path::new("/logs"), &mut p).unwrap();
        assert_eq!(t.id, "wallpaper-next");
        assert_eq!(t.label, "rotate wallpaper");
        assert_eq!(t.tags, vec!["desk", "misc"]);
        assert_eq!(t.variants[0].label, "ban current");
        assert_eq!(t.variants[0].steps.as_ref().unwrap()[0].cmd, "pkill -USR2 -f wallpaper.sh");
        assert_eq!(t.steps[0].cmd, "/home/u/.local/bin/wallpaper-next");
        assert!(parse_script("#!/bin/sh\necho hi\n", Path::new("/x/y"), Path::new("/l"), &mut p).is_none());
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/a/b"), "/a/b");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(unit_safe("a.b c"), "a_b_c");
    }
}
