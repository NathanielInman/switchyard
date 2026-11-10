//! The board: window, layout, state and input.

use crate::anim::{animations_enabled, Motion, Spring};
use crate::logview::LogView;
use crate::stage::Stage;
use crate::tile::Tile;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge as LsEdge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};
use yard_core::manifest::Confirm;
use yard_core::model::*;
use yard_core::protocol::{AckPhase, Event, OpenWhat, Reply, Request};
use yard_core::size::human;

pub enum Msg {
    Event(Event),
    Connected,
    Disconnected,
    Reply(Reply),
    Shelf(ShelfData),
}

thread_local! {
    static CLOCK_SKEW: Cell<f64> = const { Cell::new(0.0) };
}

/// The daemon's idea of "now", so elapsed times match its timestamps.
pub fn now_corrected() -> f64 {
    now() + CLOCK_SKEW.with(|c| c.get())
}

const HINTS: &str = "asdfghjklqwertyuiopzxcvbnm";
const HOLD_MS: f64 = 1200.0;
const LONG_PRESS_MS: u128 = 450;

#[derive(Clone, PartialEq)]
enum Mode {
    Grid,
    Log(String),
    Shelf,
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
    class: Option<&'static str>,
}

struct Menu {
    id: String,
    items: Vec<MenuItem>,
    list: gtk::ListBox,
    entry: Option<(gtk::Entry, Option<String>)>,
}

struct Hold {
    id: String,
    started: Instant,
    key: Option<char>,
    params: BTreeMap<String, String>,
    variant: Option<String>,
}

struct Press {
    id: String,
    started: Instant,
    consumed: bool,
}

#[derive(Clone)]
enum ShelfRow {
    Task(String),
    Tool(String),
    Bind,
}

pub struct Board {
    window: gtk::ApplicationWindow,
    stage: Stage,
    spring: Spring,
    tabs_box: gtk::Box,
    search: gtk::Entry,
    problems: gtk::Label,
    mainline: gtk::Box,
    ml_lamp: gtk::Label,
    ml_label: gtk::Label,
    ml_gauge: gtk::DrawingArea,
    ml_text: gtk::Label,
    stack: gtk::Stack,
    flow: gtk::FlowBox,
    grid_scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    log: LogView,
    shelf_list: gtk::ListBox,
    recent: gtk::Box,
    footer: gtk::Label,
    menu_holder: gtk::Box,
    tx: async_channel::Sender<Msg>,

    snap: RefCell<Option<Snapshot>>,
    tiles: RefCell<HashMap<String, Rc<Tile>>>,
    order: RefCell<Vec<String>>,
    tab: Cell<usize>,
    mode: RefCell<Mode>,
    visible: Cell<bool>,
    selected: Cell<Option<usize>>,
    hints: RefCell<Vec<(char, String)>>,
    menu: RefCell<Option<Menu>>,
    shelf: RefCell<Option<ShelfData>>,
    shelf_rows: RefCell<Vec<ShelfRow>>,
    hold: RefCell<Option<Hold>>,
    press: RefCell<Option<Press>>,
    /// A key whose hold already fired; its repeats are ignored until release.
    spent_key: Cell<Option<char>>,
    toast_until: Cell<Option<Instant>>,
    ticker: RefCell<Option<glib::SourceId>>,
    last_slow: Cell<Option<Instant>>,
    connected: Cell<bool>,
    gauge_frac: Cell<f64>,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l.set_xalign(0.0);
    l
}

/// Subsequence match with a score: earlier and contiguous hits score higher.
fn fuzzy(hay: &str, needle: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for n in needle.to_lowercase().chars() {
        let found = hay[pos..].iter().position(|c| *c == n)? + pos;
        score += 10 - (found as i64).min(9);
        if last.is_some_and(|l| l + 1 == found) {
            score += 8;
        }
        last = Some(found);
        pos = found + 1;
    }
    Some(score)
}

impl Board {
    pub fn new(app: &gtk::Application, tx: async_channel::Sender<Msg>) -> Rc<Board> {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("switchyard");
        window.set_decorated(false);
        window.set_title(Some("Switchyard"));
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("switchyard"));
        for e in [LsEdge::Top, LsEdge::Bottom, LsEdge::Left, LsEdge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::Exclusive);

        // ---- header
        let tabs_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tabs_box.add_css_class("tabs");
        tabs_box.set_hexpand(true);
        let search = gtk::Entry::new();
        search.add_css_class("search");
        search.set_placeholder_text(Some("/  to filter"));
        search.set_width_chars(16);
        search.set_focusable(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.append(&tabs_box);
        header.append(&search);

        let problems = label("", "problems");
        problems.set_visible(false);
        problems.set_wrap(true);

        // ---- main line
        let mainline = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        mainline.add_css_class("mainline");
        let ml_lamp = gtk::Label::new(Some("●"));
        ml_lamp.add_css_class("lamp");
        ml_lamp.add_css_class("building");
        let ml_label = label("", "");
        ml_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        ml_label.set_hexpand(true);
        let ml_gauge = gtk::DrawingArea::new();
        ml_gauge.add_css_class("gauge");
        ml_gauge.set_content_height(6);
        ml_gauge.set_content_width(90);
        ml_gauge.set_valign(gtk::Align::Center);
        let ml_text = label("", "dim");
        mainline.append(&ml_lamp);
        mainline.append(&ml_label);
        mainline.append(&ml_gauge);
        mainline.append(&ml_text);
        mainline.set_visible(false);

        // ---- grid
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(2);
        flow.set_max_children_per_line(4);
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        flow.set_valign(gtk::Align::Start);
        flow.set_focusable(false);
        let empty = label("", "dim");
        empty.set_xalign(0.5);
        empty.set_margin_top(30);
        empty.set_margin_bottom(30);
        empty.set_wrap(true);
        empty.set_justify(gtk::Justification::Center);
        let grid_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        grid_box.append(&flow);
        grid_box.append(&empty);
        let grid_scroller = gtk::ScrolledWindow::new();
        grid_scroller.set_child(Some(&grid_box));
        grid_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        grid_scroller.set_propagate_natural_height(true);
        grid_scroller.set_max_content_height(560);

        let log = LogView::new();

        let shelf_list = gtk::ListBox::new();
        shelf_list.set_selection_mode(gtk::SelectionMode::Single);
        let shelf_scroller = gtk::ScrolledWindow::new();
        shelf_scroller.set_child(Some(&shelf_list));
        shelf_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        shelf_scroller.set_min_content_height(320);
        shelf_scroller.set_max_content_height(520);
        shelf_scroller.set_propagate_natural_height(true);
        shelf_scroller.add_css_class("shelf");

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideUpDown);
        stack.set_transition_duration(if animations_enabled() { 220 } else { 0 });
        stack.set_vhomogeneous(false);
        stack.set_interpolate_size(true);
        stack.add_named(&grid_scroller, Some("grid"));
        stack.add_named(&log.root, Some("log"));
        stack.add_named(&shelf_scroller, Some("shelf"));

        let recent = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        recent.add_css_class("recent");

        let footer = label("", "footer");
        footer.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body.append(&header);
        body.append(&problems);
        body.append(&mainline);
        body.append(&stack);
        body.append(&recent);
        body.append(&footer);

