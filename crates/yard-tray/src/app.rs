//! The board as an egui app: state, daemon messages, input and layout.

use crate::draw::{self, *};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, Event as UiEvent, Id, Key, Pos2, Rect, Sense, Stroke, StrokeKind};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use yard_core::manifest::Confirm;
use yard_core::model::*;
use yard_core::protocol::{AckPhase, Event, OpenWhat, Reply, Request, SubKind};
use yard_core::search::{rank, ranked, score};
use yard_core::size::human;

pub const WIDTH: f32 = 680.0;
const HINTS: &str = "asdfghjklqwertyuiopzxcvbnm";
const HOLD_MS: f32 = 1200.0;
const LONG_PRESS_MS: u128 = 450;
const TILE_H: f32 = 98.0;

pub enum Msg {
    Event(Event),
    Connected,
    Disconnected,
    Reply(Reply),
    Shelf(ShelfData),
    History(yard_core::protocol::HistoryData),
}

#[derive(Clone, PartialEq)]
enum Mode {
    Grid,
    Log(String),
    Shelf,
    History,
    Strip(String),
}

#[derive(Clone)]
enum Action {
    Run { variant: Option<String>, params: BTreeMap<String, String>, clean: bool, confirm: Confirm },
    Custom(String),
    Stop,
    Log,
    Editor,
    Terminal,
    CopyCommand,
    Shelve(bool),
}

struct MenuItem {
    label: String,
    detail: String,
    key: Option<char>,
    action: Action,
    star: Option<(String, String, bool)>,
    color: Color32,
}

struct MenuState {
    id: String,
    title: String,
    items: Vec<MenuItem>,
    selected: usize,
    /// Text entry: (text, Some(param) for a custom value, None for type-to-confirm).
    entry: Option<(String, Option<String>)>,
    pending: Option<(Option<String>, BTreeMap<String, String>)>,
}

struct Hold {
    id: String,
    started: Instant,
    key: Option<Key>,
    params: BTreeMap<String, String>,
    variant: Option<String>,
}

struct LogState {
    path: std::path::PathBuf,
    lines: Vec<String>,
    offset: u64,
    error: Option<usize>,
    follow: bool,
    scroll_to: Option<usize>,
    polled: Instant,
}

pub struct App {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    ctx: egui::Context,
    snap: Option<Snapshot>,
    skew: f64,
    visible: bool,
    shown_at: Instant,
    hide_at: Option<Instant>,
    last_hidden: Option<Instant>,
    /// Where the window goes: horizontal center, the edge it hangs from, and
    /// which side of that edge.
    place: Option<(f32, f32, Edge)>,
    win_size: egui::Vec2,
    anchor: Option<Anchor>,
    mode: Mode,
    tab: usize,
    filter: String,
    focus_filter: bool,
    selected: Option<usize>,
    menu: Option<MenuState>,
    hold: Option<Hold>,
    spent_key: Option<Key>,
    press: Option<(String, Instant, bool)>,
    toast: Option<(String, bool, Instant)>,
    log: Option<LogState>,
    shelf: Option<ShelfData>,
    shelf_sel: usize,
    shelf_rows: Vec<ShelfRow>,
    hist: crate::hist::HistState,
    history_rev: u64,
    order: Vec<String>,
    hints: Vec<(char, String)>,
    was_focused: bool,
    started: bool,
    show_now: bool,
    #[cfg(any(windows, target_os = "macos"))]
    tray: Option<crate::tray::Tray>,
    #[cfg(any(windows, target_os = "macos"))]
    tray_rx: Option<Receiver<crate::tray::TrayMsg>>,
}

#[derive(Clone)]
enum ShelfRow {
    Section(String),
    Task(String, String, String),
    Tool(String, String),
    Bind(String, String),
}

