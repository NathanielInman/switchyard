//! A task tile: lamp, name, subtitle, status line, progress, sparkline and
//! repo badges. Built once per task and updated in place from snapshots.

use crate::anim::{Motion, Spring};
use gtk::prelude::*;
use gtk::{cairo, glib};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;
use yard_core::manifest::Confirm;
use yard_core::model::{ago, clock, duration, Phase, TaskView};
use yard_core::size::human;

pub const PHASE_CLASSES: &[&str] = &["idle", "queued", "waiting", "building", "running", "ok", "failed", "seen"];

pub struct Tile {
    pub id: String,
    pub spring: Spring,
    pub root: gtk::Box,
    lamp: gtk::DrawingArea,
    name: gtk::Label,
    hint: gtk::Label,
    sub: gtk::Label,
    status: gtk::Label,
    progress: gtk::DrawingArea,
    spark: gtk::DrawingArea,
    badges: gtk::Box,
    pub view: RefCell<TaskView>,
    /// 0..1 while a hold-to-confirm press is in progress.
    pub hold: Cell<f64>,
    pub pulse: Cell<f64>,
}

fn rgba(w: &impl IsA<gtk::Widget>) -> (f64, f64, f64, f64) {
    let c = w.color();
    (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64)
}

fn set_phase_class(w: &impl IsA<gtk::Widget>, phase: &str) {
    for c in PHASE_CLASSES {
        if *c != phase {
            w.remove_css_class(c);
        }
    }
    w.add_css_class(phase);
}

impl Tile {
    pub fn new(view: &TaskView) -> Rc<Tile> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("tile");
        root.set_size_request(132, 84);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        let lamp = gtk::DrawingArea::new();
        lamp.set_content_width(16);
        lamp.set_content_height(16);
        lamp.add_css_class("lamp");
        lamp.set_valign(gtk::Align::Center);
        let name = gtk::Label::new(None);
        name.add_css_class("name");
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let hint = gtk::Label::new(None);
        hint.add_css_class("hint");
        hint.set_valign(gtk::Align::Center);
        hint.set_visible(false);
        top.append(&lamp);
        top.append(&name);
        top.append(&hint);

        let sub = gtk::Label::new(None);
        sub.add_css_class("sub");
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let status = gtk::Label::new(None);
        status.add_css_class("status");
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let progress = gtk::DrawingArea::new();
        progress.set_content_height(3);
        progress.add_css_class("progress");
        progress.set_margin_top(4);
        progress.set_visible(false);

        let spark = gtk::DrawingArea::new();
        spark.set_content_height(22);
        spark.add_css_class("spark");
        spark.set_margin_top(3);
        spark.set_visible(false);

        let badges = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        badges.set_margin_top(3);
        badges.set_visible(false);

        root.append(&top);
        root.append(&sub);
        root.append(&status);
        root.append(&progress);
        root.append(&spark);
        root.append(&badges);

