//! The History tab and run strips for the egui board.

use crate::draw::*;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind};
use std::collections::BTreeSet;
use yard_core::history::{Episode, Lamp, RunRecord};
use yard_core::model::{duration, Outcome};
use yard_core::protocol::HistoryData;
use yard_core::size::human;

pub enum Action {
    OpenStrip(String),
    Replay(RunRecord),
    OpenLog(RunRecord),
    CopyError(RunRecord),
}

#[derive(Default)]
pub struct HistState {
    pub data: Option<HistoryData>,
    pub expanded: BTreeSet<usize>,
    pub selected: usize,
    pub strip_task: Option<String>,
    pub strip_sel: Option<usize>,
    pub rows: usize,
}

fn mark_color(m: &str) -> Color32 {
    match m {
        "stop" => STOP,
        "retry" => CAUTION,
        _ => GO,
    }
}

pub fn lamp(p: &egui::Painter, c: egui::Pos2, l: &Lamp) {
    let r = 4.6;
    match l {
        Lamp::Fix => {
            for (i, k) in [2.2f32, 1.7, 1.3].iter().enumerate() {
                p.circle_filled(c, r * k, GO.gamma_multiply(0.06 * (i as f32 + 1.0)));
            }
            p.circle_filled(c, r, GO);
            // Left half red: a filled half disc.
            let n = 16;
            let mut pts = vec![c];
            for i in 0..=n {
                let a = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * (i as f32 / n as f32);
                pts.push(c + vec2(a.cos(), a.sin()) * r);
            }
            p.add(egui::Shape::convex_polygon(pts, STOP, Stroke::NONE));
        }
        other => {
            let col = match other {
                Lamp::Go => GO,
                Lamp::Stop => STOP,
                Lamp::Caution => CAUTION,
                _ => FROST,
            };
            for (i, k) in [2.2f32, 1.7, 1.3].iter().enumerate() {
                p.circle_filled(c, r * k, col.gamma_multiply(0.06 * (i as f32 + 1.0)));
            }
            p.circle_filled(c, r, col);
        }
    }
}

impl HistState {
    pub fn set(&mut self, d: HistoryData) {
        yard_core::history::set_local_offset(d.offset);
        self.data = Some(d);
        if self.strip_task.is_some() {
            let n = self.strip_runs().len();
            self.strip_sel = self.strip_sel.filter(|s| *s < n).or(n.checked_sub(1));
        }
    }

    fn episodes(&self) -> Vec<(String, Vec<Episode>)> {
        self.data.as_ref().map(|d| d.days.iter().map(|day| (day.label.clone(), day.episodes.clone())).collect()).unwrap_or_default()
    }

    pub fn selected_task(&self) -> Option<String> {
        self.episodes().into_iter().flat_map(|(_, e)| e).nth(self.selected).map(|e| e.task)
    }

    pub fn move_sel(&mut self, d: i64) {
        if self.rows == 0 {
            return;
        }
        self.selected = (self.selected as i64 + d).clamp(0, self.rows as i64 - 1) as usize;
    }

    pub fn toggle(&mut self, i: usize) {
        if !self.expanded.remove(&i) {
            self.expanded.insert(i);
        }
    }

    pub fn strip_runs(&self) -> Vec<RunRecord> {
        let (Some(t), Some(d)) = (&self.strip_task, &self.data) else { return vec![] };
        let runs: Vec<RunRecord> = d.records.iter().filter(|r| &r.task == t).cloned().collect();
        let start = runs.len().saturating_sub(60);
        runs[start..].to_vec()
    }

    pub fn open_strip(&mut self, task: &str) {
        self.strip_task = Some(task.to_string());
        self.strip_sel = self.strip_runs().len().checked_sub(1);
    }

    pub fn step_run(&mut self, d: i64) {
        let n = self.strip_runs().len() as i64;
        if n == 0 {
            return;
        }
        let cur = self.strip_sel.map(|s| s as i64).unwrap_or(n - 1);
        self.strip_sel = Some((cur + d).clamp(0, n - 1) as usize);
    }

    pub fn selected_run(&self) -> Option<RunRecord> {
        let runs = self.strip_runs();
        self.strip_sel.and_then(|i| runs.get(i).cloned())
    }

