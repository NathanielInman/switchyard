//! Filtering and ranking for the board's search and the Shelf. The Linux
//! board and the macOS and Windows tray both call these, so a query finds
//! the same things in the same order everywhere.

use crate::model::ShelfItem;

/// How well `needle` matches `hay`, or `None` when it doesn't. Every letter
/// of the needle has to appear in order (so "lf" finds "lanternfall"), and
/// higher scores go to, in order of weight:
///
/// - the whole needle as one run of text, more so at the start of a word
///   ("clip" in "Clipboard history" beats "clip" inside "eclipse");
/// - letters at the start of words ("ws" in "workspace switch");
/// - letters next to each other;
/// - a match that starts early.
///
/// An empty needle matches everything with score 0. Case is ignored.
pub fn score(hay: &str, needle: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let fuzzy = subsequence(&hay, &needle)?;
    let run = (0..hay.len().saturating_sub(needle.len() - 1))
        .filter(|&i| hay[i..i + needle.len()] == needle[..])
        .map(|i| if word_start(&hay, i) { 2000 } else { 1000 })
        .max()
        .unwrap_or(0);
    Some(run + fuzzy)
}

/// Shelf items matching `q` on their name and description, best first.
/// Ties keep their order, so an empty query lists everything as given.
pub fn ranked<'a>(items: &'a [ShelfItem], q: &str) -> Vec<&'a ShelfItem> {
    rank(items, q, |t| format!("{} {}", t.name, t.desc))
}

/// `items` matching `q` on the text `hay` gives for each, best first. The
/// sort is stable, so ties keep their order.
pub fn rank<'a, T>(items: &'a [T], q: &str, hay: impl Fn(&T) -> String) -> Vec<&'a T> {
    let mut hits: Vec<(i64, &T)> = items.iter().filter_map(|t| Some((score(&hay(t), q)?, t))).collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, t)| t).collect()
}

/// A Shelf description split at its first " · " into what the item does
/// and the search words after it: "Clipboard history · paste copy" gives
/// ("Clipboard history", "paste copy"). The words are "" when there are none.
pub fn split_tags(desc: &str) -> (&str, &str) {
    match desc.split_once(" · ") {
        Some((what, tags)) => (what.trim_end(), tags.trim()),
        None => (desc, ""),
    }
}

fn word_start(hay: &[char], i: usize) -> bool {
    i == 0 || !hay[i - 1].is_alphanumeric()
}

/// The best way to pick `needle`'s letters out of `hay` in order. Picking
/// the first letter found misses better matches later on ("ws" would take
/// the w in "show"), so this tries them all: best[i] is the top score with
/// the current needle letter at hay[i].
fn subsequence(hay: &[char], needle: &[char]) -> Option<i64> {
    const NONE: i64 = i64::MIN / 2;
    let letter = |i: usize| 1 + if word_start(hay, i) { 10 } else { 0 };
    let mut best: Vec<i64> = (0..hay.len()).map(|i| if hay[i] == needle[0] { letter(i) + 9 - (i as i64).min(9) } else { NONE }).collect();
    for &n in &needle[1..] {
        let mut next = vec![NONE; hay.len()];
        // The best score with the previous letter anywhere before i - 1.
        let mut before = NONE;
        for i in 1..hay.len() {
            if i >= 2 {
                before = before.max(best[i - 2]);
            }
            if hay[i] == n {
                let adjacent = if best[i - 1] > NONE { best[i - 1] + 8 } else { NONE };
                let from = before.max(adjacent);
                if from > NONE {
                    next[i] = from + letter(i);
                }
            }
        }
        best = next;
    }
    best.into_iter().filter(|s| *s > NONE).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, desc: &str) -> ShelfItem {
        ShelfItem { name: name.into(), desc: desc.into(), cmd: None }
    }

    fn names<'a>(items: &[&'a ShelfItem]) -> Vec<&'a str> {
        items.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn letters_in_order() {
        assert!(score("lanternfall", "lf").is_some());
        assert!(score("lanternfall", "xz").is_none());
        assert!(score("lanternfall", "fl").is_some());
        assert!(score("lanternfall", "lfz").is_none());
        assert_eq!(score("anything", ""), Some(0));
        assert!(score("Lanternfall", "LAN").is_some());
    }

    #[test]
    fn adjacent_letters_beat_scattered_ones() {
        assert!(score("lanternfall", "lan").unwrap() > score("lanternfall", "lnl").unwrap());
    }

    #[test]
    fn word_starts_beat_letters_inside_words() {
        // The w and s of "brownish" come first, but the later w and s start words.
        assert!(score("workspace switch", "ws").unwrap() > score("brownish", "ws").unwrap());
        assert!(score("brownish · workspace switch", "ws").unwrap() > score("brownish", "ws").unwrap());
        assert!(score("Clipboard history", "clip").unwrap() > score("eclipse", "clip").unwrap());
    }

    #[test]
    fn a_run_of_text_beats_any_scatter() {
        let binds = vec![
            item("Super + 1", "Go to workspace 1 · switch desktop"),
            item("Super + Alt + V", "Clipboard history · paste copy"),
            item("Super + Q", "Close window · quit"),
        ];
        assert_eq!(names(&ranked(&binds, "paste")), ["Super + Alt + V", "Super + 1"]);
        assert_eq!(names(&ranked(&binds, "")), ["Super + 1", "Super + Alt + V", "Super + Q"]);
    }

    #[test]
    fn rank_keeps_ties_in_order() {
        let words = ["alpha one", "alpha two", "beta"];
        assert_eq!(rank(&words, "alpha", |w| w.to_string()), [&"alpha one", &"alpha two"]);
    }

    #[test]
    fn tags_split_at_the_dot() {
        assert_eq!(split_tags("Clipboard history · paste copy yank"), ("Clipboard history", "paste copy yank"));
        assert_eq!(split_tags("Close window"), ("Close window", ""));
        assert_eq!(split_tags("Go to workspace 1 · switch · desktop"), ("Go to workspace 1", "switch · desktop"));
    }

    #[test]
    fn needle_longer_than_hay() {
        assert!(score("ab", "abc").is_none());
    }
}
