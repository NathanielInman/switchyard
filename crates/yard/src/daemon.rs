//! The daemon: one event loop that owns every task's state.

use crate::history::History;
use crate::spec::{exit_file, spec_file, RunSpec};
use crate::sys;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use yard_core::config::Config;
use yard_core::errors::{failing_section, find_first_error, ErrorHit};
use yard_core::manifest::{self, NotifyMode, Rerun, Step, TaskDef};
use yard_core::model::*;
use yard_core::paths;
use yard_core::protocol::*;
use yard_core::size::{human, systemd as systemd_size};
use yard_core::Problem;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

enum Msg {
    Client { req: Request, stream: UnixStream },
    Tick,
    Git(Vec<(PathBuf, Option<GitInfo>)>),
    Wake,
}

struct Sub {
    kind: SubKind,
    stream: UnixStream,
    last: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RunPhase {
    Build,
    Launch,
}

#[derive(Debug, Clone, Default)]
struct Pending {
    params: BTreeMap<String, String>,
    variant: Option<String>,
    clean: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunRec {
    id: u64,
    task: String,
    phase: RunPhase,
    unit: String,
    started: f64,
    run_cmd: Option<String>,
    heavy: bool,
    build_secs: f64,
    had_steps: bool,
    #[serde(default)]
    stopping: bool,
    #[serde(skip)]
    restart: Option<Pending>,
    #[serde(skip)]
    cgroup: Option<PathBuf>,
    #[serde(skip)]
    cg_tries: u32,
    #[serde(skip)]
    gone_since: Option<Instant>,
}

#[derive(Debug, Default)]
struct TaskRt {
    phase: Phase,
    since: f64,
    step: Option<StepInfo>,
    message: Option<String>,
    error: Option<ErrorHit>,
    run: Option<u64>,
    pending: Option<Pending>,
    mem: Option<u64>,
    cpu: Option<f32>,
    mem_hist: VecDeque<u64>,
    cpu_prev: Option<(u64, Instant)>,
    capture_offset: u64,
}

impl TaskRt {
    fn set(&mut self, phase: Phase) {
        self.phase = phase;
        self.since = now();
    }
    fn clear_live(&mut self) {
        self.run = None;
        self.step = None;
        self.mem = None;
        self.cpu = None;
        self.cpu_prev = None;
    }
}

pub struct Daemon {
    tx: Sender<Msg>,
    cfg: Config,
    tasks: Vec<TaskDef>,
    problems: Vec<Problem>,
    fp: manifest::Fingerprint,
    rt: HashMap<String, TaskRt>,
    runs: BTreeMap<u64, RunRec>,
    next_run: u64,
    queue: VecDeque<String>,
    mainline: Option<String>,
    history: History,
    history_dirty: bool,
    subs: Vec<Sub>,
    board_visible: bool,
    pending_board: Option<(Event, Instant)>,
    board_spawned: Option<Instant>,
    git: HashMap<PathBuf, GitInfo>,
    git_at: Option<Instant>,
    git_busy: bool,
    shelf: Option<(ShelfData, Instant)>,
    ticks: u64,
    dirty: bool,
    base_env: BTreeMap<String, String>,
}

pub fn run() -> Result<()> {
    let rt = paths::runtime_dir();
    std::fs::create_dir_all(&rt)?;
    let _ = std::fs::set_permissions(&rt, std::fs::Permissions::from_mode(0o700));
    std::fs::create_dir_all(paths::runs_dir())?;
    let sock = paths::socket_path();
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            bail!("a Switchyard daemon is already running on {}", sock.display());
        }
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
        libc::signal(libc::SIGHUP, on_signal as *const () as usize);
    }

    let (tx, rx) = channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || handle_conn(stream, tx));
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(1000));
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        });
    }

    let mut d = Daemon::new(tx);
    d.reload(true);
    d.adopt();
    eprintln!(
        "switchyard: {} tasks, socket {}",
        d.tasks.len(),
        sock.display()
    );
    d.event_loop(rx);
    d.history.save();
    d.save_runs();
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

fn handle_conn(stream: UnixStream, tx: Sender<Msg>) {
    let Ok(read_half) = stream.try_clone() else { return };
    let reader = BufReader::new(read_half);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let sub = matches!(req, Request::Subscribe { .. });
                let Ok(s) = stream.try_clone() else { break };
                if tx.send(Msg::Client { req, stream: s }).is_err() {
                    break;
                }
                if sub {
                    return;
                }
            }
            Err(e) => {
                let mut s = &stream;
                let r = Reply::err(format!("bad request: {e}"));
                let _ = writeln!(s, "{}", serde_json::to_string(&r).unwrap_or_default());
            }
        }
    }
}

fn write_line(stream: &mut UnixStream, line: &str) -> bool {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
    stream.write_all(line.as_bytes()).is_ok() && stream.write_all(b"\n").is_ok()
}

