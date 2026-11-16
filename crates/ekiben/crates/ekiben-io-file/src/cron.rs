//! Cron schedules of the `generate` input: the robfig/cron v3 parser Bento builds with
//! `SecondOptional | Minute | Hour | Dom | Month | Dow | Descriptor` (`input_generate.go`
//! `parseCronExpression`): five or six fields, `@yearly`-style descriptors, `@every <duration>`
//! and a `TZ=<IANA name>` prefix (UTC when absent).

use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use ekiben_core::config::parse_duration_str;

const STAR: u64 = 1 << 63;

#[derive(Clone, Debug)]
pub enum Schedule {
    /// `@every <d>`: `d` rounded down to whole seconds, at least one.
    Every(Duration),
    Spec(Box<Spec>),
}

#[derive(Clone, Debug)]
pub struct Spec {
    second: u64,
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    tz: Tz,
}

struct Bounds {
    min: u32,
    max: u32,
    names: &'static [&'static str],
}

const SECONDS: Bounds = Bounds {
    min: 0,
    max: 59,
    names: &[],
};
const MINUTES: Bounds = SECONDS;
const HOURS: Bounds = Bounds {
    min: 0,
    max: 23,
    names: &[],
};
const DOM: Bounds = Bounds {
    min: 1,
    max: 31,
    names: &[],
};
const MONTHS: Bounds = Bounds {
    min: 1,
    max: 12,
    names: &[
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ],
};
const DOW: Bounds = Bounds {
    min: 0,
    max: 6,
    names: &["sun", "mon", "tue", "wed", "thu", "fri", "sat"],
};

/// Parses an `interval` that is not a duration, with robfig's error wording.
pub fn parse(expr: &str) -> Result<Schedule, String> {
    let (tz, spec) = match expr.strip_prefix("TZ=") {
        Some(rest) => {
            let (name, spec) = rest.split_once(' ').unwrap_or((rest, ""));
            let tz = name
                .parse::<Tz>()
                .map_err(|_| format!("provided bad location {name}: unknown time zone {name}"))?;
            (tz, spec.trim())
        }
        None => (Tz::UTC, expr.trim()),
    };
    if let Some(desc) = spec.strip_prefix('@') {
        return descriptor(desc, tz);
    }
    let mut fields: Vec<&str> = spec.split_whitespace().collect();
    if !(5..=6).contains(&fields.len()) {
        return Err(format!(
            "expected 5 to 6 fields, found {}: [{}]",
            fields.len(),
            fields.join(" ")
        ));
    }
    if fields.len() == 5 {
        fields.insert(0, "0");
    }
    Ok(Schedule::Spec(Box::new(Spec {
        second: field(fields[0], &SECONDS)?,
        minute: field(fields[1], &MINUTES)?,
        hour: field(fields[2], &HOURS)?,
        dom: field(fields[3], &DOM)?,
        month: field(fields[4], &MONTHS)?,
        dow: field(fields[5], &DOW)?,
        tz,
    })))
}

fn descriptor(desc: &str, tz: Tz) -> Result<Schedule, String> {
    let all = |b: &Bounds| bits(b.min, b.max, 1) | STAR;
    let spec = |dom: u64, month: u64, dow: u64, hour: u64| {
        Ok(Schedule::Spec(Box::new(Spec {
            second: 1,
            minute: 1,
            hour,
            dom,
            month,
            dow,
            tz,
        })))
    };
    match desc {
        "yearly" | "annually" => spec(1 << 1, 1 << 1, all(&DOW), 1),
        "monthly" => spec(1 << 1, all(&MONTHS), all(&DOW), 1),
        "weekly" => spec(all(&DOM), all(&MONTHS), 1, 1),
        "daily" | "midnight" => spec(all(&DOM), all(&MONTHS), all(&DOW), 1),
        "hourly" => spec(all(&DOM), all(&MONTHS), all(&DOW), all(&HOURS)),
        _ => match desc.strip_prefix("every ") {
            Some(d) => {
                let d = parse_duration_str(d.trim()).map_err(|e| {
                    format!("failed to parse duration @{desc}: {}", e.to_string().trim())
                })?;
                Ok(Schedule::Every(Duration::from_secs(d.as_secs().max(1))))
            }
            None => Err(format!("unrecognized descriptor: @{desc}")),
        },
    }
}

