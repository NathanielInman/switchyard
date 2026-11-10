//! The History tab (a day-by-day feed of episodes) and a task's strip (one bar
//! per run) with a card for the selected run.

use gtk::prelude::*;
use gtk::{cairo, glib};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::rc::Rc;
use yard_core::history::{Episode, Lamp, RunRecord};
use yard_core::model::{duration, Outcome};
use yard_core::protocol::HistoryData;
use yard_core::size::human;

const GO: (f64, f64, f64) = (0.639, 0.745, 0.549);
const STOP: (f64, f64, f64) = (0.749, 0.380, 0.416);
const CAUTION: (f64, f64, f64) = (0.922, 0.796, 0.545);
const FROST: (f64, f64, f64) = (0.533, 0.753, 0.816);
const VIOLET: (f64, f64, f64) = (0.706, 0.557, 0.678);
const DIM: (f64, f64, f64) = (0.486, 0.514, 0.557);

pub enum Action {
    OpenStrip(String),
    Replay(RunRecord),
    OpenLog(RunRecord),
    CopyError(RunRecord),
}

fn mark_color(m: &str) -> (f64, f64, f64) {
    match m {
        "stop" => STOP,
        "retry" => CAUTION,
        _ => GO,
    }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l.set_xalign(0.0);
    l
}

fn lamp_widget(lamp: Lamp) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(14);
    a.set_content_height(14);
    a.set_valign(gtk::Align::Center);
    a.set_draw_func(move |_, cr, w, h| {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, 4.6);
        let glow = |cr: &cairo::Context, c: (f64, f64, f64)| {
            let g = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, r * 1.9);
            g.add_color_stop_rgba(0.0, c.0, c.1, c.2, 0.4);
            g.add_color_stop_rgba(1.0, c.0, c.1, c.2, 0.0);
            let _ = cr.set_source(&g);
            cr.arc(cx, cy, r * 1.9, 0.0, 2.0 * PI);
            let _ = cr.fill();
        };
        match lamp {
            Lamp::Fix => {
                glow(cr, GO);
                cr.set_source_rgb(STOP.0, STOP.1, STOP.2);
                cr.arc(cx, cy, r, PI / 2.0, 1.5 * PI);
                let _ = cr.fill();
                cr.set_source_rgb(GO.0, GO.1, GO.2);
                cr.arc(cx, cy, r, -PI / 2.0, PI / 2.0);
                let _ = cr.fill();
            }
            _ => {
                let c = match lamp {
                    Lamp::Go => GO,
                    Lamp::Stop => STOP,
                    Lamp::Caution => CAUTION,
                    _ => FROST,
                };
                glow(cr, c);
                cr.set_source_rgb(c.0, c.1, c.2);
                cr.arc(cx, cy, r, 0.0, 2.0 * PI);
                let _ = cr.fill();
            }
        }
    });
    a
}

fn marks_widget(marks: &[String]) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    let marks = marks.to_vec();
    a.set_content_width((marks.len() as i32 * 10).max(10));
    a.set_content_height(14);
    a.set_halign(gtk::Align::Start);
    a.set_draw_func(move |_, cr, _, h| {
        for (i, m) in marks.iter().enumerate() {
            let c = mark_color(m);
            cr.set_source_rgb(c.0, c.1, c.2);
            crate::tile::rounded(cr, i as f64 * 10.0, 0.0, 7.0, h as f64, 2.0);
            let _ = cr.fill();
        }
    });
    a
}

pub struct HistoryView {
    pub feed_root: gtk::Box,
    stats: gtk::Box,
    feed: gtk::Box,
    feed_scroller: gtk::ScrolledWindow,
    pub strip_root: gtk::Box,
    strip_title: gtk::Label,
    strip_sub: gtk::Label,
    strip_callout: gtk::Label,
    strip_lamp: gtk::Box,
    strip_area: gtk::DrawingArea,
    card: gtk::Box,
    pub data: RefCell<Option<HistoryData>>,
    expanded: RefCell<BTreeSet<usize>>,
    pub selected: Cell<usize>,
    rows: RefCell<Vec<(gtk::Box, Episode)>>,
    pub strip_task: RefCell<Option<String>>,
    pub strip_sel: Cell<Option<usize>>,
    on_action: RefCell<Option<Rc<dyn Fn(Action)>>>,
}