/// "2026-11-09 22:41:07" in local time.
pub fn local_time(secs: f64) -> String {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    let n = unsafe { libc::getrandom(b.as_mut_ptr() as *mut libc::c_void, 8, 0) };
    if n == 8 {
        u64::from_le_bytes(b)
    } else {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (t.as_nanos() as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
    }
}

/// `random:u30` and friends become a number; anything else is literal.
fn resolve_value(v: &str) -> String {
    let Some(kind) = v.strip_prefix("random") else { return v.to_string() };
    let bits = kind.trim_start_matches(':').trim_start_matches('u').parse::<u32>().unwrap_or(30).clamp(1, 63);
    (random_u64() & ((1u64 << bits) - 1)).to_string()
}

fn substitute(cmd: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = cmd.to_string();
    for (k, v) in params {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn read_tail(path: &Path, max: u64) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > max {
        let _ = f.seek(SeekFrom::Start(len - max));
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

impl Daemon {
    fn new(tx: Sender<Msg>) -> Daemon {
        let (cfg, problems) = Config::load();
        let start = (now() as u64 % 10_000_000) * 100;
        Daemon {
            tx,
            cfg,
            tasks: vec![],
            problems,
            fp: vec![],
            rt: HashMap::new(),
            runs: BTreeMap::new(),
            next_run: start,
            queue: VecDeque::new(),
            mainline: None,
            history: History::load(),
            history_dirty: false,
            subs: vec![],
            board_visible: false,
            pending_board: None,
            board_spawned: None,
            git: HashMap::new(),
            git_at: None,
            git_busy: false,
            shelf: None,
            ticks: 0,
            dirty: true,
            base_env: BTreeMap::new(),
        }
    }

    fn event_loop(&mut self, rx: Receiver<Msg>) {
        while !SHUTDOWN.load(Ordering::SeqCst) {
            let msg = match rx.recv_timeout(Duration::from_millis(1500)) {
                Ok(m) => m,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            match msg {
                Msg::Client { req, stream } => self.handle(req, stream),
                Msg::Tick => self.tick(),
                Msg::Git(r) => {
                    self.git_busy = false;
                    for (dir, info) in r {
                        match info {
                            Some(g) => {
                                self.git.insert(dir, g);
                            }
                            None => {
                                self.git.remove(&dir);
                            }
                        }
                    }
                    self.dirty = true;
                }
                Msg::Wake => {}
            }
            self.flush();
        }
    }

    // ----------------------------------------------------------- loading

    fn reload(&mut self, force: bool) {
        let (cfg, cfg_problems) = Config::load();
        let fp = manifest::fingerprint(&cfg);
        if !force && fp == self.fp {
            return;
        }
        self.fp = fp;
        self.cfg = cfg;
        let found = manifest::discover(&self.cfg);
        let mut tasks = found.tasks;
        // A task that disappears while it runs stays until it finishes.
        for t in &self.tasks {
            let active = self.rt.get(&t.id).is_some_and(|r| r.phase.active());
            if active && !tasks.iter().any(|n| n.id == t.id) {
                tasks.push(t.clone());
            }
        }
        self.tasks = tasks;
        self.problems = cfg_problems;
        self.problems.extend(found.problems);
        for t in &self.tasks {
            self.rt.entry(t.id.clone()).or_insert_with(|| TaskRt { since: now(), ..Default::default() });
        }
        let ids: Vec<String> = self.tasks.iter().map(|t| t.id.clone()).collect();
        self.rt.retain(|k, _| ids.contains(k));
        self.queue.retain(|k| ids.contains(k));
        self.base_env = self.compute_env();
        self.shelf = None;
        self.dirty = true;
    }

    fn compute_env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let pass = [
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "LANG",
            "LC_ALL",
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_SESSION_DESKTOP",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "SSH_AUTH_SOCK",
            "SWITCHYARD_CONFIG_DIR",
            "SWITCHYARD_STATE_DIR",
        ];
        for k in pass.iter().map(|s| s.to_string()).chain(self.cfg.env_passthrough.iter().cloned()) {
            if let Ok(v) = std::env::var(&k) {
                env.insert(k, v);
            }
        }
        let mut path: Vec<String> = self
            .cfg
            .path_prepend
            .iter()
            .map(|p| paths::expand(p).to_string_lossy().to_string())
            .collect();
        for p in std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()).split(':') {
            if !p.is_empty() && !path.iter().any(|x| x == p) {
                path.push(p.to_string());
            }
        }
        env.insert("PATH".into(), path.join(":"));
        env.insert("SWITCHYARD_RUNTIME_DIR".into(), paths::runtime_dir().to_string_lossy().to_string());
        env
    }

    fn def(&self, id: &str) -> Option<&TaskDef> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn rt(&mut self, id: &str) -> &mut TaskRt {
        self.rt.entry(id.to_string()).or_default()
    }

    // ----------------------------------------------------------- requests

    fn handle(&mut self, req: Request, mut stream: UnixStream) {
        let reply = match req {
            Request::Ping => Reply::msg(format!("switchyard {}", env!("CARGO_PKG_VERSION"))),
            Request::Snapshot => Reply {
                ok: true,
                snapshot: Some(Box::new(self.snapshot())),
                ..Default::default()
            },
            Request::Subscribe { kind } => {
                self.subscribe(kind, stream);
                return;
            }
            Request::Run { id, params, variant, clean } => self.request_run(&id, Pending { params, variant, clean }),
            Request::Stop { id } => self.stop(&id),
            Request::Ack { phase, id } => self.ack(phase, id.as_deref()),
            Request::Toggle { anchor } => self.board_event(Event::Toggle { anchor }),
            Request::Show { anchor } => self.board_event(Event::Show { anchor }),
            Request::Hide => self.board_event(Event::Hide),
            Request::BoardVisible { visible } => {
                self.board_visible = visible;
                if visible {
                    self.refresh_git(false);
                }
                Reply::ok()
            }
            Request::Shelf => Reply {
                ok: true,
                shelf: Some(self.shelf_data()),
                ..Default::default()
            },
            Request::Reload => {
                self.reload(true);
                Reply::msg(format!("{} tasks, {} problems", self.tasks.len(), self.problems.len()))
            }
            Request::Star { id, param, value, starred } => {
                self.history.star(&id, &param, &value, starred, now());
                self.history_dirty = true;
                self.dirty = true;
                Reply::ok()
            }
            Request::Shelve { id, shelved } => {
                self.history.shelved.insert(id, shelved);
                self.history_dirty = true;
                self.dirty = true;
                Reply::ok()
            }
            Request::Touch { id } => {
                self.history.record_start(&id, now());
                self.history_dirty = true;
                self.dirty = true;
                Reply::ok()
            }
            Request::Open { what, id, file, line, tool } => self.open(what, id, file, line, tool),
            Request::Progress { run, index, total, name, attempt } => {
                if let Some(rec) = self.runs.get(&run) {
                    let task = rec.task.clone();
                    let r = self.rt(&task);
                    r.step = Some(StepInfo { index, total, name, attempt });
                    self.dirty = true;
                }
                Reply::ok()
            }
            Request::Finished { run, phase: _, code, oom } => {
                self.finished(run, code, oom);
                Reply::ok()
            }
            Request::Shutdown => {
                SHUTDOWN.store(true, Ordering::SeqCst);
                Reply::msg("shutting down")
            }
        };
        let line = serde_json::to_string(&reply).unwrap_or_default();
        write_line(&mut stream, &line);
    }

    fn subscribe(&mut self, kind: SubKind, stream: UnixStream) {
        let mut sub = Sub { kind, stream, last: None };
        let ok = match kind {
            SubKind::Bar => {
                let line = self.bar_line();
                sub.last = Some(line.clone());
                write_line(&mut sub.stream, &line)
            }
            _ => {
                let ev = Event::Snapshot(Box::new(self.snapshot()));
                write_line(&mut sub.stream, &serde_json::to_string(&ev).unwrap_or_default())
            }
        };
        if !ok {
            return;
        }
        if kind == SubKind::Board {
            if let Some((ev, at)) = self.pending_board.take() {
                if at.elapsed() < Duration::from_secs(10) {
                    write_line(&mut sub.stream, &serde_json::to_string(&ev).unwrap_or_default());
                }
            }
        }
        self.subs.push(sub);
    }

    fn board_event(&mut self, ev: Event) -> Reply {
        let line = serde_json::to_string(&ev).unwrap_or_default();
        let mut delivered = false;
        for s in self.subs.iter_mut().filter(|s| s.kind == SubKind::Board) {
            if write_line(&mut s.stream, &line) {
                delivered = true;
            }
        }
        self.subs.retain(|s| s.kind != SubKind::Board || s.stream.peer_addr().is_ok());
        if delivered {
            return Reply::ok();
        }
        // No board yet: start one and hand it the event when it connects.
        let ev = match ev {
            Event::Toggle { anchor } => Event::Show { anchor },
            Event::Hide => return Reply::msg("the board is not running"),
            other => other,
        };
        self.pending_board = Some((ev, Instant::now()));
        if self.board_spawned.is_none_or(|t| t.elapsed() > Duration::from_secs(8)) {
            self.board_spawned = Some(Instant::now());
            self.spawn_board();
        }
        Reply::msg("starting the board")
    }

    fn spawn_board(&self) {
        let has_unit = sys::systemctl_user(&["cat", "switchyard-board.service"]).is_some_and(|t| !t.is_empty());
        if has_unit {
            let mut c = Command::new("systemctl");
            c.args(["--user", "start", "switchyard-board.service"]);
            let _ = sys::spawn_detached(c);
            return;
        }
        let bin = sys::sibling_binary("yard-board");
        let mut c = Command::new(bin);
        let log = paths::state_dir().join("board.log");
        let _ = std::fs::create_dir_all(paths::state_dir());
        unsafe {
            use std::os::unix::process::CommandExt;
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            if let Ok(f2) = f.try_clone() {
                c.stdout(f).stderr(f2);
            }
        }
        if let Ok(mut child) = c.spawn() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    fn ack(&mut self, phase: AckPhase, id: Option<&str>) -> Reply {
        let mut changed = 0;
        for (tid, r) in self.rt.iter_mut() {
            if id.is_some_and(|i| i != tid) {
                continue;
            }
            let next = match (phase, r.phase) {
                (AckPhase::Open, Phase::Failed) => Some(Phase::Seen),
                (AckPhase::Close, Phase::Seen) => Some(Phase::Idle),
                (AckPhase::All, Phase::Failed | Phase::Seen) => Some(Phase::Idle),
                _ => None,
            };
            if let Some(p) = next {
                r.phase = p;
                if p == Phase::Idle {
                    r.message = None;
                }
                changed += 1;
            }
        }
        if changed > 0 {
            self.dirty = true;
        }
        Reply::msg(format!("{changed} acknowledged"))
    }

    // ----------------------------------------------------------- running

    fn request_run(&mut self, id: &str, p: Pending) -> Reply {
        let Some(def) = self.def(id).cloned() else {
            return Reply::err(format!("no task named \"{id}\""));
        };
        if let Some(v) = &p.variant {
            if def.variant(v).is_none() {
                return Reply::err(format!("{} has no variant \"{v}\"", def.label));
            }
        }
        let phase = self.rt(id).phase;
        match phase {
            Phase::Queued | Phase::Waiting => return Reply::msg(format!("{} is already queued", def.label)),
            Phase::Building => return Reply::msg(format!("{} is already building", def.label)),
            Phase::Running => {
                if def.rerun == Rerun::Ignore {
                    return Reply::msg(format!("{} is already running", def.label));
                }
                let run = self.rt(id).run;
                if let Some(rec) = run.and_then(|r| self.runs.get_mut(&r)) {
                    rec.stopping = true;
                    rec.restart = Some(p);
                    let unit = rec.unit.clone();
                    stop_unit(&unit);
                    self.dirty = true;
                    return Reply::msg(format!("restarting {}", def.label));
                }
            }
            _ => {}
        }
        let has_steps = !self.steps_for(&def, &p).is_empty();
        if def.heavy && has_steps {
            if !self.queue.contains(&def.id) {
                self.queue.push_back(def.id.clone());
            }
            let r = self.rt(id);
            r.set(Phase::Queued);
            r.pending = Some(p);
            r.message = None;
            r.error = None;
            self.dirty = true;
            self.schedule();
            let phase = self.rt(id).phase;
            return match phase {
                Phase::Building => Reply::msg(format!("building {}", def.label)),
                Phase::Waiting => Reply::msg(format!("{} is waiting for memory", def.label)),
                _ => Reply::msg(format!("{} queued behind the main line", def.label)),
            };
        }
        self.start(&def, p);
        Reply::msg(format!("started {}", def.label))
    }

    fn steps_for(&self, def: &TaskDef, p: &Pending) -> Vec<Step> {
        let variant = p.variant.as_ref().and_then(|v| def.variant(v));
        let mut steps = variant.and_then(|v| v.steps.clone()).unwrap_or_else(|| def.steps.clone());
        if p.clean {
            if let Some(c) = &def.clean {
                steps.insert(0, Step { name: "clean".into(), cmd: c.clone() });
            }
        }
        if let Some(c) = &def.check {
            steps.push(Step { name: "check".into(), cmd: c.clone() });
        }
        steps
    }

    fn schedule(&mut self) {
        loop {
            if self.mainline.is_some() {
                break;
            }
            let Some(head) = self.queue.front().cloned() else { break };
            if self.def(&head).is_none() {
                self.queue.pop_front();
                continue;
            }
            let min_free = self.cfg.mainline.min_free_bytes();
            let mem = sys::meminfo();
            if mem.available < min_free {
                let r = self.rt(&head);
                if r.phase != Phase::Waiting {
                    r.set(Phase::Waiting);
                }
                r.message = Some(format!(
                    "waiting for {} free ({} available)",
                    human(min_free),
                    human(mem.available)
                ));
                self.dirty = true;
                break;
            }
            self.queue.pop_front();
            self.mainline = Some(head.clone());
            let p = self.rt(&head).pending.take().unwrap_or_default();
            let def = self.def(&head).cloned().unwrap();
            self.start(&def, p);
        }
        // Everyone behind the head shows as queued.
        let queue: Vec<String> = self.queue.iter().skip(1).cloned().collect();
        for id in queue {
            let r = self.rt(&id);
            if r.phase == Phase::Waiting {
                r.set(Phase::Queued);
                r.message = None;
            }
        }
    }

    fn release_mainline(&mut self, task: &str) {
        if self.mainline.as_deref() == Some(task) {
            self.mainline = None;
        }
    }

    /// Starts a task's build phase (or its run phase when it has no steps).
    fn start(&mut self, def: &TaskDef, p: Pending) {
        let at = now();
        let variant = p.variant.as_ref().and_then(|v| def.variant(v)).cloned();
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        for pd in &def.params {
            let v = p
                .params
                .get(&pd.name)
                .cloned()
                .or_else(|| variant.as_ref().and_then(|v| v.params.get(&pd.name).cloned()))
                .unwrap_or_else(|| pd.default.clone());
            params.insert(pd.name.clone(), resolve_value(&v));
        }
        if let Some(v) = &variant {
            for (k, val) in &v.params {
                params.entry(k.clone()).or_insert_with(|| resolve_value(val));
            }
        }
        for (k, v) in &p.params {
            params.entry(k.clone()).or_insert_with(|| v.clone());
        }
        let steps: Vec<Step> = self
            .steps_for(def, &p)
            .into_iter()
            .map(|s| Step { name: s.name, cmd: substitute(&s.cmd, &params) })
            .collect();
        let run_cmd = variant
            .as_ref()
            .and_then(|v| v.run.clone())
            .or_else(|| def.run.clone())
            .map(|r| substitute(&r, &params));

        self.history.record_start(&def.id, at);
        for pd in &def.params {
            if let Some(v) = params.get(&pd.name) {
                self.history.record_param(&def.id, &pd.name, v, at, pd.history);
            }
        }
        self.history_dirty = true;

        // Fresh log: keep the previous run as .1.
        if let Some(d) = def.log.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if def.log.exists() {
            let _ = std::fs::rename(&def.log, def.log.with_extension("log.1"));
        }
        let run = self.next_run;
        self.next_run += 1;
        {
            let mut header = format!("===== switchyard · {} · {} =====\n", def.label, local_time(at));
            if let Some(v) = &variant {
                header.push_str(&format!("variant: {}\n", v.label));
            }
            if !params.is_empty() {
                let ps: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
                header.push_str(&format!("params: {}\n", ps.join(" ")));
            }
            let _ = std::fs::write(&def.log, header);
        }
        let log_len = std::fs::metadata(&def.log).map(|m| m.len()).unwrap_or(0);

        let scratch = paths::runs_dir().join(format!("{run}.scratch"));
        let _ = std::fs::create_dir_all(&scratch);
        let spec = RunSpec {
            run,
            task: def.id.clone(),
            label: def.label.clone(),
            steps: steps.clone(),
            run_cmd: run_cmd.clone(),
            log: def.log.clone(),
            retry: def.retry.clone(),
            watch: def.watch.clone(),
            scratch: scratch.clone(),
        };
        let _ = std::fs::create_dir_all(paths::runs_dir());
        if let Err(e) = std::fs::write(spec_file(run), serde_json::to_string(&spec).unwrap_or_default()) {
            self.release_mainline(&def.id);
            self.fail_now(&def.id, format!("could not write the run spec: {e}"));
            return;
        }
        let heavy = def.heavy && !steps.is_empty();
        let rec = RunRec {
            id: run,
            task: def.id.clone(),
            phase: if steps.is_empty() { RunPhase::Launch } else { RunPhase::Build },
            unit: String::new(),
            started: at,
            run_cmd,
            heavy,
            build_secs: 0.0,
            had_steps: !steps.is_empty(),
            stopping: false,
            restart: None,
            cgroup: None,
            cg_tries: 0,
            gone_since: None,
        };
        {
            let r = self.rt(&def.id);
            r.error = None;
            r.message = None;
            r.mem_hist.clear();
            r.capture_offset = log_len;
            r.pending = None;
        }
        self.launch_unit(def, rec);
    }

    fn launch_unit(&mut self, def: &TaskDef, mut rec: RunRec) {
        let phase_name = match rec.phase {
            RunPhase::Build => "build",
            RunPhase::Launch => "launch",
        };
        let unit = format!("switchyard-{}-{}-{}", manifest::unit_safe(&def.id), rec.id, phase_name);
        rec.unit = unit.clone();
        if !def.dir.is_dir() {
            self.release_mainline(&def.id);
            self.fail_now(&def.id, format!("directory not found: {}", paths::tilde(&def.dir)));
            return;
        }
        let mut c = Command::new("systemd-run");
        c.args(["--user", "--quiet", "--collect"]);
        c.arg(format!("--unit={unit}"));
        c.arg(format!("--description=switchyard: {} {}", def.label, phase_name));
        c.arg(format!("--working-directory={}", def.dir.display()));
        c.args(["-p", "KillMode=control-group", "-p", "TimeoutStopSec=10"]);
        if rec.phase == RunPhase::Build {
            let mem = if rec.heavy {
                Some(def.mem.unwrap_or(self.cfg.mainline.mem_bytes()))
            } else {
                def.mem
            };
            if let Some(m) = mem {
                c.args(["-p", &format!("MemoryMax={}", systemd_size(m)), "-p", "MemorySwapMax=0"]);
            }
            let idle = def.idle_cpu.unwrap_or(rec.heavy && self.cfg.mainline.idle_cpu);
            if idle {
                c.args(["-p", "CPUWeight=idle", "-p", &format!("Nice={}", self.cfg.mainline.nice)]);
            }
        }
        let mut env = self.base_env.clone();
        for (k, v) in &def.env {
            env.insert(k.clone(), paths::expand(v).to_string_lossy().to_string());
        }
        env.insert("YARD_TASK".into(), def.id.clone());
        env.insert("YARD_RUN".into(), rec.id.to_string());
        env.insert(
            "YARD_SCRATCH".into(),
            paths::runs_dir().join(format!("{}.scratch", rec.id)).to_string_lossy().to_string(),
        );
        env.insert("YARD_LOG".into(), def.log.to_string_lossy().to_string());
        for (k, v) in &env {
            c.arg(format!("--setenv={k}={v}"));
        }
        let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("yard"));
        c.arg("--").arg(me).arg(format!("__{phase_name}")).arg(spec_file(rec.id));
        c.stdin(std::process::Stdio::null());
        let out = c.output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                self.release_mainline(&def.id);
                self.fail_now(&def.id, format!("systemd-run failed: {err}"));
                return;
            }
            Err(e) => {
                self.release_mainline(&def.id);
                self.fail_now(&def.id, format!("could not run systemd-run: {e}"));
                return;
            }
        }
        let r = self.rt(&def.id);
        r.run = Some(rec.id);
        match rec.phase {
            RunPhase::Build => {
                r.set(Phase::Building);
                r.step = Some(StepInfo { index: 0, total: 0, name: "starting".into(), attempt: 1 });
            }
            RunPhase::Launch => {
                r.set(Phase::Running);
                r.step = None;
            }
        }
        self.runs.insert(rec.id, rec);
        self.save_runs();
        self.dirty = true;
    }

    fn stop(&mut self, id: &str) -> Reply {
        let Some(label) = self.def(id).map(|d| d.label.clone()) else {
            return Reply::err(format!("no task named \"{id}\""));
        };
        let phase = self.rt(id).phase;
        match phase {
            Phase::Queued | Phase::Waiting => {
                self.queue.retain(|q| q != id);
                let r = self.rt(id);
                r.set(Phase::Idle);
                r.pending = None;
                r.message = None;
                self.dirty = true;
                self.schedule();
                Reply::msg(format!("{label} removed from the queue"))
            }
            Phase::Building | Phase::Running => {
                let run = self.rt(id).run;
                if let Some(rec) = run.and_then(|r| self.runs.get_mut(&r)) {
                    rec.stopping = true;
                    rec.restart = None;
                    let unit = rec.unit.clone();
                    stop_unit(&unit);
                }
                Reply::msg(format!("stopping {label}"))
            }
            _ => Reply::msg(format!("{label} is not running")),
        }
    }

    fn finished(&mut self, run: u64, code: i32, oom: bool) {
        let Some(rec) = self.runs.remove(&run) else { return };
        self.save_runs();
        let _ = std::fs::remove_file(exit_file(run, "build"));
        if code != 0 {
            let mut c = Command::new("systemctl");
            c.args(["--user", "reset-failed", &rec.unit]);
            let _ = sys::spawn_detached(c);
        }
        match rec.phase {
            RunPhase::Build => self.end_build(rec, code, oom),
            RunPhase::Launch => self.end_launch(rec, code, oom),
        }
        self.dirty = true;
        self.schedule();
    }

    fn cleanup_run(&self, run: u64) {
        let _ = std::fs::remove_file(spec_file(run));
        let _ = std::fs::remove_file(exit_file(run, "build"));
        let _ = std::fs::remove_file(exit_file(run, "launch"));
        let _ = std::fs::remove_dir_all(paths::runs_dir().join(format!("{run}.scratch")));
    }

    fn end_build(&mut self, mut rec: RunRec, code: i32, oom: bool) {
        self.release_mainline(&rec.task);
        let Some(def) = self.def(&rec.task).cloned() else {
            self.cleanup_run(rec.id);
            return;
        };
        let secs = now() - rec.started;
        if rec.stopping {
            self.cleanup_run(rec.id);
            self.stopped(&def, rec.restart.take());
            return;
        }
        if code == 0 {
            self.history.record_duration(&def.id, secs);
            self.history_dirty = true;
            if rec.run_cmd.is_some() {
                rec.phase = RunPhase::Launch;
                rec.build_secs = secs;
                rec.cgroup = None;
                rec.cg_tries = 0;
                rec.gone_since = None;
                if def.notify == NotifyMode::Always || (def.notify == NotifyMode::Auto && secs >= 20.0) {
                    sys::notify(&def.label, &format!("Built in {}, launching", duration(secs)), "low");
                }
                self.rt(&def.id).clear_live();
                self.launch_unit(&def, rec);
            } else {
                self.cleanup_run(rec.id);
                self.succeed(&def, secs, true);
            }
        } else {
            let step = self.rt(&def.id).step.as_ref().map(|s| s.index).filter(|i| *i > 0);
            self.cleanup_run(rec.id);
            self.fail(&def, code, oom, step, secs, true);
        }
    }

    fn end_launch(&mut self, mut rec: RunRec, code: i32, oom: bool) {
        self.cleanup_run(rec.id);
        let Some(def) = self.def(&rec.task).cloned() else { return };
        let secs = now() - rec.started;
        if rec.stopping {
            self.stopped(&def, rec.restart.take());
            return;
        }
        if code == 0 {
            self.succeed(&def, if rec.had_steps { rec.build_secs } else { secs }, false);
        } else {
            self.fail(&def, code, oom, None, secs, false);
        }
    }

    fn stopped(&mut self, def: &TaskDef, restart: Option<Pending>) {
        {
            let r = self.rt(&def.id);
            r.clear_live();
            r.set(Phase::Idle);
            r.message = None;
        }
        self.history.task(&def.id).last = Some(LastRun {
            outcome: Outcome::Stopped,
            at: now(),
            duration: 0.0,
            message: None,
        });
        self.history_dirty = true;
        if let Some(p) = restart {
            let id = def.id.clone();
            let _ = self.request_run(&id, p);
        }
    }

    fn succeed(&mut self, def: &TaskDef, secs: f64, build_only: bool) {
        {
            let r = self.rt(&def.id);
            r.clear_live();
            r.set(Phase::Ok);
            r.message = None;
            r.error = None;
        }
        self.history.task(&def.id).last = Some(LastRun { outcome: Outcome::Ok, at: now(), duration: secs, message: None });
        self.history_dirty = true;
        let notify = match def.notify {
            NotifyMode::Always => true,
            NotifyMode::Auto => build_only && secs >= 20.0,
            _ => false,
        };
        if notify || def.notify_tail > 0 && def.notify != NotifyMode::Never && def.notify != NotifyMode::Failure {
            let mut body = format!("Done in {}", duration(secs));
            if def.notify_tail > 0 {
                let tail = read_tail(&def.log, 64 * 1024);
                let lines: Vec<&str> = tail.lines().filter(|l| !l.starts_with("=====")).collect();
                let start = lines.len().saturating_sub(def.notify_tail);
                body = lines[start..].join("\n");
            }
            sys::notify(&def.label, &body, "normal");
        }
    }

    fn fail_now(&mut self, id: &str, message: String) {
        let label = self.def(id).map(|d| d.label.clone()).unwrap_or_else(|| id.to_string());
        let notify = self.def(id).map(|d| d.notify != NotifyMode::Never).unwrap_or(true);
        {
            let r = self.rt(id);
            r.clear_live();
            r.set(Phase::Failed);
            r.message = Some(message.clone());
            r.error = None;
        }
        self.history.task(id).last = Some(LastRun { outcome: Outcome::Failed, at: now(), duration: 0.0, message: Some(message.clone()) });
        self.history_dirty = true;
        self.dirty = true;
        if notify {
            sys::notify(&format!("{label} failed"), &message, "critical");
        }
    }

    fn fail(&mut self, def: &TaskDef, code: i32, oom: bool, step: Option<usize>, secs: f64, build: bool) {
        let text = read_tail(&def.log, 4 * 1024 * 1024);
        let lines: Vec<&str> = text.lines().collect();
        let (offset, section) = failing_section(&lines, if build { step } else { None });
        let custom = def.error_regex.as_ref().and_then(|r| regex::Regex::new(r).ok());
        let mut hit = find_first_error(section, &def.errors, custom.as_ref());
        if let Some(h) = hit.as_mut() {
            h.index += offset;
        }
        let total = self.rt(&def.id).step.as_ref().map(|s| s.total).unwrap_or(0);
        let message = if oom {
            let cap = def.mem.unwrap_or(self.cfg.mainline.mem_bytes());
            format!("killed: hit the {} memory cap", human(cap))
        } else if !build {
            if code >= 128 {
                format!("crashed (signal {})", code - 128)
            } else {
                format!("exited with {code}")
            }
        } else if let Some(s) = step {
            if code >= 128 {
                format!("step {s} of {total} crashed (signal {})", code - 128)
            } else {
                format!("exit {code} in step {s} of {total}")
            }
        } else {
            format!("exit {code}")
        };
        let body = hit.as_ref().map(|h| h.text.clone()).unwrap_or_else(|| {
            lines
                .iter()
                .rev()
                .filter(|l| !l.starts_with("=====") && !l.trim().is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        });
        {
            let r = self.rt(&def.id);
            r.clear_live();
            r.set(Phase::Failed);
            r.message = Some(message.clone());
            r.error = hit;
        }
        self.history.task(&def.id).last = Some(LastRun {
            outcome: Outcome::Failed,
            at: now(),
            duration: secs,
            message: Some(message.clone()),
        });
        self.history_dirty = true;
        if def.notify != NotifyMode::Never {
            let body = if body.is_empty() { message } else { format!("{message}\n{body}") };
            sys::notify(&format!("{} failed", def.label), &body, "critical");
        }
    }

    // ----------------------------------------------------------- the clock

    fn tick(&mut self) {
        self.ticks += 1;
        let t = now();
        let flash = self.cfg.ok_flash as f64;
        for r in self.rt.values_mut() {
            if r.phase == Phase::Ok && t - r.since >= flash {
                r.phase = Phase::Idle;
                self.dirty = true;
            }
        }
        let ids: Vec<u64> = self.runs.keys().copied().collect();
        let sample = self.ticks % 2 == 0;
        for id in ids {
            self.watch_run(id, sample);
        }
        if !self.runs.is_empty() && sample {
            self.dirty = true;
        }
        self.scan_captures();
        if !self.queue.is_empty() && self.mainline.is_none() {
            self.schedule();
        }
        if self.ticks % 3 == 0 {
            self.reload(false);
        }
        if self.board_visible && self.git_at.is_none_or(|g| g.elapsed() > Duration::from_secs(30)) {
            self.refresh_git(false);
        }
        if self.history_dirty && self.ticks % 2 == 0 {
            self.history.save();
            self.history_dirty = false;
        }
    }

    fn watch_run(&mut self, id: u64, sample: bool) {
        let Some(rec) = self.runs.get_mut(&id) else { return };
        if rec.cgroup.is_none() && rec.cg_tries < 4 {
            rec.cg_tries += 1;
            rec.cgroup = sys::cgroup_path(&rec.unit);
        }
        let alive = rec.cgroup.as_ref().is_some_and(|c| sys::cgroup_alive(c));
        if alive {
            rec.gone_since = None;
            let cg = rec.cgroup.clone().unwrap();
            let task = rec.task.clone();
            let mem = sys::read_u64(&cg.join("memory.current"));
            let usage = sys::cpu_usage_usec(&cg);
            let r = self.rt(&task);
            r.mem = mem;
            if let Some(u) = usage {
                let nowi = Instant::now();
                if let Some((pu, pt)) = r.cpu_prev {
                    let wall = nowi.duration_since(pt).as_micros().max(1) as f64;
                    r.cpu = Some(((u.saturating_sub(pu)) as f64 / wall * 100.0) as f32);
                }
                r.cpu_prev = Some((u, nowi));
            }
            if sample {
                if let Some(m) = mem {
                    r.mem_hist.push_back(m);
                    while r.mem_hist.len() > 60 {
                        r.mem_hist.pop_front();
                    }
                }
            }
            return;
        }
        let since = *rec.gone_since.get_or_insert_with(Instant::now);
        if since.elapsed() < Duration::from_millis(1500) {
            return;
        }
        // The unit is gone and the runner did not report. Find out why.
        let rec = rec.clone();
        let phase = match rec.phase {
            RunPhase::Build => "build",
            RunPhase::Launch => "launch",
        };
        if let Ok(t) = std::fs::read_to_string(exit_file(rec.id, phase)) {
            let mut it = t.split_whitespace();
            let code = it.next().and_then(|c| c.parse().ok()).unwrap_or(1);
            let oom = it.next() == Some("1");
            self.finished(rec.id, code, oom);
            return;
        }
        let props = sys::unit_properties(&rec.unit, &["ActiveState", "Result", "ExecMainStatus"]);
        let state = props.get("ActiveState").map(String::as_str).unwrap_or("");
        if matches!(state, "active" | "activating" | "reloading") {
            // Still running; the cgroup path was wrong. Look it up again.
            if let Some(r) = self.runs.get_mut(&id) {
                r.cgroup = None;
                r.cg_tries = 0;
                r.gone_since = None;
            }
            return;
        }
        let result = props.get("Result").map(String::as_str).unwrap_or("");
        let oom = result == "oom-kill";
        let code = props
            .get("ExecMainStatus")
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|c| *c != 0)
            .unwrap_or(if rec.stopping { 0 } else { 1 });
        self.finished(rec.id, code, oom);
    }

    fn scan_captures(&mut self) {
        if self.ticks % 2 != 0 {
            return;
        }
        let mut found: Vec<(String, String, String, usize)> = vec![];
        for def in &self.tasks {
            if !def.params.iter().any(|p| p.capture.is_some()) {
                continue;
            }
            let Some(r) = self.rt.get_mut(&def.id) else { continue };
            if !r.phase.active() || r.run.is_none() {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&def.log) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < r.capture_offset {
                r.capture_offset = 0;
            }
            if len == r.capture_offset {
                continue;
            }
            let _ = f.seek(SeekFrom::Start(r.capture_offset));
            let mut buf = vec![];
            let _ = f.take(512 * 1024).read_to_end(&mut buf);
            // Only consume whole lines.
            let end = buf.iter().rposition(|b| *b == b'\n').map(|p| p + 1).unwrap_or(0);
            r.capture_offset += end as u64;
            let text = String::from_utf8_lossy(&buf[..end]);
            for p in &def.params {
                let Some(re) = p.capture.as_ref().and_then(|c| regex::Regex::new(c).ok()) else { continue };
                for line in text.lines() {
                    if let Some(c) = re.captures(line) {
                        let v = c.name("value").or_else(|| c.get(1)).map(|m| m.as_str().to_string());
                        if let Some(v) = v {
                            found.push((def.id.clone(), p.name.clone(), v, p.history));
                        }
                    }
                }
            }
        }
        let t = now();
        for (id, param, v, keep) in found {
            self.history.record_param(&id, &param, &v, t, keep);
            self.history_dirty = true;
            self.dirty = true;
        }
    }

    fn refresh_git(&mut self, force: bool) {
        if self.git_busy && !force {
            return;
        }
        self.git_busy = true;
        self.git_at = Some(Instant::now());
        let mut dirs: Vec<PathBuf> = self
            .tasks
            .iter()
            .filter(|t| t.origin != manifest::Origin::Script)
            .map(|t| t.dir.clone())
            .collect();
        dirs.sort();
        dirs.dedup();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res: Vec<(PathBuf, Option<GitInfo>)> = dirs.into_iter().map(|d| {
                let g = sys::git_info(&d);
                (d, g)
            }).collect();
            let _ = tx.send(Msg::Git(res));
        });
    }

    // ----------------------------------------------------------- output

    fn snapshot(&self) -> Snapshot {
        let t = now();
        let stale_secs = self.cfg.shelf.stale_days as f64 * 86400.0;
        let tasks: Vec<TaskView> = self
            .tasks
            .iter()
            .map(|def| {
                let empty = TaskRt::default();
                let r = self.rt.get(&def.id).unwrap_or(&empty);
                let hist = self.history.tasks.get(&def.id);
                let last_used = self.history.last_used(&def.id);
                TaskView {
                    id: def.id.clone(),
                    label: def.label.clone(),
                    description: def.description.clone(),
                    tags: def.tags.clone(),
                    pin: def.pin,
                    heavy: def.heavy,
                    confirm: def.confirm,
                    shelf: self.history.shelved.get(&def.id).copied().unwrap_or(def.shelf),
                    stale: def.pin.is_none() && last_used.is_some_and(|u| t - u > stale_secs),
                    phase: r.phase,
                    since: r.since,
                    step: r.step.clone(),
                    message: r.message.clone(),
                    error: r.error.clone(),
                    mem: r.mem,
                    cpu: r.cpu,
                    mem_history: r.mem_hist.iter().copied().collect(),
                    last: hist.and_then(|h| h.last.clone()),
                    typical: self.history.typical(&def.id),
                    params: def
                        .params
                        .iter()
                        .map(|p| ParamView {
                            name: p.name.clone(),
                            default: p.default.clone(),
                            choices: p.choices.clone(),
                            history: hist.and_then(|h| h.params.get(&p.name).cloned()).unwrap_or_default(),
                        })
                        .collect(),
                    variants: def
                        .variants
                        .iter()
                        .map(|v| VariantView {
                            name: v.name.clone(),
                            label: v.label.clone(),
                            confirm: v.confirm.unwrap_or(def.confirm),
                        })
                        .collect(),
                    has_clean: def.clean.is_some(),
                    has_run: def.run.is_some(),
                    log: def.log.to_string_lossy().to_string(),
                    dir: def.dir.to_string_lossy().to_string(),
                    git: self.git.get(&def.dir).cloned(),
                    last_used,
                    frecency: self.history.frecency(&def.id, t),
                    queue_pos: self.queue.iter().position(|q| *q == def.id),
                    command: def.command_preview(),
                }
            })
            .collect();
        let mem = sys::meminfo();
        let mainline = MainlineView {
            current: self.mainline.clone(),
            queue: self.queue.iter().cloned().collect(),
            waiting_memory: self
                .queue
                .front()
                .is_some_and(|h| self.rt.get(h).is_some_and(|r| r.phase == Phase::Waiting)),
            mem_available: mem.available,
            mem_total: mem.total,
            min_free: self.cfg.mainline.min_free_bytes(),
            cap: self.cfg.mainline.mem_bytes(),
            current_mem: self.mainline.as_ref().and_then(|m| self.rt.get(m)).and_then(|r| r.mem),
        };
        let aggregate = Aggregate::from_tasks(&tasks);
        let tabs = if self.cfg.tabs.is_empty() {
            vec![yard_core::config::TabConfig { name: "All".into(), tags: vec![] }]
        } else {
            self.cfg.tabs.clone()
        };
        Snapshot {
            tasks,
            mainline,
            aggregate,
            tabs,
            problems: self.problems.clone(),
            board: self.cfg.board.clone(),
            has_editor: !self.cfg.editor.is_empty(),
            has_terminal: !self.cfg.terminal.is_empty(),
            now: t,
        }
    }

    fn bar_line(&self) -> String {
        let tasks: Vec<TaskView> = self.snapshot().tasks;
        let a = Aggregate::from_tasks(&tasks);
        let class = a.phase.as_str().to_string();
        let count = a.count();
        let icon = &self.cfg.bar.icon;
        let text = match self.cfg.bar.format.get(&class) {
            Some(f) => f.replace("{icon}", icon).replace("{count}", &count.to_string()),
            None => {
                if self.cfg.bar.show_count && count > 1 && matches!(a.phase, Phase::Failed | Phase::Building | Phase::Queued) {
                    format!("{icon} {count}")
                } else {
                    icon.clone()
                }
            }
        };
        let line = BarLine {
            text: text.trim().to_string(),
            class: class.clone(),
            tooltip: sys::escape_markup(&a.tooltip()),
            alt: class,
        };
        serde_json::to_string(&line).unwrap_or_default()
    }

    fn flush(&mut self) {
        if !self.dirty || self.subs.is_empty() {
            self.dirty = false;
            return;
        }
        self.dirty = false;
        let wants_snapshot = self.subs.iter().any(|s| s.kind != SubKind::Bar);
        let snap_line = if wants_snapshot {
            serde_json::to_string(&Event::Snapshot(Box::new(self.snapshot()))).unwrap_or_default()
        } else {
            String::new()
        };
        let bar = self.bar_line();
        let mut dead = vec![];
        for (i, s) in self.subs.iter_mut().enumerate() {
            let ok = match s.kind {
                SubKind::Bar => {
                    if s.last.as_deref() == Some(bar.as_str()) {
                        true
                    } else {
                        s.last = Some(bar.clone());
                        write_line(&mut s.stream, &bar)
                    }
                }
                _ => write_line(&mut s.stream, &snap_line),
            };
            if !ok {
                dead.push(i);
            }
        }
        for i in dead.into_iter().rev() {
            self.subs.remove(i);
        }
    }

    fn shelf_data(&mut self) -> ShelfData {
        if let Some((d, at)) = &self.shelf {
            if at.elapsed() < Duration::from_secs(600) {
                return d.clone();
            }
        }
        let mut tools: Vec<ShelfItem> = self
            .cfg
            .shelf
            .extra
            .iter()
            .map(|t| ShelfItem { name: t.name.clone(), desc: t.desc.clone(), cmd: t.cmd.clone() })
            .collect();
        if self.cfg.shelf.tools == "pacman" {
            for t in sys::pacman_tools() {
                if !tools.iter().any(|x| x.name == t.name) {
                    tools.push(t);
                }
            }
        }
        let binds = if self.cfg.shelf.binds {
            yard_core::hypr::binds()
                .into_iter()
                .map(|(k, d)| ShelfItem { name: k, desc: d, cmd: None })
                .collect()
        } else {
            vec![]
        };
        let d = ShelfData { tools, binds };
        self.shelf = Some((d.clone(), Instant::now()));
        d
    }

    fn open(&mut self, what: OpenWhat, id: Option<String>, file: Option<String>, line: Option<u32>, tool: Option<String>) -> Reply {
        let def = id.as_deref().and_then(|i| self.def(i)).cloned();
        let dir = def.as_ref().map(|d| d.dir.clone()).unwrap_or_else(paths::home);
        let term = self.cfg.terminal.clone();
        let spawn = |argv: Vec<String>, cwd: &Path| -> Reply {
            if argv.is_empty() {
                return Reply::err("nothing configured to open this with");
            }
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]).current_dir(cwd);
            match sys::spawn_detached(c) {
                Ok(()) => Reply::ok(),
                Err(e) => Reply::err(format!("could not start {}: {e}", argv[0])),
            }
        };
        match what {
            OpenWhat::Editor => {
                let target = match &file {
                    Some(f) => {
                        if let Some(rest) = f.strip_prefix("res://") {
                            dir.join(rest)
                        } else {
                            paths::expand_in(f, &dir)
                        }
                    }
                    None => dir.clone(),
                };
                let argv = self
                    .cfg
                    .editor
                    .iter()
                    .map(|a| {
                        a.replace("{file}", &target.to_string_lossy())
                            .replace("{line}", &line.unwrap_or(1).to_string())
                            .replace("{dir}", &dir.to_string_lossy())
                    })
                    .collect();
                spawn(argv, &dir)
            }
            OpenWhat::Terminal => {
                let mut argv = term.clone();
                while argv.last().is_some_and(|a| a == "-e" || a == "--hold" || a == "--") {
                    argv.pop();
                }
                spawn(argv, &dir)
            }
            OpenWhat::Log => {
                let Some(def) = def else { return Reply::err("which task?") };
                let mut argv = term;
                argv.extend(["tail", "-n", "+1", "-f"].map(String::from));
                argv.push(def.log.to_string_lossy().to_string());
                spawn(argv, &dir)
            }
            OpenWhat::Tool | OpenWhat::Man => {
                let Some(name) = tool else { return Reply::err("which tool?") };
                let mut argv = term;
                if what == OpenWhat::Man {
                    argv.push("man".into());
                    argv.push(name);
                } else {
                    let cmd = self
                        .shelf
                        .as_ref()
                        .and_then(|(d, _)| d.tools.iter().find(|t| t.name == name).and_then(|t| t.cmd.clone()))
                        .unwrap_or(name);
                    argv.extend(["bash".to_string(), "-lc".to_string(), cmd]);
                }
                spawn(argv, &paths::home())
            }
        }
    }

    // ----------------------------------------------------------- persistence

    fn save_runs(&self) {
        let path = paths::runs_dir().join("active.json");
        let recs: Vec<&RunRec> = self.runs.values().collect();
        let _ = std::fs::write(path, serde_json::to_string(&recs).unwrap_or_default());
    }

    /// Picks up units that kept running while the daemon was down.
    fn adopt(&mut self) {
        let path = paths::runs_dir().join("active.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let Ok(recs) = serde_json::from_str::<Vec<RunRec>>(&text) else { return };
        for rec in recs {
            self.next_run = self.next_run.max(rec.id + 1);
            if self.def(&rec.task).is_none() {
                self.cleanup_run(rec.id);
                continue;
            }
            let since = match rec.phase {
                RunPhase::Launch => rec.started + rec.build_secs,
                RunPhase::Build => rec.started,
            };
            let r = self.rt(&rec.task);
            r.run = Some(rec.id);
            r.since = since;
            r.phase = match rec.phase {
                RunPhase::Build => Phase::Building,
                RunPhase::Launch => Phase::Running,
            };
            if rec.heavy && rec.phase == RunPhase::Build {
                self.mainline = Some(rec.task.clone());
            }
            self.runs.insert(rec.id, rec);
        }
        self.dirty = true;
        let _ = self.tx.send(Msg::Wake);
    }
}

fn stop_unit(unit: &str) {
    let mut c = Command::new("systemctl");
    c.args(["--user", "stop", "--no-block", unit]);
    let _ = sys::spawn_detached(c);
}
