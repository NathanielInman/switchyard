//! Where Switchyard keeps things. Every location can be moved with an
//! environment variable, which the tests and the demo use to stay isolated.

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands a leading `~` and `$HOME`/`${HOME}`.
pub fn expand(s: &str) -> PathBuf {
    let s = s.trim();
    let h = home();
    let hs = h.to_string_lossy();
    let replaced = s.replace("${HOME}", &hs).replace("$HOME", &hs);
    if replaced == "~" {
        h
    } else if let Some(rest) = replaced.strip_prefix("~/") {
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
        Ok(rest) => format!("~/{}", rest.display()),
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
    env_dir("XDG_CONFIG_HOME")
        .unwrap_or_else(|| home().join(".config"))
        .join("switchyard")
}

pub fn state_dir() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_STATE_DIR") {
        return d;
    }
    env_dir("XDG_STATE_HOME")
        .unwrap_or_else(|| home().join(".local/state"))
        .join("switchyard")
}

pub fn runtime_dir() -> PathBuf {
    if let Some(d) = env_dir("SWITCHYARD_RUNTIME_DIR") {
        return d;
    }
    match env_dir("XDG_RUNTIME_DIR") {
        Some(d) => d.join("switchyard"),
        None => PathBuf::from(format!("/tmp/switchyard-{}", unsafe { libc::getuid() })),
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
        assert_eq!(expand_in("rel", Path::new("/base")), PathBuf::from("/base/rel"));
        assert_eq!(tilde(&h.join("code")), "~/code");
    }
}
