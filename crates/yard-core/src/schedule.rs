//! Schedules: when a task runs on its own.
//!
//! A schedule is written the way people say it (`hourly`, `03:00`,
//! `mon-fri 09:00`, `every 15m`) or as five cron fields, and every form
//! compiles to the same five fields. Times are local wall-clock times, as in
//! cron.

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// A parsed schedule. It keeps the text it was written as, which is what
/// boards show and what it serializes back to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Schedule {
    text: String,
    minutes: u64,
    hours: u32,
    /// Bits 1 to 31.
    days: u32,
    /// Bits 1 to 12.
    months: u16,
    /// Bits 0 to 6, Sunday is 0.
    weekdays: u8,
    /// Day of month and day of week fields that start with `*`. When both are
    /// restricted cron runs on either, otherwise on both.
    any_day: bool,
    any_weekday: bool,
}

impl TryFrom<String> for Schedule {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Schedule::parse(&s)
    }
}

impl From<Schedule> for String {
    fn from(s: Schedule) -> String {
        s.text
    }
}

impl Schedule {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Parses `hourly`, `daily`, `weekly`, `monthly`, `yearly` (with or
    /// without cron's `@`), `HH:MM`, `<days> HH:MM`, `every 15m`, `every 2h`
    /// or five cron fields. The error says what is wrong, without the text.
    pub fn parse(text: &str) -> Result<Schedule, String> {
        let t = text.trim();
        let cron = to_cron(&t.to_lowercase())?;
        let f: Vec<&str> = cron.split_whitespace().collect();
        if f.len() != 5 {
            return Err("not a schedule: use hourly, daily, 03:00, mon-fri 09:00, every 15m or five cron fields".into());
        }
        let s = Schedule {
            text: t.to_string(),
            minutes: field(f[0], 0, 59, &[])?,
            hours: field(f[1], 0, 23, &[])? as u32,
            days: field(f[2], 1, 31, &[])? as u32,
            months: field(f[3], 1, 12, &MONTHS)? as u16,
            weekdays: weekdays(f[4])?,
            any_day: f[2].starts_with('*'),
            any_weekday: f[4].starts_with('*'),
        };
        // "0 0 31 2 *" parses but never comes round.
        let probe = NaiveDate::from_ymd_opt(2001, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
        if s.next_naive(probe).is_none() {
            return Err("never comes round".into());
        }
        Ok(s)
    }

    fn day_matches(&self, d: NaiveDate) -> bool {
        let dom = self.days & (1 << d.day()) != 0;
        let dow = self.weekdays & (1 << d.weekday().num_days_from_sunday()) != 0;
        if self.any_day || self.any_weekday {
            dom && dow
        } else {
            dom || dow
        }
    }

    /// The first matching minute strictly after `after`, in wall-clock time.
    fn next_naive(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut t = after.with_second(0)?.with_nanosecond(0)? + TimeDelta::minutes(1);
        for _ in 0..100_000 {
            if self.months & (1 << t.month()) == 0 {
                let (y, m) = if t.month() == 12 { (t.year() + 1, 1) } else { (t.year(), t.month() + 1) };
                t = NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
            } else if !self.day_matches(t.date()) {
                t = t.date().succ_opt()?.and_hms_opt(0, 0, 0)?;
            } else if self.hours & (1 << t.hour()) == 0 {
                t = t.with_minute(0)? + TimeDelta::hours(1);
            } else if self.minutes & (1 << t.minute()) == 0 {
                t += TimeDelta::minutes(1);
            } else {
                return Some(t);
            }
        }
        None
    }

    /// The next time it fires after `after`. A wall-clock time that a
    /// daylight saving change skips is skipped; one it repeats fires once.
    pub fn next_after<Tz: TimeZone>(&self, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        let tz = after.timezone();
        let mut naive = after.naive_local();
        for _ in 0..1000 {
            let n = self.next_naive(naive)?;
            let at = match tz.from_local_datetime(&n) {
                LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(t),
                LocalResult::None => None,
            };
            match at {
                Some(t) if t > *after => return Some(t),
                _ => naive = n,
            }
        }
        None
    }

    /// [`Schedule::next_after`] on unix seconds in the local timezone.
    pub fn next_unix(&self, after: f64) -> Option<f64> {
        let t = chrono::Local.timestamp_opt(after.floor() as i64, 0).single()?;
        self.next_after(&t).map(|t| t.timestamp() as f64)
    }
}

/// How a board names the next run: "03:00" when it is less than a day away,
/// "Tue 03:00" within the week, "Dec 1" after that. Each form stays true
/// until the run happens, so a label sent once never needs correcting.
pub fn next_label(next: f64, now: f64) -> String {
    let Some(t) = chrono::Local.timestamp_opt(next as i64, 0).single() else { return String::new() };
    let ahead = next - now;
    if ahead < 23.0 * 3600.0 {
        t.format("%H:%M").to_string()
    } else if ahead < 6.0 * 86400.0 {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%b %-d").to_string()
    }
}

/// When a one-off run should happen, from now: `18:00` (the next one),
/// `today 18:00`, `tomorrow 08:00`, `fri 17:30` (the next Friday, today if
/// still ahead), `2026-11-24 08:00`, or `in 30m` / `in 2h` / `in 1d` (also
/// `+30m`). The error says what is wrong.
pub fn when_after<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Result<DateTime<Tz>, String> {
    let t = text.trim().to_lowercase();
    let bad = || format!("\"{}\" is not a time: use 18:00, tomorrow 08:00, fri 17:30, 2026-11-24 08:00 or in 30m", text.trim());
    if let Some(d) = t.strip_prefix("in ").or_else(|| t.strip_prefix('+')) {
        let d = d.trim();
        let split = d.find(|c: char| !c.is_ascii_digit()).unwrap_or(d.len());
        let n: i64 = d[..split].parse().map_err(|_| bad())?;
        let secs = match d[split..].trim() {
            "s" | "sec" | "secs" | "second" | "seconds" => n,
            "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
            "h" | "hr" | "hour" | "hours" => n * 3600,
            "d" | "day" | "days" => n * 86400,
            _ => return Err(bad()),
        };
        return Ok(now.clone() + TimeDelta::seconds(secs));
    }
    let tz = now.timezone();
    let local = |n: NaiveDateTime| match tz.from_local_datetime(&n) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(t),
        LocalResult::None => None,
    };
    if let Ok(n) = NaiveDateTime::parse_from_str(&t.replace('t', " "), "%Y-%m-%d %H:%M") {
        let at = local(n).ok_or_else(bad)?;
        if at <= *now {
            return Err(format!("{} is in the past", text.trim()));
        }
        return Ok(at);
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let (day, time) = match words.as_slice() {
        [time] => (None, *time),
        [day, time] => (Some(*day), *time),
        _ => return Err(bad()),
    };
    let (h, m) = clock(time).map_err(|_| bad())?;
    let today = now.naive_local().date();
    let at_day = |d: NaiveDate| d.and_hms_opt(h, m, 0).and_then(local);
    let ahead = |d: NaiveDate| at_day(d).filter(|a| a > now);
    match day {
        None => (0..2).filter_map(|i| ahead(today + TimeDelta::days(i))).next().ok_or_else(bad),
        Some("today") => ahead(today).ok_or_else(|| format!("{} has passed today", time)),
        Some("tomorrow") => at_day(today + TimeDelta::days(1)).ok_or_else(bad),
        Some(d) => {
            let want = WEEKDAYS.iter().position(|w| d.starts_with(w)).ok_or_else(bad)? as u32;
            (0..8)
                .map(|i| today + TimeDelta::days(i))
                .filter(|d| d.weekday().num_days_from_sunday() == want)
                .find_map(ahead)
                .ok_or_else(bad)
        }
    }
}

/// [`when_after`] in unix seconds, local time.
pub fn when_unix(text: &str, now: f64) -> Result<f64, String> {
    let n = chrono::Local.timestamp_opt(now.floor() as i64, 0).single().ok_or("the clock is out of range")?;
    when_after(text, &n).map(|t| t.timestamp() as f64)
}

fn to_cron(t: &str) -> Result<String, String> {
    let t = t.strip_prefix('@').unwrap_or(t);
    let fixed = match t {
        "hourly" => "0 * * * *",
        "daily" | "midnight" => "0 0 * * *",
        "weekly" => "0 0 * * 0",
        "monthly" => "0 0 1 * *",
        "yearly" | "annually" => "0 0 1 1 *",
        "reboot" => return Err("@reboot has no time of day; start it from your session's autostart instead".into()),
        _ => "",
    };
    if !fixed.is_empty() {
        return Ok(fixed.into());
    }
    if let Some(n) = t.strip_prefix("every ") {
        return every(n.trim());
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    match words.as_slice() {
        [time] if time.contains(':') => {
            let (h, m) = clock(time)?;
            Ok(format!("{m} {h} * * *"))
        }
        [days, time] if time.contains(':') => {
            let (h, m) = clock(time)?;
            let d = match *days {
                "daily" => "*",
                "weekdays" => "1-5",
                "weekends" => "0,6",
                d => d,
            };
            Ok(format!("{m} {h} * * {d}"))
        }
        _ => Ok(t.to_string()),
    }
}

fn clock(s: &str) -> Result<(u32, u32), String> {
    let bad = || format!("\"{s}\" is not a time like 03:00");
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    let h: u32 = h.parse().map_err(|_| bad())?;
    let m: u32 = m.parse().map_err(|_| bad())?;
    if h > 23 || m > 59 {
        return Err(bad());
    }
    Ok((h, m))
}

/// `every 15m` is cron's `*/15`: counted from the top of the hour, and
/// `every 2h` from midnight, so the times are the same every day.
fn every(n: &str) -> Result<String, String> {
    let split = n.find(|c: char| !c.is_ascii_digit()).unwrap_or(n.len());
    let bad = || format!("\"every {n}\": write it like every 15m or every 2h");
    let num = n[..split].parse::<u32>().ok().filter(|v| *v > 0).ok_or_else(bad)?;
    let mins = match n[split..].trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => num,
        "h" | "hr" | "hour" | "hours" => num.saturating_mul(60),
        _ => return Err(bad()),
    };
    if mins < 60 {
        Ok(format!("*/{mins} * * * *"))
    } else if mins.is_multiple_of(60) && mins / 60 < 24 {
        Ok(format!("0 */{} * * *", mins / 60))
    } else if mins == 1440 {
        Ok("0 0 * * *".into())
    } else {
        Err(format!("\"every {n}\" does not divide an hour or a day; write five cron fields instead"))
    }
}

fn field(s: &str, lo: u32, hi: u32, names: &[&str]) -> Result<u64, String> {
    let mut bits = 0u64;
    for part in s.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, st)) => (r, st.parse::<u32>().ok().filter(|v| *v > 0).ok_or_else(|| format!("bad step in \"{part}\""))?),
            None => (part, 1),
        };
        let (a, b) = if range == "*" {
            (lo, hi)
        } else if let Some((x, y)) = range.split_once('-') {
            (value(x, lo, hi, names)?, value(y, lo, hi, names)?)
        } else {
            let v = value(range, lo, hi, names)?;
            (v, if part.contains('/') { hi } else { v })
        };
        if a > b {
            return Err(format!("\"{part}\" runs backwards"));
        }
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok(bits)
}

