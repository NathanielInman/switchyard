//! Where Switchyard keeps things. Every location can be moved with an
//! environment variable, which the tests and the demo use to stay isolated.

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Replaces `$VAR` and `${VAR}` with environment values (unset becomes empty).
pub fn expand_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let (name, next) = if chars[i + 1] == '{' {
                match chars[i + 2..].iter().position(|c| *c == '}') {
                    Some(end) => (chars[i + 2..i + 2 + end].iter().collect::<String>(), i + 3 + end),
                    None => (String::new(), i),
                }
            } else {
                let starts = chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_';
                let len = if starts {
                    chars[i + 1..]
                        .iter()
                        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                        .count()
                } else {
                    0
                };
                (chars[i + 1..i + 1 + len].iter().collect::<String>(), i + 1 + len)
            };
            if !name.is_empty() {
                // $HOME means the home directory everywhere, Windows included.
                let val = if name == "HOME" { home().to_string_lossy().to_string() } else { std::env::var(&name).unwrap_or_default() };
                out.push_str(&val);
                i = next;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Expands a leading `~` and environment variables.
pub fn expand(s: &str) -> PathBuf {
    let s = s.trim();
    let h = home();
    let replaced = expand_vars(s);
    if replaced == "~" {
        h
    } else if let Some(rest) = replaced.strip_prefix("~/").or_else(|| replaced.strip_prefix("~\\")) {
        h.join(rest)
    } else {
        PathBuf::from(replaced)
    }
}

/// Like [`expand`], but relative paths resolve against `base`.
pub fn expand_in(s: &str, base: &Path) -> PathBuf {
    let p = expand(s);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

/// Shows a path with `~` in place of the home directory.
pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_CONFIG_DIR") {
        return d;
    }
    if cfg!(windows) {
        return env_dir("APPDATA").unwrap_or_else(|| home().join("AppData/Roaming")).join("switchyard");
    }
    // XDG layout on macOS too: it is where developers keep dot-files.
    env_dir("XDG_CONFIG_HOME")
        .unwrap_or_else(|| home().join(".config"))
        .join("switchyard")
}

pub fn state_dir() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_STATE_DIR") {
        return d;
    }
    if cfg!(windows) {
        return local_app_data().join("switchyard").join("state");
    }
    env_dir("XDG_STATE_HOME")
        .unwrap_or_else(|| home().join(".local/state"))
        .join("switchyard")
}

#[cfg(windows)]
fn local_app_data() -> PathBuf {
    env_dir("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData/Local"))
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn local_app_data() -> PathBuf {
    home()
}

/// Per-user directory for the socket and run files.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_RUNTIME_DIR") {
        return d;
    }
    #[cfg(windows)]
    {
        local_app_data().join("switchyard").join("run")
    }
    #[cfg(unix)]
    {
        if let Some(d) = env_dir("XDG_RUNTIME_DIR") {
            return d.join("switchyard");
        }
        // macOS: $TMPDIR is already per-user.
        let base = env_dir("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
        base.join(format!("switchyard-{}", unsafe { libc::getuid() }))
    }
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("yard.sock")
}

pub fn runs_dir() -> PathBuf {
    runtime_dir().join("runs")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn overlay_dir() -> PathBuf {
    config_dir().join("tasks.d")
}

pub fn style_file() -> PathBuf {
    config_dir().join("style.css")
}

pub fn state_file() -> PathBuf {
    state_dir().join("state.json")
}

pub fn default_log_dir() -> PathBuf {
    state_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home() {
        let h = home();
        assert_eq!(expand("~"), h);
        assert_eq!(expand("~/x/y"), h.join("x/y"));
        assert_eq!(expand("$HOME/a"), h.join("a"));
        assert_eq!(expand("/abs"), PathBuf::from("/abs"));
        #[cfg(unix)]
        assert_eq!(expand_in("rel", Path::new("/base")), PathBuf::from("/base/rel"));
        assert_eq!(tilde(&h.join("code")), format!("~{}code", std::path::MAIN_SEPARATOR));
        std::env::set_var("YARD_TEST_VAR", "/v");
        assert_eq!(expand("${YARD_TEST_VAR}/a"), PathBuf::from("/v/a"));
        assert_eq!(expand("$YARD_TEST_VAR/b"), PathBuf::from("/v/b"));
        assert_eq!(expand_vars("cost $5"), "cost $5");
    }
}
