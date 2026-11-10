//! The tray (Windows) or menu bar (macOS) icon: a small signal head whose lit
//! lamp is the aggregate state.

use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use yard_core::model::{Anchor, Edge, Phase};

pub enum TrayMsg {
    /// Left click: toggle the board next to the icon.
    Toggle(Anchor),
    Open,
    Ack,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    open_id: tray_icon::menu::MenuId,
    ack_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    shown: Option<Phase>,
}

const RED: [u8; 3] = [0xbf, 0x61, 0x6a];
const AMBER: [u8; 3] = [0xeb, 0xcb, 0x8b];
const GREEN: [u8; 3] = [0xa3, 0xbe, 0x8c];
const FROST: [u8; 3] = [0x88, 0xc0, 0xd0];
/// The housing's rim: light, so the dark head reads on a dark taskbar too.
const RIM: [u8; 3] = [0xd8, 0xde, 0xe9];

/// Draws a 32x32 signal head: a rounded housing with three lamps.
fn signal(lit: [Option<[u8; 3]>; 3]) -> Icon {
    const N: usize = 32;
    let mut px = vec![0u8; N * N * 4];
    let mut put = |x: usize, y: usize, c: [u8; 3], a: f32| {
        let i = (y * N + x) * 4;
        let src = a.clamp(0.0, 1.0);
        let dst = px[i + 3] as f32 / 255.0;
        let out = src + dst * (1.0 - src);
        for k in 0..3 {
            let mixed = (c[k] as f32 * src + px[i + k] as f32 * dst * (1.0 - src)) / out.max(1e-6);
            px[i + k] = mixed as u8;
        }
        px[i + 3] = (out * 255.0) as u8;
    };
    // Housing: a rounded rectangle 14 wide, 30 tall, centered.
    let (x0, x1, y0, y1, r) = (9.0f32, 23.0f32, 1.0f32, 31.0f32, 6.0f32);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(x0 + r, x1 - r);
            let cy = fy.clamp(y0 + r, y1 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let inside = 1.0 - (d - r + 0.5).clamp(0.0, 1.0);
            if inside > 0.0 {
                put(x, y, [0x1b, 0x1d, 0x20], inside);
                // The outer 2px, so it survives the 16px tray at 100%.
                let rim = (d - r + 2.5).clamp(0.0, 1.0) * inside;
                if rim > 0.0 {
                    put(x, y, RIM, rim);
                }
            }
        }
    }
    for (i, lamp) in lit.iter().enumerate() {
        let (cx, cy) = (16.0f32, 7.0 + 9.0 * i as f32);
        let color = lamp.unwrap_or([0x3a, 0x3f, 0x46]);
        for y in 0..N {
            for x in 0..N {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = 1.0 - (d - 3.2).clamp(0.0, 1.0);
                if a > 0.0 {
                    put(x, y, color, a);
                }
            }
        }
    }
    Icon::from_rgba(px, N as u32, N as u32).expect("valid icon")
}

fn icon_for(p: Phase) -> Icon {
    match p {
        Phase::Failed | Phase::Seen => signal([Some(RED), None, None]),
        Phase::Building | Phase::Queued | Phase::Waiting => signal([None, Some(AMBER), None]),
        Phase::Ok => signal([None, None, Some(GREEN)]),
        Phase::Running => signal([None, None, Some(FROST)]),
        Phase::Idle => signal([None, None, None]),
    }
}

impl Tray {
    pub fn new(tx: Sender<TrayMsg>, repaint: impl Fn() + Send + Sync + Clone + 'static) -> Option<Tray> {
        let menu = Menu::new();
        let open = MenuItem::new("Open the board", true, None);
        let ack = MenuItem::new("Acknowledge failures", true, None);
        let quit = MenuItem::new("Quit Switchyard", true, None);
        let _ = menu.append_items(&[&open, &ack, &PredefinedMenuItem::separator(), &quit]);
        let icon = TrayIconBuilder::new()
            .with_icon(icon_for(Phase::Idle))
            .with_tooltip("Switchyard")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        let (open_id, ack_id, quit_id) = (open.id().clone(), ack.id().clone(), quit.id().clone());
        {
            let tx = tx.clone();
            let repaint = repaint.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { rect, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    let _ = tx.send(TrayMsg::Toggle(anchor_for(rect)));
                    repaint();
                }
            }));
        }
        {
            let (o, a, q) = (open_id.clone(), ack_id.clone(), quit_id.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let msg = if e.id == o {
                    TrayMsg::Open
                } else if e.id == a {
                    TrayMsg::Ack
                } else if e.id == q {
                    TrayMsg::Quit
                } else {
                    return;
                };
                let _ = tx.send(msg);
                repaint();
            }));
        }
        Some(Tray { icon, open_id, ack_id, quit_id, shown: Some(Phase::Idle) })
    }

    pub fn update(&mut self, phase: Phase, tooltip: &str) {
        if self.shown != Some(phase) {
            let _ = self.icon.set_icon(Some(icon_for(phase)));
            self.shown = Some(phase);
        }
        let _ = self.icon.set_tooltip(Some(tooltip));
        let _ = (&self.open_id, &self.ack_id, &self.quit_id);
    }
}

/// The board opens from the icon: below a top menu bar (macOS), above a
/// bottom taskbar (Windows, by default). Physical pixels; the board converts.
fn anchor_for(rect: tray_icon::Rect) -> Anchor {
    let (x, y) = (rect.position.x, rect.position.y);
    let (w, h) = (rect.size.width as f64, rect.size.height as f64);
    let top = cfg!(target_os = "macos") || y < 200.0;
    Anchor {
        monitor: None,
        edge: if top { Edge::Top } else { Edge::Bottom },
        x: x + w / 2.0,
        y: if top { y + h } else { y },
        width: 0.0,
        height: 0.0,
        reserved: [0.0; 4],
        center: false,
        physical: true,
    }
}
