//! `~/.config/switchyard/config.toml`.

use crate::paths;
use crate::size::parse_size;
use crate::Problem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scan: Vec<String>,
    pub scripts: Vec<String>,
    pub path_prepend: Vec<String>,
    pub env_passthrough: Vec<String>,
    pub log_dir: Option<String>,
    pub ok_flash: u64,
    pub terminal: Vec<String>,
    pub editor: Vec<String>,
    /// How steps run: the program and its "run this string" flag. Empty picks
    /// bash on Linux and macOS, and pwsh, powershell or cmd on Windows.
    pub shell: Vec<String>,
    pub mainline: MainlineConfig,
    pub board: BoardConfig,
    #[serde(rename = "tab")]
    pub tabs: Vec<TabConfig>,
    pub shelf: ShelfConfig,
    pub bar: BarConfig,
    /// The optional Inspection tab; `None` turns it and its scanning off.
    pub inspection: Option<crate::inspect::InspectionConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scan: vec![],
            scripts: vec![],
            path_prepend: vec!["~/.local/bin".into(), "~/.cargo/bin".into()],
            env_passthrough: vec![],
            log_dir: None,
            ok_flash: 6,
            terminal: default_terminal(),
            editor: default_editor(),
            shell: vec![],
            mainline: MainlineConfig::default(),
            board: BoardConfig::default(),
            tabs: vec![],
            shelf: ShelfConfig::default(),
            bar: BarConfig::default(),
            inspection: None,
        }
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn default_terminal() -> Vec<String> {
    if cfg!(target_os = "macos") {
        strings(&["open", "-a", "Terminal"])
    } else if cfg!(windows) {
        strings(&["wt.exe", "--"])
    } else {
        strings(&["kitty", "--hold", "-e"])
    }
}

fn default_editor() -> Vec<String> {
    if cfg!(any(target_os = "macos", windows)) {
        strings(&["code", "--goto", "{file}:{line}"])
    } else {
        strings(&["kitty", "-e", "nvim", "+{line}", "{file}"])
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MainlineConfig {
    pub min_free: String,
    pub mem: String,
    pub idle_cpu: bool,
    pub nice: i32,
    /// Hold the next heavy build while a running Roundhouse says the machine
    /// needs acting on. No effect when Roundhouse is not running.
    pub respect_roundhouse: bool,
}

impl Default for MainlineConfig {
    fn default() -> Self {
        MainlineConfig { min_free: "8G".into(), mem: "16G".into(), idle_cpu: true, nice: 19, respect_roundhouse: true }
    }
}

impl MainlineConfig {
    pub fn min_free_bytes(&self) -> u64 {
        parse_size(&self.min_free).unwrap_or(8 << 30)
    }
    pub fn mem_bytes(&self) -> u64 {
        parse_size(&self.mem).unwrap_or(16 << 30)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BoardConfig {
    pub width: u32,
    pub font: Option<String>,
    pub bar_namespace: String,
    /// Fraction of the monitor height the panel may use.
    pub max_height: f64,
    /// Gap between the bar and the panel, in pixels.
    pub gap: i32,
}

impl Default for BoardConfig {
    fn default() -> Self {
        BoardConfig { width: 640, font: None, bar_namespace: "waybar".into(), max_height: 0.8, gap: 8 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabConfig {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShelfConfig {
    pub stale_days: u64,
    pub tools: String,
    pub binds: bool,
    #[serde(rename = "tool")]
    pub extra: Vec<ShelfTool>,
}

impl Default for ShelfConfig {
    fn default() -> Self {
        ShelfConfig { stale_days: 30, tools: "none".into(), binds: true, extra: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ShelfTool {
    pub name: String,
    #[serde(default)]
    pub desc: String,
    /// Command to run instead of the bare name.
    #[serde(default)]
    pub cmd: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    pub icon: String,
    /// Optional text per state; `{icon}` and `{count}` are replaced.
    pub format: std::collections::BTreeMap<String, String>,
    pub show_count: bool,
}

impl Default for BarConfig {
    fn default() -> Self {
        BarConfig { icon: "●".into(), format: Default::default(), show_count: true }
    }
}

impl Config {
    /// Loads the config file. A broken file yields the defaults plus a problem
    /// so the board can say what is wrong instead of refusing to start.
    pub fn load() -> (Config, Vec<Problem>) {
        Self::load_from(&paths::config_file())
    }

    pub fn load_from(path: &std::path::Path) -> (Config, Vec<Problem>) {
        match std::fs::read_to_string(path) {
            Err(_) => (Config::default(), vec![]),
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => (c, vec![]),
                Err(e) => (Config::default(), vec![Problem { file: paths::tilde(path), message: e.to_string().trim().to_string() }]),
            },
        }
    }

    pub fn log_dir(&self) -> PathBuf {
        match &self.log_dir {
            Some(d) => paths::expand(d),
            None => paths::default_log_dir(),
        }
    }

    pub fn scan_dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![];
        for pat in &self.scan {
            let p = paths::expand(pat);
            let s = p.to_string_lossy().to_string();
            if s.contains(['*', '?', '[']) {
                if let Ok(entries) = glob::glob(&s) {
                    for e in entries.flatten() {
                        if e.is_dir() {
                            out.push(e);
                        }
                    }
                }
            } else if p.is_dir() {
                out.push(p);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let text = r#"
            scan = ["~/code/*"]
            scripts = ["~/.local/bin"]
            ok_flash = 4
            terminal = ["foot", "-e"]
            [mainline]
            min_free = "16G"
            mem = "12G"
            [board]
            width = 700
            font = "Iosevka"
            [[tab]]
            name = "Games"
            tags = ["games"]
            [shelf]
            tools = "pacman"
            [[shelf.tool]]
            name = "tokei"
            desc = "count lines"
            [bar]
            icon = "S"
            [bar.format]
            failed = "{icon} {count}"
            [inspection]
            roots = ["~/code/*"]
            [inspection.severity]
            foreign_author = "act"
        "#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.ok_flash, 4);
        assert_eq!(c.mainline.min_free_bytes(), 16 << 30);
        assert_eq!(c.mainline.mem_bytes(), 12 << 30);
        assert_eq!(c.tabs[0].name, "Games");
        assert_eq!(c.shelf.extra[0].name, "tokei");
        assert_eq!(c.bar.format["failed"], "{icon} {count}");
        assert_eq!(c.board.bar_namespace, "waybar");
        let i = c.inspection.unwrap();
        assert_eq!(i.roots, vec!["~/code/*"]);
        assert_eq!(i.severity("foreign_author"), crate::inspect::Severity::Act);
        assert!(toml::from_str::<Config>("").unwrap().inspection.is_none());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("scna = []").is_err());
    }
}