    /// The feed: the week's numbers, then episodes by day.
    pub fn feed(&mut self, ui: &mut egui::Ui, max_h: f32) -> Option<Action> {
        let Some(d) = self.data.clone() else {
            ui.label(egui::RichText::new("Loading history…").color(DIM));
            return None;
        };
        let w = &d.week;
        let stats: Vec<(String, String, bool)> = vec![
            (duration(w.building_secs), "building this week".into(), false),
            (duration(w.waiting_secs), "waiting on the main line".into(), false),
            match &w.flakiest {
                Some((t, n)) => (t.clone(), format!("flakiest: {n} retries"), true),
                None => (format!("{} of {}", w.runs - w.failures, w.runs), "runs passed".into(), false),
            },
            match &w.slower {
                Some((t, g)) => (format!("+{:.0}%", g * 100.0), format!("{t}, slower lately"), true),
                None => ("steady".into(), "no task got slower".into(), false),
            },
        ];
        ui.horizontal(|ui| {
            let gap = 8.0;
            let bw = (ui.available_width() - gap * 3.0) / 4.0;
            ui.spacing_mut().item_spacing.x = gap;
            for (v, k, warn) in stats {
                let (r, _) = ui.allocate_exact_size(vec2(bw, 44.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, CornerRadius::same(10), TILE);
                let v = elide(p, &v, mono(14.0), bw - 20.0);
                p.text(pos2(r.left() + 10.0, r.top() + 15.0), Align2::LEFT_CENTER, v, mono(14.0), if warn { CAUTION } else { FG });
                let k = elide(p, &k, mono(10.5), bw - 20.0);
                p.text(pos2(r.left() + 10.0, r.top() + 32.0), Align2::LEFT_CENTER, k, mono(10.5), DIM);
            }
        });
        if d.records.is_empty() {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Nothing has run yet. Finished runs show up here, folded into episodes by day.").color(DIM));
            return None;
        }
        let mut act = None;
        let mut toggle = None;
        let eps = self.episodes();
        let rows: usize = eps.iter().map(|(_, e)| e.len()).sum();
        self.rows = rows;
        let est: f32 = eps.len() as f32 * 26.0 + rows as f32 * 46.0 + self.expanded.len() as f32 * 60.0;
        let h = est.min(max_h - 60.0);
        egui::ScrollArea::vertical().min_scrolled_height(h).max_height(h).auto_shrink([false, false]).show(ui, |ui| {
            let mut idx = 0usize;
            for (day, list) in eps {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(day).font(mono(10.5)).color(DIM));
                for e in list {
                    let open = self.expanded.contains(&idx);
                    let h = if open { 104.0 } else { 42.0 };
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                    let p = ui.painter();
                    if resp.hovered() || idx == self.selected || open {
                        p.rect_filled(r, CornerRadius::same(8), TILE);
                    }
                    let y = r.top() + 13.0;
                    lamp(p, pos2(r.left() + 12.0, y + 3.0), &e.lamp);
                    let who = elide(p, &e.who, mono(12.5), 110.0);
                    p.text(pos2(r.left() + 26.0, y + 3.0), Align2::LEFT_CENTER, who, mono(12.5), Color32::from_rgb(0xec, 0xee, 0xed));
                    let tx = r.left() + 146.0;
                    let when_r = p.text(pos2(r.right() - 8.0, y), Align2::RIGHT_CENTER, &e.when, mono(11.0), DIM);
                    let room = when_r.left() - tx - 10.0;
                    let what_r = p.text(pos2(tx, y), Align2::LEFT_CENTER, elide(p, &e.what, mono(12.5), room), mono(12.5), FG);
                    if let Some((t, k)) = &e.chip {
                        let col = match k.as_str() {
                            "bad" => STOP,
                            "ok" => GO,
                            _ => CAUTION,
                        };
                        let g = p.layout_no_wrap(t.clone(), mono(10.5), col);
                        let cr = Rect::from_min_size(pos2(what_r.right() + 8.0, y - 8.0), g.size() + vec2(10.0, 2.0));
                        if cr.right() < when_r.left() - 4.0 {
                            p.rect_stroke(cr, CornerRadius::same(5), Stroke::new(1.0, col.gamma_multiply(0.5)), StrokeKind::Inside);
                            p.galley(cr.min + vec2(5.0, 1.0), g, col);
                        }
                    }
                    p.text(pos2(tx, y + 17.0), Align2::LEFT_CENTER, elide(p, &e.sub, mono(11.0), r.right() - tx - 10.0), mono(11.0), DIM);
                    if open {
                        let my = r.top() + 50.0;
                        for (i, m) in e.marks.iter().enumerate() {
                            let mr = Rect::from_min_size(pos2(tx + i as f32 * 10.0, my), vec2(7.0, 14.0));
                            p.rect_filled(mr, CornerRadius::same(2), mark_color(m));
                        }
                        p.text(
                            pos2(tx, my + 26.0),
                            Align2::LEFT_CENTER,
                            elide(p, &e.detail, mono(11.0), r.right() - tx - 10.0),
                            mono(11.0),
                            DIM,
                        );
                        let label = format!("o  open {}'s runs", e.who);
                        let g = p.layout_no_wrap(label.clone(), mono(11.0), FG);
                        let br = Rect::from_min_size(pos2(tx, my + 36.0), g.size() + vec2(14.0, 6.0));
                        p.rect_stroke(br, CornerRadius::same(6), Stroke::new(1.0, BORDER), StrokeKind::Inside);
                        p.galley(br.min + vec2(7.0, 3.0), g, FG);
                        if resp.clicked() && resp.interact_pointer_pos().is_some_and(|pt| br.contains(pt)) {
                            act = Some(Action::OpenStrip(e.task.clone()));
                        } else if resp.clicked() {
                            toggle = Some(idx);
                        }
                    } else if resp.clicked() {
                        toggle = Some(idx);
                    }
                    idx += 1;
                }
            }
        });
        if let Some(i) = toggle {
            self.selected = i;
            self.toggle(i);
        }
        act
    }

