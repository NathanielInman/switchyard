//! Painting: palette, lamps, tiles, sparklines and gauges.

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind};
use yard_core::manifest::Confirm;
use yard_core::model::{ago, clock, duration, Outcome, Phase, TaskView};
use yard_core::size::human;

pub const PANEL: Color32 = Color32::from_rgba_premultiplied(30, 32, 36, 245);
pub const BORDER: Color32 = Color32::from_rgb(0x41, 0x46, 0x4e);
pub const FG: Color32 = Color32::from_rgb(0xc5, 0xc8, 0xc6);
pub const DIM: Color32 = Color32::from_rgb(0x7c, 0x83, 0x8e);
pub const TILE: Color32 = Color32::from_rgb(0x2a, 0x2d, 0x32);
pub const TILE_HOVER: Color32 = Color32::from_rgb(0x33, 0x37, 0x3d);
pub const WELL: Color32 = Color32::from_rgb(0x0f, 0x10, 0x12);
pub const ACCENT: Color32 = Color32::from_rgb(0x88, 0xc0, 0xd0);
pub const GO: Color32 = Color32::from_rgb(0xa3, 0xbe, 0x8c);
pub const CAUTION: Color32 = Color32::from_rgb(0xeb, 0xcb, 0x8b);
pub const STOP: Color32 = Color32::from_rgb(0xbf, 0x61, 0x6a);
pub const FROST: Color32 = Color32::from_rgb(0x88, 0xc0, 0xd0);
pub const IDLE: Color32 = Color32::from_rgb(0x55, 0x5b, 0x64);

pub fn phase_color(p: Phase) -> Color32 {
    match p {
        Phase::Failed | Phase::Seen => STOP,
        Phase::Building | Phase::Queued | Phase::Waiting => CAUTION,
        Phase::Running => FROST,
        Phase::Ok => GO,
        Phase::Idle => IDLE,
    }
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// A lamp with a soft glow; `pulse` 0..1 dims it for building tasks; `hold`
/// draws the hold-to-confirm ring.
pub fn lamp(p: &egui::Painter, c: Pos2, phase: Phase, pulse: f32, hold: f32, opacity: f32) {
    let color = phase_color(phase);
    let r = 4.6;
    let alpha = if matches!(phase, Phase::Building | Phase::Waiting) { 0.55 + 0.45 * pulse } else { 1.0 } * opacity;
    if phase == Phase::Queued {
        p.circle_stroke(c, r - 0.9, Stroke::new(1.8, color.gamma_multiply(opacity)));
    } else {
        if phase != Phase::Idle {
            for (i, k) in [2.2f32, 1.7, 1.3].iter().enumerate() {
                p.circle_filled(c, r * k, color.gamma_multiply(0.06 * (i as f32 + 1.0) * alpha));
            }
        }
        p.circle_filled(c, r, color.gamma_multiply(alpha));
    }
    if hold > 0.0 {
        let n = 40;
        let pts: Vec<Pos2> = (0..=((n as f32 * hold) as usize))
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * (i as f32 / n as f32);
                c + vec2(a.cos(), a.sin()) * (r + 3.0)
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(2.0, color)));
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
                .map(|s| if s.total > 1 { format!("{} {}/{}", s.name, s.index, s.total) } else { s.name.clone() })
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
                    Outcome::Ok => "ok",
                    Outcome::Failed => "failed",
                    Outcome::Stopped => "stopped",
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

pub fn error_code(text: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\b((?:CS|TS|MSB|NU|E)\d{3,5})\b").unwrap());
    re.captures(text).map(|c| c[1].to_string())
}

fn ellipsize(p: &egui::Painter, text: &str, font: &FontId, max: f32) -> String {
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

pub struct TileLook {
    pub hover: bool,
    pub selected: bool,
    pub hint: Option<char>,
    pub pulse: f32,
    pub hold: f32,
    pub opacity: f32,
    pub now: f64,
}

/// Paints one tile into `rect`.
pub fn tile(p: &egui::Painter, rect: Rect, v: &TaskView, look: &TileLook) {
    let o = look.opacity;
    let pc = phase_color(v.phase);
    let mut bg = if look.hover { TILE_HOVER } else { TILE };
    if matches!(v.phase, Phase::Failed | Phase::Seen) {
        bg = lerp(bg, STOP, 0.12);
    }
    p.rect_filled(rect, CornerRadius::same(11), bg.gamma_multiply(o));
    let border = if look.selected {
        ACCENT
    } else {
        match v.phase {
            Phase::Idle => Color32::TRANSPARENT,
            Phase::Queued | Phase::Waiting => pc.gamma_multiply(0.3),
            _ => pc.gamma_multiply(0.5),
        }
    };
    if border != Color32::TRANSPARENT {
        p.rect_stroke(rect, CornerRadius::same(11), Stroke::new(1.0, border.gamma_multiply(o)), StrokeKind::Inside);
    }
    let inner = rect.shrink2(vec2(10.0, 9.0));
    let mut y = inner.top() + 8.0;
    lamp(p, pos2(inner.left() + 5.0, y), v.phase, look.pulse, look.hold, o);
    let name_font = FontId::new(13.5, egui::FontFamily::Monospace);
    let hint_w = if look.hint.is_some() { 20.0 } else { 0.0 };
    let name = ellipsize(p, &v.label, &name_font, inner.width() - 18.0 - hint_w);
    p.text(pos2(inner.left() + 16.0, y), Align2::LEFT_CENTER, name, name_font, FG.gamma_multiply(o));
    if let Some(h) = look.hint {
        let r = Rect::from_center_size(pos2(inner.right() - 7.0, y), vec2(15.0, 16.0));
        p.rect_filled(r, CornerRadius::same(4), CAUTION.gamma_multiply(o));
        p.text(r.center(), Align2::CENTER_CENTER, h.to_string(), mono(10.5), Color32::from_rgb(0x1b, 0x1d, 0x20).gamma_multiply(o));
    }
    y += 17.0;
    let sub_font = mono(10.5);
    let sub = subtitle(v);
    p.text(pos2(inner.left(), y), Align2::LEFT_CENTER, ellipsize(p, &sub, &sub_font, inner.width()), sub_font, DIM.gamma_multiply(o));
    y += 16.0;
    let st_color = if v.phase == Phase::Idle { DIM } else { pc };
    let st_font = mono(11.5);
    p.text(
        pos2(inner.left(), y),
        Align2::LEFT_CENTER,
        ellipsize(p, &status_line(v, look.now), &st_font, inner.width()),
        st_font,
        st_color.gamma_multiply(o),
    );
    y += 11.0;
    if v.phase == Phase::Building {
        let bar = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 3.0));
        p.rect_filled(bar, CornerRadius::same(1), Color32::from_white_alpha(20).gamma_multiply(o));
        let frac = match v.typical {
            Some(t) if t > 1.0 => ((look.now - v.since) / t).clamp(0.02, 0.97) as f32,
            _ => {
                // Unknown length: a sliding segment.
                let t = (look.now / 1.6).fract() as f32;
                let seg = 0.28;
                let x0 = (t * (1.0 + seg) - seg).max(0.0);
                let x1 = (t * (1.0 + seg)).min(1.0);
                let r =
                    Rect::from_min_max(pos2(bar.left() + bar.width() * x0, bar.top()), pos2(bar.left() + bar.width() * x1, bar.bottom()));
                p.rect_filled(r, CornerRadius::same(1), CAUTION.gamma_multiply(o));
                -1.0
            }
        };
        if frac >= 0.0 {
            let r = Rect::from_min_size(bar.min, vec2(bar.width() * frac, 3.0));
            p.rect_filled(r, CornerRadius::same(1), CAUTION.gamma_multiply(o));
        }
        y += 6.0;
    }
    if matches!(v.phase, Phase::Building | Phase::Running) && v.mem_history.len() >= 2 {
        let area = Rect::from_min_size(pos2(inner.left(), y + 2.0), vec2(inner.width(), 20.0));
        spark(p, area, &v.mem_history, pc, o);
    }
    if v.confirm != Confirm::None && v.phase == Phase::Idle {
        let label = if v.confirm == Confirm::Hold { "hold" } else { "type to confirm" };
        let f = mono(10.0);
        let w = p.layout_no_wrap(label.to_string(), f.clone(), FG).size().x + 10.0;
        let r = Rect::from_min_size(pos2(inner.left(), inner.bottom() - 15.0), vec2(w, 15.0));
        p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, CAUTION.gamma_multiply(0.5 * o)), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, label, f, CAUTION.gamma_multiply(o));
    }
}