fn field(expr: &str, b: &Bounds) -> Result<u64, String> {
    expr.split(',').try_fold(0, |acc, r| Ok(acc | range(r, b)?))
}

fn range(expr: &str, b: &Bounds) -> Result<u64, String> {
    let (lo_hi, step) = match expr.split_once('/') {
        Some((lh, s)) if !s.contains('/') => (lh, Some(s)),
        Some(_) => return Err(format!("too many slashes: {expr}")),
        None => (expr, None),
    };
    let (start, mut end, mut extra) = if lo_hi == "*" || lo_hi == "?" {
        (b.min, b.max, STAR)
    } else {
        match lo_hi.split('-').collect::<Vec<_>>()[..] {
            [one] => {
                let v = int_or_name(one, b)?;
                (v, v, 0)
            }
            [lo, hi] => (int_or_name(lo, b)?, int_or_name(hi, b)?, 0),
            _ => return Err(format!("too many hyphens: {expr}")),
        }
    };
    let step = match step {
        None => 1,
        Some(s) => {
            let s = positive_int(s)?;
            if !lo_hi.contains('-') {
                end = b.max;
            }
            if s > 1 {
                extra = 0;
            }
            s
        }
    };
    if start < b.min {
        return Err(format!(
            "beginning of range ({start}) below minimum ({}): {expr}",
            b.min
        ));
    }
    if end > b.max {
        return Err(format!(
            "end of range ({end}) above maximum ({}): {expr}",
            b.max
        ));
    }
    if start > end {
        return Err(format!(
            "beginning of range ({start}) beyond end of range ({end}): {expr}"
        ));
    }
    if step == 0 {
        return Err(format!("step of range should be a positive number: {expr}"));
    }
    Ok(bits(start, end, step) | extra)
}

fn int_or_name(s: &str, b: &Bounds) -> Result<u32, String> {
    let lower = s.to_ascii_lowercase();
    if let Some(i) = b.names.iter().position(|n| *n == lower) {
        return Ok(i as u32 + b.min);
    }
    positive_int(s)
}

fn positive_int(s: &str) -> Result<u32, String> {
    let v: i64 = s.parse().map_err(|_| {
        format!("failed to parse int from {s}: strconv.Atoi: parsing {s:?}: invalid syntax")
    })?;
    u32::try_from(v).map_err(|_| format!("negative number ({v}) not allowed: {s}"))
}

fn bits(min: u32, max: u32, step: u32) -> u64 {
    (min..=max.min(62))
        .step_by(step as usize)
        .fold(0, |acc, i| acc | 1 << i)
}

fn has(set: u64, v: u32) -> bool {
    set & (1 << v) != 0
}

impl Schedule {
    /// The first activation strictly after `now`; `None` when a spec never fires within five years.
    pub fn next(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Every(d) => {
                let d = TimeDelta::from_std(*d).ok()?;
                Some(now + d - TimeDelta::nanoseconds(i64::from(now.nanosecond())))
            }
            Schedule::Spec(s) => s.next(now),
        }
    }
}

impl Spec {
    fn day_matches(&self, t: &NaiveDateTime) -> bool {
        let dom = has(self.dom, t.day());
        let dow = has(self.dow, t.weekday().num_days_from_sunday());
        if self.dom & STAR != 0 || self.dow & STAR != 0 {
            dom && dow
        } else {
            dom || dow
        }
    }