        let spring = Spring::new(&root, Motion::TILE);
        let tile = Rc::new(Tile {
            id: view.id.clone(),
            spring,
            root,
            lamp,
            name,
            hint,
            sub,
            status,
            progress,
            spark,
            badges,
            view: RefCell::new(view.clone()),
            hold: Cell::new(0.0),
            pulse: Cell::new(0.0),
        });
        tile.install_drawing();
        tile
    }

    fn install_drawing(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.lamp.set_draw_func(move |area, cr, w, h| {
            let Some(t) = weak.upgrade() else { return };
            let phase = t.view.borrow().phase;
            let (r, g, b, _) = rgba(area);
            let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
            let rad = 4.6;
            let pulse = t.pulse.get();
            let lit = !matches!(phase, Phase::Idle);
            let alpha = if matches!(phase, Phase::Building | Phase::Waiting) { 0.55 + 0.45 * pulse } else { 1.0 };
            if phase == Phase::Queued {
                cr.set_source_rgba(r, g, b, 1.0);
                cr.set_line_width(1.8);
                cr.arc(cx, cy, rad - 0.9, 0.0, 2.0 * PI);
                let _ = cr.stroke();
            } else {
                if lit {
                    let glow = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, rad * 1.9);
                    glow.add_color_stop_rgba(0.0, r, g, b, 0.45 * alpha);
                    glow.add_color_stop_rgba(1.0, r, g, b, 0.0);
                    let _ = cr.set_source(&glow);
                    cr.arc(cx, cy, rad * 1.9, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                }
                cr.set_source_rgba(r, g, b, alpha);
                cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
                let _ = cr.fill();
            }
            let hold = t.hold.get();
            if hold > 0.0 {
                cr.set_source_rgba(r, g, b, 1.0);
                cr.set_line_width(2.0);
                cr.arc(cx, cy, rad + 2.6, -PI / 2.0, -PI / 2.0 + 2.0 * PI * hold);
                let _ = cr.stroke();
            }
        });

        let weak = Rc::downgrade(self);
        self.progress.set_draw_func(move |area, cr, w, h| {
            let Some(t) = weak.upgrade() else { return };
            let (r, g, b, _) = rgba(area);
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
            rounded(cr, 0.0, 0.0, w, h, h / 2.0);
            let _ = cr.fill();
            let frac = t.progress_fraction();
            match frac {
                Some(f) => {
                    cr.set_source_rgba(r, g, b, 0.95);
                    rounded(cr, 0.0, 0.0, (w * f).max(h), h, h / 2.0);
                    let _ = cr.fill();
                }
                None => {
                    // Unknown length: a sliding segment.
                    let seg = w * 0.28;
                    let x = (w + seg) * t.pulse_phase() - seg;
                    cr.set_source_rgba(r, g, b, 0.9);
                    rounded(cr, x.max(0.0), 0.0, seg.min(w - x.max(0.0)).max(0.0), h, h / 2.0);
                    let _ = cr.fill();
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.spark.set_draw_func(move |area, cr, w, h| {
            let Some(t) = weak.upgrade() else { return };
            let v = t.view.borrow();
            let data = &v.mem_history;
            if data.len() < 2 {
                return;
            }
            let (r, g, b, _) = rgba(area);
            let (w, h) = (w as f64, h as f64);
            let max = data.iter().copied().max().unwrap_or(1).max(1) as f64 * 1.15;
            let n = data.len().max(30);
            let step = w / (n - 1) as f64;
            let x0 = w - step * (data.len() - 1) as f64;
            let pt = |i: usize, val: u64| (x0 + step * i as f64, h - 1.0 - (val as f64 / max) * (h - 3.0));
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.05);
            cr.set_line_width(1.0);
            cr.move_to(0.0, h - 0.5);
            cr.line_to(w, h - 0.5);
            let _ = cr.stroke();
            let (fx, fy) = pt(0, data[0]);
            cr.move_to(fx, h);
            cr.line_to(fx, fy);
            for (i, val) in data.iter().enumerate().skip(1) {
                let (x, y) = pt(i, *val);
                cr.line_to(x, y);
            }
            let (lx, ly) = pt(data.len() - 1, data[data.len() - 1]);
            cr.line_to(lx, h);
            cr.close_path();
            cr.set_source_rgba(r, g, b, 0.16);
            let _ = cr.fill();
            let (fx, fy) = pt(0, data[0]);
            cr.move_to(fx, fy);
            for (i, val) in data.iter().enumerate().skip(1) {
                let (x, y) = pt(i, *val);
                cr.line_to(x, y);
            }
            cr.set_source_rgba(r, g, b, 0.95);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
            cr.arc(lx, ly, 2.4, 0.0, 2.0 * PI);
            let _ = cr.fill();
        });
    }

    fn pulse_phase(&self) -> f64 {
        let t = glib::monotonic_time() as f64 / 1e6;
        (t / 1.6).fract()
    }

    fn progress_fraction(&self) -> Option<f64> {
        let v = self.view.borrow();
        let now = crate::board::now_corrected();
        let typical = v.typical?;
        if typical <= 1.0 {
            return None;
        }
        Some(((now - v.since) / typical).clamp(0.02, 0.97))
    }

    pub fn widget(&self) -> &Spring {
        &self.spring
    }

    pub fn set_hint(&self, h: Option<char>) {
        match h {
            Some(c) => {
                self.hint.set_text(&c.to_string());
                self.hint.set_visible(true);
            }
            None => self.hint.set_visible(false),
        }
    }

    pub fn set_selected(&self, on: bool) {
        if on {
            self.root.add_css_class("selected");
        } else {
            self.root.remove_css_class("selected");
        }
    }

    /// Advances the pulse and redraws animated parts. Returns true while the
    /// tile has something moving.
    pub fn animate(&self) -> bool {
        let phase = self.view.borrow().phase;
        let moving = matches!(phase, Phase::Building | Phase::Waiting) || self.hold.get() > 0.0;
        if moving {
            let t = glib::monotonic_time() as f64 / 1e6;
            self.pulse.set(0.5 + 0.5 * (t * std::f64::consts::TAU / 1.4).sin());
            self.lamp.queue_draw();
        }
        if self.progress.is_visible() {
            self.progress.queue_draw();
        }
        moving
    }

    pub fn redraw_lamp(&self) {
        self.lamp.queue_draw();
    }

    pub fn update(&self, v: &TaskView, now: f64) {
        *self.view.borrow_mut() = v.clone();
        let phase = v.phase.as_str();
        set_phase_class(&self.root, phase);
        set_phase_class(&self.lamp, phase);
        set_phase_class(&self.spark, phase);
        if v.stale && v.phase == Phase::Idle {
            self.root.add_css_class("stale");
        } else {
            self.root.remove_css_class("stale");
        }
        self.name.set_text(&v.label);
        self.sub.set_text(&self.subtitle(v));
        self.status.set_text(&status_line(v, now));
        self.root.set_tooltip_text(Some(&tooltip(v)));
        let building = v.phase == Phase::Building;
        self.progress.set_visible(building);
        let show_spark = matches!(v.phase, Phase::Building | Phase::Running) && v.mem_history.len() >= 2;
        self.spark.set_visible(show_spark);
        self.spark.queue_draw();
        self.progress.queue_draw();
        self.lamp.queue_draw();
        self.update_badges(v);
    }

    fn subtitle(&self, v: &TaskView) -> String {
        if let Some(seed) = v.params.iter().find_map(|p| p.history.first().map(|h| (p.name.clone(), h.value.clone()))) {
            if matches!(v.phase, Phase::Running | Phase::Ok | Phase::Idle) && v.last.is_some() {
                return format!("{} {}", seed.0, seed.1);
            }
        }
        v.description.clone()
    }

    fn update_badges(&self, v: &TaskView) {
        while let Some(c) = self.badges.first_child() {
            self.badges.remove(&c);
        }
        let mut any = false;
        let add = |text: String, class: Option<&str>| {
            let l = gtk::Label::new(Some(&text));
            l.add_css_class("badge");
            if let Some(c) = class {
                l.add_css_class(c);
            }
            self.badges.append(&l);
        };
        if let Some(g) = &v.git {
            if g.dirty > 0 {
                add(format!("● {}", g.dirty), Some("warn"));
                any = true;
            }
            if g.ahead > 0 {
                add(format!("↑{}", g.ahead), Some("ok"));
                any = true;
            }
            if g.behind > 0 {
                add(format!("↓{}", g.behind), Some("warn"));
                any = true;
            }
            if any && !g.branch.is_empty() && g.branch != "main" && g.branch != "master" {
                add(g.branch.clone(), None);
            }
        }
        if v.confirm == Confirm::Hold {
            add("hold".into(), Some("warn"));
            any = true;
        } else if v.confirm == Confirm::Type {
            add("type to confirm".into(), Some("warn"));
            any = true;
        }
        self.badges.set_visible(any);
    }
}

pub fn status_line(v: &TaskView, now: f64) -> String {
    let elapsed = now - v.since;
    match v.phase {
        Phase::Building => {
            let step = v
                .step
                .as_ref()
                .filter(|s| s.total > 0)
                .map(|s| {
                    let attempt = if s.attempt > 1 { format!(" (try {})", s.attempt) } else { String::new() };
                    if s.total > 1 {
                        format!("{} {}/{}{attempt}", s.name, s.index, s.total)
                    } else {
                        format!("{}{attempt}", s.name)
                    }
                })
                .unwrap_or_else(|| "starting".into());
            format!("{step} · {}", clock(elapsed))
        }
        Phase::Running => match v.mem {
            Some(m) => format!("running {} · {}", clock(elapsed), human(m)),
            None => format!("running {}", clock(elapsed)),
        },
        Phase::Queued => format!("queued #{}{}", v.queue_pos.unwrap_or(0) + 1, if v.heavy { " · heavy" } else { "" }),
        Phase::Waiting => "waiting for memory".into(),
        Phase::Ok => match &v.last {
            Some(l) if l.duration >= 1.0 => format!("ok · {}", duration(l.duration)),
            _ => "ok".into(),
        },
        Phase::Failed | Phase::Seen => {
            let what = v.error.as_ref().and_then(|e| error_code(&e.text)).or_else(|| v.message.clone()).unwrap_or_else(|| "failed".into());
            format!("failed {} · {what}", ago(now - v.since).replace(" ago", ""))
        }
        Phase::Idle => match &v.last {
            Some(l) => {
                let o = match l.outcome {
                    yard_core::model::Outcome::Ok => "ok",
                    yard_core::model::Outcome::Failed => "failed",
                    yard_core::model::Outcome::Stopped => "stopped",
                };
                if v.stale {
                    format!("unused {} · Shelf?", ago(now - v.last_used.unwrap_or(l.at)))
                } else {
                    format!("idle · {o} {}", ago(now - l.at))
                }
            }
            None => "idle · never run".into(),
        },
    }
}

/// "CS0103", "TS2304", "E0425": the short code a build tool gives an error.
pub fn error_code(text: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\b((?:CS|TS|MSB|NU|E)\d{3,5})\b").unwrap());
    re.captures(text).map(|c| c[1].to_string())
}

fn tooltip(v: &TaskView) -> String {
    let mut lines = vec![v.label.clone()];
    if let Some(m) = &v.message {
        lines.push(m.clone());
    }
    if let Some(e) = &v.error {
        lines.push(e.text.clone());
    }
    if let Some(t) = v.typical {
        lines.push(format!("usually builds in {}", duration(t)));
    }
    if v.heavy {
        lines.push("heavy: builds on the main line".into());
    }
    lines.push(v.command.clone());
    lines.join("\n")
}

pub fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}
