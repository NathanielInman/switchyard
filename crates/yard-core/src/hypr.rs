//! Asks Hyprland where the bar is and where the click happened, by talking to
//! its IPC socket directly (no `hyprctl` processes).

use crate::model::{Anchor, Edge};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

pub fn socket() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let mut candidates = vec![];
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(rt).join("hypr").join(&sig).join(".socket.sock"));
    }
    candidates.push(PathBuf::from("/tmp/hypr").join(&sig).join(".socket.sock"));
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(not(unix))]
pub fn query(_cmd: &str) -> Result<String> {
    Err(anyhow!("Hyprland only runs on Linux"))
}

#[cfg(unix)]
pub fn query(cmd: &str) -> Result<String> {
    let path = socket().ok_or_else(|| anyhow!("not running under Hyprland"))?;
    let mut s = std::os::unix::net::UnixStream::connect(path)?;
    s.set_read_timeout(Some(Duration::from_millis(800)))?;
    s.write_all(cmd.as_bytes())?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub transform: i64,
    #[serde(default)]
    pub reserved: Vec<f64>,
    #[serde(default)]
    pub focused: bool,
}

fn one() -> f64 {
    1.0
}

impl Monitor {
    /// Size in layout (logical) pixels.
    pub fn logical(&self) -> (f64, f64) {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = (self.width / s, self.height / s);
        if self.transform % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let (w, h) = self.logical();
        x >= self.x && x < self.x + w && y >= self.y && y < self.y + h
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Layer {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub namespace: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MonitorLayers {
    levels: BTreeMap<String, Vec<Layer>>,
}

#[derive(Debug, Clone, Deserialize)]
struct CursorPos {
    x: f64,
    y: f64,
}

/// Works out the anchor from raw IPC replies. Split out so it can be tested
/// against recorded output.
pub fn compute_anchor(cursor_json: &str, monitors_json: &str, layers_json: &str, namespace: &str, center: bool) -> Result<Anchor> {
    let monitors: Vec<Monitor> = serde_json::from_str(monitors_json)?;
    let cursor: Option<CursorPos> = serde_json::from_str(cursor_json).ok();
    let mon = match &cursor {
        Some(c) => monitors.iter().find(|m| m.contains(c.x, c.y)),
        None => None,
    }
    .or_else(|| monitors.iter().find(|m| m.focused))
    .or_else(|| monitors.first())
    .ok_or_else(|| anyhow!("no monitors"))?;
    let (w, h) = mon.logical();
    let mut reserved = [0.0; 4];
    for (i, r) in mon.reserved.iter().take(4).enumerate() {
        reserved[i] = *r;
    }
    let (cx, cy) = match &cursor {
        Some(c) => (c.x - mon.x, c.y - mon.y),
        None => (w / 2.0, h / 2.0),
    };

    let layers: BTreeMap<String, MonitorLayers> = serde_json::from_str(layers_json).unwrap_or_default();
    let bar = layers.get(&mon.name).and_then(|ml| ml.levels.values().flatten().find(|l| l.namespace == namespace).cloned());
    let edge = match bar {
        Some(b) => {
            let (bx, by) = (b.x - mon.x, b.y - mon.y);
            if b.w >= b.h {
                if by + b.h / 2.0 < h / 2.0 {
                    Edge::Top
                } else {
                    Edge::Bottom
                }
            } else if bx + b.w / 2.0 < w / 2.0 {
                Edge::Left
            } else {
                Edge::Right
            }
        }
        None => {
            // No bar found: the side with the largest reserved space.
            let idx = (0..4).max_by(|a, b| reserved[*a].partial_cmp(&reserved[*b]).unwrap()).unwrap_or(3);
            if reserved[idx] <= 0.0 {
                Edge::Bottom
            } else {
                [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom][idx]
            }
        }
    };
    Ok(Anchor { monitor: Some(mon.name.clone()), edge, x: cx, y: cy, width: w, height: h, reserved, center, physical: false })
}

/// Queries Hyprland for the current anchor. `None` outside Hyprland.
pub fn anchor(namespace: &str, center: bool) -> Option<Anchor> {
    socket()?;
    let cursor = query("j/cursorpos").unwrap_or_default();
    let monitors = query("j/monitors").ok()?;
    let layers = query("j/layers").unwrap_or_default();
    let cursor = if center { String::new() } else { cursor };
    compute_anchor(&cursor, &monitors, &layers, namespace, center).ok()
}

/// Key binds with descriptions, for the Shelf.
pub fn binds() -> Vec<(String, String)> {
    query("j/binds").map(|text| parse_binds(&text)).unwrap_or_default()
}

/// `hyprctl -j binds` as (keys, description) pairs: "Super + Alt + V",
/// "Clipboard history".
fn parse_binds(text: &str) -> Vec<(String, String)> {
    #[derive(Deserialize)]
    struct Bind {
        #[serde(default)]
        modmask: u32,
        #[serde(default)]
        key: String,
        #[serde(default)]
        keycode: u32,
        #[serde(default)]
        mouse: bool,
        #[serde(default)]
        description: String,
        #[serde(default)]
        dispatcher: String,
        #[serde(default)]
        arg: String,
    }
    let Ok(binds) = serde_json::from_str::<Vec<Bind>>(text) else { return vec![] };
    binds
        .into_iter()
        // A bind by key code ("code:191") has no key name, only the code.
        .filter(|b| !b.key.is_empty() || b.keycode != 0)
        .map(|b| {
            let mut keys = vec![];
            for (bit, name) in [(64, "Super"), (4, "Ctrl"), (8, "Alt"), (1, "Shift")] {
                if b.modmask & bit != 0 {
                    keys.push(name.to_string());
                }
            }
            keys.push(key_name(&b.key, b.keycode, b.mouse));
            let desc = if b.description.is_empty() { describe(&b.dispatcher, &b.arg) } else { b.description };
            (keys.join(" + "), desc)
        })
        .collect()
}

/// What a bind without a description runs. The hyprlang config reports the
/// dispatcher and its argument ("exec kitty"). The Lua config reports
/// "HL.Dispatcher(exec_cmd)" and a number that only means something inside
/// Hyprland, so only the dispatcher's name is worth showing.
fn describe(dispatcher: &str, arg: &str) -> String {
    if let Some(name) = dispatcher.strip_prefix("HL.Dispatcher(").and_then(|d| d.strip_suffix(')')) {
        if arg.is_empty() || arg.chars().all(|c| c.is_ascii_digit()) {
            return name.to_string();
        }
        return format!("{name} {arg}");
    }
    if arg.is_empty() {
        dispatcher.to_string()
    } else {
        format!("{dispatcher} {arg}")
    }
}

/// A key as it's printed on the keyboard: "]" for bracketright, "F13" for
/// code:191, "Volume up" for XF86AudioRaiseVolume. `mouse` marks a held
/// mouse bind (bindm), which drags rather than clicks.
fn key_name(key: &str, keycode: u32, mouse: bool) -> String {
    if key.is_empty() {
        // X keycodes are evdev codes plus 8: F13 to F24 are 191 to 202.
        return match keycode {
            191..=202 => format!("F{}", keycode - 178),
            _ => format!("code:{keycode}"),
        };
    }
    if let Some(button) = key.strip_prefix("mouse:") {
        let which = match button {
            "272" => "Left",
            "273" => "Right",
            "274" => "Middle",
            "275" => "Back",
            "276" => "Forward",
            other => return format!("Mouse {other}"),
        };
        return format!("{which} {}", if mouse { "drag" } else { "click" });
    }
    let named = match key.to_ascii_lowercase().as_str() {
        "mouse_down" => "Scroll down",
        "mouse_up" => "Scroll up",
        "mouse_left" => "Scroll left",
        "mouse_right" => "Scroll right",
        "bracketleft" => "[",
        "bracketright" => "]",
        "braceleft" => "{",
        "braceright" => "}",
        "parenleft" => "(",
        "parenright" => ")",
        "slash" => "/",
        "backslash" => "\\",
        "equal" => "=",
        "minus" => "-",
        "plus" => "+",
        "comma" => ",",
        "period" => ".",
        "semicolon" => ";",
        "apostrophe" => "'",
        "grave" => "`",
        "return" => "Enter",
        "escape" => "Esc",
        "space" => "Space",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "print" => "Print",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "xf86audioraisevolume" => "Volume up",
        "xf86audiolowervolume" => "Volume down",
        "xf86audiomute" => "Mute",
        "xf86audiomicmute" => "Mic mute",
        "xf86audioplay" => "Play",
        "xf86audiopause" => "Pause",
        "xf86audiostop" => "Stop",
        "xf86audionext" => "Next track",
        "xf86audioprev" => "Previous track",
        "xf86monbrightnessup" => "Brightness up",
        "xf86monbrightnessdown" => "Brightness down",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    if let Some(rest) = key.strip_prefix("XF86") {
        return rest.to_string();
    }
    if key.chars().count() == 1 {
        return key.to_uppercase();
    }
    key.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITORS: &str = r#"[
      {"name":"DP-3","x":0,"y":0,"width":2560,"height":1440,"scale":1.0,"transform":0,"reserved":[0,0,0,24],"focused":false},
      {"name":"DP-2","x":2560,"y":0,"width":2560,"height":1440,"scale":1.0,"transform":0,"reserved":[0,0,0,24],"focused":true}
    ]"#;
    const LAYERS: &str = r#"{
      "DP-2":{"levels":{"0":[],"2":[{"address":"0x1","x":2560,"y":1416,"w":2560,"h":24,"namespace":"waybar","pid":1}]}},
      "DP-3":{"levels":{"2":[{"address":"0x2","x":0,"y":1416,"w":2560,"h":24,"namespace":"waybar","pid":1}]}}
    }"#;

    #[test]
    fn bottom_bar_click() {
        let a = compute_anchor(r#"{"x":4870,"y":1428}"#, MONITORS, LAYERS, "waybar", false).unwrap();
        assert_eq!(a.monitor.as_deref(), Some("DP-2"));
        assert_eq!(a.edge, Edge::Bottom);
        assert_eq!(a.x, 2310.0);
        assert_eq!(a.reserved, [0.0, 0.0, 0.0, 24.0]);
        assert_eq!(a.width, 2560.0);
    }

    #[test]
    fn top_and_side_bars() {
        let top = r#"{"DP-3":{"levels":{"2":[{"x":0,"y":0,"w":2560,"h":30,"namespace":"waybar"}]}}}"#;
        let a = compute_anchor(r#"{"x":100,"y":10}"#, MONITORS, top, "waybar", false).unwrap();
        assert_eq!(a.edge, Edge::Top);
        let left = r#"{"DP-3":{"levels":{"2":[{"x":0,"y":0,"w":40,"h":1440,"namespace":"waybar"}]}}}"#;
        let a = compute_anchor(r#"{"x":10,"y":700}"#, MONITORS, left, "waybar", false).unwrap();
        assert_eq!(a.edge, Edge::Left);
    }

    #[test]
    fn no_bar_falls_back_to_reserved() {
        let a = compute_anchor(r#"{"x":10,"y":10}"#, MONITORS, "{}", "waybar", false).unwrap();
        assert_eq!(a.edge, Edge::Bottom);
        let a = compute_anchor("", MONITORS, LAYERS, "waybar", true).unwrap();
        assert_eq!(a.monitor.as_deref(), Some("DP-2"));
        assert!(a.center);
    }

    #[test]
    fn scaled_monitor() {
        let m = r#"[{"name":"eDP-1","x":0,"y":0,"width":2880,"height":1800,"scale":2.0,"reserved":[0,32,0,0]}]"#;
        let a = compute_anchor(r#"{"x":700,"y":5}"#, m, "{}", "waybar", false).unwrap();
        assert_eq!(a.width, 1440.0);
        assert_eq!(a.edge, Edge::Top);
    }

    #[test]
    fn binds_read_as_keys_and_names() {
        let json = r#"[
          {"modmask":72,"key":"V","keycode":0,"mouse":false,"description":"Clipboard history · paste","dispatcher":"HL.Dispatcher(exec_cmd)","arg":"33"},
          {"modmask":0,"key":"","keycode":191,"mouse":false,"description":"Dictation","dispatcher":"HL.Dispatcher(exec_cmd)","arg":"48"},
          {"modmask":64,"key":"mouse:272","keycode":0,"mouse":true,"description":"","dispatcher":"HL.Dispatcher(window.drag)","arg":""},
          {"modmask":65,"key":"bracketright","keycode":0,"mouse":false,"description":"","dispatcher":"HL.Dispatcher(exec_cmd)","arg":"12"},
          {"modmask":0,"key":"XF86AudioRaiseVolume","keycode":0,"mouse":false,"description":"","dispatcher":"exec","arg":"swayosd-client --output-volume +5"},
          {"modmask":4,"key":"","keycode":0,"mouse":false,"description":"nothing to press","dispatcher":"exec","arg":""}
        ]"#;
        let got = parse_binds(json);
        let want = [
            ("Super + Alt + V", "Clipboard history · paste"),
            ("F13", "Dictation"),
            ("Super + Left drag", "window.drag"),
            ("Super + Shift + ]", "exec_cmd"),
            ("Volume up", "exec swayosd-client --output-volume +5"),
        ];
        assert_eq!(got, want.map(|(k, d)| (k.to_string(), d.to_string())));
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name("q", 0, false), "Q");
        assert_eq!(key_name("Slash", 0, false), "/");
        assert_eq!(key_name("Return", 0, false), "Enter");
        assert_eq!(key_name("mouse:273", 0, false), "Right click");
        assert_eq!(key_name("XF86Calculator", 0, false), "Calculator");
        assert_eq!(key_name("", 202, false), "F24");
        assert_eq!(key_name("", 66, false), "code:66");
        assert_eq!(key_name("Home", 0, false), "Home");
    }
}
