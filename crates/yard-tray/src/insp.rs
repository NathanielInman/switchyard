//! The Inspection tab for the egui board: a row per repo, tag chips, and a
//! card per repo with its details and hold-to-confirm clean-up actions.

use crate::draw::*;
use crate::hist::elide;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, Key, Rect, Sense, Stroke, StrokeKind};
use std::time::Instant;
use yard_core::inspect::{span, ActionKind, InspectionData, Lamp, Repo, Severity};
use yard_core::model::ago;
use yard_core::size::human;

const HOLD_MS: f32 = 1200.0;

pub enum Action {
    Act(String, ActionKind),
    Terminal(String),
    Editor(String),
}

struct Hold {
    dir: String,
    kind: ActionKind,
    started: Instant,
    key: Option<Key>,
}

#[derive(Default)]
pub struct InspState {
    pub data: Option<InspectionData>,
    tag: Option<String>,
    pub selected: usize,
    expanded: Option<String>,
    /// The visible repos' dirs, in order, as last drawn.
    rows: Vec<String>,
    hold: Option<Hold>,
    spent: Option<Key>,
    scroll_to_selected: bool,
    filter: String,
}

fn lamp_color(l: Lamp) -> Color32 {
    match l {
        Lamp::Act => STOP,
        Lamp::Tidy => CAUTION,
        Lamp::Clear => GO,
        Lamp::Quiet => IDLE,
    }
}

fn lamp(p: &egui::Painter, c: egui::Pos2, l: Lamp) {
    let col = lamp_color(l);
    if l != Lamp::Quiet {
        for (i, k) in [2.2f32, 1.7, 1.3].iter().enumerate() {
            p.circle_filled(c, 4.6 * k, col.gamma_multiply(0.06 * (i as f32 + 1.0)));
        }
    }
    p.circle_filled(c, 4.6, col);
}

fn sparkline(p: &egui::Painter, r: Rect, values: &[u32], labels: &[String]) {
    let n = values.len().max(2);
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    let base = r.bottom() - 12.0;
    let step = (r.width() - 8.0) / (n - 1) as f32;
    let pts: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(r.left() + 4.0 + i as f32 * step, base - (*v as f32 / max) * (base - r.top() - 4.0)))
        .collect();
    p.line_segment([pos2(r.left() + 4.0, base), pos2(r.right() - 4.0, base)], Stroke::new(1.0, Color32::from_white_alpha(26)));
    if pts.len() >= 2 {
        let mut fill = vec![pos2(pts[0].x, base)];
        fill.extend(pts.iter().copied());
        fill.push(pos2(pts.last().unwrap().x, base));
        // The area is not convex in general; fill it a column at a time.
        for w in pts.windows(2) {
            p.add(egui::Shape::convex_polygon(
                vec![pos2(w[0].x, base), w[0], w[1], pos2(w[1].x, base)],
                FROST.gamma_multiply(0.16),
                Stroke::NONE,
            ));
        }
        p.add(egui::Shape::line(pts.clone(), Stroke::new(1.6, FROST)));
        p.circle_filled(*pts.last().unwrap(), 2.6, FROST);
    }
    let names = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
    for (i, l) in labels.iter().enumerate() {
        let m: usize = l.rsplit('-').next().and_then(|m| m.parse().ok()).unwrap_or(1);
        p.text(pos2(r.left() + 4.0 + i as f32 * step, r.bottom() - 4.0), Align2::CENTER_CENTER, names[m.clamp(1, 12) - 1], mono(8.5), DIM);
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(title).font(mono(9.5)).color(DIM));
}

fn chip(ui: &mut egui::Ui, text: &str, col: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), mono(10.5), col);
    let (r, _) = ui.allocate_exact_size(g.size() + vec2(10.0, 4.0), Sense::hover());
    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, col.gamma_multiply(0.5)), StrokeKind::Inside);
    ui.painter().galley(r.min + vec2(5.0, 2.0), g, col);
}

