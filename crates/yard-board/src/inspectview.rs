//! The Inspection tab: one row per repo with a lamp and the one thing that
//! most needs you, tag chips to narrow the list, and a card per repo with
//! its details and the clean-up actions.

use gtk::prelude::*;
use gtk::{cairo, glib};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;
use std::time::Instant;
use yard_core::inspect::{span, ActionKind, InspectionData, Lamp, Repo, Severity};
use yard_core::model::ago;
use yard_core::size::human;

const GO: (f64, f64, f64) = (0.639, 0.745, 0.549);
const STOP: (f64, f64, f64) = (0.749, 0.380, 0.416);
const CAUTION: (f64, f64, f64) = (0.922, 0.796, 0.545);
const FROST: (f64, f64, f64) = (0.533, 0.753, 0.816);
const IDLE: (f64, f64, f64) = (0.333, 0.357, 0.392);
const DIM: (f64, f64, f64) = (0.486, 0.514, 0.557);

/// How long an action key or button has to be held.
const HOLD_MS: f64 = 1200.0;

pub enum Action {
    Act(String, ActionKind),
    Terminal(String),
    Editor(String),
}

struct Hold {
    dir: String,
    kind: ActionKind,
    started: Instant,
    key: Option<char>,
    bar: gtk::ProgressBar,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l.set_xalign(0.0);
    l
}

fn lamp_color(l: Lamp) -> (f64, f64, f64) {
    match l {
        Lamp::Act => STOP,
        Lamp::Tidy => CAUTION,
        Lamp::Clear => GO,
        Lamp::Quiet => IDLE,
    }
}

fn lamp_widget(lamp: Lamp) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(14);
    a.set_content_height(14);
    a.set_valign(gtk::Align::Center);
    a.set_draw_func(move |_, cr, w, h| {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, 4.6);
        let c = lamp_color(lamp);
        if lamp != Lamp::Quiet {
            let g = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, r * 1.9);
            g.add_color_stop_rgba(0.0, c.0, c.1, c.2, 0.4);
            g.add_color_stop_rgba(1.0, c.0, c.1, c.2, 0.0);
            let _ = cr.set_source(&g);
            cr.arc(cx, cy, r * 1.9, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        cr.set_source_rgb(c.0, c.1, c.2);
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let _ = cr.fill();
    });
    a
}

/// Commits per month: an area under a line, the last month marked.
fn sparkline(values: Vec<u32>, labels: Vec<String>) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(250);
    a.set_content_height(46);
    a.set_halign(gtk::Align::Start);
    a.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let (pad, base) = (4.0, h - 13.0);
        let n = values.len().max(2);
        let max = values.iter().copied().max().unwrap_or(0).max(1) as f64;
        let step = (w - pad * 2.0) / (n - 1) as f64;
        let pts: Vec<(f64, f64)> =
            values.iter().enumerate().map(|(i, v)| (pad + i as f64 * step, base - (*v as f64 / max) * (base - 4.0))).collect();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.1);
        cr.set_line_width(1.0);
        cr.move_to(pad, base + 0.5);
        cr.line_to(w - pad, base + 0.5);
        let _ = cr.stroke();
        if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
            cr.move_to(first.0, base);
            for p in &pts {
                cr.line_to(p.0, p.1);
            }
            cr.line_to(last.0, base);
            cr.close_path();
            cr.set_source_rgba(FROST.0, FROST.1, FROST.2, 0.16);
            let _ = cr.fill();
            cr.move_to(first.0, first.1);
            for p in &pts[1..] {
                cr.line_to(p.0, p.1);
            }
            cr.set_source_rgb(FROST.0, FROST.1, FROST.2);
            cr.set_line_width(1.6);
            cr.set_line_join(cairo::LineJoin::Round);
            let _ = cr.stroke();
            cr.arc(last.0, last.1, 2.6, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        cr.set_source_rgb(DIM.0, DIM.1, DIM.2);
        cr.set_font_size(8.0);
        let names = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
        for (i, l) in labels.iter().enumerate() {
            let m: usize = l.rsplit('-').next().and_then(|m| m.parse().ok()).unwrap_or(1);
            let t = names[(m.clamp(1, 12)) - 1];
            let x = pad + i as f64 * step;
            if let Ok(ext) = cr.text_extents(t) {
                cr.move_to(x - ext.width() / 2.0, h - 2.0);
                let _ = cr.show_text(t);
            }
        }
    });
    a
}