fn connection(tx: Sender<Msg>, ctx: egui::Context) {
    loop {
        if let Ok(events) = yard_core::protocol::subscribe(SubKind::Board) {
            let _ = tx.send(Msg::Connected);
            ctx.request_repaint();
            for ev in events {
                match ev {
                    Ok(e) => {
                        if tx.send(Msg::Event(e)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Msg::Disconnected);
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_millis(800));
    }
}

fn letter(k: Key) -> Option<char> {
    let name = k.name();
    (name.len() == 1).then(|| name.chars().next().unwrap().to_ascii_lowercase()).filter(|c| c.is_ascii_alphabetic())
}

fn digit(k: Key) -> Option<usize> {
    let name = k.name();
    (name.len() == 1).then(|| name.chars().next().unwrap()).and_then(|c| c.to_digit(10)).map(|d| d as usize)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, show_now: bool) -> App {
        let ctx = cc.egui_ctx.clone();
        ctx.global_style_mut(|s| {
            s.override_font_id = Some(mono(13.0));
            s.visuals = egui::Visuals::dark();
            s.visuals.panel_fill = Color32::TRANSPARENT;
            s.visuals.window_fill = PANEL;
            s.visuals.extreme_bg_color = WELL;
            s.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
            s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
            s.interaction.selectable_labels = false;
        });
        let (tx, rx) = channel();
        {
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || connection(tx, ctx));
        }
        App {
            rx,
            tx,
            ctx,
            snap: None,
            skew: 0.0,
            visible: false,
            shown_at: Instant::now(),
            hide_at: None,
            last_hidden: None,
            place: None,
            win_size: vec2(WIDTH, 200.0),
            anchor: None,
            mode: Mode::Grid,
            tab: 0,
            filter: String::new(),
            focus_filter: false,
            selected: None,
            menu: None,
            hold: None,
            spent_key: None,
            press: None,
            toast: None,
            log: None,
            shelf: None,
            shelf_sel: 0,
            shelf_rows: vec![],
            hist: Default::default(),
            history_rev: u64::MAX,
            order: vec![],
            hints: vec![],
            was_focused: false,
            started: false,
            show_now,
            #[cfg(any(windows, target_os = "macos"))]
            tray: None,
            #[cfg(any(windows, target_os = "macos"))]
            tray_rx: None,
        }
    }

    fn now(&self) -> f64 {
        now() + self.skew
    }

    fn task(&self, id: &str) -> Option<TaskView> {
        self.snap.as_ref()?.tasks.iter().find(|t| t.id == id).cloned()
    }

    fn send(&self, req: Request) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = yard_core::protocol::request(&req).unwrap_or_else(|e| Reply::err(format!("{e:#}")));
            if !(r.ok && r.message.is_none()) {
                let _ = tx.send(Msg::Reply(r));
                ctx.request_repaint();
            }
        });
    }

    fn send_quiet(&self, req: Request) {
        std::thread::spawn(move || {
            let _ = yard_core::protocol::request(&req);
        });
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now() + Duration::from_millis(3200)));
    }

    // ------------------------------------------------------------- show/hide

    fn toggle(&mut self, anchor: Option<Anchor>) {
        if self.visible {
            self.hide();
        } else if self.last_hidden.is_some_and(|t| t.elapsed() < Duration::from_millis(350)) {
            // The click that took focus away already closed it.
        } else {
            self.show(anchor);
        }
    }

    fn show(&mut self, anchor: Option<Anchor>) {
        self.anchor = anchor.clone();
        let ppp = self.ctx.pixels_per_point().max(0.5);
        self.place = match &anchor {
            Some(a) if !a.center => {
                let scale = if a.physical { ppp } else { 1.0 };
                Some((a.x as f32 / scale, a.y as f32 / scale, a.edge))
            }
            _ => None,
        };
        let failed = self.snap.as_ref().and_then(|s| {
            s.tasks.iter().filter(|t| t.phase == Phase::Failed).max_by(|a, b| a.since.partial_cmp(&b.since).unwrap()).map(|t| t.id.clone())
        });
        self.menu = None;
        self.hold = None;
        self.filter.clear();
        self.selected = None;
        match failed {
            Some(id) => self.open_log(&id),
            None => {
                if let Mode::Log(_) = self.mode {
                    self.mode = Mode::Grid;
                }
            }
        }
        let was = self.visible;
        self.visible = true;
        self.hide_at = None;
        self.shown_at = Instant::now();
        self.was_focused = false;
        self.reposition();
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if !was {
            self.send_quiet(Request::BoardVisible { visible: true });
            self.send_quiet(Request::Ack { phase: AckPhase::Open, id: None });
        }
    }

    fn monitor(&self) -> egui::Vec2 {
        self.ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1080.0))
    }

    /// Puts the window next to its anchor for its current size.
    fn reposition(&self) {
        let mon = self.monitor();
        let (w, h) = (self.win_size.x, self.win_size.y);
        let pos = match self.place {
            Some((cx, ey, edge)) => {
                let left = (cx - w / 2.0).clamp(0.0, (mon.x - w).max(0.0));
                let top = match edge {
                    Edge::Top => ey + 6.0,
                    _ => ey - h - 6.0,
                };
                pos2(left, top.max(0.0))
            }
            None => pos2(((mon.x - w) / 2.0).max(0.0), ((mon.y - h) / 2.0 * 0.85).max(0.0)),
        };
        self.ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.menu = None;
        self.hold = None;
        self.press = None;
        self.hide_at = Some(Instant::now() + Duration::from_millis(130));
        self.last_hidden = Some(Instant::now());
        self.send_quiet(Request::BoardVisible { visible: false });
        self.send_quiet(Request::Ack { phase: AckPhase::Close, id: None });
    }

    // ------------------------------------------------------------- data

    fn handle(&mut self, m: Msg) {
        match m {
            Msg::Connected => {}
            Msg::Disconnected => self.toast("lost the daemon; reconnecting", true),
            Msg::Reply(r) => {
                if let Some(e) = r.error {
                    self.toast(e, true);
                } else if let Some(m) = r.message {
                    self.toast(m, false);
                }
            }
            Msg::Shelf(d) => {
                self.shelf = Some(d);
            }
            Msg::History(d) => self.hist.set(d),
            Msg::Event(ev) => match ev {
                Event::Snapshot(s) => {
                    self.skew = s.now - now();
                    let rev = s.history_rev;
                    self.snap = Some(*s);
                    if rev != self.history_rev && matches!(self.mode, Mode::History | Mode::Strip(_)) {
                        self.history_rev = rev;
                        self.fetch_history();
                    }
                }
                Event::Show { anchor } => self.show(anchor),
                Event::Hide => self.hide(),
                Event::Toggle { anchor } => self.toggle(anchor),
                Event::Bar(_) => {}
            },
        }
    }

    fn tabs(&self) -> Vec<yard_core::config::TabConfig> {
        self.snap.as_ref().map(|s| s.tabs.clone()).unwrap_or_default()
    }

    fn in_tab(t: &TaskView, tab: &yard_core::config::TabConfig) -> bool {
        !t.shelf && (tab.tags.is_empty() || t.tags.iter().any(|g| tab.tags.contains(g)))
    }

    fn visible_tasks(&self) -> Vec<TaskView> {
        let Some(s) = self.snap.as_ref() else { return vec![] };
        if !self.filter.is_empty() {
            let mut scored: Vec<(i64, &TaskView)> = s
                .tasks
                .iter()
                .filter_map(|t| {
                    score(&format!("{} {} {} {}", t.label, t.id, t.tags.join(" "), t.description), &self.filter).map(|sc| (sc, t))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.frecency.partial_cmp(&a.1.frecency).unwrap_or(std::cmp::Ordering::Equal)));
            return scored.into_iter().map(|(_, t)| t.clone()).collect();
        }
        let tab = s.tabs.get(self.tab).or_else(|| s.tabs.first());
        let mut list: Vec<TaskView> = s.tasks.iter().filter(|t| tab.is_none_or(|tab| Self::in_tab(t, tab))).cloned().collect();
        list.sort_by(|a, b| {
            (a.pin.is_none(), a.pin.unwrap_or(0), a.label.to_lowercase()).cmp(&(
                b.pin.is_none(),
                b.pin.unwrap_or(0),
                b.label.to_lowercase(),
            ))
        });
        list
    }

    fn fetch_history(&self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            if let Ok(r) = yard_core::protocol::request(&Request::History { id: None, limit: Some(800) }) {
                if let Some(h) = r.history {
                    let _ = tx.send(Msg::History(*h));
                    ctx.request_repaint();
                }
            }
        });
    }

    fn history_action(&mut self, a: crate::hist::Action) {
        use crate::hist::Action;
        match a {
            Action::OpenStrip(t) => {
                self.hist.open_strip(&t);
                self.mode = Mode::Strip(t);
            }
            Action::Replay(r) => {
                let confirm = self.task(&r.task).map(|v| v.confirm).unwrap_or_default();
                self.run_with(&r.task, r.params.clone(), r.variant.clone(), false, confirm);
            }
            Action::OpenLog(r) => {
                self.send(Request::Open { what: OpenWhat::Log, id: Some(r.task.clone()), file: r.log.clone(), line: None, tool: None });
                self.hide();
            }
            Action::CopyError(r) => {
                if let Some(e) = &r.error {
                    self.ctx.copy_text(e.text.clone());
                    self.toast("Copied the error", false);
                }
            }
        }
    }

    fn select_tab(&mut self, i: usize) {
        let n = self.tabs().len();
        self.menu = None;
        self.selected = None;
        if i == n {
            self.mode = Mode::History;
            self.history_rev = self.snap.as_ref().map(|s| s.history_rev).unwrap_or(0);
            self.fetch_history();
        } else if i > n {
            self.mode = Mode::Shelf;
            self.shelf_sel = 0;
            // Refetch on every open so edited key binds show; the last copy
            // draws until the reply lands.
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                if let Ok(r) = yard_core::protocol::request(&Request::Shelf) {
                    if let Some(d) = r.shelf {
                        let _ = tx.send(Msg::Shelf(d));
                        ctx.request_repaint();
                    }
                }
            });
        } else {
            self.tab = i;
            self.mode = Mode::Grid;
            self.shown_at = Instant::now() - Duration::from_millis(60);
        }
    }

    // ------------------------------------------------------------- actions

    fn activate(&mut self, id: &str) {
        let Some(v) = self.task(id) else { return };
        if matches!(v.phase, Phase::Failed | Phase::Seen) {
            self.open_log(id);
            return;
        }
        match v.confirm {
            Confirm::Hold => self.toast(format!("Hold {} for a moment to run it", v.label), false),
            Confirm::Type => self.open_type_confirm(id, BTreeMap::new(), None),
            Confirm::None => self.send(Request::Run { id: id.to_string(), params: BTreeMap::new(), variant: None, clean: false }),
        }
    }

    fn run_with(&mut self, id: &str, params: BTreeMap<String, String>, variant: Option<String>, clean: bool, confirm: Confirm) {
        match confirm {
            Confirm::None => self.send(Request::Run { id: id.to_string(), params, variant, clean }),
            Confirm::Type => self.open_type_confirm(id, params, variant),
            Confirm::Hold => self.toast("Guarded: hold the tile (or its letter) to run it", false),
        }
    }

    fn open_log(&mut self, id: &str) {
        let Some(v) = self.task(id) else { return };
        self.menu = None;
        let path = std::path::PathBuf::from(&v.log);
        let (text, end) = read_tail(&path, 0, 768 * 1024)
            .unwrap_or_else(|| (format!("No log yet for {}.\nIt will appear at {} on the first run.", v.label, v.log), 0));
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let error = v.error.as_ref().and_then(|e| {
            let t = e.text.trim_end();
            lines.iter().rposition(|l| yard_core::errors::strip_ansi(l).trim_end() == t)
        });
        let scroll_to = Some(error.unwrap_or(lines.len().saturating_sub(1)));
        self.log = Some(LogState {
            path,
            lines,
            offset: end,
            error,
            follow: matches!(v.phase, Phase::Building | Phase::Running),
            scroll_to,
            polled: Instant::now(),
        });
        self.mode = Mode::Log(id.to_string());
    }

    fn poll_log(&mut self) {
        let Some(l) = self.log.as_mut() else { return };
        if l.polled.elapsed() < Duration::from_millis(500) {
            return;
        }
        l.polled = Instant::now();
        let len = std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(0);
        if len == l.offset {
            return;
        }
        if len < l.offset {
            if let Some((t, end)) = read_tail(&l.path, 0, 768 * 1024) {
                l.lines = t.lines().map(String::from).collect();
                l.offset = end;
            }
        } else if let Some((t, end)) = read_tail(&l.path, l.offset, 768 * 1024) {
            if let Some(cut) = t.rfind('\n') {
                l.lines.extend(t[..cut].lines().map(String::from));
                l.offset = end - (t.len() - cut - 1) as u64;
            }
        }
        if l.follow {
            l.scroll_to = Some(l.lines.len().saturating_sub(1));
        }
    }

    fn log_key(&mut self, k: char) {
        let Mode::Log(id) = self.mode.clone() else { return };
        let Some(v) = self.task(&id) else { return };
        match k {
            'y' => {
                let text = self
                    .log
                    .as_ref()
                    .map(|l| match l.error {
                        Some(i) => l.lines[i..(i + 6).min(l.lines.len())].join("\n"),
                        None => l.lines[l.lines.len().saturating_sub(20)..].join("\n"),
                    })
                    .unwrap_or_default();
                self.ctx.copy_text(text);
                self.toast(if v.error.is_some() { "Copied the error with context" } else { "Copied the end of the log" }, false);
            }
            'e' => {
                let (file, line) = v.error.as_ref().map(|e| (e.file.clone(), e.line)).unwrap_or((None, None));
                self.send(Request::Open { what: OpenWhat::Editor, id: Some(id), file, line, tool: None });
                self.hide();
            }
            'r' => {
                self.send(Request::Ack { phase: AckPhase::All, id: Some(id.clone()) });
                self.mode = Mode::Grid;
                if v.confirm == Confirm::None {
                    self.send(Request::Run { id, params: BTreeMap::new(), variant: None, clean: false });
                } else {
                    self.activate(&id);
                }
            }
            'f' => {
                if let Some(l) = self.log.as_mut() {
                    l.follow = !l.follow;
                    if l.follow {
                        l.scroll_to = Some(l.lines.len().saturating_sub(1));
                    }
                }
            }
            'a' => {
                self.send(Request::Ack { phase: AckPhase::All, id: Some(id) });
                self.mode = Mode::Grid;
            }
            't' => {
                self.send(Request::Open { what: OpenWhat::Log, id: Some(id), file: None, line: None, tool: None });
                self.hide();
            }
            '\u{1b}' => self.mode = Mode::Grid,
            _ => {}
        }
    }

    fn open_menu(&mut self, id: &str) {
        let Some(v) = self.task(id) else { return };
        let now = self.now();
        let guard = |c: Confirm| match c {
            Confirm::Hold => " (hold)",
            Confirm::Type => " (type to confirm)",
            Confirm::None => "",
        };
        let color = |c: Confirm| if c == Confirm::None { FG } else { STOP };
        let mut items = vec![];
        let run_label = match v.phase {
            Phase::Running => "Restart",
            Phase::Building | Phase::Queued | Phase::Waiting => "Run (already going)",
            _ => "Run",
        };
        items.push(MenuItem {
            label: format!("{run_label}{}", guard(v.confirm)),
            detail: String::new(),
            key: Some('\n'),
            action: Action::Run { variant: None, params: BTreeMap::new(), clean: false, confirm: v.confirm },
            star: None,
            color: color(v.confirm),
        });
        let mut d = 1u32;
        let mut next = || {
            let c = if d <= 9 { char::from_digit(d, 10) } else { None };
            d += 1;
            c
        };
        for var in &v.variants {
            items.push(MenuItem {
                label: format!("{}{}", var.label, guard(var.confirm)),
                detail: String::new(),
                key: next(),
                action: Action::Run { variant: Some(var.name.clone()), params: BTreeMap::new(), clean: false, confirm: var.confirm },
                star: None,
                color: color(var.confirm),
            });
        }
        for p in &v.params {
            for h in p.history.iter().take(6) {
                items.push(MenuItem {
                    label: format!("Replay {} {}{}", p.name, h.value, if h.starred { " ★" } else { "" }),
                    detail: ago(now - h.at),
                    key: next(),
                    action: Action::Run {
                        variant: None,
                        params: [(p.name.clone(), h.value.clone())].into(),
                        clean: false,
                        confirm: v.confirm,
                    },
                    star: Some((p.name.clone(), h.value.clone(), h.starred)),
                    color: CAUTION,
                });
            }
            let mut name = p.name.clone();
            if let Some(f) = name.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            items.push(MenuItem {
                label: format!("{name}…"),
                detail: p.choices.join(", "),
                key: Some(':'),
                action: Action::Custom(p.name.clone()),
                star: None,
                color: FG,
            });
        }
        let plain = |label: &str, key: char, action: Action, color: Color32| MenuItem {
            label: label.into(),
            detail: String::new(),
            key: Some(key),
            action,
            star: None,
            color,
        };
        if v.has_clean {
            items.push(plain(
                "Clean build",
                'c',
                Action::Run { variant: None, params: BTreeMap::new(), clean: true, confirm: v.confirm },
                FG,
            ));
        }
        if v.phase.active() {
            items.push(plain("Stop", 'x', Action::Stop, STOP));
        }
        items.push(plain("Open log", 'l', Action::Log, FG));
        items.push(plain("Open in editor", 'e', Action::Editor, FG));
        items.push(plain("Terminal here", 't', Action::Terminal, FG));
        items.push(plain("Copy command", 'y', Action::CopyCommand, FG));
        items.push(plain(if v.shelf { "Back to the board" } else { "Move to the Shelf" }, 'm', Action::Shelve(!v.shelf), FG));
        self.menu = Some(MenuState { id: id.to_string(), title: v.label.clone(), items, selected: 0, entry: None, pending: None });
    }

    fn open_type_confirm(&mut self, id: &str, params: BTreeMap<String, String>, variant: Option<String>) {
        let Some(v) = self.task(id) else { return };
        self.menu = Some(MenuState {
            id: id.to_string(),
            title: format!("Type \"{}\" to run {}", v.id, v.label),
            items: vec![],
            selected: 0,
            entry: Some((String::new(), None)),
            pending: Some((variant, params)),
        });
    }

    fn menu_activate(&mut self, idx: usize) {
        let Some(m) = self.menu.as_ref() else { return };
        let Some(item) = m.items.get(idx) else { return };
        let (id, action) = (m.id.clone(), item.action.clone());
        match action {
            Action::Run { variant, params, clean, confirm } => {
                self.menu = None;
                self.run_with(&id, params, variant, clean, confirm);
            }
            Action::Custom(name) => {
                if let Some(m) = self.menu.as_mut() {
                    m.title = format!("{} {name}", m.title);
                    m.items.clear();
                    m.entry = Some((String::new(), Some(name)));
                }
            }
            Action::Stop => {
                self.menu = None;
                self.send(Request::Stop { id });
            }
            Action::Log => self.open_log(&id),
            Action::Editor => {
                self.menu = None;
                self.send(Request::Open { what: OpenWhat::Editor, id: Some(id), file: None, line: None, tool: None });
                self.hide();
            }
            Action::Terminal => {
                self.menu = None;
                self.send(Request::Open { what: OpenWhat::Terminal, id: Some(id), file: None, line: None, tool: None });
                self.hide();
            }
            Action::CopyCommand => {
                self.menu = None;
                if let Some(v) = self.task(&id) {
                    self.ctx.copy_text(v.command.clone());
                    self.toast("Copied the command", false);
                }
            }
            Action::Shelve(on) => {
                self.menu = None;
                self.send_quiet(Request::Shelve { id, shelved: on });
                self.toast(if on { "Moved to the Shelf" } else { "Back on the board" }, false);
            }
        }
    }

    fn menu_entry_done(&mut self) {
        let Some(m) = self.menu.take() else { return };
        let Some((text, param)) = m.entry else { return };
        match param {
            Some(name) => {
                if text.trim().is_empty() {
                    return;
                }
                let confirm = self.task(&m.id).map(|v| v.confirm).unwrap_or_default();
                self.run_with(&m.id, [(name, text.trim().to_string())].into(), None, false, confirm);
            }
            None => {
                if text.trim() != m.id {
                    self.toast("That does not match. Nothing ran.", true);
                    return;
                }
                let (variant, params) = m.pending.unwrap_or_default();
                self.send(Request::Run { id: m.id, params, variant, clean: false });
            }
        }
    }

    fn start_hold(&mut self, id: &str, key: Option<Key>) {
        self.hold = Some(Hold { id: id.to_string(), started: Instant::now(), key, params: BTreeMap::new(), variant: None });
    }

    fn cancel_hold(&mut self, id: &str) -> bool {
        if self.hold.as_ref().is_some_and(|h| h.id == id) {
            self.hold = None;
            self.toast("Released early. Nothing ran.", false);
            return true;
        }
        false
    }

    fn check_hold(&mut self) {
        let fire = self.hold.as_ref().is_some_and(|h| h.started.elapsed().as_secs_f32() * 1000.0 >= HOLD_MS);
        if fire {
            let h = self.hold.take().unwrap();
            self.spent_key = h.key;
            self.menu = None;
            self.send(Request::Run { id: h.id, params: h.params, variant: h.variant, clean: false });
        }
    }

    // ------------------------------------------------------------- keys

    fn keys(&mut self, events: &[UiEvent], filter_focused: bool) {
        for e in events {
            let UiEvent::Key { key, pressed, repeat, modifiers, .. } = e else { continue };
            let (key, pressed, repeat, shift) = (*key, *pressed, *repeat, modifiers.shift);
            if !pressed {
                if self.spent_key == Some(key) {
                    self.spent_key = None;
                } else if let Some(h) = self.hold.as_ref().filter(|h| h.key == Some(key)) {
                    let id = h.id.clone();
                    self.cancel_hold(&id);
                }
                continue;
            }
            if repeat && (self.hold.is_some() || self.spent_key == Some(key)) {
                continue;
            }
            // Menus.
            if let Some(m) = self.menu.as_mut() {
                if key == Key::Escape {
                    self.menu = None;
                    continue;
                }
                if m.entry.is_some() {
                    if key == Key::Enter {
                        self.menu_entry_done();
                    }
                    continue;
                }
                match key {
                    Key::ArrowDown | Key::J => m.selected = (m.selected + 1).min(m.items.len().saturating_sub(1)),
                    Key::ArrowUp | Key::K => m.selected = m.selected.saturating_sub(1),
                    Key::Enter => {
                        let i = m.selected;
                        self.menu_activate(i);
                    }
                    Key::S => {
                        if let Some((param, value, starred)) = m.items.get(m.selected).and_then(|i| i.star.clone()) {
                            let id = m.id.clone();
                            self.send_quiet(Request::Star { id, param, value, starred: !starred });
                            self.menu = None;
                            self.toast(if starred { "Unstarred" } else { "Starred: it stays in the list" }, false);
                        }
                    }
                    _ => {
                        let c = letter(key).or_else(|| digit(key).and_then(|d| char::from_digit(d as u32, 10)));
                        let c = if key == Key::Colon || (key == Key::Semicolon && shift) { Some(':') } else { c };
                        if let Some(c) = c {
                            if let Some(p) = m.items.iter().position(|i| i.key == Some(c)) {
                                self.menu_activate(p);
                            }
                        }
                    }
                }
                continue;
            }
            if filter_focused {
                match key {
                    Key::Escape => {
                        if self.filter.is_empty() {
                            self.ctx.memory_mut(|m| m.surrender_focus(Id::unique("filter")));
                        } else {
                            self.filter.clear();
                        }
                    }
                    Key::Enter => {
                        let i = self.selected.unwrap_or(0);
                        if let Some(id) = self.order.get(i).cloned() {
                            self.activate(&id);
                        }
                    }
                    Key::ArrowDown => {
                        self.ctx.memory_mut(|m| m.surrender_focus(Id::unique("filter")));
                        self.selected = Some(0);
                    }
                    _ => {}
                }
                continue;
            }
            if let Mode::Strip(_) = self.mode {
                match key {
                    Key::Escape | Key::Backspace => self.mode = Mode::History,
                    Key::ArrowLeft => self.hist.step_run(-1),
                    Key::ArrowRight => self.hist.step_run(1),
                    _ => {
                        if let Some(r) = self.hist.selected_run() {
                            use crate::hist::Action;
                            match letter(key) {
                                Some('r') => self.history_action(Action::Replay(r)),
                                Some('l') => self.history_action(Action::OpenLog(r)),
                                Some('y') => self.history_action(Action::CopyError(r)),
                                _ => {}
                            }
                        }
                    }
                }
                continue;
            }
            if self.mode == Mode::History && !matches!(key, Key::Escape | Key::Tab) {
                match key {
                    Key::ArrowDown | Key::J => self.hist.move_sel(1),
                    Key::ArrowUp | Key::K => self.hist.move_sel(-1),
                    Key::Enter | Key::Space => {
                        let i = self.hist.selected;
                        self.hist.toggle(i);
                    }
                    Key::O | Key::ArrowRight => {
                        if let Some(t) = self.hist.selected_task() {
                            self.history_action(crate::hist::Action::OpenStrip(t));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if let Mode::Log(_) = self.mode {
                if key == Key::Escape {
                    self.log_key('\u{1b}');
                } else if let Some(c) = letter(key) {
                    if "yerfat".contains(c) {
                        self.log_key(c);
                    }
                }
                continue;
            }
            match key {
                Key::Escape => {
                    if !self.filter.is_empty() {
                        self.filter.clear();
                    } else {
                        self.hide();
                    }
                    continue;
                }
                Key::Tab => {
                    let n = self.tabs().len() + 2;
                    let cur = match self.mode {
                        Mode::Shelf => n - 1,
                        Mode::History | Mode::Strip(_) => n - 2,
                        _ => self.tab,
                    };
                    self.select_tab(if shift { (cur + n - 1) % n } else { (cur + 1) % n });
                    continue;
                }
                Key::Slash => {
                    self.focus_filter = true;
                    continue;
                }
                _ => {}
            }
            if self.mode == Mode::Shelf {
                match key {
                    Key::ArrowDown => self.shelf_move(1),
                    Key::ArrowUp => self.shelf_move(-1),
                    Key::Enter => self.shelf_activate(self.shelf_sel, false),
                    Key::M => self.shelf_activate(self.shelf_sel, true),
                    _ => {}
                }
                continue;
            }
            let cols = 4i64;
            let n = self.order.len() as i64;
            let mv = |s: Option<usize>, d: i64| -> Option<usize> {
                if n == 0 {
                    return None;
                }
                Some(match s {
                    None => 0,
                    Some(c) => (c as i64 + d).clamp(0, n - 1) as usize,
                })
            };
            match key {
                Key::ArrowRight => self.selected = mv(self.selected, 1),
                Key::ArrowLeft => self.selected = mv(self.selected, -1),
                Key::ArrowDown => self.selected = mv(self.selected, cols),
                Key::ArrowUp => self.selected = mv(self.selected, -cols),
                Key::Enter => {
                    if let Some(id) = self.order.get(self.selected.unwrap_or(0)).cloned() {
                        self.activate(&id);
                    }
                }
                Key::Space => {
                    if let Some(id) = self.order.get(self.selected.unwrap_or(0)).cloned() {
                        self.open_menu(&id);
                    }
                }
                _ => {
                    if let Some(d) = digit(key).filter(|d| *d >= 1) {
                        self.select_tab(d - 1);
                    } else if let Some(c) = letter(key) {
                        if let Some(id) = self.hints.iter().find(|(h, _)| *h == c).map(|(_, id)| id.clone()) {
                            if shift {
                                self.open_menu(&id);
                            } else if let Some(v) = self.task(&id) {
                                if v.confirm == Confirm::Hold && !matches!(v.phase, Phase::Failed | Phase::Seen) {
                                    self.start_hold(&id, Some(key));
                                } else {
                                    self.activate(&id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn shelf_move(&mut self, d: i64) {
        let n = self.shelf_rows.len() as i64;
        let mut i = self.shelf_sel as i64;
        loop {
            i += d;
            if i < 0 || i >= n {
                return;
            }
            if !matches!(self.shelf_rows[i as usize], ShelfRow::Section(_)) {
                self.shelf_sel = i as usize;
                return;
            }
        }
    }

    fn shelf_activate(&mut self, i: usize, man: bool) {
        match self.shelf_rows.get(i).cloned() {
            Some(ShelfRow::Task(id, _, _)) if !man => self.activate(&id),
            Some(ShelfRow::Tool(name, _)) => {
                let what = if man { OpenWhat::Man } else { OpenWhat::Tool };
                self.send(Request::Open { what, id: None, file: None, line: None, tool: Some(name) });
                self.hide();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- drawing

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let tabs = self.tabs();
            let mut entries: Vec<(String, usize)> = tabs
                .iter()
                .map(|t| (t.name.clone(), self.snap.as_ref().map(|s| s.tasks.iter().filter(|x| Self::in_tab(x, t)).count()).unwrap_or(0)))
                .collect();
            let shelf_n = self.snap.as_ref().map(|s| s.tasks.iter().filter(|t| t.shelf || t.stale).count()).unwrap_or(0)
                + self.shelf.as_ref().map(|d| d.tools.len()).unwrap_or(0);
            entries.push(("History".into(), 0));
            entries.push(("Shelf".into(), shelf_n));
            let active = match self.mode {
                Mode::Shelf => entries.len() - 1,
                Mode::History | Mode::Strip(_) => entries.len() - 2,
                _ => self.tab,
            };
            for (i, (name, n)) in entries.into_iter().enumerate() {
                let text = if n > 0 { format!("{name} {n}") } else { name };
                let galley = ui.painter().layout_no_wrap(text.clone(), mono(12.5), FG);
                let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), Sense::click());
                if i == active {
                    ui.painter().rect_filled(rect, CornerRadius::same(7), TILE_HOVER);
                }
                let col = if i == active || resp.hovered() { FG } else { DIM };
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, mono(12.5), col);
                if resp.clicked() {
                    self.select_tab(i);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let te = egui::TextEdit::singleline(&mut self.filter)
                    .id(Id::unique("filter"))
                    .hint_text("/  to filter")
                    .desired_width(150.0)
                    .font(mono(12.5));
                let r = ui.add(te);
                if self.focus_filter {
                    r.request_focus();
                    self.focus_filter = false;
                }
                if r.changed() {
                    self.selected = if self.filter.is_empty() { None } else { Some(0) };
                    if self.mode != Mode::Shelf {
                        self.mode = Mode::Grid;
                    }
                }
            });
        });
    }

    fn mainline(&self, ui: &mut egui::Ui) {
        let Some(s) = self.snap.as_ref() else { return };
        let m = &s.mainline;
        if m.current.is_none() && m.queue.is_empty() {
            return;
        }
        let label_of = |id: &str| s.tasks.iter().find(|t| t.id == id).map(|t| t.label.clone()).unwrap_or_else(|| id.to_string());
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        let p = ui.painter();
        let tint = if m.waiting_memory { STOP } else { CAUTION };
        p.rect_filled(rect, CornerRadius::same(9), tint.gamma_multiply(0.08));
        p.rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, tint.gamma_multiply(0.3)), StrokeKind::Inside);
        let pulse = 0.55 + 0.45 * ((self.now() * std::f64::consts::TAU / 1.4).sin() as f32 * 0.5 + 0.5);
        draw::lamp(p, pos2(rect.left() + 14.0, rect.center().y), Phase::Building, pulse, 0.0, 1.0);
        let (title, right, frac) = match &m.current {
            Some(cur) => {
                let step = s
                    .tasks
                    .iter()
                    .find(|t| &t.id == cur)
                    .and_then(|t| t.step.as_ref().filter(|st| st.total > 0).map(|st| format!(" · {}", st.name)))
                    .unwrap_or_default();
                let used = m.current_mem.unwrap_or(0);
                let waiting = match m.queue.len() {
                    0 => String::new(),
                    1 => format!(" · {} waiting", label_of(&m.queue[0])),
                    n => format!(" · {n} waiting"),
                };
                (
                    format!("Main line: {}{step}", label_of(cur)),
                    format!("{} of {}{waiting}", human(used), human(m.cap)),
                    used as f32 / m.cap.max(1) as f32,
                )
            }
            None => {
                let head = m.queue.first().map(|h| label_of(h)).unwrap_or_default();
                if m.waiting_memory {
                    (
                        format!("Main line: {head} waiting for memory"),
                        format!("needs {} free, {} available", human(m.min_free), human(m.mem_available)),
                        1.0 - m.mem_available as f32 / m.mem_total.max(1) as f32,
                    )
                } else {
                    (format!("Main line: {head} next"), String::new(), 0.0)
                }
            }
        };
        p.text(pos2(rect.left() + 26.0, rect.center().y), Align2::LEFT_CENTER, title, mono(12.0), FG);
        let rg = p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, right, mono(11.5), DIM);
        let g = Rect::from_min_size(pos2(rg.left() - 100.0, rect.center().y - 3.0), vec2(88.0, 6.0));
        draw::gauge(p, g, frac, tint);
    }

    fn grid(&mut self, ui: &mut egui::Ui, max_h: f32) {
        let list = self.visible_tasks();
        let ids: Vec<String> = list.iter().map(|t| t.id.clone()).collect();
        if ids != self.order {
            self.order = ids;
        }
        self.hints = list.iter().zip(HINTS.chars()).map(|(t, c)| (c, t.id.clone())).collect();
        if list.is_empty() {
            let msg = if !self.filter.is_empty() {
                format!("Nothing matches \"{}\".", self.filter)
            } else if self.snap.as_ref().is_none_or(|s| s.tasks.is_empty()) {
                "No tasks yet.\nAdd `scan` paths to your config, or run `yard add` in a repo.".to_string()
            } else {
                "Nothing on this tab.".to_string()
            };
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(msg).color(DIM)));
            ui.add_space(30.0);
            return;
        }
        let now = self.now();
        let pulse = ((now * std::f64::consts::TAU / 1.4).sin() as f32) * 0.5 + 0.5;
        let since_show = self.shown_at.elapsed().as_secs_f32() * 1000.0;
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let rows = list.len().div_ceil(4) as f32;
        let want = (rows * (TILE_H + 8.0)).min(max_h);
        egui::ScrollArea::vertical().min_scrolled_height(want).max_height(want).auto_shrink([false, false]).show(ui, |ui| {
            let gap = 8.0;
            let cols = 4usize;
            let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
            for (row, chunk) in list.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (col, v) in chunk.iter().enumerate() {
                        let i = row * cols + col;
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, TILE_H), Sense::click());
                        // Staggered entrance: each tile fades up a beat after the last.
                        let t = ((since_show - 70.0 - i as f32 * 26.0) / 260.0).clamp(0.0, 1.0);
                        let e = draw::ease_out(t);
                        let r = rect.translate(vec2(0.0, (1.0 - e) * 10.0));
                        let hold = self
                            .hold
                            .as_ref()
                            .filter(|h| h.id == v.id)
                            .map(|h| (h.started.elapsed().as_secs_f32() * 1000.0 / HOLD_MS).min(1.0))
                            .unwrap_or(0.0);
                        let look = draw::TileLook {
                            hover: resp.hovered(),
                            selected: self.selected == Some(i),
                            hint: self.hints.get(i).map(|h| h.0),
                            pulse,
                            hold,
                            opacity: e,
                            now,
                        };
                        draw::tile(ui.painter(), r, v, &look);
                        if resp.secondary_clicked() {
                            self.open_menu(&v.id);
                        } else if resp.is_pointer_button_down_on() && pointer_down && self.press.is_none() && self.hold.is_none() {
                            if self.menu.is_some() {
                                self.menu = None;
                            } else if v.confirm == Confirm::Hold && !matches!(v.phase, Phase::Failed | Phase::Seen) {
                                self.start_hold(&v.id, None);
                            } else {
                                self.press = Some((v.id.clone(), Instant::now(), false));
                            }
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
        // Releases and long presses are judged after layout, once per frame.
        if !pointer_down {
            if let Some(h) = self.hold.as_ref().filter(|h| h.key.is_none()) {
                let id = h.id.clone();
                self.cancel_hold(&id);
            }
            if let Some((id, _, consumed)) = self.press.take() {
                if !consumed {
                    self.activate(&id);
                }
            }
        } else if let Some((id, at, consumed)) = self.press.as_mut() {
            if !*consumed && at.elapsed().as_millis() >= LONG_PRESS_MS {
                *consumed = true;
                let id = id.clone();
                self.open_menu(&id);
            }
        }
    }

    fn log_view(&mut self, ui: &mut egui::Ui, max_h: f32) {
        let Mode::Log(id) = self.mode.clone() else { return };
        let Some(v) = self.task(&id) else { return };
        let now = self.now();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
            draw::lamp(ui.painter(), r.center(), v.phase, 1.0, 0.0, 1.0);
            ui.label(egui::RichText::new(&v.label).font(mono(14.0)).color(FG).strong());
            let (msg, col) = match v.phase {
                Phase::Failed | Phase::Seen => (v.message.clone().unwrap_or_else(|| "failed".into()), STOP),
                _ => (draw::status_line(&v, now), DIM),
            };
            ui.label(egui::RichText::new(msg).color(col));
        });
        ui.add_space(4.0);
        let Some(l) = self.log.as_mut() else { return };
        let scroll_to = l.scroll_to.take();
        egui::Frame::new().fill(WELL).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            let want = (l.lines.len() as f32 * 17.0 + 8.0).clamp(160.0, max_h - 70.0);
            egui::ScrollArea::both().min_scrolled_height(want).max_height(want).auto_shrink([false, false]).show(ui, |ui| {
                for (i, line) in l.lines.iter().enumerate() {
                    let err = l.error.is_some_and(|e| i >= e && i < e + 3 && (i == e || line.starts_with(char::is_whitespace)));
                    let color = if err {
                        Color32::from_rgb(0xff, 0xd5, 0xd9)
                    } else if line.starts_with("===== build ok") || line.starts_with("===== run ended · exit 0") {
                        GO
                    } else if line.starts_with("===== exit") || line.starts_with("===== run ended") {
                        Color32::from_rgb(0xe0, 0x84, 0x8c)
                    } else if line.starts_with("=====") {
                        FROST
                    } else {
                        Color32::from_rgb(0x9a, 0xa1, 0xaa)
                    };
                    let text = egui::RichText::new(yard_core::errors::strip_ansi(line)).font(mono(11.5)).color(color);
                    let resp = if err {
                        egui::Frame::new().fill(Color32::from_rgb(0x4a, 0x2a, 0x2f)).show(ui, |ui| ui.label(text)).response
                    } else {
                        ui.label(text)
                    };
                    if scroll_to == Some(i) {
                        resp.scroll_to_me(Some(if l.follow { egui::Align::BOTTOM } else { egui::Align::Center }));
                    }
                }
            });
        });
        ui.add_space(6.0);
        let has_err = v.error.is_some();
        let follow = l.follow;
        let open_label = v.error.as_ref().and_then(|e| e.line).map(|n| format!("e  open :{n}")).unwrap_or_else(|| "e  open repo".into());
        let mut clicked = None;
        ui.horizontal(|ui| {
            let btn = |ui: &mut egui::Ui, label: &str, primary: bool, active: bool| {
                let b = egui::Button::new(egui::RichText::new(label).font(mono(11.5)).color(if primary {
                    Color32::from_rgb(0x1b, 0x1d, 0x20)
                } else if active {
                    FROST
                } else {
                    FG
                }))
                .fill(if primary { STOP } else { Color32::TRANSPARENT })
                .stroke(Stroke::new(
                    1.0,
                    if primary {
                        STOP
                    } else if active {
                        FROST
                    } else {
                        BORDER
                    },
                ))
                .corner_radius(CornerRadius::same(7));
                ui.add(b).clicked()
            };
            if btn(ui, if has_err { "y  copy error" } else { "y  copy tail" }, has_err, false) {
                clicked = Some('y');
            }
            if btn(ui, &open_label, false, false) {
                clicked = Some('e');
            }
            if btn(ui, "r  run again", false, false) {
                clicked = Some('r');
            }
            if btn(ui, "f  follow", false, follow) {
                clicked = Some('f');
            }
            if matches!(v.phase, Phase::Failed | Phase::Seen) && btn(ui, "a  acknowledge", false, false) {
                clicked = Some('a');
            }
            if btn(ui, "esc  back", false, false) {
                clicked = Some('\u{1b}');
            }
        });
        if let Some(c) = clicked {
            self.log_key(c);
        }
    }

    fn shelf_view(&mut self, ui: &mut egui::Ui, max_h: f32) {
        let mut rows = vec![];
        let now = self.now();
        let q = self.filter.clone();
        if let Some(s) = self.snap.as_ref() {
            let shelved: Vec<&TaskView> = s.tasks.iter().filter(|t| t.shelf || t.stale).collect();
            let tasks = rank(&shelved, &q, |t| format!("{} {}", t.label, t.description));
            if !tasks.is_empty() {
                rows.push(ShelfRow::Section("TASKS".into()));
                rows.extend(tasks.into_iter().map(|t| ShelfRow::Task(t.id.clone(), t.label.clone(), draw::status_line(t, now))));
            }
        }
        match &self.shelf {
            None => rows.push(ShelfRow::Section("loading tools…".into())),
            Some(d) => {
                let tools = ranked(&d.tools, &q);
                if !tools.is_empty() {
                    rows.push(ShelfRow::Section("TOOLS".into()));
                    rows.extend(tools.into_iter().map(|t| ShelfRow::Tool(t.name.clone(), t.desc.clone())));
                }
                let binds = ranked(&d.binds, &q);
                if !binds.is_empty() {
                    rows.push(ShelfRow::Section("KEY BINDS".into()));
                    rows.extend(binds.into_iter().map(|t| ShelfRow::Bind(t.name.clone(), t.desc.clone())));
                }
                if rows.is_empty() {
                    rows.push(ShelfRow::Section("The Shelf is empty. Tools come from [[shelf.tool]] in the config.".into()));
                }
            }
        }
        if matches!(rows.get(self.shelf_sel), Some(ShelfRow::Section(_)) | None) {
            self.shelf_sel = rows.iter().position(|r| !matches!(r, ShelfRow::Section(_))).unwrap_or(0);
        }
        let mut activate = None;
        let want = (rows.len() as f32 * 24.0 + 8.0).clamp(120.0, max_h);
        egui::ScrollArea::vertical().min_scrolled_height(want).max_height(want).auto_shrink([false, false]).show(ui, |ui| {
            for (i, r) in rows.iter().enumerate() {
                match r {
                    ShelfRow::Section(t) => {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(t).font(mono(11.0)).color(DIM));
                    }
                    ShelfRow::Task(_, name, desc) | ShelfRow::Tool(name, desc) | ShelfRow::Bind(name, desc) => {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                        if i == self.shelf_sel || resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(6), TILE_HOVER);
                        }
                        ui.painter().text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, name, mono(12.5), FG);
                        ui.painter().text(pos2(rect.left() + 170.0, rect.center().y), Align2::LEFT_CENTER, desc, mono(12.0), DIM);
                        if resp.clicked() {
                            activate = Some(i);
                        }
                    }
                }
            }
        });
        self.shelf_rows = rows;
        if let Some(i) = activate {
            self.shelf_sel = i;
            self.shelf_activate(i, false);
        }
    }

    fn recent(&mut self, ui: &mut egui::Ui) {
        if self.mode != Mode::Grid || !self.filter.is_empty() {
            return;
        }
        let Some(s) = self.snap.as_ref() else { return };
        let mut recent: Vec<&TaskView> =
            s.tasks.iter().filter(|t| t.last_used.is_some() && !(self.order.contains(&t.id) && t.pin.is_some())).collect();
        recent.sort_by(|a, b| b.last_used.partial_cmp(&a.last_used).unwrap_or(std::cmp::Ordering::Equal));
        if recent.is_empty() {
            return;
        }
        let items: Vec<(String, String)> = recent.into_iter().take(4).map(|t| (t.id.clone(), t.label.clone())).collect();
        let mut run = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Recent").color(DIM).font(mono(11.5)));
            for (id, label) in items {
                let b = egui::Button::new(egui::RichText::new(label).font(mono(11.5)).color(FG))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10));
                if ui.add(b).clicked() {
                    run = Some(id);
                }
            }
        });
        if let Some(id) = run {
            self.activate(&id);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let p = ui.painter();
        p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BORDER));
        if let Some((t, err, until)) = &self.toast {
            if Instant::now() < *until {
                p.text(pos2(rect.left() + 4.0, rect.center().y + 2.0), Align2::LEFT_CENTER, t, mono(11.0), if *err { STOP } else { FG });
                return;
            }
        }
        let text = if self.menu.is_some() {
            "↵ choose   1-9 pick   s star   esc back".to_string()
        } else {
            match &self.mode {
                Mode::Grid => "a-z run   ⇧ variants   / filter   tab next   esc close".into(),
                Mode::Log(_) => self.log.as_ref().map(|l| format!("{}   t tail in a terminal", l.path.display())).unwrap_or_default(),
                Mode::Shelf => "↵ run   m man page   / filter   tab next   esc close".into(),
                Mode::History => "↑↓ move   ↵ expand   o open runs   tab next   esc close".into(),
                Mode::Strip(_) => "←→ runs   r replay   l log   y copy error   esc back".into(),
            }
        };
        p.text(pos2(rect.left() + 4.0, rect.center().y + 2.0), Align2::LEFT_CENTER, text, mono(11.0), DIM);
    }

    fn menu_overlay(&mut self, ctx: &egui::Context, panel: Rect) {
        let Some(m) = self.menu.as_mut() else { return };
        let mut activate = None;
        let mut entry_done = false;
        egui::Area::new(Id::unique("menu")).order(egui::Order::Foreground).pivot(Align2::CENTER_CENTER).fixed_pos(panel.center()).show(
            ctx,
            |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x1f, 0x22, 0x26))
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(11))
                    .inner_margin(egui::Margin::same(8))
                    .shadow(egui::Shadow { offset: [0, 12], blur: 30, spread: 0, color: Color32::from_black_alpha(150) })
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        ui.label(egui::RichText::new(&m.title).font(mono(13.0)).color(FG).strong());
                        ui.add_space(4.0);
                        for (i, it) in m.items.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                            if resp.hovered() {
                                m.selected = i;
                            }
                            if i == m.selected {
                                ui.painter().rect_filled(rect, CornerRadius::same(7), TILE_HOVER);
                            }
                            ui.painter().text(
                                pos2(rect.left() + 8.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                &it.label,
                                mono(12.5),
                                it.color,
                            );
                            let mut x = rect.right() - 8.0;
                            if let Some(k) = it.key {
                                let s = if k == '\n' { "↵".to_string() } else { k.to_string() };
                                let r = ui.painter().text(pos2(x, rect.center().y), Align2::RIGHT_CENTER, s, mono(10.5), DIM);
                                ui.painter().rect_stroke(
                                    r.expand2(vec2(4.0, 1.0)),
                                    CornerRadius::same(4),
                                    Stroke::new(1.0, BORDER),
                                    StrokeKind::Outside,
                                );
                                x = r.left() - 12.0;
                            }
                            if !it.detail.is_empty() {
                                ui.painter().text(pos2(x, rect.center().y), Align2::RIGHT_CENTER, &it.detail, mono(11.5), DIM);
                            }
                            if resp.clicked() {
                                activate = Some(i);
                            }
                        }
                        if let Some((text, param)) = m.entry.as_mut() {
                            let hint = match param {
                                Some(p) => format!("a value for {p}"),
                                None => m.id.clone(),
                            };
                            let r = ui.add(egui::TextEdit::singleline(text).hint_text(hint).font(mono(12.5)).desired_width(f32::INFINITY));
                            r.request_focus();
                            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                entry_done = true;
                            }
                        }
                    });
            },
        );
        if entry_done {
            self.menu_entry_done();
        } else if let Some(i) = activate {
            self.menu_activate(i);
        }
    }
}