    fn next(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local = now.with_timezone(&self.tz).naive_local();
        let mut t = local.with_nanosecond(0)? + TimeDelta::seconds(1);
        let limit = t.year() + 5;
        while t.year() <= limit {
            if !has(self.month, t.month()) {
                let (y, m) = if t.month() == 12 {
                    (t.year() + 1, 1)
                } else {
                    (t.year(), t.month() + 1)
                };
                t = chrono::NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.day_matches(&t) {
                t = t.date().succ_opt()?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !has(self.hour, t.hour()) {
                t = t.with_minute(0)?.with_second(0)? + TimeDelta::hours(1);
                continue;
            }
            if !has(self.minute, t.minute()) {
                t = t.with_second(0)? + TimeDelta::minutes(1);
                continue;
            }
            if !has(self.second, t.second()) {
                t += TimeDelta::seconds(1);
                continue;
            }
            // A local time inside a DST gap does not exist: skip it; a repeated one fires once.
            match self.tz.from_local_datetime(&t).earliest() {
                Some(at) if at.with_timezone(&Utc) > now => return Some(at.with_timezone(&Utc)),
                _ => t += TimeDelta::seconds(1),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("rfc3339")
            .with_timezone(&Utc)
    }

    fn next(expr: &str, from: &str) -> String {
        parse(expr)
            .expect("parses")
            .next(at(from))
            .expect("fires")
            .to_rfc3339()
    }

    // Ported from robfig/cron v3 spec_test.go TestActivation and TestNext (UTC rows).
    #[test]
    fn next_activation_matches_robfig() {
        for (from, expr, want) in [
            (
                "2012-07-09T14:45:00Z",
                "0 0/15 * * * *",
                "2012-07-09T15:00:00+00:00",
            ),
            (
                "2012-07-09T14:59:59Z",
                "0 0/15 * * * *",
                "2012-07-09T15:00:00+00:00",
            ),
            (
                "2012-07-09T14:59:00Z",
                "0 0/15 * * * *",
                "2012-07-09T15:00:00+00:00",
            ),
            (
                "2012-07-09T23:46:00Z",
                "0 */15 * * * *",
                "2012-07-10T00:00:00+00:00",
            ),
            (
                "2012-07-09T23:45:00Z",
                "0 20-35/15 * * * *",
                "2012-07-10T00:20:00+00:00",
            ),
            (
                "2012-07-09T23:35:51Z",
                "15/35 20-35/15 * * * *",
                "2012-07-10T00:20:15+00:00",
            ),
            (
                "2012-07-09T23:35:51Z",
                "15/35 20-35/15 10-12 * * *",
                "2012-07-10T10:20:15+00:00",
            ),
            (
                "2012-07-09T23:35:51Z",
                "15/35 20-35/15 * 9-20 * *",
                "2012-07-10T00:20:15+00:00",
            ),
            (
                "2012-07-09T23:35:51Z",
                "15/35 20-35/15 * 9-20 Jul *",
                "2012-07-10T00:20:15+00:00",
            ),
            (
                "2012-07-09T23:35:00Z",
                "0 0 0 9 Apr-Oct ?",
                "2012-08-09T00:00:00+00:00",
            ),
            (
                "2012-07-09T23:35:00Z",
                "0 0 0 */5 Apr,Aug,Oct Mon",
                "2012-08-01T00:00:00+00:00",
            ),
            (
                "2012-07-09T23:35:00Z",
                "0 0 0 */5 Oct Mon",
                "2012-10-01T00:00:00+00:00",
            ),
            (
                "2012-07-09T23:35:00Z",
                "0 0 0 * Feb Mon",
                "2013-02-04T00:00:00+00:00",
            ),
            (
                "2012-07-09T23:35:00Z",
                "0 0 0 * Feb Mon/2",
                "2013-02-01T00:00:00+00:00",
            ),
            (
                "2012-12-31T23:59:45Z",
                "0 * * * * *",
                "2013-01-01T00:00:00+00:00",
            ),
            (
                "2012-01-01T00:00:00Z",
                "0 0 0 29 Feb ?",
                "2012-02-29T00:00:00+00:00",
            ),
            (
                "2012-02-29T00:00:00Z",
                "0 0 0 29 Feb ?",
                "2016-02-29T00:00:00+00:00",
            ),
            (
                "2012-07-15T00:00:00Z",
                "0 0 0 1,15 * Sun",
                "2012-07-22T00:00:00+00:00",
            ),
            (
                "2012-07-15T00:00:00Z",
                "0 0 0 1,15 * *",
                "2012-08-01T00:00:00+00:00",
            ),
            (
                "2012-07-15T00:00:00Z",
                "0 0 0 * * Sun",
                "2012-07-22T00:00:00+00:00",
            ),
            (
                "2012-07-09T14:45:00Z",
                "*/15 * * * *",
                "2012-07-09T15:00:00+00:00",
            ),
        ] {
            assert_eq!(next(expr, from), want, "{expr} after {from}");
        }
    }

    #[test]
    fn descriptors_and_every() {
        assert_eq!(
            next("@hourly", "2012-07-09T14:45:00Z"),
            "2012-07-09T15:00:00+00:00"
        );
        assert_eq!(
            next("@daily", "2012-07-09T14:45:00Z"),
            "2012-07-10T00:00:00+00:00"
        );
        assert_eq!(
            next("@weekly", "2012-07-09T14:45:00Z"),
            "2012-07-15T00:00:00+00:00"
        );
        assert_eq!(
            next("@monthly", "2012-07-09T14:45:00Z"),
            "2012-08-01T00:00:00+00:00"
        );
        assert_eq!(
            next("@yearly", "2012-07-09T14:45:00Z"),
            "2013-01-01T00:00:00+00:00"
        );
        assert_eq!(
            next("@every 1m30s", "2012-07-09T14:45:00.25Z"),
            "2012-07-09T14:46:30+00:00"
        );
        assert_eq!(
            next("@every 10ms", "2012-07-09T14:45:00Z"),
            "2012-07-09T14:45:01+00:00"
        );
    }

    #[test]
    fn a_tz_prefix_fires_in_local_time_and_skips_a_dst_gap() {
        // 2026-03-29 02:30 does not exist in Bratislava (CET -> CEST at 02:00).
        assert_eq!(
            next("TZ=Europe/Bratislava 0 30 2 * * *", "2026-03-28T12:00:00Z"),
            "2026-03-30T00:30:00+00:00"
        );
        assert_eq!(
            next("TZ=Europe/Bratislava 0 0 9 * * *", "2026-11-15T12:00:00Z"),
            "2026-11-16T08:00:00+00:00"
        );
    }

    #[test]
    fn bad_expressions_name_the_fault_like_robfig() {
        for (expr, want) in [
            ("bogus", "expected 5 to 6 fields, found 1: [bogus]"),
            ("* * * * * * *", "expected 5 to 6 fields, found 7"),
            ("61 * * * *", "end of range (61) above maximum (59): 61"),
            ("@every x", "failed to parse duration @every x"),
            ("@often", "unrecognized descriptor: @often"),
            (
                "TZ=Mars/Olympus * * * * *",
                "provided bad location Mars/Olympus",
            ),
            (
                "CRON_TZ=UTC * * * * *",
                "failed to parse int from CRON_TZ=UTC",
            ),
            (
                "5-1 * * * *",
                "beginning of range (5) beyond end of range (1)",
            ),
            ("*/0 * * * *", "step of range should be a positive number"),
            ("1-2-3 * * * *", "too many hyphens: 1-2-3"),
            ("1/2/3 * * * *", "too many slashes: 1/2/3"),
            ("-1 * * * *", "failed to parse int from "),
        ] {
            let err = parse(expr).expect_err(expr);
            assert!(err.contains(want), "{expr}: {err}");
        }
    }
}
