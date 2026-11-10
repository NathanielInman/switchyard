//! Run history: one record per finished run, and the views built from it.
//!
//! A history should answer questions, not store output: when did this start
//! breaking and what fixed it, is it getting slower, what did I spend today
//! waiting on. So runs fold into **episodes** (twelve green runs are one line;
//! a failure streak followed by a pass is one "broken, then fixed" line), and
//! the week folds into four numbers.

use crate::errors::ErrorHit;
use crate::model::{duration, Outcome};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepTime {
    pub name: String,
    pub secs: f64,
    /// More than 1 when the retry policy had to step in.
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GitRef {
    pub sha: String,
    pub branch: String,
    pub dirty: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub run: u64,
    pub task: String,
    pub label: String,
    /// When it was asked for (unix seconds).
    pub requested: f64,
    pub started: f64,
    pub ended: f64,
    pub outcome: Outcome,
    #[serde(default)]
    pub code: i32,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<ErrorHit>,
    #[serde(default)]
    pub steps: Vec<StepTime>,
    /// Time in the main-line queue, memory waits included.
    #[serde(default)]
    pub queued_secs: f64,
    #[serde(default)]
    pub build_secs: f64,
    #[serde(default)]
    pub run_secs: f64,
    #[serde(default)]
    pub peak_mem: Option<u64>,
    #[serde(default)]
    pub mem_cap: Option<u64>,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub git: Option<GitRef>,
    /// This run's own copy of its log, when one was kept.
    #[serde(default)]
    pub log: Option<String>,
}

impl RunRecord {
    pub fn retried(&self) -> bool {
        self.steps.iter().any(|s| s.attempts > 1)
    }

    /// The time that matters for "is it getting slower": the build when there
    /// is one, otherwise the whole run.
    pub fn work_secs(&self) -> f64 {
        if self.build_secs > 0.0 {
            self.build_secs
        } else {
            (self.ended - self.started).max(0.0)
        }
    }

    /// "go", "stop" or "retry": how a run is drawn.
    pub fn mark(&self) -> &'static str {
        match self.outcome {
            Outcome::Failed => "stop",
            _ if self.retried() => "retry",
            _ => "go",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Lamp {
    Go,
    Stop,
    /// Broken, then fixed: drawn half red, half green.
    Fix,
    Caution,
    Frost,
}

/// One line of the feed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Episode {
    pub task: String,
    pub who: String,
    pub lamp: Lamp,
    pub what: String,
    pub sub: String,
    /// A highlighted chip: text and kind ("warn", "ok", "bad").
    pub chip: Option<(String, String)>,
    pub when: String,
    pub marks: Vec<String>,
    pub detail: String,
    pub runs: Vec<u64>,
    pub ended: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Day {
    pub label: String,
    pub episodes: Vec<Episode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WeekStats {
    pub building_secs: f64,
    pub waiting_secs: f64,
    pub runs: usize,
    pub failures: usize,
    /// "task step" with the most retries, and how many.
    pub flakiest: Option<(String, u32)>,
    /// The task whose work time grew the most against its earlier runs.
    pub slower: Option<(String, f64)>,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(v[v.len() / 2])
}

/// Hours and minutes in local time, "13:40".
pub fn clock_time(t: f64) -> String {
    let (y, m, d, h, min) = local_parts(t);
    let _ = (y, m, d);
    format!("{h}:{min:02}")
}

/// Local calendar parts without a timezone library: the daemon's notion of
/// local time comes in as an offset (`TZ_OFFSET`, set by [`set_local_offset`]).
fn local_parts(t: f64) -> (i64, u32, u32, u32, u32) {
    let secs = t as i64 + LOCAL_OFFSET.load(std::sync::atomic::Ordering::Relaxed);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil(days);
    (y, m, d, (rem / 3600) as u32, ((rem / 60) % 60) as u32)
}

static LOCAL_OFFSET: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Seconds east of UTC, so days break at local midnight.
pub fn set_local_offset(secs: i64) {
    LOCAL_OFFSET.store(secs, std::sync::atomic::Ordering::Relaxed);
}

fn civil(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "today", "yday", "mon": a short day name for chart labels.
pub fn short_day(t: f64, now: f64) -> String {
    match day_index(now) - day_index(t) {
        0 => "today".into(),
        1 => "yday".into(),
        _ => ["thu", "fri", "sat", "sun", "mon", "tue", "wed"][day_index(t).rem_euclid(7) as usize].into(),
    }
}

fn day_index(t: f64) -> i64 {
    (t as i64 + LOCAL_OFFSET.load(std::sync::atomic::Ordering::Relaxed)).div_euclid(86400)
}

fn day_label(day: i64, today: i64) -> String {
    match today - day {
        0 => "TODAY".into(),
        1 => "YESTERDAY".into(),
        n if n < 7 => ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"][day.rem_euclid(7) as usize].into(),
        _ => {
            let (_, m, d) = civil(day);
            let months = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
            format!("{} {d}", months[(m - 1) as usize])
        }
    }
}

fn short(sha: &Option<GitRef>) -> Option<String> {
    sha.as_ref().filter(|g| !g.sha.is_empty()).map(|g| g.sha.chars().take(7).collect())
}

/// The step that grew the most between two sets of runs, with the growth.
fn grown_step(before: &[&RunRecord], now: &[&RunRecord]) -> Option<(String, f64)> {
    let per_step = |rs: &[&RunRecord]| {
        let mut m: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rs {
            for s in &r.steps {
                m.entry(s.name.clone()).or_default().push(s.secs);
            }
        }
        m.into_iter().filter_map(|(k, v)| median(v).map(|x| (k, x))).collect::<BTreeMap<_, _>>()
    };
    let (a, b) = (per_step(before), per_step(now));
    b.iter()
        .filter_map(|(k, v)| a.get(k).filter(|x| **x > 1.0).map(|x| (k.clone(), v / x - 1.0)))
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
}

/// Folds records (any order) into episodes grouped by local day, newest first.
pub fn episodes(records: &[RunRecord], now: f64) -> Vec<Day> {
    let mut by_task: BTreeMap<&str, Vec<&RunRecord>> = BTreeMap::new();
    for r in records {
        by_task.entry(&r.task).or_default().push(r);
    }
    let mut eps: Vec<Episode> = vec![];
    for (task, mut runs) in by_task {
        runs.sort_by(|a, b| a.ended.partial_cmp(&b.ended).unwrap());
        // Split into same-day stretches, then into streaks inside a day.
        let mut i = 0;
        while i < runs.len() {
            let day = day_index(runs[i].ended);
            let mut j = i;
            while j < runs.len() && day_index(runs[j].ended) == day {
                j += 1;
            }
            let today_runs = &runs[i..j];
            let earlier: Vec<&RunRecord> = runs[..i].iter().rev().filter(|r| r.outcome == Outcome::Ok).take(10).copied().collect();
            let mut k = 0;
            while k < today_runs.len() {
                let failing = today_runs[k].outcome == Outcome::Failed;
                let mut e = k;
                while e < today_runs.len() && (today_runs[e].outcome == Outcome::Failed) == failing {
                    e += 1;
                }
                let streak = &today_runs[k..e];
                eps.push(if failing {
                    let fix = today_runs.get(e).copied();
                    let consumed = if fix.is_some() { e + 1 } else { e };
                    let ep = broken(task, streak, fix, now);
                    k = consumed;
                    ep
                } else {
                    let ep = green(task, streak, &earlier);
                    k = e;
                    ep
                });
            }
            i = j;
        }
    }
    eps.sort_by(|a, b| b.ended.partial_cmp(&a.ended).unwrap());
    let today = day_index(now);
    let mut days: Vec<Day> = vec![];
    for e in eps {
        let label = day_label(day_index(e.ended), today);
        match days.last_mut() {
            Some(d) if d.label == label => d.episodes.push(e),
            _ => days.push(Day { label, episodes: vec![e] }),
        }
    }
    days
}

fn broken(task: &str, fails: &[&RunRecord], fix: Option<&RunRecord>, now: f64) -> Episode {
    let first = fails[0];
    let last = fails[fails.len() - 1];
    let err = fails
        .iter()
        .rev()
        .find_map(|r| r.error.as_ref())
        .map(|e| {
            let loc = match (&e.file, e.line) {
                (Some(f), Some(l)) => format!(" {}:{l}", f.rsplit(['/', '\\']).next().unwrap_or(f)),
                _ => String::new(),
            };
            match error_code(&e.text) {
                Some(c) => format!("{c}{loc}"),
                None if !loc.is_empty() => loc.trim().to_string(),
                None => e.text.chars().take(60).collect::<String>(),
            }
        })
        .or_else(|| last.message.clone())
        .unwrap_or_else(|| "failed".into());
    let n = fails.len();
    let fails_txt = if n == 1 { "1 fail".to_string() } else { format!("{n} fails") };
    let mut marks: Vec<String> = fails.iter().map(|r| r.mark().to_string()).collect();
    let mut runs: Vec<u64> = fails.iter().map(|r| r.run).collect();
    match fix {
        Some(f) => {
            marks.push(f.mark().into());
            runs.push(f.run);
            let mins = ((f.ended - first.ended) / 60.0).round().max(1.0);
            let by = short(&f.git).map(|s| format!(", fixed by {s}")).unwrap_or_else(|| ", then passed".into());
            Episode {
                task: task.into(),
                who: f.label.clone(),
                lamp: Lamp::Fix,
                what: format!("broken for {}{by}", duration(mins * 60.0).replace(" 0s", "")),
                sub: format!("{fails_txt} · {err}"),
                chip: None,
                when: clock_time(f.ended),
                marks,
                runs,
                detail: format!(
                    "First failure at {}{}. Passed again at {}.",
                    clock_time(first.ended),
                    short(&first.git).map(|s| format!(" on {s}")).unwrap_or_default(),
                    clock_time(f.ended)
                ),
                ended: f.ended,
            }
        }
        None => Episode {
            task: task.into(),
            who: last.label.clone(),
            lamp: Lamp::Stop,
            what: if n == 1 { "failed".into() } else { format!("failing, {n} in a row") },
            sub: err,
            chip: Some(("still red".into(), "bad".into())),
            when: clock_time(last.ended),
            marks,
            runs,
            detail: format!(
                "Failing since {} ({} ago). {}",
                clock_time(first.ended),
                duration((now - first.ended).max(0.0)),
                last.message.clone().unwrap_or_default()
            ),
            ended: last.ended,
        },
    }
}

/// "CS0103", "TS2339", "E0425": the short code a compiler gives an error.
pub fn error_code(text: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\b([A-Z]{1,4}\d{3,5})\b").unwrap());
    re.captures(text).map(|c| c[1].to_string())
}

fn green(task: &str, runs: &[&RunRecord], earlier: &[&RunRecord]) -> Episode {
    let last = runs[runs.len() - 1];
    let n = runs.len();
    let stopped = runs.iter().all(|r| r.outcome == Outcome::Stopped);
    let med = median(runs.iter().map(|r| r.work_secs()).collect()).unwrap_or(0.0);
    let usual = median(earlier.iter().map(|r| r.work_secs()).collect());
    let retries: u32 = runs.iter().flat_map(|r| r.steps.iter()).map(|s| s.attempts.saturating_sub(1)).sum();
    let queued: f64 = runs.iter().map(|r| r.queued_secs).sum();
    let what = if stopped {
        if n == 1 { "ran, stopped by hand".into() } else { format!("{n} sessions") }
    } else if n == 1 {
        "ok".into()
    } else {
        format!("{n} ok")
    };
    let mut chip = None;
    let mut lamp = if stopped { Lamp::Frost } else { Lamp::Go };
    let mut sub = if stopped {
        let ran = median(runs.iter().map(|r| r.run_secs.max(r.ended - r.started)).collect()).unwrap_or(0.0);
        format!("ran {}", duration(ran))
    } else if med > 0.5 {
        format!("{} median", duration(med))
    } else {
        String::new()
    };
    let mut detail = String::new();
    if let Some(u) = usual.filter(|u| *u > 2.0) {
        let growth = med / u - 1.0;
        if growth >= 0.2 && n >= 2 && !stopped {
            let step = grown_step(earlier, runs).filter(|(_, g)| *g > 0.15);
            sub = format!("usually {}, now {}", duration(u), duration(med));
            chip = Some((
                format!("+{:.0}%{}", growth * 100.0, step.as_ref().map(|(s, _)| format!(" {s}")).unwrap_or_default()),
                "warn".into(),
            ));
            lamp = Lamp::Caution;
            detail = match &step {
                Some((s, g)) => format!("The {s} step is {:.0}% slower than in the runs before.", g * 100.0),
                None => "Every run took longer than the ones before.".into(),
            };
        } else if growth <= -0.2 && n >= 2 {
            chip = Some((format!("{:.0}% faster", -growth * 100.0), "ok".into()));
        }
    }
    if let Some((k, v)) = last.params.iter().next() {
        sub = if sub.is_empty() { format!("{k} {v}") } else { format!("{sub} · {k} {v}") };
    }
    if retries > 0 {
        let s = if retries == 1 { "retry" } else { "retries" };
        if detail.is_empty() {
            detail = format!("{retries} {s}: a step crashed and passed when tried again.");
        }
        if chip.is_none() {
            chip = Some((format!("{retries} {s}"), "warn".into()));
        }
    }
    if queued >= 60.0 {
        let q = format!("waited {} on the main line", duration(queued));
        if chip.is_none() && n == 1 {
            lamp = Lamp::Caution;
            chip = Some((q.clone(), "warn".into()));
        }
        detail = if detail.is_empty() { format!("{q}.") } else { format!("{detail} {q}.") };
    }
    if detail.is_empty() {
        detail = match (runs[0].git.as_ref(), last.git.as_ref()) {
            (Some(a), Some(b)) if a.sha != b.sha => format!("{} to {} on {}.", &a.sha[..a.sha.len().min(7)], &b.sha[..b.sha.len().min(7)], b.branch),
            (_, Some(b)) if !b.sha.is_empty() => format!("{} on {}.", &b.sha[..b.sha.len().min(7)], b.branch),
            _ => "Nothing unusual.".into(),
        };
    }
    Episode {
        task: task.into(),
        who: last.label.clone(),
        lamp,
        what,
        sub,
        chip,
        when: if n == 1 { clock_time(last.ended) } else { format!("since {}", clock_time(runs[0].started)) },
        marks: runs.iter().map(|r| r.mark().to_string()).collect(),
        runs: runs.iter().map(|r| r.run).collect(),
        detail,
        ended: last.ended,
    }
}

/// The week in four numbers.
pub fn week_stats(records: &[RunRecord], now: f64) -> WeekStats {
    let week: Vec<&RunRecord> = records.iter().filter(|r| now - r.ended < 7.0 * 86400.0).collect();
    let mut s = WeekStats { runs: week.len(), ..Default::default() };
    let mut retries: BTreeMap<String, u32> = BTreeMap::new();
    for r in &week {
        s.building_secs += r.build_secs;
        s.waiting_secs += r.queued_secs;
        if r.outcome == Outcome::Failed {
            s.failures += 1;
        }
        for st in &r.steps {
            if st.attempts > 1 {
                *retries.entry(format!("{} {}", r.label, st.name)).or_default() += st.attempts - 1;
            }
        }
    }
    s.flakiest = retries.into_iter().max_by_key(|(_, n)| *n);
    let mut by_task: BTreeMap<&str, Vec<&RunRecord>> = BTreeMap::new();
    for r in records.iter().filter(|r| r.outcome == Outcome::Ok) {
        by_task.entry(&r.label).or_default().push(r);
    }
    s.slower = by_task
        .into_iter()
        .filter_map(|(label, mut rs)| {
            rs.sort_by(|a, b| a.ended.partial_cmp(&b.ended).unwrap());
            if rs.len() < 6 {
                return None;
            }
            let split = rs.len() * 2 / 3;
            let before = median(rs[..split].iter().map(|r| r.work_secs()).collect())?;
            let after = median(rs[split..].iter().map(|r| r.work_secs()).collect())?;
            (before > 2.0).then(|| (label.to_string(), after / before - 1.0))
        })
        .filter(|(_, g)| *g >= 0.15)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(run: u64, task: &str, ended: f64, outcome: Outcome, build: f64) -> RunRecord {
        RunRecord {
            run,
            task: task.into(),
            label: task.into(),
            requested: ended - build,
            started: ended - build,
            ended,
            outcome,
            code: if outcome == Outcome::Failed { 1 } else { 0 },
            message: None,
            error: None,
            steps: vec![StepTime { name: "build".into(), secs: build, attempts: 1 }],
            queued_secs: 0.0,
            build_secs: build,
            run_secs: 0.0,
            peak_mem: None,
            mem_cap: None,
            params: BTreeMap::new(),
            variant: None,
            git: Some(GitRef { sha: format!("{run:07}"), branch: "main".into(), dirty: 0 }),
            log: None,
        }
    }

    const DAY: f64 = 86400.0;

    #[test]
    fn folds_greens_and_fixes() {
        let t0 = 100.0 * DAY + 9.0 * 3600.0;
        let mut rs = vec![];
        for i in 0..5 {
            rs.push(rec(i, "api", t0 + i as f64 * 600.0, Outcome::Ok, 100.0));
        }
        for i in 5..8 {
            rs.push(rec(i, "api", t0 + i as f64 * 600.0, Outcome::Failed, 30.0));
        }
        rs.push(rec(8, "api", t0 + 8.0 * 600.0, Outcome::Ok, 100.0));
        let days = episodes(&rs, t0 + DAY / 3.0);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].label, "TODAY");
        let eps = &days[0].episodes;
        assert_eq!(eps.len(), 2, "{eps:#?}");
        assert_eq!(eps[0].lamp, Lamp::Fix);
        assert!(eps[0].what.contains("fixed by 0000008"), "{}", eps[0].what);
        assert_eq!(eps[0].marks, vec!["stop", "stop", "stop", "go"]);
        assert_eq!(eps[1].what, "5 ok");
    }

    #[test]
    fn flags_slowdowns_and_open_failures() {
        let t0 = 200.0 * DAY + 9.0 * 3600.0;
        let mut rs = vec![];
        for i in 0..6 {
            rs.push(rec(i, "web", t0 - DAY + i as f64 * 600.0, Outcome::Ok, 100.0));
        }
        for i in 6..10 {
            rs.push(rec(i, "web", t0 + i as f64 * 600.0, Outcome::Ok, 140.0));
        }
        rs.push(rec(10, "docs", t0 + 3600.0, Outcome::Failed, 5.0));
        let days = episodes(&rs, t0 + 4.0 * 3600.0);
        assert_eq!(days[0].label, "TODAY");
        let web = days[0].episodes.iter().find(|e| e.task == "web").unwrap();
        assert_eq!(web.lamp, Lamp::Caution);
        assert!(web.chip.as_ref().unwrap().0.starts_with("+40%"), "{:?}", web.chip);
        let docs = days[0].episodes.iter().find(|e| e.task == "docs").unwrap();
        assert_eq!(docs.lamp, Lamp::Stop);
        assert_eq!(days[1].label, "YESTERDAY");
        let w = week_stats(&rs, t0 + 4.0 * 3600.0);
        assert_eq!(w.runs, 11);
        assert_eq!(w.failures, 1);
        assert!(w.slower.unwrap().1 > 0.3);
    }

    #[test]
    fn codes() {
        assert_eq!(error_code("src/a.ts(3,5): error TS2339: nope").as_deref(), Some("TS2339"));
        assert_eq!(error_code("error[E0425]: cannot find").as_deref(), Some("E0425"));
        assert_eq!(error_code("plain failure"), None);
    }

    #[test]
    fn calendar() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_402), (2025, 11, 10));
        set_local_offset(0);
        assert_eq!(clock_time(3600.0 * 13.0 + 40.0 * 60.0), "13:40");
        assert_eq!(day_label(5, 5), "TODAY");
        assert_eq!(day_label(4, 5), "YESTERDAY");
    }
}