impl InspState {
    pub fn set(&mut self, d: InspectionData) {
        self.data = Some(d);
    }

    fn visible(&self) -> Vec<Repo> {
        let Some(d) = self.data.as_ref() else { return vec![] };
        d.repos
            .iter()
            .filter(|r| yard_core::inspect::matches(r, &self.filter) && self.tag.as_ref().is_none_or(|t| r.tags.contains(t)))
            .cloned()
            .collect()
    }

    pub fn clear_tag(&mut self) -> bool {
        let had = self.tag.take().is_some();
        if had {
            self.selected = 0;
        }
        had
    }

    pub fn move_sel(&mut self, d: i64) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected as i64 + d).clamp(0, self.rows.len() as i64 - 1) as usize;
        self.scroll_to_selected = true;
    }

    pub fn toggle(&mut self) {
        let dir = self.rows.get(self.selected).cloned();
        self.expanded = if self.expanded == dir { None } else { dir };
        self.scroll_to_selected = true;
    }

    pub fn selected_dir(&self) -> Option<String> {
        self.rows.get(self.selected).cloned()
    }

    pub fn holding(&self) -> bool {
        self.hold.is_some()
    }

    /// An action key went down on the open card. True when it started a hold.
    pub fn key_down(&mut self, key: Key, c: char) -> bool {
        if self.spent == Some(key) || self.hold.as_ref().is_some_and(|h| h.key == Some(key)) {
            return true;
        }
        let Some(dir) = self.selected_dir().filter(|d| self.expanded.as_ref() == Some(d)) else { return false };
        let kind = self
            .data
            .as_ref()
            .and_then(|d| d.repos.iter().find(|r| r.dir == dir))
            .and_then(|r| r.actions.iter().find(|a| a.key == c))
            .map(|a| a.kind);
        let Some(kind) = kind else { return false };
        self.hold = Some(Hold { dir, kind, started: Instant::now(), key: Some(key) });
        true
    }

    /// A key came up. True when it ended a hold early.
    pub fn key_up(&mut self, key: Key) -> bool {
        if self.spent == Some(key) {
            self.spent = None;
            return false;
        }
        if self.hold.as_ref().is_some_and(|h| h.key == Some(key)) {
            self.hold = None;
            return true;
        }
        false
    }

    pub fn cancel_hold(&mut self) -> bool {
        self.hold.take().is_some()
    }

    /// Fires a hold that has run its course.
    pub fn check_hold(&mut self) -> Option<Action> {
        let done = self.hold.as_ref().is_some_and(|h| h.started.elapsed().as_secs_f32() * 1000.0 >= HOLD_MS);
        if !done {
            return None;
        }
        let h = self.hold.take()?;
        self.spent = h.key;
        Some(Action::Act(h.dir, h.kind))
    }

    fn progress(&self, dir: &str, kind: ActionKind) -> f32 {
        match &self.hold {
            Some(h) if h.dir == dir && h.kind == kind => (h.started.elapsed().as_secs_f32() * 1000.0 / HOLD_MS).min(1.0),
            _ => 0.0,
        }
    }

    pub fn view(&mut self, ui: &mut egui::Ui, max_h: f32, filter: &str, now: f64) -> Option<Action> {
        if self.filter != filter {
            self.filter = filter.to_string();
            self.selected = 0;
        }
        let Some(d) = self.data.clone() else {
            ui.label(egui::RichText::new("Reading your repos…").color(DIM));
            return None;
        };
        let act_n = d.repos.iter().filter(|r| r.lamp == Lamp::Act).count();
        let tidy_n = d.repos.iter().filter(|r| r.lamp == Lamp::Tidy).count();
        let wt: usize = d.repos.iter().map(|r| r.status.worktrees.iter().filter(|w| w.merged && w.dirty == 0).count()).sum();
        let out: u64 = d.repos.iter().filter_map(|r| r.signals.as_ref()).flat_map(|s| s.output.iter().map(|o| o.bytes)).sum();
        let stats = [
            (act_n.to_string(), "to act on", if act_n > 0 { STOP } else { FG }),
            (tidy_n.to_string(), "to tidy", if tidy_n > 0 { CAUTION } else { FG }),
            (wt.to_string(), "merged worktrees", FG),
            (human(out), "build output", FG),
        ];
        ui.horizontal(|ui| {
            let gap = 8.0;
            let bw = (ui.available_width() - gap * 3.0) / 4.0;
            ui.spacing_mut().item_spacing.x = gap;
            for (v, k, col) in stats {
                let (r, _) = ui.allocate_exact_size(vec2(bw, 44.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, CornerRadius::same(10), TILE);
                p.text(pos2(r.left() + 10.0, r.top() + 15.0), Align2::LEFT_CENTER, v, mono(14.0), col);
                p.text(pos2(r.left() + 10.0, r.top() + 32.0), Align2::LEFT_CENTER, k, mono(10.5), DIM);
            }
        });
        // Tag chips.
        let mut tags: Vec<String> = vec!["all".into()];
        tags.extend(d.tags.iter().map(|(t, _)| t.clone()));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
            for t in tags {
                let on = if t == "all" { self.tag.is_none() } else { self.tag.as_deref() == Some(t.as_str()) };
                let g = ui.painter().layout_no_wrap(t.clone(), mono(11.0), if on { FG } else { DIM });
                let (r, resp) = ui.allocate_exact_size(g.size() + vec2(16.0, 4.0), Sense::click());
                let p = ui.painter();
                if on {
                    p.rect_filled(r, CornerRadius::same(7), TILE_HOVER);
                }
                p.rect_stroke(r, CornerRadius::same(7), Stroke::new(1.0, if on { ACCENT } else { BORDER }), StrokeKind::Inside);
                p.galley(r.min + vec2(8.0, 2.0), g, if on || resp.hovered() { FG } else { DIM });
                if resp.clicked() {
                    self.tag = (t != "all").then_some(t);
                    self.selected = 0;
                }
            }
        });

        let repos = self.visible();
        self.rows = repos.iter().map(|r| r.dir.clone()).collect();
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
            ui.add_space(10.0);
            ui.label(egui::RichText::new(text).color(DIM));
            return None;
        }
        if self.selected >= repos.len() {
            self.selected = repos.len() - 1;
        }
        let mut action = None;
        let mut clicked = None;
        let h = (max_h - 110.0).max(200.0);
        egui::ScrollArea::vertical().max_height(h).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut group: Option<String> = None;
            for (i, r) in repos.iter().enumerate() {
                if group.as_ref() != Some(&r.group) {
                    group = Some(r.group.clone());
                    if !r.group.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(r.group.to_uppercase()).font(mono(10.5)).color(DIM));
                    }
                }
                let open = self.expanded.as_deref() == Some(r.dir.as_str());
                let selected = i == self.selected;
                let frame = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .corner_radius(CornerRadius::same(8))
                    .fill(if selected || open { TILE } else { Color32::TRANSPARENT });
                let resp = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (line, line_resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    let p = ui.painter();
                    let y = line.top() + 9.0;
                    lamp(p, pos2(line.left() + 6.0, y + 4.0), r.lamp);
                    p.text(
                        pos2(line.left() + 20.0, y + 4.0),
                        Align2::LEFT_CENTER,
                        elide(p, &r.name, mono(12.5), 150.0),
                        mono(12.5),
                        Color32::from_rgb(0xec, 0xee, 0xed),
                    );
                    let tx = line.left() + 180.0;
                    let when = r.status.last_commit.map(|t| span(now - t)).unwrap_or_default();
                    let wr = p.text(pos2(line.right(), y), Align2::RIGHT_CENTER, when, mono(11.0), DIM);
                    let head_col = if r.lamp == Lamp::Quiet { DIM } else { FG };
                    p.text(
                        pos2(tx, y),
                        Align2::LEFT_CENTER,
                        elide(p, &r.headline, mono(12.5), wr.left() - tx - 10.0),
                        mono(12.5),
                        head_col,
                    );
                    p.text(pos2(tx, y + 17.0), Align2::LEFT_CENTER, elide(p, &r.sub, mono(11.0), line.right() - tx), mono(11.0), DIM);
                    if line_resp.clicked() {
                        clicked = Some(i);
                    }
                    if open {
                        if let Some(a) = self.card(ui, r, now) {
                            action = Some(a);
                        }
                    }
                });
                if selected && self.scroll_to_selected {
                    resp.response.scroll_to_me(None);
                    self.scroll_to_selected = false;
                }
            }
        });
        if let Some(i) = clicked {
            self.selected = i;
            self.toggle();
        }
        action
    }

    fn card(&mut self, ui: &mut egui::Ui, r: &Repo, now: f64) -> Option<Action> {
        let mut action = None;
        let s = &r.status;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                section(ui, "NOW");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                    chip(ui, &s.branch, FG);
                    for f in &r.findings {
                        let col = match f.severity {
                            Severity::Act => STOP,
                            Severity::Tidy => CAUTION,
                            _ => FG,
                        };
                        chip(ui, &f.text, col);
                    }
                    if s.dirty == 0 {
                        chip(ui, "clean", GO);
                    }
                    if s.has_remotes && s.unpushed == 0 && s.ahead == 0 {
                        chip(ui, "pushed", GO);
                    }
                });
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
                    ui.label(egui::RichText::new(t).font(mono(11.0)).color(DIM));
                }
                if let Some(sig) = &r.signals {
                    if !sig.hotspots.is_empty() {
                        section(ui, "HOTSPOTS · CHANGES · FIXES");
                        for h in sig.hotspots.iter().take(6) {
                            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 15.0), Sense::hover());
                            let p = ui.painter();
                            p.text(pos2(row.right(), row.center().y), Align2::RIGHT_CENTER, h.fixes.to_string(), mono(11.0), CAUTION);
                            p.text(pos2(row.right() - 34.0, row.center().y), Align2::RIGHT_CENTER, h.changes.to_string(), mono(11.0), DIM);
                            let room = row.width() - 80.0;
                            p.text(row.left_center(), Align2::LEFT_CENTER, elide_start(p, &h.file, mono(11.0), room), mono(11.0), FG);
                        }
                    }
                    if sig.months.iter().any(|m| m.commits > 0) {
                        section(ui, "RHYTHM · COMMITS PER MONTH");
                        ui.horizontal(|ui| {
                            let (sr, _) = ui.allocate_exact_size(vec2(250.0, 46.0), Sense::hover());
                            let vals: Vec<u32> = sig.months.iter().map(|m| m.commits).collect();
                            let labels: Vec<String> = sig.months.iter().map(|m| m.label.clone()).collect();
                            sparkline(ui.painter(), sr, &vals, &labels);
                            ui.label(egui::RichText::new(format!("{} commits in the window", sig.commits)).font(mono(11.0)).color(DIM));
                        });
                    }
                    let humans = sig.authors.iter().filter(|a| !a.bot).count();
                    let others = sig.authors.iter().any(|a| !a.me);
                    if humans > 1 || (r.mine && others) {
                        section(ui, if humans > 1 { "PEOPLE" } else { "CONTRIBUTORS" });
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
                            let col = if !a.me && r.mine && a.month > 0 { CAUTION } else { FG };
                            ui.label(egui::RichText::new(t).font(mono(11.0)).color(col));
                        }
                        if let Some(top) = sig.authors.first().filter(|a| humans > 1 && a.recent == 0 && !a.bot) {
                            ui.label(
                                egui::RichText::new(format!("{} wrote the most and has no commits in six months.", top.name))
                                    .font(mono(11.0))
                                    .color(CAUTION),
                            );
                        }
                    }
                    if !sig.firefights.is_empty() {
                        section(ui, &format!("FIREFIGHTING · {} THIS YEAR", sig.firefights.len()));
                        for f in sig.firefights.iter().take(3) {
                            let p = ui.painter();
                            let t = elide(p, f, mono(11.0), ui.available_width());
                            ui.label(egui::RichText::new(t).font(mono(11.0)).color(DIM));
                        }
                    }
                    if !sig.output.is_empty() {
                        section(ui, "BUILD OUTPUT");
                        let parts: Vec<String> = sig.output.iter().map(|o| format!("{} {}", o.dir, human(o.bytes))).collect();
                        ui.label(egui::RichText::new(parts.join(" · ")).font(mono(11.0)).color(DIM));
                    }
                }
                ui.add_space(4.0);
                for a in &r.actions {
                    let text = format!("{}  {} · hold", a.key, a.label);
                    let g = ui.painter().layout_no_wrap(text, mono(11.5), CAUTION);
                    let (br, resp) = ui.allocate_exact_size(g.size() + vec2(18.0, 10.0), Sense::click_and_drag());
                    let p = ui.painter();
                    if resp.hovered() {
                        p.rect_filled(br, CornerRadius::same(7), CAUTION.gamma_multiply(0.08));
                    }
                    p.rect_stroke(br, CornerRadius::same(7), Stroke::new(1.0, CAUTION.gamma_multiply(0.5)), StrokeKind::Inside);
                    p.galley(br.min + vec2(9.0, 4.0), g, CAUTION);
                    let frac = self.progress(&r.dir, a.kind);
                    if frac > 0.0 {
                        let bar = Rect::from_min_size(pos2(br.left() + 4.0, br.bottom() - 3.0), vec2((br.width() - 8.0) * frac, 2.0));
                        p.rect_filled(bar, CornerRadius::same(1), CAUTION);
                    }
                    // Mouse: press and hold on the button.
                    let mine = self.hold.as_ref().is_some_and(|h| h.dir == r.dir && h.kind == a.kind && h.key.is_none());
                    if resp.is_pointer_button_down_on() {
                        if !mine {
                            self.hold = Some(Hold { dir: r.dir.clone(), kind: a.kind, started: Instant::now(), key: None });
                        }
                    } else if mine {
                        self.hold = None;
                    }
                }
                ui.horizontal(|ui| {
                    for (label, which) in [("t  terminal", 0), ("e  editor", 1)] {
                        let g = ui.painter().layout_no_wrap(label.to_string(), mono(11.5), FG);
                        let (br, resp) = ui.allocate_exact_size(g.size() + vec2(18.0, 10.0), Sense::click());
                        let p = ui.painter();
                        p.rect_stroke(br, CornerRadius::same(7), Stroke::new(1.0, BORDER), StrokeKind::Inside);
                        p.galley(br.min + vec2(9.0, 4.0), g, FG);
                        if resp.clicked() {
                            action = Some(if which == 0 { Action::Terminal(r.dir.clone()) } else { Action::Editor(r.dir.clone()) });
                        }
                    }
                });
            });
        });
        action
    }
}

/// Like `elide`, but keeps the end: the file name matters more than the path.
fn elide_start(p: &egui::Painter, text: &str, font: egui::FontId, max: f32) -> String {
    if p.layout_no_wrap(text.to_string(), font.clone(), FG).size().x <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for skip in 1..chars.len() {
        let t: String = std::iter::once('…').chain(chars[skip..].iter().copied()).collect();
        if p.layout_no_wrap(t.clone(), font.clone(), FG).size().x <= max {
            return t;
        }
    }
    text.to_string()
}
