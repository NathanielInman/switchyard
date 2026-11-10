//! Finds the first real error in a log.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const MATCHERS: &[&str] = &[
    "csharp", "rust", "go", "typescript", "gradle", "godot", "python", "generic",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorHit {
    /// Index of the matching line within the lines searched.
    pub index: usize,
    pub text: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub col: Option<u32>,
    pub matcher: String,
}

struct Matcher {
    name: &'static str,
    re: Regex,
    /// A follow-up regex looked for in the next few lines to find a location.
    location: Option<Regex>,
}

fn matchers() -> &'static [Matcher] {
    static M: OnceLock<Vec<Matcher>> = OnceLock::new();
    M.get_or_init(|| {
        let m = |name, re: &str, loc: Option<&str>| Matcher {
            name,
            re: Regex::new(re).unwrap(),
            location: loc.map(|l| Regex::new(l).unwrap()),
        };
        vec![
            m(
                "csharp",
                r"^\s*(?P<file>[^\s(][^(]*?)\((?P<line>\d+),(?P<col>\d+)\): error (?P<code>CS\d+):",
                None,
            ),
            m("csharp", r"^\s*(?:\S+ )?: ?error (?P<code>(CS|MSB|NU)\d+):", None),
            m(
                "rust",
                r"^error(\[E\d+\])?: ",
                Some(r"^\s*--> (?P<file>[^:\s]+):(?P<line>\d+):(?P<col>\d+)"),
            ),
            m(
                "go",
                r"^(?:\./)?(?P<file>[^\s:]+\.go):(?P<line>\d+):(?P<col>\d+): ",
                None,
            ),
            m(
                "typescript",
                r"^(?P<file>[^\s(]+\.[cm]?tsx?)\((?P<line>\d+),(?P<col>\d+)\): error TS\d+",
                None,
            ),
            m(
                "typescript",
                r"^(?P<file>\S+\.[cm]?tsx?):(?P<line>\d+):(?P<col>\d+) - error TS\d+",
                None,
            ),
            m(
                "gradle",
                r"^e: (?:file://)?(?P<file>[^:\s]+\.kts?):(?P<line>\d+):(?P<col>\d+) ",
                None,
            ),
            m(
                "gradle",
                r"^(?P<file>\S+\.java):(?P<line>\d+): error: ",
                None,
            ),
            m("gradle", r"^\* What went wrong:", None),
            m("gradle", r"^FAILURE: Build failed", None),
            m(
                "godot",
                r"^(?:USER )?(?:SCRIPT )?ERROR: ",
                Some(r"at: .*\((?P<file>res://[^:)]+):(?P<line>\d+)\)"),
            ),
            m(
                "godot",
                r"^(?:USER )?(?:SCRIPT )?ERROR: ",
                Some(r"^\s*at: .*\((?P<file>[^:()]+\.(?:gd|cs|cpp|h)):(?P<line>\d+)\)"),
            ),
            m("godot", r"^Parse Error: ", None),
            m("godot", r"^Unhandled [Ee]xception", None),
            m(
                "python",
                r"^Traceback \(most recent call last\)",
                Some(r#"^\s*File "(?P<file>[^"]+)", line (?P<line>\d+)"#),
            ),
            m("generic", r"(?i)^\s*(error|fatal)(\[[^\]]*\])?: ", None),
            m("generic", r"panicked at ", None),
            m("generic", r"(?i)^\s*(npm|yarn|pnpm) ERR!", None),
            m("generic", r"(?i): command not found$", None),
        ]
    })
}

/// Searches `lines` top to bottom and returns the first one any enabled
/// matcher accepts. An empty `enabled` list means every built-in matcher.
pub fn find_first_error(lines: &[&str], enabled: &[String], custom: Option<&Regex>) -> Option<ErrorHit> {
    let on = |name: &str| enabled.is_empty() || enabled.iter().any(|e| e == name);
    for (i, raw) in lines.iter().enumerate() {
        let line = strip_ansi(raw);
        if line.starts_with("=====") {
            continue;
        }
        if let Some(re) = custom {
            if let Some(c) = re.captures(&line) {
                return Some(hit(i, &line, &c, "custom"));
            }
        }
        for m in matchers() {
            if !on(m.name) {
                continue;
            }
            let Some(c) = m.re.captures(&line) else { continue };
            let mut h = hit(i, &line, &c, m.name);
            if h.file.is_none() {
                if let Some(loc) = &m.location {
                    let look = if m.name == "python" { 40 } else { 6 };
                    // Python tracebacks name the innermost frame last.
                    let mut found = None;
                    for next in lines.iter().skip(i + 1).take(look) {
                        let n = strip_ansi(next);
                        if let Some(lc) = loc.captures(&n) {
                            found = Some((
                                lc.name("file").map(|f| f.as_str().to_string()),
                                lc.name("line").and_then(|l| l.as_str().parse().ok()),
                                lc.name("col").and_then(|l| l.as_str().parse().ok()),
                            ));
                            if m.name != "python" {
                                break;
                            }
                        }
                    }
                    if let Some((f, l, c)) = found {
                        h.file = f;
                        h.line = l;
                        h.col = c;
                    }
                }
            }
            return Some(h);
        }
    }
    None
}