impl HistoryView {
    pub fn new() -> Rc<HistoryView> {
        let feed_root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let stats = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        stats.set_homogeneous(true);
        let feed = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let feed_scroller = gtk::ScrolledWindow::new();
        feed_scroller.set_child(Some(&feed));
        feed_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        feed_scroller.set_propagate_natural_height(true);
        feed_scroller.set_max_content_height(520);
        feed_root.append(&stats);
        feed_root.append(&feed_scroller);

        let strip_root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let strip_lamp = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let strip_title = label("", "hist-name");
        let strip_sub = label("", "dim");
        let strip_callout = label("", "hist-callout");
        strip_callout.set_hexpand(true);
        strip_callout.set_xalign(1.0);
        strip_callout.set_ellipsize(gtk::pango::EllipsizeMode::End);
        head.append(&strip_lamp);
        head.append(&strip_title);
        head.append(&strip_sub);
        head.append(&strip_callout);
        let strip_area = gtk::DrawingArea::new();
        strip_area.set_content_height(150);
        strip_area.set_hexpand(true);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("hist-card");
        strip_root.append(&head);
        strip_root.append(&strip_area);
        strip_root.append(&card);

        let v = Rc::new(HistoryView {
            feed_root,
            stats,
            feed,
            feed_scroller,
            strip_root,
            strip_title,
            strip_sub,
            strip_callout,
            strip_lamp,
            strip_area,
            card,
            data: RefCell::new(None),
            expanded: RefCell::new(BTreeSet::new()),
            selected: Cell::new(0),
            rows: RefCell::new(vec![]),
            strip_task: RefCell::new(None),
            strip_sel: Cell::new(None),
            on_action: RefCell::new(None),
        });
        v.wire();
        v
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

    fn wire(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.strip_area.set_draw_func(move |area, cr, wd, ht| {
            if let Some(v) = w.upgrade() {
                v.draw_strip(area, cr, wd as f64, ht as f64);
            }
        });
        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(self);
        click.connect_pressed(move |_, _, x, _| {
            let Some(v) = w.upgrade() else { return };
            let n = v.strip_runs().len();
            if n == 0 {
                return;
            }
            let wd = v.strip_area.width() as f64;
            let (left, right) = (34.0, 10.0);
            let i = (((x - left) / ((wd - left - right) / n as f64)).floor() as isize).clamp(0, n as isize - 1) as usize;
            v.select_run(Some(i));
        });
        self.strip_area.add_controller(click);
    }

    pub fn set_data(self: &Rc<Self>, d: HistoryData) {
        yard_core::history::set_local_offset(d.offset);
        *self.data.borrow_mut() = Some(d);
        self.render_feed();
        if self.strip_task.borrow().is_some() {
            let n = self.strip_runs().len();
            let sel = self.strip_sel.get().filter(|s| *s < n).or(n.checked_sub(1));
            self.select_run(sel);
        }
    }

    fn episodes(&self) -> Vec<(String, Vec<Episode>)> {
        self.data
            .borrow()
            .as_ref()
            .map(|d| d.days.iter().map(|day| (day.label.clone(), day.episodes.clone())).collect())
            .unwrap_or_default()
    }

    pub fn render_feed(self: &Rc<Self>) {
        while let Some(c) = self.stats.first_child() {
            self.stats.remove(&c);
        }
        while let Some(c) = self.feed.first_child() {
            self.feed.remove(&c);
        }
        let data = self.data.borrow().clone();
        let Some(d) = data else {
            self.feed.append(&label("Loading history…", "dim"));
            return;
        };
        let w = &d.week;
        let stat = |v: &str, k: &str, warn: bool| {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("hist-stat");
            let l = label(v, "v");
            if warn {
                l.add_css_class("warn");
            }
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let k = label(k, "k");
            k.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.append(&l);
            b.append(&k);
            b
        };
        self.stats.append(&stat(&duration(w.building_secs), "building this week", false));
        self.stats.append(&stat(&duration(w.waiting_secs), "waiting on the main line", false));
        match &w.flakiest {
            Some((t, n)) => self.stats.append(&stat(t, &format!("flakiest: {n} retries"), true)),
            None => self.stats.append(&stat(&format!("{} of {}", w.runs - w.failures, w.runs), "runs passed", false)),
        }
        match &w.slower {
            Some((t, g)) => self.stats.append(&stat(&format!("+{:.0}%", g * 100.0), &format!("{t}, slower lately"), true)),
            None => self.stats.append(&stat("steady", "no task got slower", false)),
        }
        if d.records.is_empty() {
            let l = label("Nothing has run yet. Finished runs show up here, folded into episodes by day.", "dim");
            l.set_wrap(true);
            l.set_margin_top(20);
            self.feed.append(&l);
            self.rows.borrow_mut().clear();
            return;
        }
        let mut rows = vec![];
        let mut idx = 0usize;
        for (day, eps) in self.episodes() {
            let l = label(&day, "hist-day");
            self.feed.append(&l);
            for e in eps {
                let row = self.episode_row(&e, idx);
                self.feed.append(&row);
                rows.push((row, e));
                idx += 1;
            }
        }
        *self.rows.borrow_mut() = rows;
        self.highlight();
    }

    fn episode_row(self: &Rc<Self>, e: &Episode, idx: usize) -> gtk::Box {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
        outer.add_css_class("hist-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&lamp_widget(e.lamp.clone()));
        let who = label(&e.who, "who");
        who.set_width_chars(13);
        who.set_max_width_chars(13);
        who.set_ellipsize(gtk::pango::EllipsizeMode::End);
        line.append(&who);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let what = label(&e.what, "what");
        what.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&what);
        if let Some((t, k)) = &e.chip {
            let c = label(t, "hist-chip");
            c.add_css_class(k);
            top.append(&c);
        }
        text.append(&top);
        let sub = label(&e.sub, "dim");
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&sub);
        line.append(&text);
        let when = label(&e.when, "dim");
        when.set_valign(gtk::Align::Start);
        line.append(&when);
        outer.append(&line);
        if self.expanded.borrow().contains(&idx) {
            let more = gtk::Box::new(gtk::Orientation::Vertical, 4);
            more.set_margin_start(24);
            more.append(&marks_widget(&e.marks));
            let det = label(&e.detail, "dim");
            det.set_wrap(true);
            more.append(&det);
            let btn = gtk::Button::with_label(&format!("o  open {}'s runs", e.who));
            btn.set_focusable(false);
            btn.set_halign(gtk::Align::Start);
            btn.add_css_class("hist-btn");
            let w = Rc::downgrade(self);
            let task = e.task.clone();
            btn.connect_clicked(move |_| {
                if let Some(v) = w.upgrade() {
                    v.act(Action::OpenStrip(task.clone()));
                }
            });
            more.append(&btn);
            outer.append(&more);
        }
        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(self);
        click.connect_released(move |_, _, _, _| {
            if let Some(v) = w.upgrade() {
                v.selected.set(idx);
                v.toggle(idx);
            }
        });
        line.add_controller(click);
        outer
    }