fn value(s: &str, lo: u32, hi: u32, names: &[&str]) -> Result<u32, String> {
    let v = match s.parse::<u32>() {
        Ok(v) => v,
        Err(_) => names.iter().position(|n| *n == s).map(|i| i as u32 + lo).ok_or_else(|| format!("\"{s}\" is not a number"))?,
    };
    if v < lo || v > hi {
        return Err(format!("{v} is outside {lo}-{hi}"));
    }
    Ok(v)
}

/// Day of week, where 7 is Sunday as well as 0.
fn weekdays(s: &str) -> Result<u8, String> {
    let b = field(s, 0, 7, &WEEKDAYS)?;
    Ok(((b | (b >> 7)) & 0x7f) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    fn at(s: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap().and_utc()
    }

    fn next(sched: &str, from: &str) -> String {
        let s = Schedule::parse(sched).unwrap_or_else(|e| panic!("{sched}: {e}"));
        s.next_after(&at(from)).unwrap().format("%Y-%m-%d %H:%M %a").to_string()
    }

    #[test]
    fn spoken_forms() {
        // 2026-11-23 is a Monday.
        assert_eq!(next("hourly", "2026-11-23 08:31"), "2026-11-23 09:00 Mon");
        assert_eq!(next("@hourly", "2026-11-23 09:00"), "2026-11-23 10:00 Mon");
        assert_eq!(next("daily", "2026-11-23 08:31"), "2026-11-24 00:00 Tue");
        assert_eq!(next("03:00", "2026-11-23 02:59"), "2026-11-23 03:00 Mon");
        assert_eq!(next("03:00", "2026-11-23 03:00"), "2026-11-24 03:00 Tue");
        assert_eq!(next("daily 03:00", "2026-11-23 04:00"), "2026-11-24 03:00 Tue");
        assert_eq!(next("weekly", "2026-11-23 08:00"), "2026-11-29 00:00 Sun");
        assert_eq!(next("monthly", "2026-11-23 08:00"), "2026-12-01 00:00 Tue");
        assert_eq!(next("yearly", "2026-11-23 08:00"), "2027-01-01 00:00 Fri");
        assert_eq!(next("Mon-Fri 09:00", "2026-11-28 10:00"), "2026-11-30 09:00 Mon");
        assert_eq!(next("weekends 10:30", "2026-11-23 10:00"), "2026-11-28 10:30 Sat");
        assert_eq!(next("every 15m", "2026-11-23 08:31"), "2026-11-23 08:45 Mon");
        assert_eq!(next("every 2h", "2026-11-23 08:31"), "2026-11-23 10:00 Mon");
        assert_eq!(next("every 24h", "2026-11-23 08:31"), "2026-11-24 00:00 Tue");
    }

    #[test]
    fn cron_fields() {
        assert_eq!(next("55 2 * * *", "2026-11-23 08:31"), "2026-11-24 02:55 Tue");
        assert_eq!(next("*/20 9-17 * * mon-fri", "2026-11-23 17:45"), "2026-11-24 09:00 Tue");
        assert_eq!(next("0 0 * jan,jul *", "2026-11-23 08:00"), "2027-01-01 00:00 Fri");
        // Sunday is 0 or 7.
        assert_eq!(next("0 12 * * 7", "2026-11-23 08:00"), "2026-11-29 12:00 Sun");
        // Both day fields restricted: either one matches.
        assert_eq!(next("0 0 1 * fri", "2026-11-23 08:00"), "2026-11-27 00:00 Fri");
        // Only one restricted: both must.
        assert_eq!(next("0 0 */2 * *", "2026-11-23 08:00"), "2026-11-25 00:00 Wed");
        // A leap day comes round, eventually.
        assert_eq!(next("0 0 29 2 *", "2026-11-23 08:00"), "2028-02-29 00:00 Tue");
    }

    #[test]
    fn wall_clock_in_the_zone() {
        let cst = FixedOffset::west_opt(6 * 3600).unwrap();
        let from = cst.with_ymd_and_hms(2026, 11, 23, 2, 30, 0).unwrap();
        let s = Schedule::parse("03:00").unwrap();
        let n = s.next_after(&from).unwrap();
        assert_eq!(n.format("%H:%M").to_string(), "03:00");
        assert_eq!((n - from).num_minutes(), 30);
    }

    #[test]
    fn rejects() {
        for bad in [
            "",
            "sometimes",
            "0 0 * *",
            "61 * * * *",
            "0 25 * * *",
            "0 0 31 2 *",
            "@reboot",
            "every 90m",
            "every 0m",
            "25:00",
            "0 0 * * blursday",
            "5-1 * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?} parsed");
        }
        assert!(Schedule::parse("@reboot").unwrap_err().contains("autostart"));
    }

    fn when(text: &str, from: &str) -> String {
        when_after(text, &at(from)).unwrap_or_else(|e| panic!("{text}: {e}")).format("%Y-%m-%d %H:%M %a").to_string()
    }

    #[test]
    fn one_off_times() {
        // 2026-11-23 is a Monday.
        assert_eq!(when("18:00", "2026-11-23 09:00"), "2026-11-23 18:00 Mon");
        assert_eq!(when("08:00", "2026-11-23 09:00"), "2026-11-24 08:00 Tue");
        assert_eq!(when("today 18:00", "2026-11-23 09:00"), "2026-11-23 18:00 Mon");
        assert_eq!(when("tomorrow 08:00", "2026-11-23 09:00"), "2026-11-24 08:00 Tue");
        assert_eq!(when("Fri 17:30", "2026-11-23 09:00"), "2026-11-27 17:30 Fri");
        assert_eq!(when("monday 08:00", "2026-11-23 09:00"), "2026-11-30 08:00 Mon");
        assert_eq!(when("mon 10:00", "2026-11-23 09:00"), "2026-11-23 10:00 Mon");
        assert_eq!(when("2026-11-24 08:00", "2026-11-23 09:00"), "2026-11-24 08:00 Tue");
        assert_eq!(when("2026-11-24T08:00", "2026-11-23 09:00"), "2026-11-24 08:00 Tue");
        assert_eq!(when("in 30m", "2026-11-23 09:00"), "2026-11-23 09:30 Mon");
        assert_eq!(when("+2h", "2026-11-23 09:00"), "2026-11-23 11:00 Mon");
        assert_eq!(when("in 1d", "2026-11-23 09:00"), "2026-11-24 09:00 Tue");
        for bad in ["", "later", "today 08:00", "2026-11-22 08:00", "in 5 fortnights", "blursday 08:00", "25:00"] {
            assert!(when_after(bad, &at("2026-11-23 09:00")).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn keeps_its_text() {
        let s = Schedule::parse("  Mon-Fri 09:00 ").unwrap();
        assert_eq!(s.text(), "Mon-Fri 09:00");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"Mon-Fri 09:00\"");
        assert_eq!(serde_json::from_str::<Schedule>(&json).unwrap(), s);
        assert!(serde_json::from_str::<Schedule>("\"never\"").is_err());
    }
}