    /// One task's runs as bars, and a card for the selected one.
    pub fn strip(&mut self, ui: &mut egui::Ui, now: f64) -> Option<Action> {
        let runs = self.strip_runs();
        let task = self.strip_task.clone()?;
        let label = runs.last().map(|r| r.label.clone()).unwrap_or(task.clone());
        let callout = self
            .episodes()
            .into_iter()
            .flat_map(|(_, e)| e)
            .find(|e| e.task == task && e.chip.as_ref().is_some_and(|(_, k)| k == "warn" || k == "bad"))
            .map(|e| format!("▲ {} · {}", e.chip.unwrap().0, e.when));
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
            let l = match runs.last().map(|r| r.outcome) {
                Some(Outcome::Failed) => Lamp::Stop,
                _ => Lamp::Go,
            };
            lamp(ui.painter(), r.center(), &l);
            ui.label(egui::RichText::new(&label).font(mono(14.0)).color(FG).strong());
            ui.label(egui::RichText::new(format!("{} runs", runs.len())).font(mono(11.5)).color(DIM));
            if let Some(c) = callout {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(c).font(mono(11.5)).color(CAUTION));
                });
            }
        });
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::click());
        let p = ui.painter();
        let (left, right, top, base) = (rect.left() + 34.0, rect.right() - 10.0, rect.top() + 8.0, rect.bottom() - 22.0);
        if runs.is_empty() {
            return None;
        }
        let max = runs.iter().map(|r| r.work_secs()).fold(60.0f64, f64::max) * 1.15;
        let y = |s: f64| base - ((s / max) as f32) * (base - top);
        let step_min = ((max / 60.0) / 3.0).ceil().max(1.0);
        let mut m = step_min;
        while m * 60.0 < max {
            p.line_segment([pos2(left, y(m * 60.0)), pos2(right, y(m * 60.0))], Stroke::new(1.0, Color32::from_white_alpha(14)));
            p.text(pos2(rect.left() + 2.0, y(m * 60.0)), Align2::LEFT_CENTER, format!("{m}m"), mono(10.0), DIM);
            m += step_min;
        }
        let mut ok: Vec<f64> = runs.iter().filter(|r| r.outcome == Outcome::Ok).map(|r| r.work_secs()).collect();
        ok.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if ok.len() >= 4 {
            let (lo, hi) = (ok[ok.len() / 4], ok[ok.len() * 3 / 4]);
            p.rect_filled(
                Rect::from_min_max(pos2(left, y(hi)), pos2(right, y(lo).max(y(hi) + 2.0))),
                CornerRadius::ZERO,
                FROST.gamma_multiply(0.12),
            );
        }
        let step = (right - left) / runs.len() as f32;
        let mut last_day = String::new();
        for (i, r) in runs.iter().enumerate() {
            let x = left + i as f32 * step;
            let day = yard_core::history::short_day(r.ended, now);
            if day != last_day {
                p.line_segment([pos2(x - 0.5, top), pos2(x - 0.5, base + 4.0)], Stroke::new(1.0, Color32::from_white_alpha(18)));
                p.text(pos2(x + 2.0, base + 12.0), Align2::LEFT_CENTER, &day, mono(10.0), DIM);
                last_day = day;
            }
            let bw = (step * 0.64).max(2.0);
            let br = Rect::from_min_max(pos2(x + (step - bw) / 2.0, y(r.work_secs())), pos2(x + (step + bw) / 2.0, base));
            p.rect_filled(br, CornerRadius::same(2), mark_color(r.mark()));
        }
        p.line_segment([pos2(left, base), pos2(right, base)], Stroke::new(1.0, Color32::from_white_alpha(46)));
        if let Some(i) = self.strip_sel {
            let sr = Rect::from_min_size(pos2(left + i as f32 * step, top - 2.0), vec2(step, base - top + 4.0));
            p.rect_stroke(sr, CornerRadius::same(3), Stroke::new(1.5, FROST), StrokeKind::Outside);
        }
        if resp.clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let i = (((pt.x - left) / step).floor() as i64).clamp(0, runs.len() as i64 - 1) as usize;
                self.strip_sel = Some(i);
            }
        }
        self.card(ui, &runs, now)
    }

    fn card(&self, ui: &mut egui::Ui, runs: &[RunRecord], now: f64) -> Option<Action> {
        let r = self.selected_run()?;
        let mut act = None;
        egui::Frame::new().fill(TILE).corner_radius(CornerRadius::same(11)).inner_margin(egui::Margin::same(11)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                let (lr, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
                let l = match (r.outcome, r.retried()) {
                    (Outcome::Failed, _) => Lamp::Stop,
                    (_, true) => Lamp::Caution,
                    (Outcome::Stopped, _) => Lamp::Frost,
                    _ => Lamp::Go,
                };
                lamp(ui.painter(), lr.center(), &l);
                ui.label(egui::RichText::new(format!("{} · run {}", r.label, r.run % 100_000)).font(mono(12.5)).color(FG).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}",
                        yard_core::history::short_day(r.ended, now),
                        yard_core::history::clock_time(r.started)
                    ))
                    .font(mono(11.5))
                    .color(DIM),
                );
                let (txt, col) = match r.outcome {
                    Outcome::Ok => (format!("ok in {}", duration(r.work_secs())), GO),
                    Outcome::Failed => (format!("failed after {}", duration(r.work_secs())), STOP),
                    Outcome::Stopped => (format!("ran {}, stopped", duration(r.run_secs.max(r.ended - r.started))), CAUTION),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(txt).font(mono(11.0)).color(col));
                });
            });
            if !r.steps.is_empty() {
                let (br, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                let p = ui.painter();
                let total: f64 = r.steps.iter().map(|s| s.secs.max(0.5)).sum();
                let n = r.steps.len();
                let mut x = br.left();
                let palette = [FROST, GO, Color32::from_rgb(0xb4, 0x8e, 0xad), Color32::from_rgb(0x81, 0xa1, 0xc1)];
                for (i, s) in r.steps.iter().enumerate() {
                    let w = ((s.secs.max(0.5) / total) as f32) * (br.width() - 2.0 * (n as f32 - 1.0));
                    let c = if r.outcome == Outcome::Failed && i == n - 1 {
                        STOP
                    } else if s.attempts > 1 {
                        CAUTION
                    } else {
                        palette[i % palette.len()]
                    };
                    let sr = Rect::from_min_size(pos2(x, br.top()), vec2(w, br.height()));
                    p.rect_filled(sr, CornerRadius::same(5), c);
                    let text = if s.attempts > 1 {
                        format!("{} (try {}) {}", s.name, s.attempts, duration(s.secs))
                    } else {
                        format!("{} {}", s.name, duration(s.secs))
                    };
                    let g = p.layout_no_wrap(text, mono(11.0), Color32::from_rgb(0x1b, 0x1d, 0x20));
                    if g.size().x + 12.0 < w {
                        p.galley(pos2(sr.left() + 7.0, sr.center().y - g.size().y / 2.0), g, Color32::from_rgb(0x1b, 0x1d, 0x20));
                    }
                    x += w + 2.0;
                }
            }
            if let Some(e) = &r.error {
                egui::Frame::new()
                    .fill(STOP.gamma_multiply(0.18))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(&e.text).font(mono(11.5)).color(Color32::from_rgb(0xff, 0xd5, 0xd9)));
                    });
            }
            let usual = {
                let mut v: Vec<f64> = runs.iter().filter(|x| x.outcome == Outcome::Ok && x.run != r.run).map(|x| x.work_secs()).collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                v.get(v.len() / 2).copied()
            };
            let mut facts: Vec<(String, String)> = vec![];
            match (r.outcome, usual) {
                (Outcome::Ok, Some(u)) if u > 1.0 => {
                    let g = r.work_secs() / u - 1.0;
                    facts.push((
                        if g.abs() < 0.1 {
                            "usual pace".into()
                        } else {
                            format!("{}{:.0}% vs usual", if g > 0.0 { "+" } else { "" }, g * 100.0)
                        },
                        format!("usual is {}", duration(u)),
                    ));
                }
                (Outcome::Failed, _) => {
                    if let Some(g) =
                        runs.iter().rev().filter(|x| x.ended < r.ended).find(|x| x.outcome == Outcome::Ok).and_then(|g| g.git.clone())
                    {
                        facts.push(("first red after".into(), format!("{} on {}", &g.sha[..g.sha.len().min(7)], g.branch)));
                    }
                }
                _ => {}
            }
            if let Some(pk) = r.peak_mem {
                facts.push((
                    format!("{} peak", human(pk)),
                    r.mem_cap.map(|c| format!("of the {} cap", human(c))).unwrap_or_else(|| "no cap".into()),
                ));
            }
            facts.push((format!("queued {}", yard_core::model::clock(r.queued_secs)), "on the main line".into()));
            if let Some(g) = &r.git {
                facts.push((g.sha[..g.sha.len().min(7)].to_string(), g.branch.clone()));
            }
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0 * (facts.len() as f32 - 1.0)) / facts.len() as f32;
                for (b, k) in facts {
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        ui.label(egui::RichText::new(b).font(mono(12.5)).color(FG));
                        ui.label(egui::RichText::new(k).font(mono(10.5)).color(DIM));
                    });
                }
            });
            ui.horizontal(|ui| {
                let btn = |ui: &mut egui::Ui, text: &str, primary: bool| {
                    ui.add(
                        egui::Button::new(egui::RichText::new(text).font(mono(11.5)).color(if primary {
                            Color32::from_rgb(0x1b, 0x1d, 0x20)
                        } else {
                            FG
                        }))
                        .fill(if primary { FROST } else { Color32::TRANSPARENT })
                        .stroke(Stroke::new(1.0, if primary { FROST } else { BORDER }))
                        .corner_radius(CornerRadius::same(7)),
                    )
                    .clicked()
                };
                if r.error.is_some() && btn(ui, "y  copy error", true) {
                    act = Some(Action::CopyError(r.clone()));
                }
                if btn(ui, "r  replay", r.error.is_none()) {
                    act = Some(Action::Replay(r.clone()));
                }
                if r.log.is_some() && btn(ui, "l  open this run's log", false) {
                    act = Some(Action::OpenLog(r.clone()));
                }
            });
        });
        act
    }
}

fn elide(p: &egui::Painter, text: &str, font: egui::FontId, max: f32) -> String {
    let width = |s: &str| p.layout_no_wrap(s.to_string(), font.clone(), FG).size().x;
    if width(text) <= max {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() && width(&format!("{}…", chars.iter().collect::<String>())) > max {
        chars.pop();
    }
    format!("{}…", chars.iter().collect::<String>())
}