        let menu_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        menu_holder.set_halign(gtk::Align::Center);
        menu_holder.set_valign(gtk::Align::Center);
        menu_holder.set_visible(false);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&body));
        overlay.add_overlay(&menu_holder);

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("panel");
        panel.append(&overlay);
        panel.set_overflow(gtk::Overflow::Hidden);

        let spring = Spring::new(&panel, Motion::PANEL);
        let stage = Stage::new(&spring, 640.0, 0.8, 8.0);
        window.set_child(Some(&stage));

        let b = Rc::new(Board {
            window,
            stage,
            spring,
            tabs_box,
            search,
            problems,
            mainline,
            ml_lamp,
            ml_label,
            ml_gauge,
            ml_text,
            stack,
            flow,
            grid_scroller,
            empty,
            log,
            shelf_list,
            recent,
            footer,
            menu_holder,
            tx,
            snap: RefCell::new(None),
            tiles: RefCell::new(HashMap::new()),
            order: RefCell::new(vec![]),
            tab: Cell::new(0),
            mode: RefCell::new(Mode::Grid),
            visible: Cell::new(false),
            selected: Cell::new(None),
            hints: RefCell::new(vec![]),
            menu: RefCell::new(None),
            shelf: RefCell::new(None),
            shelf_rows: RefCell::new(vec![]),
            hold: RefCell::new(None),
            press: RefCell::new(None),
            spent_key: Cell::new(None),
            toast_until: Cell::new(None),
            ticker: RefCell::new(None),
            last_slow: Cell::new(None),
            connected: Cell::new(false),
            gauge_frac: Cell::new(0.0),
        });
        b.wire();
        b
    }

    fn wire(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.stage.connect_outside(move || {
            if let Some(b) = w.upgrade() {
                b.hide();
            }
        });
        let w = Rc::downgrade(self);
        self.stage.connect_placed(move |(ox, oy)| {
            if let Some(b) = w.upgrade() {
                b.spring.set_origin(ox, oy);
            }
        });
        let w = Rc::downgrade(self);
        self.ml_gauge.set_draw_func(move |area, cr, wd, ht| {
            let Some(b) = w.upgrade() else { return };
            let c = area.color();
            let (wd, ht) = (wd as f64, ht as f64);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.1);
            crate::tile::rounded(cr, 0.0, 0.0, wd, ht, ht / 2.0);
            let _ = cr.fill();
            cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 1.0);
            crate::tile::rounded(cr, 0.0, 0.0, (wd * b.gauge_frac.get()).max(ht), ht, ht / 2.0);
            let _ = cr.fill();
        });

        // Keyboard.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| match w.upgrade() {
            Some(b) => b.key_pressed(key, state),
            None => glib::Propagation::Proceed,
        });
        let w = Rc::downgrade(self);
        keys.connect_key_released(move |_, key, _, _| {
            if let Some(b) = w.upgrade() {
                b.key_released(key);
            }
        });
        self.window.add_controller(keys);

        let w = Rc::downgrade(self);
        self.search.connect_changed(move |e| {
            if let Some(b) = w.upgrade() {
                // The "/" that focused the box can arrive as text too.
                let t = e.text();
                if t.starts_with('/') {
                    e.set_text(t.trim_start_matches('/'));
                    e.set_position(-1);
                    return;
                }
                b.selected.set(if b.search.text().is_empty() { None } else { Some(0) });
                if *b.mode.borrow() == Mode::Shelf {
                    b.render_shelf();
                } else {
                    b.set_mode(Mode::Grid);
                    b.render_grid(false);
                }
                b.render_recent();
            }
        });
        let w = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            if let Some(b) = w.upgrade() {
                b.activate_selected();
            }
        });

        let w = Rc::downgrade(self);
        self.shelf_list.connect_row_activated(move |_, row| {
            if let Some(b) = w.upgrade() {
                b.activate_shelf_row(row.index());
            }
        });

        // Log buttons.
        let w = Rc::downgrade(self);
        let act = move |k: char| {
            let w = w.clone();
            move |_: &gtk::Button| {
                if let Some(b) = w.upgrade() {
                    b.log_key(k);
                }
            }
        };
        self.log.copy_btn.connect_clicked(act('y'));
        self.log.open_btn.connect_clicked(act('e'));
        self.log.run_btn.connect_clicked(act('r'));
        self.log.follow_btn.connect_clicked(act('f'));
        self.log.ack_btn.connect_clicked(act('a'));
        self.log.back_btn.connect_clicked(act('\u{1b}'));
    }

    // ------------------------------------------------------------- messages

    pub fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Connected => {
                self.connected.set(true);
            }
            Msg::Disconnected => {
                if self.connected.get() {
                    self.toast("lost the daemon; reconnecting", true);
                }
                self.connected.set(false);
            }
            Msg::Reply(r) => {
                if let Some(e) = r.error {
                    self.toast(&e, true);
                } else if let Some(m) = r.message {
                    self.toast(&m, false);
                }
            }
            Msg::Shelf(d) => {
                *self.shelf.borrow_mut() = Some(d);
                if *self.mode.borrow() == Mode::Shelf {
                    self.render_shelf();
                }
            }
            Msg::Event(ev) => match ev {
                Event::Snapshot(s) => self.apply_snapshot(*s),
                Event::Show { anchor } => self.show(anchor),
                Event::Hide => self.hide(),
                Event::Toggle { anchor } => {
                    if self.visible.get() {
                        self.hide();
                    } else {
                        self.show(anchor);
                    }
                }
                Event::Bar(_) => {}
            },
        }
    }

    fn send(&self, req: Request) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = yard_core::protocol::request(&req).unwrap_or_else(|e| Reply::err(format!("{e:#}")));
            let quiet = r.ok && r.message.is_none();
            if !quiet {
                let _ = tx.send_blocking(Msg::Reply(r));
            }
        });
    }

    fn send_quiet(&self, req: Request) {
        std::thread::spawn(move || {
            let _ = yard_core::protocol::request(&req);
        });
    }

    fn apply_snapshot(self: &Rc<Self>, s: Snapshot) {
        CLOCK_SKEW.with(|c| c.set(s.now - now()));
        let geometry_changed = self.snap.borrow().as_ref().is_none_or(|o| o.board != s.board);
        if geometry_changed {
            self.stage.set_geometry(s.board.width as f64, s.board.max_height, s.board.gap as f64);
            if let Some(font) = &s.board.font {
                crate::apply_font(font);
            }
        }
        // Drop tiles for tasks that no longer exist.
        {
            let ids: Vec<&str> = s.tasks.iter().map(|t| t.id.as_str()).collect();
            self.tiles.borrow_mut().retain(|k, _| ids.contains(&k.as_str()));
        }
        *self.snap.borrow_mut() = Some(s);
        self.render(false);
    }

    fn task(&self, id: &str) -> Option<TaskView> {
        self.snap.borrow().as_ref()?.tasks.iter().find(|t| t.id == id).cloned()
    }

    // ------------------------------------------------------------- show/hide

    fn show(self: &Rc<Self>, anchor: Option<Anchor>) {
        if let Some(name) = anchor.as_ref().and_then(|a| a.monitor.clone()) {
            if let Some(m) = find_monitor(&name) {
                self.window.set_monitor(Some(&m));
            }
        }
        self.stage.set_anchor(anchor);
        self.close_menu();
        self.search.set_text("");
        self.selected.set(None);
        *self.hold.borrow_mut() = None;
        // A fresh failure opens straight to its log.
        let failed = self.snap.borrow().as_ref().and_then(|s| {
            s.tasks
                .iter()
                .filter(|t| t.phase == Phase::Failed)
                .max_by(|a, b| a.since.partial_cmp(&b.since).unwrap())
                .map(|t| t.id.clone())
        });
        let was_visible = self.visible.get();
        self.visible.set(true);
        match failed {
            Some(id) => self.open_log(&id),
            None => {
                let m = self.mode.borrow().clone();
                if let Mode::Log(_) = m {
                    self.set_mode(Mode::Grid);
                }
            }
        }
        self.render(true);
        self.window.set_visible(true);
        self.window.present();
        self.spring.play_in(0);
        self.grid_scroller.vadjustment().set_value(0.0);
        if !was_visible {
            self.send_quiet(Request::BoardVisible { visible: true });
            self.send_quiet(Request::Ack { phase: AckPhase::Open, id: None });
        }
        self.start_ticker();
    }

    fn hide(self: &Rc<Self>) {
        if !self.visible.get() {
            return;
        }
        self.visible.set(false);
        *self.hold.borrow_mut() = None;
        *self.press.borrow_mut() = None;
        self.close_menu();
        let w = Rc::downgrade(self);
        self.spring.play_out(move || {
            if let Some(b) = w.upgrade() {
                if !b.visible.get() {
                    b.window.set_visible(false);
                }
            }
        });
        self.send_quiet(Request::BoardVisible { visible: false });
        self.send_quiet(Request::Ack { phase: AckPhase::Close, id: None });
        if let Some(t) = self.ticker.borrow_mut().take() {
            t.remove();
        }
    }

    fn start_ticker(self: &Rc<Self>) {
        if self.ticker.borrow().is_some() {
            return;
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(b) = w.upgrade() else { return glib::ControlFlow::Break };
            if !b.visible.get() {
                *b.ticker.borrow_mut() = None;
                return glib::ControlFlow::Break;
            }
            b.frame();
            glib::ControlFlow::Continue
        });
        *self.ticker.borrow_mut() = Some(id);
    }

    /// Runs every frame while the board is open.
    fn frame(self: &Rc<Self>) {
        let anim = animations_enabled();
        for id in self.order.borrow().iter() {
            if let Some(t) = self.tiles.borrow().get(id) {
                if anim {
                    t.animate();
                }
            }
        }
        // Hold-to-confirm progress.
        let fire = {
            let hold = self.hold.borrow();
            match hold.as_ref() {
                Some(h) => {
                    let p = (h.started.elapsed().as_secs_f64() * 1000.0 / HOLD_MS).min(1.0);
                    if let Some(t) = self.tiles.borrow().get(&h.id) {
                        t.hold.set(p);
                        t.redraw_lamp();
                    }
                    p >= 1.0
                }
                None => false,
            }
        };
        if fire {
            if let Some(h) = self.hold.borrow_mut().take() {
                self.spent_key.set(h.key);
                if let Some(t) = self.tiles.borrow().get(&h.id) {
                    t.hold.set(0.0);
                    t.redraw_lamp();
                }
                self.close_menu();
                self.send(Request::Run { id: h.id, params: h.params, variant: h.variant, clean: false });
            }
        }
        // Long press opens variants.
        let long = {
            let mut p = self.press.borrow_mut();
            match p.as_mut() {
                Some(pr) if !pr.consumed && pr.started.elapsed().as_millis() >= LONG_PRESS_MS => {
                    pr.consumed = true;
                    Some(pr.id.clone())
                }
                _ => None,
            }
        };
        if let Some(id) = long {
            self.open_menu(&id);
        }
        let slow_due = self.last_slow.get().is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if slow_due {
            self.last_slow.set(Some(Instant::now()));
            self.slow_tick();
        }
    }

    /// Twice a second: clocks, log follow, toast expiry.
    fn slow_tick(self: &Rc<Self>) {
        let now = now_corrected();
        let snap = self.snap.borrow().clone();
        if let Some(s) = &snap {
            for id in self.order.borrow().iter() {
                if let (Some(t), Some(v)) = (self.tiles.borrow().get(id), s.tasks.iter().find(|x| &x.id == id)) {
                    if v.phase.active() || v.phase == Phase::Failed {
                        t.update(v, now);
                    }
                }
            }
        }
        if let Mode::Log(id) = self.mode.borrow().clone() {
            self.log.poll();
            if let Some(v) = self.task(&id) {
                self.log.update_head(&v);
            }
        }
        if self.toast_until.get().is_some_and(|t| Instant::now() >= t) {
            self.toast_until.set(None);
            self.render_footer();
        }
    }

    // ------------------------------------------------------------- rendering

    fn render(self: &Rc<Self>, animate: bool) {
        self.render_tabs();
        self.render_problems();
        self.render_mainline();
        match self.mode.borrow().clone() {
            Mode::Grid => self.render_grid(animate),
            Mode::Shelf => self.render_shelf(),
            Mode::Log(id) => {
                if let Some(v) = self.task(&id) {
                    self.log.update_head(&v);
                }
                self.render_grid(false);
            }
        }
        self.render_recent();
        self.render_footer();
        if let Some(m) = self.menu.borrow().as_ref() {
            let _ = m;
        }
    }

    fn tabs(&self) -> Vec<yard_core::config::TabConfig> {
        self.snap.borrow().as_ref().map(|s| s.tabs.clone()).unwrap_or_default()
    }

    fn in_tab(t: &TaskView, tab: &yard_core::config::TabConfig) -> bool {
        !t.shelf && (tab.tags.is_empty() || t.tags.iter().any(|g| tab.tags.contains(g)))
    }

    fn render_tabs(self: &Rc<Self>) {
        while let Some(c) = self.tabs_box.first_child() {
            self.tabs_box.remove(&c);
        }
        let snap = self.snap.borrow();
        let Some(s) = snap.as_ref() else { return };
        let mut entries: Vec<(String, usize)> = s
            .tabs
            .iter()
            .map(|tab| (tab.name.clone(), s.tasks.iter().filter(|t| Self::in_tab(t, tab)).count()))
            .collect();
        let shelf_count = s.tasks.iter().filter(|t| t.shelf || t.stale).count()
            + self.shelf.borrow().as_ref().map(|d| d.tools.len()).unwrap_or(0);
        entries.push(("Shelf".into(), shelf_count));
        let active = if *self.mode.borrow() == Mode::Shelf { entries.len() - 1 } else { self.tab.get() };
        for (i, (name, n)) in entries.into_iter().enumerate() {
            let b = gtk::Button::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.append(&gtk::Label::new(Some(&name)));
            if n > 0 {
                let c = gtk::Label::new(Some(&n.to_string()));
                c.add_css_class("count");
                row.append(&c);
            }
            b.set_child(Some(&row));
            b.set_focusable(false);
            if i == active {
                b.add_css_class("active");
            }
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(b) = w.upgrade() {
                    b.select_tab(i);
                }
            });
            self.tabs_box.append(&b);
        }
    }

    fn select_tab(self: &Rc<Self>, i: usize) {
        let n = self.tabs().len();
        self.close_menu();
        if i >= n {
            self.set_mode(Mode::Shelf);
            if self.shelf.borrow().is_none() {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    if let Ok(r) = yard_core::protocol::request(&Request::Shelf) {
                        if let Some(d) = r.shelf {
                            let _ = tx.send_blocking(Msg::Shelf(d));
                        }
                    }
                });
            }
            self.render_shelf();
        } else {
            self.tab.set(i);
            self.set_mode(Mode::Grid);
            self.selected.set(None);
            *self.order.borrow_mut() = vec![];
            self.render_grid(true);
        }
        self.render_tabs();
        self.render_recent();
        self.render_footer();
    }

    fn render_problems(&self) {
        let snap = self.snap.borrow();
        let Some(s) = snap.as_ref() else { return };
        if s.problems.is_empty() {
            self.problems.set_visible(false);
            return;
        }
        let first = &s.problems[0];
        let more = if s.problems.len() > 1 { format!(" (+{} more)", s.problems.len() - 1) } else { String::new() };
        self.problems.set_text(&format!("⚠ {}: {}{more}", first.file, first.message));
        let all: Vec<String> = s.problems.iter().map(|p| format!("{}: {}", p.file, p.message)).collect();
        self.problems.set_tooltip_text(Some(&all.join("\n")));
        self.problems.set_visible(true);
    }

    fn render_mainline(&self) {
        let snap = self.snap.borrow();
        let Some(s) = snap.as_ref() else { return };
        let m = &s.mainline;
        let label_of = |id: &str| s.tasks.iter().find(|t| t.id == id).map(|t| t.label.clone()).unwrap_or_else(|| id.to_string());
        if m.current.is_none() && m.queue.is_empty() {
            self.mainline.set_visible(false);
            return;
        }
        self.mainline.set_visible(true);
        if m.waiting_memory {
            self.mainline.add_css_class("memory");
        } else {
            self.mainline.remove_css_class("memory");
        }
        match &m.current {
            Some(cur) => {
                let step = s
                    .tasks
                    .iter()
                    .find(|t| &t.id == cur)
                    .and_then(|t| t.step.as_ref().filter(|st| st.total > 0).map(|st| format!(" · {}", st.name)))
                    .unwrap_or_default();
                self.ml_label.set_text(&format!("Main line: {}{step}", label_of(cur)));
                let used = m.current_mem.unwrap_or(0);
                self.gauge_frac.set((used as f64 / m.cap.max(1) as f64).clamp(0.0, 1.0));
                self.ml_gauge.set_visible(true);
                let waiting = if m.queue.is_empty() {
                    String::new()
                } else if m.queue.len() == 1 {
                    format!(" · {} waiting", label_of(&m.queue[0]))
                } else {
                    format!(" · {} waiting", m.queue.len())
                };
                self.ml_text.set_text(&format!("{} of {}{waiting}", human(used), human(m.cap)));
            }
            None => {
                let head = m.queue.first().map(|h| label_of(h)).unwrap_or_default();
                if m.waiting_memory {
                    self.ml_label.set_text(&format!("Main line: {head} waiting for memory"));
                    self.gauge_frac.set(1.0 - (m.mem_available as f64 / m.mem_total.max(1) as f64));
                    self.ml_text.set_text(&format!("needs {} free, {} available", human(m.min_free), human(m.mem_available)));
                } else {
                    self.ml_label.set_text(&format!("Main line: {head} next"));
                    self.ml_text.set_text("");
                }
            }
        }
        self.ml_gauge.queue_draw();
    }

    fn visible_tasks(&self) -> Vec<TaskView> {
        let snap = self.snap.borrow();
        let Some(s) = snap.as_ref() else { return vec![] };
        let q = self.search.text().to_string();
        if !q.is_empty() {
            let mut scored: Vec<(i64, &TaskView)> = s
                .tasks
                .iter()
                .filter_map(|t| {
                    let hay = format!("{} {} {} {}", t.label, t.id, t.tags.join(" "), t.description);
                    fuzzy(&hay, &q).map(|sc| (sc, t))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then(b.1.frecency.partial_cmp(&a.1.frecency).unwrap_or(std::cmp::Ordering::Equal))
                    .then(a.1.label.cmp(&b.1.label))
            });
            return scored.into_iter().map(|(_, t)| t.clone()).collect();
        }
        let tabs = &s.tabs;
        let tab = tabs.get(self.tab.get()).or_else(|| tabs.first());
        let mut list: Vec<TaskView> = s
            .tasks
            .iter()
            .filter(|t| tab.is_none_or(|tab| Self::in_tab(t, tab)))
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            (a.pin.is_none(), a.pin.unwrap_or(0), a.label.to_lowercase()).cmp(&(b.pin.is_none(), b.pin.unwrap_or(0), b.label.to_lowercase()))
        });
        list
    }

    fn tile_for(self: &Rc<Self>, v: &TaskView) -> Rc<Tile> {
        if let Some(t) = self.tiles.borrow().get(&v.id) {
            return t.clone();
        }
        let t = Tile::new(v);
        self.wire_tile(&t);
        self.tiles.borrow_mut().insert(v.id.clone(), t.clone());
        t
    }

    fn wire_tile(self: &Rc<Self>, t: &Rc<Tile>) {
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let id = t.id.clone();
        let w = Rc::downgrade(self);
        click.connect_pressed(move |g, _, _, _| {
            let Some(b) = w.upgrade() else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            if g.current_button() == 3 {
                b.open_menu(&id);
                return;
            }
            if b.menu.borrow().is_some() {
                b.close_menu();
                return;
            }
            let Some(v) = b.task(&id) else { return };
            if v.confirm == Confirm::Hold && !matches!(v.phase, Phase::Failed | Phase::Seen) {
                b.start_hold(&id, None, BTreeMap::new(), None);
            } else {
                *b.press.borrow_mut() = Some(Press { id: id.clone(), started: Instant::now(), consumed: false });
            }
        });
        let id = t.id.clone();
        let w = Rc::downgrade(self);
        click.connect_released(move |g, _, _, _| {
            let Some(b) = w.upgrade() else { return };
            if g.current_button() == 3 {
                return;
            }
            if b.cancel_hold(&id) {
                return;
            }
            let press = b.press.borrow_mut().take();
            if let Some(p) = press {
                if p.id == id && !p.consumed {
                    b.activate(&id);
                }
            }
        });
        let w = Rc::downgrade(self);
        let id = t.id.clone();
        click.connect_cancel(move |_, _| {
            if let Some(b) = w.upgrade() {
                b.cancel_hold(&id);
                *b.press.borrow_mut() = None;
            }
        });
        t.root.add_controller(click);
    }

    fn render_grid(self: &Rc<Self>, animate: bool) {
        let list = self.visible_tasks();
        let ids: Vec<String> = list.iter().map(|t| t.id.clone()).collect();
        let now = now_corrected();
        let changed = *self.order.borrow() != ids;
        if changed {
            self.flow.remove_all();
            for v in &list {
                let t = self.tile_for(v);
                let w = t.widget().clone();
                if let Some(p) = w.parent() {
                    if let Some(fc) = p.downcast_ref::<gtk::FlowBoxChild>() {
                        fc.set_child(None::<&gtk::Widget>);
                    }
                }
                self.flow.append(&w);
                if let Some(fc) = w.parent() {
                    fc.set_focusable(false);
                }
            }
            *self.order.borrow_mut() = ids.clone();
        }
        // Hints in display order.
        let mut hints = vec![];
        for (i, v) in list.iter().enumerate() {
            let t = self.tile_for(v);
            t.update(v, now);
            let h = HINTS.chars().nth(i);
            t.set_hint(h);
            if let Some(c) = h {
                hints.push((c, v.id.clone()));
            }
            t.set_selected(self.selected.get() == Some(i));
            if (changed || animate) && self.visible.get() {
                t.spring.play_in(if animate { 70 + i as u32 * 26 } else { i as u32 * 18 });
            }
        }
        *self.hints.borrow_mut() = hints;
        let q = self.search.text();
        if list.is_empty() {
            let msg = if !q.is_empty() {
                format!("Nothing matches \"{q}\".")
            } else if self.snap.borrow().as_ref().is_none_or(|s| s.tasks.is_empty()) {
                "No tasks yet.\nAdd `scan` paths to ~/.config/switchyard/config.toml,\nor run `yard add` in a repo.".to_string()
            } else {
                "Nothing on this tab.".to_string()
            };
            self.empty.set_text(&msg);
            self.empty.set_visible(true);
        } else {
            self.empty.set_visible(false);
        }
    }

    fn render_recent(self: &Rc<Self>) {
        while let Some(c) = self.recent.first_child() {
            self.recent.remove(&c);
        }
        let in_grid = *self.mode.borrow() == Mode::Grid && self.search.text().is_empty();
        if !in_grid {
            self.recent.set_visible(false);
            return;
        }
        let snap = self.snap.borrow();
        let Some(s) = snap.as_ref() else { return };
        let shown = self.order.borrow();
        let mut recent: Vec<&TaskView> = s
            .tasks
            .iter()
            .filter(|t| t.last_used.is_some() && !(shown.contains(&t.id) && t.pin.is_some()))
            .collect();
        recent.sort_by(|a, b| b.last_used.partial_cmp(&a.last_used).unwrap_or(std::cmp::Ordering::Equal));
        if recent.is_empty() {
            self.recent.set_visible(false);
            return;
        }
        self.recent.set_visible(true);
        let l = label("Recent", "dim");
        self.recent.append(&l);
        for t in recent.into_iter().take(4) {
            let b = gtk::Button::with_label(&t.label);
            b.set_focusable(false);
            let id = t.id.clone();
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(b) = w.upgrade() {
                    b.activate(&id);
                }
            });
            self.recent.append(&b);
        }
    }

    fn render_footer(&self) {
        if self.toast_until.get().is_some() {
            return;
        }
        self.footer.remove_css_class("error");
        self.footer.remove_css_class("toast");
        let text = if self.menu.borrow().is_some() {
            "↵ choose   1-9 pick   s star   esc back"
        } else {
            match &*self.mode.borrow() {
                Mode::Grid => "a-z run   ⇧ variants   / filter   tab next   esc close",
                Mode::Log(_) => "",
                Mode::Shelf => "↵ run   m man page   / filter   tab next   esc close",
            }
        };
        if text.is_empty() {
            if let Mode::Log(id) = &*self.mode.borrow() {
                if let Some(v) = self.task(id) {
                    self.footer.set_text(&format!("{}   t tail in a terminal", v.log));
                    return;
                }
            }
        }
        self.footer.set_text(text);
    }

    fn toast(&self, text: &str, error: bool) {
        self.footer.set_text(text);
        self.footer.add_css_class("toast");
        if error {
            self.footer.add_css_class("error");
        } else {
            self.footer.remove_css_class("error");
        }
        self.toast_until.set(Some(Instant::now() + Duration::from_millis(3200)));
    }

    fn set_mode(&self, m: Mode) {
        let page = match &m {
            Mode::Grid => "grid",
            Mode::Log(_) => "log",
            Mode::Shelf => "shelf",
        };
        *self.mode.borrow_mut() = m;
        self.stack.set_visible_child_name(page);
        self.stage.queue_allocate();
    }

    fn render_shelf(self: &Rc<Self>) {
        while let Some(c) = self.shelf_list.first_child() {
            self.shelf_list.remove(&c);
        }
        let mut rows = vec![];
        let q = self.search.text().to_string();
        let snap = self.snap.borrow().clone();
        let now = now_corrected();
        let section = |list: &gtk::ListBox, text: &str| {
            let l = label(text, "section");
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&l));
            row.set_selectable(false);
            row.set_activatable(false);
            list.append(&row);
        };
        let item = |list: &gtk::ListBox, name: &str, desc: &str| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let n = label(name, "name");
            n.set_width_chars(16);
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let d = label(desc, "dim");
            d.set_ellipsize(gtk::pango::EllipsizeMode::End);
            d.set_hexpand(true);
            b.append(&n);
            b.append(&d);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&b));
            list.append(&row);
        };
        let mut headers = 0;
        if let Some(s) = &snap {
            let tasks: Vec<&TaskView> = s
                .tasks
                .iter()
                .filter(|t| (t.shelf || t.stale) && fuzzy(&format!("{} {}", t.label, t.description), &q).is_some())
                .collect();
            if !tasks.is_empty() {
                section(&self.shelf_list, "TASKS");
                rows.push(ShelfRow::Bind);
                headers += 1;
                for t in tasks {
                    item(&self.shelf_list, &t.label, &crate::tile::status_line(t, now));
                    rows.push(ShelfRow::Task(t.id.clone()));
                }
            }
        }
        let shelf = self.shelf.borrow().clone();
        match &shelf {
            None => {
                section(&self.shelf_list, "loading tools…");
                rows.push(ShelfRow::Bind);
            }
            Some(d) => {
                let tools: Vec<&ShelfItem> = d.tools.iter().filter(|t| fuzzy(&format!("{} {}", t.name, t.desc), &q).is_some()).collect();
                if !tools.is_empty() {
                    section(&self.shelf_list, "TOOLS");
                    rows.push(ShelfRow::Bind);
                    headers += 1;
                    for t in tools {
                        item(&self.shelf_list, &t.name, &t.desc);
                        rows.push(ShelfRow::Tool(t.name.clone()));
                    }
                }
                let binds: Vec<&ShelfItem> = d.binds.iter().filter(|t| fuzzy(&format!("{} {}", t.name, t.desc), &q).is_some()).collect();
                if !binds.is_empty() {
                    section(&self.shelf_list, "KEY BINDS");
                    rows.push(ShelfRow::Bind);
                    headers += 1;
                    for t in binds {
                        item(&self.shelf_list, &t.name, &t.desc);
                        rows.push(ShelfRow::Bind);
                    }
                }
            }
        }
        if headers == 0 && shelf.is_some() {
            section(&self.shelf_list, "The Shelf is empty. Tools come from [[shelf.tool]] or shelf.tools = \"pacman\".");
            rows.push(ShelfRow::Bind);
        }
        *self.shelf_rows.borrow_mut() = rows;
        // Select the first real row.
        let first = self.shelf_rows.borrow().iter().position(|r| !matches!(r, ShelfRow::Bind));
        if let Some(i) = first {
            if let Some(row) = self.shelf_list.row_at_index(i as i32) {
                self.shelf_list.select_row(Some(&row));
            }
        }
    }

    fn activate_shelf_row(self: &Rc<Self>, i: i32) {
        let row = self.shelf_rows.borrow().get(i as usize).cloned();
        match row {
            Some(ShelfRow::Task(id)) => self.activate(&id),
            Some(ShelfRow::Tool(name)) => {
                self.send(Request::Open { what: OpenWhat::Tool, id: None, file: None, line: None, tool: Some(name) });
                self.hide();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- actions

    /// What a click or a hint key does to a tile.
    fn activate(self: &Rc<Self>, id: &str) {
        let Some(v) = self.task(id) else { return };
        if matches!(v.phase, Phase::Failed | Phase::Seen) {
            self.open_log(id);
            return;
        }
        match v.confirm {
            Confirm::Hold => self.toast(&format!("Hold {} for a moment to run it", v.label), false),
            Confirm::Type => self.open_type_confirm(id, BTreeMap::new(), None),
            Confirm::None => {
                self.send(Request::Run { id: id.to_string(), params: BTreeMap::new(), variant: None, clean: false });
                if let Some(t) = self.tiles.borrow().get(id) {
                    t.spring.set_motion(Motion { from_scale: 0.94, from_dy: 0.0, stiffness: 600.0, damping: 18.0 });
                    t.spring.play_in(0);
                }
            }
        }
    }

    fn activate_selected(self: &Rc<Self>) {
        let i = self.selected.get().unwrap_or(0);
        let id = self.order.borrow().get(i).cloned();
        if let Some(id) = id {
            self.activate(&id);
        }
    }

    fn start_hold(&self, id: &str, key: Option<char>, params: BTreeMap<String, String>, variant: Option<String>) {
        *self.hold.borrow_mut() = Some(Hold { id: id.to_string(), started: Instant::now(), key, params, variant });
    }

    /// Ends an unfinished hold. Returns true if there was one.
    fn cancel_hold(&self, id: &str) -> bool {
        let had = self.hold.borrow().as_ref().is_some_and(|h| h.id == id);
        if had {
            *self.hold.borrow_mut() = None;
            if let Some(t) = self.tiles.borrow().get(id) {
                t.hold.set(0.0);
                t.redraw_lamp();
            }
            self.toast("Released early. Nothing ran.", false);
        }
        had
    }

    fn open_log(self: &Rc<Self>, id: &str) {
        let Some(v) = self.task(id) else { return };
        self.close_menu();
        self.log.load(&v);
        self.set_mode(Mode::Log(id.to_string()));
        self.render_recent();
        self.render_footer();
    }

    fn log_key(self: &Rc<Self>, k: char) {
        let Mode::Log(id) = self.mode.borrow().clone() else { return };
        let Some(v) = self.task(&id) else { return };
        match k {
            'y' => {
                let text = self.log.error_excerpt();
                self.window.clipboard().set_text(&text);
                self.toast(if v.error.is_some() { "Copied the error with context" } else { "Copied the end of the log" }, false);
            }
            'e' => {
                let (file, line) = v.error.as_ref().map(|e| (e.file.clone(), e.line)).unwrap_or((None, None));
                self.send(Request::Open { what: OpenWhat::Editor, id: Some(id.clone()), file, line, tool: None });
                self.hide();
            }
            'r' => {
                self.send(Request::Ack { phase: AckPhase::All, id: Some(id.clone()) });
                self.set_mode(Mode::Grid);
                self.render(false);
                if v.confirm == Confirm::None {
                    self.send(Request::Run { id: id.clone(), params: BTreeMap::new(), variant: None, clean: false });
                } else {
                    self.activate(&id);
                }
            }
            'f' => self.log.toggle_follow(),
            'a' => {
                self.send(Request::Ack { phase: AckPhase::All, id: Some(id.clone()) });
                self.set_mode(Mode::Grid);
                self.render(false);
            }
            't' => {
                self.send(Request::Open { what: OpenWhat::Log, id: Some(id.clone()), file: None, line: None, tool: None });
                self.hide();
            }
            '\u{1b}' => {
                self.set_mode(Mode::Grid);
                self.render(false);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- menus

    fn close_menu(&self) {
        if self.menu.borrow_mut().take().is_some() {
            while let Some(c) = self.menu_holder.first_child() {
                self.menu_holder.remove(&c);
            }
            self.menu_holder.set_visible(false);
            self.render_footer();
        }
    }

    fn show_menu(self: &Rc<Self>, id: &str, title: &str, items: Vec<MenuItem>, entry: Option<(String, Option<String>)>) {
        self.close_menu();
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
        box_.add_css_class("menu");
        box_.set_size_request(340, -1);
        let t = label(title, "title");
        box_.append(&t);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for it in &items {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let l = label(&it.label, it.class.unwrap_or(""));
            l.set_hexpand(true);
            row.append(&l);
            if !it.detail.is_empty() {
                row.append(&label(&it.detail, "dim"));
            }
            if let Some(k) = it.key {
                let key = match k {
                    '\n' => "↵".to_string(),
                    c => c.to_string(),
                };
                let kl = gtk::Label::new(Some(&key));
                kl.add_css_class("key");
                row.append(&kl);
            }
            list.append(&row);
        }
        if let Some(r) = list.row_at_index(0) {
            list.select_row(Some(&r));
        }
        let w = Rc::downgrade(self);
        list.connect_row_activated(move |_, row| {
            if let Some(b) = w.upgrade() {
                b.menu_activate(row.index() as usize);
            }
        });
        box_.append(&list);
        let entry_w = entry.map(|(placeholder, param)| {
            let e = gtk::Entry::new();
            e.set_placeholder_text(Some(&placeholder));
            box_.append(&e);
            let w = Rc::downgrade(self);
            e.connect_activate(move |e| {
                if let Some(b) = w.upgrade() {
                    b.menu_entry_done(e.text().to_string());
                }
            });
            (e, param)
        });
        self.menu_holder.append(&box_);
        self.menu_holder.set_visible(true);
        let focus = entry_w.as_ref().map(|(e, _)| e.clone());
        *self.menu.borrow_mut() = Some(Menu { id: id.to_string(), items, list, entry: entry_w });
        if let Some(e) = focus {
            e.grab_focus();
        }
        self.render_footer();
    }

    fn open_menu(self: &Rc<Self>, id: &str) {
        let Some(v) = self.task(id) else { return };
        let now = now_corrected();
        let mut items = vec![];
        let guarded = |c: Confirm| match c {
            Confirm::Hold => " (hold)",
            Confirm::Type => " (type to confirm)",
            Confirm::None => "",
        };
        let run_label = match v.phase {
            Phase::Running => "Restart",
            Phase::Building | Phase::Queued | Phase::Waiting => "Run (already going)",
            _ => "Run",
        };
        items.push(MenuItem {
            label: format!("{run_label}{}", guarded(v.confirm)),
            detail: String::new(),
            key: Some('\n'),
            action: Action::Run { variant: None, params: BTreeMap::new(), clean: false, confirm: v.confirm },
            star: None,
            class: if v.confirm != Confirm::None { Some("danger") } else { None },
        });
        let mut digit = 1u32;
        let mut next_digit = || {
            let d = if digit <= 9 { char::from_digit(digit, 10) } else { None };
            digit += 1;
            d
        };
        for var in &v.variants {
            items.push(MenuItem {
                label: format!("{}{}", var.label, guarded(var.confirm)),
                detail: String::new(),
                key: next_digit(),
                action: Action::Run { variant: Some(var.name.clone()), params: BTreeMap::new(), clean: false, confirm: var.confirm },
                star: None,
                class: if var.confirm != Confirm::None { Some("danger") } else { None },
            });
        }
        for p in &v.params {
            for h in p.history.iter().take(6) {
                items.push(MenuItem {
                    label: format!("Replay {} {}{}", p.name, h.value, if h.starred { " ★" } else { "" }),
                    detail: ago(now - h.at),
                    key: next_digit(),
                    action: Action::Run {
                        variant: None,
                        params: [(p.name.clone(), h.value.clone())].into(),
                        clean: false,
                        confirm: v.confirm,
                    },
                    star: Some((p.name.clone(), h.value.clone(), h.starred)),
                    class: Some("seed"),
                });
            }
            items.push(MenuItem {
                label: format!("{}…", capitalize(&p.name)),
                detail: if p.choices.is_empty() { String::new() } else { p.choices.join(", ") },
                key: Some(':'),
                action: Action::Custom(p.name.clone()),
                star: None,
                class: None,
            });
        }
        if v.has_clean {
            items.push(MenuItem { label: "Clean build".into(), detail: String::new(), key: Some('c'), action: Action::Run { variant: None, params: BTreeMap::new(), clean: true, confirm: v.confirm }, star: None, class: None });
        }
        if v.phase.active() {
            items.push(MenuItem { label: "Stop".into(), detail: String::new(), key: Some('x'), action: Action::Stop, star: None, class: Some("danger") });
        }
        items.push(MenuItem { label: "Open log".into(), detail: String::new(), key: Some('l'), action: Action::Log, star: None, class: None });
        items.push(MenuItem { label: "Open in editor".into(), detail: String::new(), key: Some('e'), action: Action::Editor, star: None, class: None });
        items.push(MenuItem { label: "Terminal here".into(), detail: String::new(), key: Some('t'), action: Action::Terminal, star: None, class: None });
        items.push(MenuItem { label: "Copy command".into(), detail: String::new(), key: Some('y'), action: Action::CopyCommand, star: None, class: None });
        items.push(MenuItem {
            label: if v.shelf { "Back to the board".into() } else { "Move to the Shelf".into() },
            detail: String::new(),
            key: Some('m'),
            action: Action::Shelve(!v.shelf),
            star: None,
            class: None,
        });
        self.show_menu(id, &v.label, items, None);
    }

    fn open_type_confirm(self: &Rc<Self>, id: &str, params: BTreeMap<String, String>, variant: Option<String>) {
        let Some(v) = self.task(id) else { return };
        let items = vec![MenuItem {
            label: v.description.clone(),
            detail: String::new(),
            key: None,
            action: Action::Run { variant, params, clean: false, confirm: Confirm::Type },
            star: None,
            class: Some("danger"),
        }];
        self.show_menu(id, &format!("Type \"{}\" to run {}", v.id, v.label), items, Some((v.id.clone(), None)));
    }

    fn menu_entry_done(self: &Rc<Self>, text: String) {
        let (id, param, action) = {
            let m = self.menu.borrow();
            let Some(m) = m.as_ref() else { return };
            let param = m.entry.as_ref().and_then(|(_, p)| p.clone());
            (m.id.clone(), param, m.items.first().map(|i| i.action.clone()))
        };
        match param {
            Some(name) => {
                // Custom parameter value.
                if text.trim().is_empty() {
                    return;
                }
                let confirm = self.task(&id).map(|v| v.confirm).unwrap_or_default();
                let params: BTreeMap<String, String> = [(name, text.trim().to_string())].into();
                self.close_menu();
                self.run_with(&id, params, None, false, confirm);
            }
            None => {
                // Type-to-confirm.
                if text.trim() != id {
                    self.toast("That does not match. Nothing ran.", true);
                    return;
                }
                self.close_menu();
                if let Some(Action::Run { variant, params, clean, .. }) = action {
                    self.send(Request::Run { id, params, variant, clean });
                }
            }
        }
    }

    fn run_with(self: &Rc<Self>, id: &str, params: BTreeMap<String, String>, variant: Option<String>, clean: bool, confirm: Confirm) {
        match confirm {
            Confirm::None => self.send(Request::Run { id: id.to_string(), params, variant, clean }),
            Confirm::Type => self.open_type_confirm(id, params, variant),
            Confirm::Hold => {
                self.toast("Guarded: hold the tile (or its letter) to run it", false);
            }
        }
    }

    fn menu_activate(self: &Rc<Self>, idx: usize) {
        let (id, action) = {
            let m = self.menu.borrow();
            let Some(m) = m.as_ref() else { return };
            let Some(item) = m.items.get(idx) else { return };
            (m.id.clone(), item.action.clone())
        };
        if self.menu.borrow().as_ref().is_some_and(|m| m.entry.is_some()) {
            return;
        }
        match action {
            Action::Run { variant, params, clean, confirm } => {
                self.close_menu();
                self.run_with(&id, params, variant, clean, confirm);
            }
            Action::Custom(name) => {
                let title = self.task(&id).map(|v| format!("{} {name}", v.label)).unwrap_or_default();
                let items = vec![MenuItem { label: format!("Enter a value for {name}"), detail: String::new(), key: None, action: Action::Custom(name.clone()), star: None, class: Some("dim") }];
                self.show_menu(&id, &title, items, Some((name.clone(), Some(name))));
            }
            Action::Stop => {
                self.close_menu();
                self.send(Request::Stop { id });
            }
            Action::Log => self.open_log(&id),
            Action::Editor => {
                self.close_menu();
                self.send(Request::Open { what: OpenWhat::Editor, id: Some(id), file: None, line: None, tool: None });
                self.hide();
            }
            Action::Terminal => {
                self.close_menu();
                self.send(Request::Open { what: OpenWhat::Terminal, id: Some(id), file: None, line: None, tool: None });
                self.hide();
            }
            Action::CopyCommand => {
                self.close_menu();
                if let Some(v) = self.task(&id) {
                    self.window.clipboard().set_text(&v.command);
                    self.toast("Copied the command", false);
                }
            }
            Action::Shelve(on) => {
                self.close_menu();
                self.send_quiet(Request::Shelve { id: id.clone(), shelved: on });
                self.toast(if on { "Moved to the Shelf" } else { "Back on the board" }, false);
            }
        }
    }

    fn menu_star(self: &Rc<Self>) {
        let (id, star) = {
            let m = self.menu.borrow();
            let Some(m) = m.as_ref() else { return };
            let idx = m.list.selected_row().map(|r| r.index() as usize).unwrap_or(0);
            (m.id.clone(), m.items.get(idx).and_then(|i| i.star.clone()))
        };
        if let Some((param, value, starred)) = star {
            self.send_quiet(Request::Star { id: id.clone(), param, value, starred: !starred });
            self.toast(if starred { "Unstarred" } else { "Starred: it stays in the list" }, false);
            self.close_menu();
        }
    }

    // ------------------------------------------------------------- keyboard

    fn key_pressed(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use glib::Propagation::{Proceed, Stop};
        if !self.visible.get() {
            return Proceed;
        }
        let ch = key.to_unicode();
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let esc = key == gdk::Key::Escape;

        // Menus first.
        if self.menu.borrow().is_some() {
            let has_entry = self.menu.borrow().as_ref().is_some_and(|m| m.entry.is_some());
            if esc {
                self.close_menu();
                return Stop;
            }
            if has_entry {
                return Proceed;
            }
            let list = self.menu.borrow().as_ref().map(|m| m.list.clone()).unwrap();
            let idx = list.selected_row().map(|r| r.index()).unwrap_or(0);
            match key {
                gdk::Key::Down | gdk::Key::j => {
                    if let Some(r) = list.row_at_index(idx + 1) {
                        list.select_row(Some(&r));
                    }
                    return Stop;
                }
                gdk::Key::Up | gdk::Key::k => {
                    if idx > 0 {
                        if let Some(r) = list.row_at_index(idx - 1) {
                            list.select_row(Some(&r));
                        }
                    }
                    return Stop;
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    self.menu_activate(idx as usize);
                    return Stop;
                }
                _ => {}
            }
            if ch == Some('s') {
                self.menu_star();
                return Stop;
            }
            if let Some(c) = ch {
                let pos = self.menu.borrow().as_ref().and_then(|m| m.items.iter().position(|i| i.key == Some(c)));
                if let Some(p) = pos {
                    self.menu_activate(p);
                }
            }
            return Stop;
        }

        // The filter box.
        if self.search_focused() {
            match key {
                gdk::Key::Escape => {
                    if self.search.text().is_empty() {
                        GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
                    } else {
                        self.search.set_text("");
                    }
                    return Stop;
                }
                gdk::Key::Down | gdk::Key::Tab => {
                    GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
                    if *self.mode.borrow() == Mode::Shelf {
                        self.shelf_list.grab_focus();
                    } else {
                        self.move_selection(0, true);
                    }
                    return Stop;
                }
                _ => return Proceed,
            }
        }

        let mode = self.mode.borrow().clone();
        if let Mode::Log(_) = mode {
            match key {
                gdk::Key::Escape => self.log_key('\u{1b}'),
                gdk::Key::Page_Down | gdk::Key::space => self.log.scroll_by(0.9),
                gdk::Key::Page_Up => self.log.scroll_by(-0.9),
                gdk::Key::Down | gdk::Key::j => self.log.scroll_by(0.15),
                gdk::Key::Up | gdk::Key::k => self.log.scroll_by(-0.15),
                _ => {
                    if let Some(c) = ch {
                        if "yerfat".contains(c) {
                            self.log_key(c);
                        }
                    }
                }
            }
            return Stop;
        }

        if esc {
            if !self.search.text().is_empty() {
                self.search.set_text("");
            } else {
                self.hide();
            }
            return Stop;
        }
        match key {
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                let n = self.tabs().len() + 1;
                let cur = if mode == Mode::Shelf { n - 1 } else { self.tab.get() };
                let back = key == gdk::Key::ISO_Left_Tab || state.contains(gdk::ModifierType::SHIFT_MASK);
                let next = if back { (cur + n - 1) % n } else { (cur + 1) % n };
                self.select_tab(next);
                return Stop;
            }
            gdk::Key::slash => {
                self.search.grab_focus();
                return Stop;
            }
            _ => {}
        }
        if mode == Mode::Shelf {
            match ch {
                Some('m') => {
                    let idx = self.shelf_list.selected_row().map(|r| r.index()).unwrap_or(-1);
                    let row = self.shelf_rows.borrow().get(idx.max(0) as usize).cloned();
                    if let Some(ShelfRow::Tool(name)) = row {
                        self.send(Request::Open { what: OpenWhat::Man, id: None, file: None, line: None, tool: Some(name) });
                        self.hide();
                    }
                    return Stop;
                }
                Some(c) if c.is_ascii_alphanumeric() && !ctrl => {
                    self.search.grab_focus();
                    self.search.set_text(&c.to_string());
                    self.search.set_position(-1);
                    return Stop;
                }
                _ => {}
            }
            return Proceed;
        }

        // Grid.
        match key {
            gdk::Key::Right | gdk::Key::l if key == gdk::Key::Right => return self.move_selection(1, false),
            gdk::Key::Left => return self.move_selection(-1, false),
            gdk::Key::Down => return self.move_selection(self.columns() as i32, false),
            gdk::Key::Up => return self.move_selection(-(self.columns() as i32), false),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.activate_selected();
                return Stop;
            }
            gdk::Key::space | gdk::Key::Menu => {
                let i = self.selected.get().unwrap_or(0);
                let id = self.order.borrow().get(i).cloned();
                if let Some(id) = id {
                    self.open_menu(&id);
                }
                return Stop;
            }
            _ => {}
        }
        if let Some(c) = ch {
            if c.is_ascii_digit() && c != '0' {
                self.select_tab(c as usize - '1' as usize);
                return Stop;
            }
            let lower = c.to_ascii_lowercase();
            let target = self.hints.borrow().iter().find(|(h, _)| *h == lower).map(|(_, id)| id.clone());
            if let Some(id) = target {
                if c.is_ascii_uppercase() || state.contains(gdk::ModifierType::SHIFT_MASK) {
                    self.open_menu(&id);
                    return Stop;
                }
                if self.hold.borrow().as_ref().is_some_and(|h| h.id == id) || self.spent_key.get() == Some(lower) {
                    return Stop; // key repeat while holding, or after the hold fired
                }
                let v = self.task(&id);
                if let Some(v) = v {
                    if v.confirm == Confirm::Hold && !matches!(v.phase, Phase::Failed | Phase::Seen) {
                        self.start_hold(&id, Some(lower), BTreeMap::new(), None);
                    } else {
                        self.activate(&id);
                    }
                }
                return Stop;
            }
        }
        Stop
    }

    fn key_released(self: &Rc<Self>, key: gdk::Key) {
        let Some(c) = key.to_unicode().map(|c| c.to_ascii_lowercase()) else { return };
        if self.spent_key.get() == Some(c) {
            self.spent_key.set(None);
            return;
        }
        let id = self.hold.borrow().as_ref().filter(|h| h.key == Some(c)).map(|h| h.id.clone());
        if let Some(id) = id {
            self.cancel_hold(&id);
        }
    }

    /// The entry's focus sits on its inner text widget, not the entry itself.
    fn search_focused(&self) -> bool {
        match GtkWindowExt::focus(&self.window) {
            Some(w) => w == *self.search.upcast_ref::<gtk::Widget>() || w.is_ancestor(&self.search),
            None => false,
        }
    }

    fn columns(&self) -> usize {
        let w = self.flow.width().max(1) as usize;
        (w / 140).clamp(2, 4)
    }

    fn move_selection(&self, delta: i32, reset: bool) -> glib::Propagation {
        let n = self.order.borrow().len() as i32;
        if n == 0 {
            return glib::Propagation::Stop;
        }
        let cur = self.selected.get().map(|s| s as i32);
        let next = match (cur, reset) {
            (_, true) | (None, _) => 0,
            (Some(c), _) => (c + delta).clamp(0, n - 1),
        };
        self.selected.set(Some(next as usize));
        for (i, id) in self.order.borrow().iter().enumerate() {
            if let Some(t) = self.tiles.borrow().get(id) {
                t.set_selected(i as i32 == next);
            }
        }
        glib::Propagation::Stop
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn find_monitor(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let list = display.monitors();
    for i in 0..list.n_items() {
        if let Some(m) = list.item(i).and_downcast::<gdk::Monitor>() {
            if m.connector().is_some_and(|c| c == name) {
                return Some(m);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_ranks() {
        assert!(fuzzy("lanternfall", "lf").is_some());
        assert!(fuzzy("lanternfall", "xz").is_none());
        assert!(fuzzy("lanternfall", "lan").unwrap() > fuzzy("lanternfall", "lnl").unwrap());
        assert_eq!(fuzzy("anything", ""), Some(0));
    }
}