pub struct InspectView {
    pub root: gtk::Box,
    stats: gtk::Box,
    chips: gtk::FlowBox,
    list: gtk::Box,
    scroller: gtk::ScrolledWindow,
    data: RefCell<Option<InspectionData>>,
    filter: RefCell<String>,
    tag: RefCell<Option<String>>,
    pub selected: Cell<usize>,
    expanded: RefCell<Option<String>>,
    rows: RefCell<Vec<(gtk::Box, Repo)>>,
    hold: RefCell<Option<Hold>>,
    /// A key whose hold already fired; its repeats are ignored until release.
    spent: Cell<Option<char>>,
    on_action: RefCell<Option<Rc<dyn Fn(Action)>>>,
}

impl InspectView {
    pub fn new() -> Rc<InspectView> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let stats = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        stats.set_homogeneous(true);
        let chips = gtk::FlowBox::new();
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_homogeneous(false);
        chips.set_max_children_per_line(12);
        chips.set_column_spacing(5);
        chips.set_row_spacing(5);
        chips.add_css_class("insp-tags");
        chips.set_halign(gtk::Align::Start);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&list));
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(520);
        root.append(&stats);
        root.append(&chips);
        root.append(&scroller);
        Rc::new(InspectView {
            root,
            stats,
            chips,
            list,
            scroller,
            data: RefCell::new(None),
            filter: RefCell::new(String::new()),
            tag: RefCell::new(None),
            selected: Cell::new(0),
            expanded: RefCell::new(None),
            rows: RefCell::new(vec![]),
            hold: RefCell::new(None),
            spent: Cell::new(None),
            on_action: RefCell::new(None),
        })
    }

    pub fn connect_action(&self, f: impl Fn(Action) + 'static) {
        *self.on_action.borrow_mut() = Some(Rc::new(f));
    }

    fn act(&self, a: Action) {
        let cb = self.on_action.borrow().clone();
        if let Some(cb) = cb {
            cb(a);
        }
    }

    pub fn set_data(self: &Rc<Self>, d: InspectionData) {
        *self.data.borrow_mut() = Some(d);
        self.render();
    }

    pub fn set_filter(self: &Rc<Self>, f: &str) {
        if *self.filter.borrow() != f {
            *self.filter.borrow_mut() = f.to_string();
            self.selected.set(0);
            self.render();
        }
    }

    /// Clears the tag chip. Returns true if one was set.
    pub fn clear_tag(self: &Rc<Self>) -> bool {
        let had = self.tag.borrow_mut().take().is_some();
        if had {
            self.selected.set(0);
            self.render();
        }
        had
    }

    fn visible(&self) -> Vec<Repo> {
        let data = self.data.borrow();
        let Some(d) = data.as_ref() else { return vec![] };
        let filter = self.filter.borrow().clone();
        let tag = self.tag.borrow().clone();
        d.repos
            .iter()
            .filter(|r| yard_core::inspect::matches(r, &filter) && tag.as_ref().is_none_or(|t| r.tags.contains(t)))
            .cloned()
            .collect()
    }

    pub fn render(self: &Rc<Self>) {
        for b in [&self.stats, &self.list] {
            while let Some(c) = b.first_child() {
                b.remove(&c);
            }
        }
        while let Some(c) = self.chips.first_child() {
            self.chips.remove(&c);
        }
        let data = self.data.borrow().clone();
        let Some(d) = data else {
            self.list.append(&label("Reading your repos…", "dim"));
            self.rows.borrow_mut().clear();
            return;
        };

        let stat = |v: &str, k: &str, class: &str| {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("hist-stat");
            let l = label(v, "v");
            if !class.is_empty() {
                l.add_css_class(class);
            }
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let k = label(k, "k");
            k.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.append(&l);
            b.append(&k);
            b
        };
        let act = d.repos.iter().filter(|r| r.lamp == Lamp::Act).count();
        let tidy = d.repos.iter().filter(|r| r.lamp == Lamp::Tidy).count();
        let worktrees: usize = d.repos.iter().map(|r| r.status.worktrees.iter().filter(|w| w.merged && w.dirty == 0).count()).sum();
        let output: u64 = d.repos.iter().filter_map(|r| r.signals.as_ref()).flat_map(|s| s.output.iter().map(|o| o.bytes)).sum();
        self.stats.append(&stat(&act.to_string(), "to act on", if act > 0 { "bad" } else { "" }));
        self.stats.append(&stat(&tidy.to_string(), "to tidy", if tidy > 0 { "warn" } else { "" }));
        self.stats.append(&stat(&worktrees.to_string(), "merged worktrees", ""));
        self.stats.append(&stat(&human(output), "build output", ""));

        let active = self.tag.borrow().clone();
        let mut chip_tags: Vec<String> = vec!["all".into()];
        chip_tags.extend(d.tags.iter().map(|(t, _)| t.clone()));
        for t in chip_tags {
            let b = gtk::Button::with_label(&t);
            b.set_focusable(false);
            b.add_css_class("insp-tag");
            let on = if t == "all" { active.is_none() } else { active.as_deref() == Some(t.as_str()) };
            if on {
                b.add_css_class("active");
            }
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(v) = w.upgrade() {
                    *v.tag.borrow_mut() = (t != "all").then(|| t.clone());
                    v.selected.set(0);
                    v.render();
                }
            });
            self.chips.insert(&b, -1);
        }

        let repos = self.visible();
        if repos.is_empty() {
            let text = if d.repos.is_empty() {
                if d.scanning {
                    "Reading your repos…"
                } else {
                    "No git repos under inspection.roots. Point it at a folder of repos, like \"~/code/*\"."
                }
            } else {
                "No repo matches the filter."
            };
            let l = label(text, "dim");
            l.set_wrap(true);
            l.set_margin_top(14);
            self.list.append(&l);
            self.rows.borrow_mut().clear();
            return;
        }
        let mut rows = vec![];
        let mut group: Option<String> = None;
        let now = crate::board::now_corrected();
        for (i, r) in repos.into_iter().enumerate() {
            if group.as_ref() != Some(&r.group) {
                group = Some(r.group.clone());
                if !r.group.is_empty() {
                    self.list.append(&label(&r.group.to_uppercase(), "hist-day"));
                }
            }
            let row = self.repo_row(&r, i, now);
            self.list.append(&row);
            rows.push((row, r));
        }
        if self.selected.get() >= rows.len() {
            self.selected.set(rows.len() - 1);
        }
        *self.rows.borrow_mut() = rows;
        self.highlight();
    }

    fn repo_row(self: &Rc<Self>, r: &Repo, idx: usize, now: f64) -> gtk::Box {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        outer.add_css_class("hist-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&lamp_widget(r.lamp));
        let name = label(&r.name, "who");
        name.set_width_chars(18);
        name.set_max_width_chars(18);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        line.append(&name);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let head = label(&r.headline, "what");
        head.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if r.lamp == Lamp::Quiet {
            head.add_css_class("dim");
        }
        text.append(&head);
        let sub = label(&r.sub, "dim");
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&sub);
        line.append(&text);
        let when = label(&r.status.last_commit.map(|t| span(now - t)).unwrap_or_default(), "dim");
        when.set_valign(gtk::Align::Start);
        line.append(&when);
        outer.append(&line);
        if self.expanded.borrow().as_deref() == Some(r.dir.as_str()) {
            outer.append(&self.card(r, now));
        }
        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(self);
        click.connect_released(move |_, _, _, _| {
            if let Some(v) = w.upgrade() {
                v.selected.set(idx);
                v.toggle();
            }
        });
        line.add_controller(click);
        outer
    }

    fn section(title: &str) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 3);
        b.append(&label(title, "insp-sec"));
        b
    }

    fn card(self: &Rc<Self>, r: &Repo, now: f64) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.set_margin_start(24);
        card.set_margin_bottom(4);
        let s = &r.status;

        // Now.
        let sec = Self::section("NOW");
        let facts = gtk::FlowBox::new();
        facts.set_selection_mode(gtk::SelectionMode::None);
        facts.set_max_children_per_line(8);
        facts.set_column_spacing(5);
        facts.set_row_spacing(4);
        facts.set_halign(gtk::Align::Start);
        let chip = |text: &str, class: &str| {
            let c = label(text, "hist-chip");
            if !class.is_empty() {
                c.add_css_class(class);
            }
            c
        };
        facts.insert(&chip(&s.branch, ""), -1);
        for f in &r.findings {
            let class = match f.severity {
                Severity::Act => "bad",
                Severity::Tidy => "warn",
                _ => "",
            };
            facts.insert(&chip(&f.text, class), -1);
        }
        if s.dirty == 0 {
            facts.insert(&chip("clean", "ok"), -1);
        }
        if s.has_remotes && s.unpushed == 0 && s.ahead == 0 {
            facts.insert(&chip("pushed", "ok"), -1);
        }
        sec.append(&facts);
        for w in &s.worktrees {
            let name = std::path::Path::new(&w.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut t = format!("worktree {name} · {}", w.branch.as_deref().unwrap_or("detached"));
            if w.merged {
                t.push_str(" · merged");
            }
            t.push_str(&if w.dirty > 0 { format!(" · {} uncommitted", w.dirty) } else { " · clean".into() });
            if let Some(lc) = w.last_commit {
                t.push_str(&format!(" · last commit {}", ago(now - lc)));
            }
            let l = label(&t, "dim");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            sec.append(&l);
        }
        card.append(&sec);

        if let Some(sig) = &r.signals {
            if !sig.hotspots.is_empty() {
                let sec = Self::section("HOTSPOTS · CHANGES · FIXES");
                let grid = gtk::Grid::new();
                grid.set_column_spacing(14);
                grid.set_row_spacing(1);
                for (i, h) in sig.hotspots.iter().take(6).enumerate() {
                    let f = label(&h.file, "");
                    f.set_hexpand(true);
                    f.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                    let c = label(&h.changes.to_string(), "dim");
                    c.set_xalign(1.0);
                    let x = label(&h.fixes.to_string(), "insp-fix");
                    x.set_xalign(1.0);
                    grid.attach(&f, 0, i as i32, 1, 1);
                    grid.attach(&c, 1, i as i32, 1, 1);
                    grid.attach(&x, 2, i as i32, 1, 1);
                }
                sec.append(&grid);
                card.append(&sec);
            }
            if sig.months.iter().any(|m| m.commits > 0) {
                let sec = Self::section("RHYTHM · COMMITS PER MONTH");
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.append(&sparkline(
                    sig.months.iter().map(|m| m.commits).collect(),
                    sig.months.iter().map(|m| m.label.clone()).collect(),
                ));
                let note = label(&format!("{} commits in the window", sig.commits), "dim");
                note.set_valign(gtk::Align::Center);
                row.append(&note);
                sec.append(&row);
                card.append(&sec);
            }
            let humans = sig.authors.iter().filter(|a| !a.bot).count();
            let others: Vec<_> = sig.authors.iter().filter(|a| !a.me).collect();
            if humans > 1 || (r.mine && !others.is_empty()) {
                let title = if humans > 1 { "PEOPLE" } else { "CONTRIBUTORS" };
                let sec = Self::section(title);
                let total: u32 = sig.authors.iter().map(|a| a.commits).sum::<u32>().max(1);
                for a in sig.authors.iter().take(6) {
                    let tag = if a.me {
                        " (you)"
                    } else if a.bot && !a.name.ends_with("[bot]") {
                        " [bot]"
                    } else {
                        ""
                    };
                    let t = format!(
                        "{}{tag} · {} commits, {}% · {} in 6 months · last {}",
                        a.name,
                        a.commits,
                        a.commits * 100 / total,
                        a.recent,
                        ago(now - a.last)
                    );
                    let l = label(&t, if !a.me && r.mine && a.month > 0 { "insp-warn" } else { "" });
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    sec.append(&l);
                }
                if let Some(top) = sig.authors.first().filter(|a| humans > 1 && a.recent == 0 && !a.bot) {
                    sec.append(&label(&format!("{} wrote the most and has no commits in six months.", top.name), "insp-warn"));
                }
                card.append(&sec);
            }
            if !sig.firefights.is_empty() {
                let sec = Self::section(&format!("FIREFIGHTING · {} THIS YEAR", sig.firefights.len()));
                for f in sig.firefights.iter().take(3) {
                    let l = label(f, "dim");
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    sec.append(&l);
                }
                card.append(&sec);
            }
            if !sig.output.is_empty() {
                let parts: Vec<String> = sig.output.iter().map(|o| format!("{} {}", o.dir, human(o.bytes))).collect();
                let sec = Self::section("BUILD OUTPUT");
                let l = label(&parts.join(" · "), "dim");
                l.set_wrap(true);
                sec.append(&l);
                card.append(&sec);
            }
        }

        // Actions.
        // Hold actions one per line, so long labels never widen the panel.
        for a in &r.actions {
            let b = self.action_button(r, a.kind, a.key, &a.label);
            b.set_halign(gtk::Align::Start);
            card.append(&b);
        }
        let acts = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let open = |text: &str, which: u8| {
            let b = gtk::Button::with_label(text);
            b.set_focusable(false);
            b.add_css_class("hist-btn");
            let w = Rc::downgrade(self);
            let dir = r.dir.clone();
            b.connect_clicked(move |_| {
                if let Some(v) = w.upgrade() {
                    v.act(if which == 0 { Action::Terminal(dir.clone()) } else { Action::Editor(dir.clone()) });
                }
            });
            b
        };
        acts.append(&open("t  terminal", 0));
        acts.append(&open("e  editor", 1));
        card.append(&acts);
        card
    }

    /// A hold-to-confirm button: press and hold (mouse or its key) until the
    /// bar fills.
    fn action_button(self: &Rc<Self>, r: &Repo, kind: ActionKind, key: char, text: &str) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.add_css_class("insp-act");
        let l = label(&format!("{key}  {text} · hold"), "");
        b.append(&l);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("insp-hold");
        bar.set_fraction(0.0);
        b.append(&bar);
        let press = gtk::GestureClick::new();
        let w = Rc::downgrade(self);
        let dir = r.dir.clone();
        let bar2 = bar.clone();
        press.connect_pressed(move |_, _, _, _| {
            if let Some(v) = w.upgrade() {
                v.start_hold(&dir, kind, None, bar2.clone());
            }
        });
        let w = Rc::downgrade(self);
        press.connect_released(move |_, _, _, _| {
            if let Some(v) = w.upgrade() {
                v.cancel_hold();
            }
        });
        let w = Rc::downgrade(self);
        press.connect_cancel(move |_, _| {
            if let Some(v) = w.upgrade() {
                v.cancel_hold();
            }
        });
        b.add_controller(press);
        b
    }

    fn start_hold(&self, dir: &str, kind: ActionKind, key: Option<char>, bar: gtk::ProgressBar) {
        *self.hold.borrow_mut() = Some(Hold { dir: dir.to_string(), kind, started: Instant::now(), key, bar });
    }

    pub fn cancel_hold(&self) -> bool {
        match self.hold.borrow_mut().take() {
            Some(h) => {
                h.bar.set_fraction(0.0);
                true
            }
            None => false,
        }
    }

    /// Every frame: fills the held button's bar, and fires when it is full.
    pub fn tick(&self) {
        let fired = {
            let hold = self.hold.borrow();
            let Some(h) = hold.as_ref() else { return };
            let p = (h.started.elapsed().as_secs_f64() * 1000.0 / HOLD_MS).min(1.0);
            h.bar.set_fraction(p);
            p >= 1.0
        };
        if fired {
            if let Some(h) = self.hold.borrow_mut().take() {
                h.bar.set_fraction(0.0);
                self.spent.set(h.key);
                self.act(Action::Act(h.dir, h.kind));
            }
        }
    }

    /// An action key went down: starts the hold for the open card's action.
    /// Returns false when no open card has an action on that key.
    pub fn key_down(&self, c: char) -> bool {
        if self.spent.get() == Some(c) || self.hold.borrow().as_ref().is_some_and(|h| h.key == Some(c)) {
            return true;
        }
        let rows = self.rows.borrow();
        let Some((row, repo)) = rows.get(self.selected.get()) else { return false };
        if self.expanded.borrow().as_deref() != Some(repo.dir.as_str()) {
            return false;
        }
        let Some(idx) = repo.actions.iter().position(|a| a.key == c) else { return false };
        // The action's bar is the progress bar inside its button, in order.
        let mut bars = vec![];
        collect_bars(row.upcast_ref(), &mut bars);
        let Some(bar) = bars.get(idx).cloned() else { return false };
        self.start_hold(&repo.dir, repo.actions[idx].kind, Some(c), bar);
        true
    }

    /// A key came up: a hold on it ends early, a spent key is ready again.
    pub fn key_up(&self, c: char) -> bool {
        if self.spent.get() == Some(c) {
            self.spent.set(None);
            return false;
        }
        if self.hold.borrow().as_ref().is_some_and(|h| h.key == Some(c)) {
            return self.cancel_hold();
        }
        false
    }

    pub fn toggle(self: &Rc<Self>) {
        let dir = self.rows.borrow().get(self.selected.get()).map(|(_, r)| r.dir.clone());
        {
            let mut ex = self.expanded.borrow_mut();
            *ex = if *ex == dir { None } else { dir };
        }
        self.render();
        let w = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(v) = w.upgrade() {
                v.scroll_to_selected();
            }
        });
    }

    pub fn move_sel(&self, d: i32) {
        let n = self.rows.borrow().len() as i32;
        if n == 0 {
            return;
        }
        self.selected.set((self.selected.get() as i32 + d).clamp(0, n - 1) as usize);
        self.highlight();
        self.scroll_to_selected();
    }

    fn scroll_to_selected(&self) {
        let rows = self.rows.borrow();
        let Some((row, _)) = rows.get(self.selected.get()) else { return };
        let Some(b) = row.compute_bounds(&self.list) else { return };
        let adj = self.scroller.vadjustment();
        let (y, h) = (b.y() as f64, b.height() as f64);
        if y < adj.value() {
            adj.set_value(y);
        } else if y + h > adj.value() + adj.page_size() {
            adj.set_value(y + h - adj.page_size());
        }
    }

    fn highlight(&self) {
        for (i, (row, _)) in self.rows.borrow().iter().enumerate() {
            if i == self.selected.get() {
                row.add_css_class("selected");
            } else {
                row.remove_css_class("selected");
            }
        }
    }

    pub fn selected_dir(&self) -> Option<String> {
        self.rows.borrow().get(self.selected.get()).map(|(_, r)| r.dir.clone())
    }

    pub fn reset_scroll(&self) {
        self.scroller.vadjustment().set_value(0.0);
    }
}

fn collect_bars(w: &gtk::Widget, out: &mut Vec<gtk::ProgressBar>) {
    if let Some(b) = w.downcast_ref::<gtk::ProgressBar>() {
        out.push(b.clone());
    }
    let mut c = w.first_child();
    while let Some(ch) = c {
        collect_bars(&ch, out);
        c = ch.next_sibling();
    }
}