fn subtitle(v: &TaskView) -> String {
    if v.last.is_some() && matches!(v.phase, Phase::Running | Phase::Ok | Phase::Idle) {
        if let Some((n, h)) = v.params.iter().find_map(|p| p.history.first().map(|h| (p.name.clone(), h.value.clone()))) {
            return format!("{n} {h}");
        }
    }
    v.description.clone()
}

pub fn spark(p: &egui::Painter, area: Rect, data: &[u64], color: Color32, o: f32) {
    let max = data.iter().copied().max().unwrap_or(1).max(1) as f32 * 1.15;
    let n = data.len().max(30);
    let step = area.width() / (n - 1) as f32;
    let x0 = area.right() - step * (data.len() - 1) as f32;
    let pts: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(x0 + step * i as f32, area.bottom() - 1.0 - (*v as f32 / max) * (area.height() - 3.0)))
        .collect();
    let mut fill = pts.clone();
    fill.push(pos2(pts.last().unwrap().x, area.bottom()));
    fill.push(pos2(pts[0].x, area.bottom()));
    p.add(egui::Shape::convex_polygon(fill, color.gamma_multiply(0.14 * o), Stroke::NONE));
    p.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, color.gamma_multiply(o))));
    p.circle_filled(*pts.last().unwrap(), 2.4, color.gamma_multiply(o));
}

pub fn gauge(p: &egui::Painter, r: Rect, frac: f32, color: Color32) {
    p.rect_filled(r, CornerRadius::same(3), Color32::from_white_alpha(26));
    let f = Rect::from_min_size(r.min, vec2((r.width() * frac.clamp(0.0, 1.0)).max(r.height()), r.height()));
    p.rect_filled(f, CornerRadius::same(3), color);
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}