fn hit(i: usize, line: &str, c: &regex::Captures, matcher: &str) -> ErrorHit {
    ErrorHit {
        index: i,
        text: line.trim_end().to_string(),
        file: c.name("file").map(|f| f.as_str().trim().to_string()),
        line: c.name("line").and_then(|l| l.as_str().parse().ok()),
        col: c.name("col").and_then(|l| l.as_str().parse().ok()),
        matcher: matcher.to_string(),
    }
}

/// Removes terminal color codes, which build tools print even into files.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\u{1b}') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The slice of a log that belongs to the newest attempt of a failing step:
/// everything after the last `===== step N/` marker (or `===== run` for the
/// run phase). Earlier retried attempts are not the error you care about.
pub fn failing_section<'a>(lines: &'a [&'a str], step: Option<usize>) -> (usize, &'a [&'a str]) {
    let marker = match step {
        Some(n) => format!("===== step {n}/"),
        None => "===== run".to_string(),
    };
    match lines.iter().rposition(|l| l.starts_with(&marker)) {
        Some(p) => (p, &lines[p..]),
        None => {
            // Fall back to the last run header, then the whole log.
            match lines.iter().rposition(|l| l.starts_with("===== switchyard")) {
                Some(p) => (p, &lines[p..]),
                None => (0, lines),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(text: &str) -> Option<ErrorHit> {
        let lines: Vec<&str> = text.lines().collect();
        find_first_error(&lines, &[], None)
    }

    #[test]
    fn csharp() {
        let h = first("  Restoring...\nScripts/Combat/HitResolver.cs(212,17): error CS0103: The name 'critRoll' does not exist\n    1 Error(s)").unwrap();
        assert_eq!(h.index, 1);
        assert_eq!(h.file.as_deref(), Some("Scripts/Combat/HitResolver.cs"));
        assert_eq!(h.line, Some(212));
        assert_eq!(h.col, Some(17));
        assert_eq!(h.matcher, "csharp");
    }

    #[test]
    fn msbuild_counts_are_not_errors() {
        assert!(first("    0 Warning(s)\n    1 Error(s)\nTime Elapsed 00:00:07.41").is_none());
    }

    #[test]
    fn rust_with_location() {
        let h = first("   Compiling x v0.1.0\nerror[E0425]: cannot find value `y` in this scope\n --> src/main.rs:4:13\n  |").unwrap();
        assert_eq!(h.file.as_deref(), Some("src/main.rs"));
        assert_eq!(h.line, Some(4));
        assert_eq!(h.matcher, "rust");
    }

    #[test]
    fn godot_location() {
        let h = first("Godot Engine v4.7\nSCRIPT ERROR: Invalid call. Nonexistent function 'foo'.\n   at: _ready (res://scripts/player.gd:42)").unwrap();
        assert_eq!(h.file.as_deref(), Some("res://scripts/player.gd"));
        assert_eq!(h.line, Some(42));
    }

    #[test]
    fn go_ts_gradle_python() {
        assert_eq!(first("# pkg\n./main.go:10:2: undefined: foo").unwrap().line, Some(10));
        assert_eq!(first("src/a.ts(3,5): error TS2304: Cannot find name 'x'.").unwrap().file.as_deref(), Some("src/a.ts"));
        assert_eq!(first("e: file:///p/App.kt:12:5 Unresolved reference: foo").unwrap().file.as_deref(), Some("/p/App.kt"));
        let p = first("Traceback (most recent call last):\n  File \"a.py\", line 1, in <module>\n  File \"b.py\", line 9, in f\nValueError: x").unwrap();
        assert_eq!(p.file.as_deref(), Some("b.py"));
        assert_eq!(p.line, Some(9));
    }

    #[test]
    fn generic_and_enabled_filter() {
        assert_eq!(first("ok\nerror: linker failed").unwrap().index, 1);
        assert_eq!(first("ok\n  fatal: not a git repository").unwrap().matcher, "generic");
        let lines = vec!["error: thing"];
        assert!(find_first_error(&lines, &["csharp".into()], None).is_none());
        let re = Regex::new("BOOM (?P<file>\\S+):(?P<line>\\d+)").unwrap();
        let lines = vec!["fine", "BOOM a.c:3"];
        let h = find_first_error(&lines, &["csharp".into()], Some(&re)).unwrap();
        assert_eq!(h.matcher, "custom");
        assert_eq!(h.line, Some(3));
    }

    #[test]
    fn ansi_and_markers() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x"), "error: x");
        assert!(first("===== exit 1 · error: stopped =====").is_none());
    }

    #[test]
    fn newest_attempt_only() {
        let log = "===== step 2/2: import =====\nERROR: crash\n===== step 2/2: import (try 2 of 3) =====\nerror: real";
        let lines: Vec<&str> = log.lines().collect();
        let (off, sec) = failing_section(&lines, Some(2));
        assert_eq!(off, 2);
        let h = find_first_error(sec, &[], None).unwrap();
        assert_eq!(h.text, "error: real");
    }
}