fn read_tail(path: &std::path::Path, from: u64, max: u64) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = if len.saturating_sub(from) > max { len - max } else { from };
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).to_string();
    if start > from && start > 0 {
        if let Some(p) = text.find('\n') {
            text = text[p + 1..].to_string();
        }
    }
    Some((text, start + buf.len() as u64))
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.started {
            self.started = true;
            #[cfg(any(windows, target_os = "macos"))]
            {
                let (tx, rx) = channel();
                let c = ctx.clone();
                self.tray = crate::tray::Tray::new(tx, move || c.request_repaint());
                self.tray_rx = Some(rx);
            }
            if self.show_now {
                self.show(None);
            } else {
                // eframe shows the window once its first frame is painted,
                // whatever the builder said; at login that left an empty
                // panel in the middle of the screen until the first toggle.
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            let msgs: Vec<crate::tray::TrayMsg> = self.tray_rx.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
            for m in msgs {
                match m {
                    crate::tray::TrayMsg::Toggle(a) => self.toggle(Some(a)),
                    crate::tray::TrayMsg::Open => self.show(None),
                    crate::tray::TrayMsg::Ack => self.send(Request::Ack { phase: AckPhase::All, id: None }),
                    crate::tray::TrayMsg::Quit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            if let (Some(t), Some(s)) = (self.tray.as_mut(), self.snap.as_ref()) {
                t.update(s.aggregate.phase, &s.aggregate.tooltip());
            }
        }
        if let Some(t) = self.hide_at {
            if Instant::now() >= t {
                self.hide_at = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        if self.visible {
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
            if focused {
                self.was_focused = true;
            } else if self.was_focused && self.show_now_is_tray() {
                self.hide();
            }
            self.check_hold();
            if let Mode::Log(_) = self.mode {
                self.poll_log();
            }
            let busy = self.shown_at.elapsed() < Duration::from_millis(900)
                || self.hold.is_some()
                || self.press.is_some()
                || self.snap.as_ref().is_some_and(|s| s.tasks.iter().any(|t| matches!(t.phase, Phase::Building | Phase::Waiting)));
            ctx.request_repaint_after(Duration::from_millis(if busy { 16 } else { 250 }));
        }
        if let Some((_, _, until)) = &self.toast {
            if Instant::now() >= *until {
                self.toast = None;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let events = ctx.input(|i| i.events.clone());
        let filter_focused = ctx.memory(|m| m.has_focus(Id::unique("filter")));
        if self.visible {
            self.keys(&events, filter_focused);
        }
        let mon = self.monitor();
        let fade = if self.visible {
            draw::ease_out((self.shown_at.elapsed().as_secs_f32() * 1000.0 / 200.0).min(1.0))
        } else {
            self.hide_at.map(|t| (t.saturating_duration_since(Instant::now()).as_secs_f32() * 1000.0 / 130.0).min(1.0)).unwrap_or(0.0)
        };
        let max_h = (mon.y * 0.8 - 190.0).max(240.0);
        // Rounded, see-through corners where the platform composites them.
        let radius = if cfg!(target_os = "macos") { 14 } else { 0 };
        let resp = egui::Area::new(Id::unique("panel")).fixed_pos(pos2(0.0, 0.0)).constrain(false).show(&ctx, |ui| {
            ui.set_opacity(fade.max(0.05));
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(radius))
                .inner_margin(egui::Margin { left: 12, right: 12, top: 12, bottom: 4 })
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 26.0);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.header(ui);
                    if let Some(p) = self.snap.as_ref().and_then(|s| s.problems.first().cloned()) {
                        ui.label(egui::RichText::new(format!("⚠ {}: {}", p.file, p.message)).color(CAUTION).font(mono(11.5)));
                    }
                    self.mainline(ui);
                    match self.mode.clone() {
                        Mode::Grid => self.grid(ui, max_h),
                        Mode::Log(_) => self.log_view(ui, max_h),
                        Mode::Shelf => self.shelf_view(ui, max_h),
                        Mode::History => {
                            if let Some(a) = self.hist.feed(ui, max_h) {
                                self.history_action(a);
                            }
                        }
                        Mode::Strip(_) => {
                            let now = self.now();
                            if let Some(a) = self.hist.strip(ui, now) {
                                self.history_action(a);
                            }
                        }
                    }
                    self.recent(ui);
                    self.footer(ui);
                });
        });
        // The window is exactly the panel; it grows away from its anchor.
        let size = resp.response.rect.size();
        if (size - self.win_size).length() > 1.0 {
            self.win_size = size;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            self.reposition();
        }
        let panel = resp.response.rect;
        // A press outside the panel closes the board.
        if self.visible && self.menu.is_none() {
            let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !panel.contains(p)));
            if outside && self.show_now_is_tray() {
                self.hide();
            }
        }
        self.menu_overlay(&ctx, panel);
    }
}

impl App {
    /// The dev window on Linux stays open; the tray board hides on blur.
    fn show_now_is_tray(&self) -> bool {
        cfg!(any(windows, target_os = "macos"))
    }
}

#[allow(dead_code)]
fn unused(_: Pos2) {}