    pub fn toggle(self: &Rc<Self>, idx: usize) {
        {
            let mut ex = self.expanded.borrow_mut();
            if !ex.remove(&idx) {
                ex.insert(idx);
            }
        }
        self.render_feed();
    }

    pub fn move_sel(&self, d: i32) {
        let n = self.rows.borrow().len() as i32;
        if n == 0 {
            return;
        }
        self.selected.set((self.selected.get() as i32 + d).clamp(0, n - 1) as usize);
        self.highlight();
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

    pub fn selected_task(&self) -> Option<String> {
        self.rows.borrow().get(self.selected.get()).map(|(_, e)| e.task.clone())
    }

    pub fn reset_scroll(&self) {
        self.feed_scroller.vadjustment().set_value(0.0);
    }

    // ------------------------------------------------------------- strip

    fn strip_runs(&self) -> Vec<RunRecord> {
        let task = self.strip_task.borrow().clone();
        let data = self.data.borrow();
        let (Some(task), Some(d)) = (task, data.as_ref()) else { return vec![] };
        let runs: Vec<RunRecord> = d.records.iter().filter(|r| r.task == task).cloned().collect();
        let start = runs.len().saturating_sub(60);
        runs[start..].to_vec()
    }

    pub fn open_strip(self: &Rc<Self>, task: &str) {
        *self.strip_task.borrow_mut() = Some(task.to_string());
        let runs = self.strip_runs();
        let label_text = runs.last().map(|r| r.label.clone()).unwrap_or_else(|| task.to_string());
        self.strip_title.set_text(&label_text);
        let days = runs.first().map(|r| ((runs.last().unwrap().ended - r.started) / 86400.0).ceil().max(1.0) as u32).unwrap_or(0);
        self.strip_sub.set_text(&format!("{} runs over {} day{}", runs.len(), days, if days == 1 { "" } else { "s" }));
        while let Some(c) = self.strip_lamp.first_child() {
            self.strip_lamp.remove(&c);
        }
        let lamp = match runs.last().map(|r| r.outcome) {
            Some(Outcome::Failed) => Lamp::Stop,
            Some(_) => Lamp::Go,
            None => Lamp::Frost,
        };
        self.strip_lamp.append(&lamp_widget(lamp));
        let callout = self
            .episodes()
            .into_iter()
            .flat_map(|(_, e)| e)
            .find(|e| e.task == task && e.chip.as_ref().is_some_and(|(_, k)| k == "warn" || k == "bad"))
            .map(|e| format!("▲ {} · {}", e.chip.unwrap().0, e.when))
            .unwrap_or_default();
        self.strip_callout.set_text(&callout);
        self.select_run(runs.len().checked_sub(1));
    }

    pub fn step_run(self: &Rc<Self>, d: i32) {
        let n = self.strip_runs().len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.strip_sel.get().map(|s| s as i32).unwrap_or(n - 1);
        self.select_run(Some((cur + d).clamp(0, n - 1) as usize));
    }

    pub fn selected_run(&self) -> Option<RunRecord> {
        let runs = self.strip_runs();
        self.strip_sel.get().and_then(|i| runs.get(i).cloned())
    }

    fn select_run(self: &Rc<Self>, i: Option<usize>) {
        self.strip_sel.set(i);
        self.strip_area.queue_draw();
        self.render_card();
    }

    fn draw_strip(&self, _area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64) {
        let runs = self.strip_runs();
        let (left, right, top, base) = (34.0, 10.0, 8.0, h - 22.0);
        if runs.is_empty() {
            return;
        }
        let max = runs.iter().map(|r| r.work_secs()).fold(60.0f64, f64::max) * 1.15;
        let y = |s: f64| base - (s / max) * (base - top);
        // Grid lines on whole minutes.
        let step_min = ((max / 60.0) / 3.0).ceil().max(1.0);
        cr.set_line_width(1.0);
        let mut m = step_min;
        while m * 60.0 < max {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
            cr.move_to(left, y(m * 60.0).round() + 0.5);
            cr.line_to(w - right, y(m * 60.0).round() + 0.5);
            let _ = cr.stroke();
            cr.set_source_rgb(DIM.0, DIM.1, DIM.2);
            cr.set_font_size(10.0);
            cr.move_to(2.0, y(m * 60.0) + 3.0);
            let _ = cr.show_text(&format!("{m}m"));
            m += step_min;
        }
        // The usual time: the middle half of passing runs.
        let mut ok: Vec<f64> = runs.iter().filter(|r| r.outcome == Outcome::Ok).map(|r| r.work_secs()).collect();
        ok.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if ok.len() >= 4 {
            let (lo, hi) = (ok[ok.len() / 4], ok[ok.len() * 3 / 4]);
            cr.set_source_rgba(FROST.0, FROST.1, FROST.2, 0.12);
            cr.rectangle(left, y(hi), w - left - right, (y(lo) - y(hi)).max(2.0));
            let _ = cr.fill();
        }
        let step = (w - left - right) / runs.len() as f64;
        let mut last_day = String::new();
        for (i, r) in runs.iter().enumerate() {
            let x = left + i as f64 * step;
            let day = day_name(r.ended);
            if day != last_day {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
                cr.move_to(x.round() - 0.5, top);
                cr.line_to(x.round() - 0.5, base + 4.0);
                let _ = cr.stroke();
                cr.set_source_rgb(DIM.0, DIM.1, DIM.2);
                cr.move_to(x + 2.0, base + 16.0);
                let _ = cr.show_text(&day);
                last_day = day;
            }
            let c = mark_color(r.mark());
            cr.set_source_rgb(c.0, c.1, c.2);
            let bw = (step * 0.64).max(2.0);
            crate::tile::rounded(cr, x + (step - bw) / 2.0, y(r.work_secs()), bw, base - y(r.work_secs()), 2.0);
            let _ = cr.fill();
        }
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
        cr.move_to(left, base + 0.5);
        cr.line_to(w - right, base + 0.5);
        let _ = cr.stroke();
        if let Some(i) = self.strip_sel.get() {
            cr.set_source_rgb(FROST.0, FROST.1, FROST.2);
            cr.set_line_width(1.5);
            crate::tile::rounded(cr, left + i as f64 * step, top - 2.0, step, base - top + 4.0, 3.0);
            let _ = cr.stroke();
        }
    }

    fn render_card(self: &Rc<Self>) {
        while let Some(c) = self.card.first_child() {
            self.card.remove(&c);
        }
        let Some(r) = self.selected_run() else {
            self.card.append(&label("No runs yet.", "dim"));
            return;
        };
        let runs = self.strip_runs();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let lamp = match (r.outcome, r.retried()) {
            (Outcome::Failed, _) => Lamp::Stop,
            (_, true) => Lamp::Caution,
            (Outcome::Stopped, _) => Lamp::Frost,
            _ => Lamp::Go,
        };
        top.append(&lamp_widget(lamp));
        top.append(&label(&format!("{} · run {}", r.label, r.run % 100_000), "who"));
        let when = label(&format!("{} {}", day_name(r.ended), yard_core::history::clock_time(r.started)), "dim");
        when.set_hexpand(true);
        top.append(&when);
        let (txt, kind) = match r.outcome {
            Outcome::Ok => (format!("ok in {}", duration(r.work_secs())), "ok"),
            Outcome::Failed => (format!("failed after {}", duration(r.work_secs())), "bad"),
            Outcome::Stopped => (format!("ran {}, stopped", duration(r.run_secs.max(r.ended - r.started))), "warn"),
        };
        let chip = label(&txt, "hist-chip");
        chip.add_css_class(kind);
        top.append(&chip);
        self.card.append(&top);

        // Where the time went.
        let steps = r.steps.clone();
        if !steps.is_empty() {
            let bar = gtk::DrawingArea::new();
            bar.set_content_height(22);
            bar.set_hexpand(true);
            let failed = r.outcome == Outcome::Failed;
            bar.set_draw_func(move |_, cr, w, h| {
                let total: f64 = steps.iter().map(|s| s.secs.max(0.5)).sum();
                let mut x = 0.0;
                let palette = [FROST, GO, VIOLET, (0.5, 0.63, 0.76)];
                let n = steps.len();
                for (i, s) in steps.iter().enumerate() {
                    let sw = (s.secs.max(0.5) / total) * (w as f64 - 2.0 * (n as f64 - 1.0));
                    let c = if failed && i == n - 1 {
                        STOP
                    } else if s.attempts > 1 {
                        CAUTION
                    } else {
                        palette[i % palette.len()]
                    };
                    cr.set_source_rgb(c.0, c.1, c.2);
                    crate::tile::rounded(cr, x, 0.0, sw, h as f64, 5.0);
                    let _ = cr.fill();
                    let text = if s.attempts > 1 { format!("{} (try {}) {}", s.name, s.attempts, duration(s.secs)) } else { format!("{} {}", s.name, duration(s.secs)) };
                    cr.set_source_rgb(0.106, 0.114, 0.125);
                    cr.set_font_size(11.0);
                    if let Ok(ext) = cr.text_extents(&text) {
                        if ext.width() + 12.0 < sw {
                            cr.move_to(x + 7.0, h as f64 / 2.0 + 4.0);
                            let _ = cr.show_text(&text);
                        }
                    }
                    x += sw + 2.0;
                }
            });
            self.card.append(&bar);
        }
        if let Some(e) = &r.error {
            let l = label(&e.text, "hist-err");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            self.card.append(&l);
        }

        // Three facts.
        let usual = {
            let mut v: Vec<f64> = runs.iter().filter(|x| x.outcome == Outcome::Ok && x.run != r.run).map(|x| x.work_secs()).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v.get(v.len() / 2).copied()
        };
        let pace = match (r.outcome, usual) {
            (Outcome::Ok, Some(u)) if u > 1.0 => {
                let g = r.work_secs() / u - 1.0;
                if g.abs() < 0.1 {
                    ("usual pace".to_string(), format!("usual is {}", duration(u)))
                } else {
                    (format!("{}{:.0}% vs usual", if g > 0.0 { "+" } else { "" }, g * 100.0), format!("usual is {}", duration(u)))
                }
            }
            (Outcome::Failed, _) => {
                let last_green = runs.iter().rev().filter(|x| x.ended < r.ended).find(|x| x.outcome == Outcome::Ok);
                match last_green.and_then(|g| g.git.as_ref()) {
                    Some(g) => ("first red after".to_string(), format!("{} on {}", &g.sha[..g.sha.len().min(7)], g.branch)),
                    None => ("failed".to_string(), r.message.clone().unwrap_or_default()),
                }
            }
            _ => ("".to_string(), String::new()),
        };
        let mem = match (r.peak_mem, r.mem_cap) {
            (Some(p), Some(c)) => (format!("{} peak", human(p)), format!("of the {} cap", human(c))),
            (Some(p), None) => (format!("{} peak", human(p)), "no cap".to_string()),
            _ => ("".to_string(), String::new()),
        };
        let queue = (format!("queued {}", yard_core::model::clock(r.queued_secs)), "on the main line".to_string());
        let git = r
            .git
            .as_ref()
            .map(|g| (g.sha[..g.sha.len().min(7)].to_string(), if g.dirty > 0 { format!("{} · {} dirty", g.branch, g.dirty) } else { g.branch.clone() }))
            .unwrap_or_default();
        let facts = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        facts.set_homogeneous(true);
        for (b, k) in [pace, mem, queue, git] {
            if b.is_empty() {
                continue;
            }
            let f = gtk::Box::new(gtk::Orientation::Vertical, 0);
            f.add_css_class("hist-fact");
            let l = label(&b, "b");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let s = label(&k, "dim");
            s.set_ellipsize(gtk::pango::EllipsizeMode::End);
            f.append(&l);
            f.append(&s);
            facts.append(&f);
        }
        self.card.append(&facts);

        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btns.add_css_class("actions");
        let mk = |text: &str, primary: bool, a: fn(RunRecord) -> Action| {
            let b = gtk::Button::with_label(text);
            b.set_focusable(false);
            if primary {
                b.add_css_class("frost");
            }
            let w = Rc::downgrade(self);
            let r = r.clone();
            b.connect_clicked(move |_| {
                if let Some(v) = w.upgrade() {
                    v.act(a(r.clone()));
                }
            });
            b
        };
        if r.error.is_some() {
            btns.append(&mk("y  copy error", true, Action::CopyError));
        } else {
            btns.append(&mk("r  replay", true, Action::Replay));
        }
        if r.log.is_some() {
            btns.append(&mk("l  open this run's log", false, Action::OpenLog));
        }
        if r.error.is_some() {
            btns.append(&mk("r  replay", false, Action::Replay));
        }
        self.card.append(&btns);
    }
}

fn day_name(t: f64) -> String {
    yard_core::history::short_day(t, crate::board::now_corrected())
}

#[allow(dead_code)]
fn _unused(_: glib::ControlFlow) {}
