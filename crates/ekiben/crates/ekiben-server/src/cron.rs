use std::time::Duration;

use ekiben_bloblang::time_format::{unix_floor, zone};
use ekiben_core::config::parse_duration_str;
use ekiben_core::CoreError;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// Parsed schedule supporting standard 5-field cron expressions (`min hour dom month dow`),
/// `@every <duration>`, `@hourly`, `@daily`, `@weekly`, `@monthly`, and optional IANA timezones (`Section 6`).
#[derive(Clone, Debug)]
pub enum CronSchedule {
    Every(Duration),
    Fields {
        minutes: u64,       // bits 0..=59
        hours: u32,         // bits 0..=23
        days_of_month: u32, // bits 1..=31
        months: u16,        // bits 1..=12
        days_of_week: u8,   // bits 0..=6 (0 = Sunday)
        dom_wildcard: bool,
        dow_wildcard: bool,
        tz: Option<TimeZone>,
    },
}

impl CronSchedule {
    pub fn parse(expr: &str, tz_name: Option<&str>) -> Result<Self, CoreError> {
        let trimmed = expr.trim();
        let tz = match tz_name.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("UTC")) {
            Some(name) => Some(
                zone(name).map_err(|_| CoreError::config(format!("unknown timezone '{name}'")))?,
            ),
            None => None,
        };

        if let Some(dur_str) = trimmed.strip_prefix("@every ") {
            let dur = parse_duration_str(dur_str.trim())?;
            if dur.is_zero() {
                return Err(CoreError::config(
                    "@every duration must be greater than zero",
                ));
            }
            return Ok(Self::Every(dur));
        }

        let normalized = match trimmed {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };

        let fields: Vec<&str> = normalized.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CoreError::config(format!(
                "cron expression '{expr}' must have 5 fields (min hour dom month dow) or '@every <duration>'"
            )));
        }

        let minutes = parse_cron_field_u64(fields[0], 0, 59, &[])?;
        let hours = parse_cron_field_u64(fields[1], 0, 23, &[])? as u32;
        let dom_wildcard = fields[2] == "*";
        let days_of_month = parse_cron_field_u64(fields[2], 1, 31, &[])? as u32;
        let months = parse_cron_field_u64(
            fields[3],
            1,
            12,
            &[
                ("JAN", 1),
                ("FEB", 2),
                ("MAR", 3),
                ("APR", 4),
                ("MAY", 5),
                ("JUN", 6),
                ("JUL", 7),
                ("AUG", 8),
                ("SEP", 9),
                ("OCT", 10),
                ("NOV", 11),
                ("DEC", 12),
            ],
        )? as u16;
        let dow_wildcard = fields[4] == "*";
        let raw_dow = parse_cron_field_u64(
            fields[4],
            0,
            7,
            &[
                ("SUN", 0),
                ("MON", 1),
                ("TUE", 2),
                ("WED", 3),
                ("THU", 4),
                ("FRI", 5),
                ("SAT", 6),
            ],
        )?;
        // Map 7 (Sunday alias) to bit 0
        let days_of_week = ((raw_dow & 0x7f) | ((raw_dow >> 7) & 1)) as u8;

        Ok(Self::Fields {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            dom_wildcard,
            dow_wildcard,
            tz,
        })
    }

    /// Computes the next scheduled fire time strictly after `after`.
    pub fn next_after(&self, after: Timestamp) -> Option<Timestamp> {
        match self {
            CronSchedule::Every(dur) => {
                let delta = SignedDuration::try_from(*dur).ok()?;
                after.checked_add(delta).ok()
            }
            CronSchedule::Fields {
                minutes,
                hours,
                days_of_month,
                months,
                days_of_week,
                dom_wildcard,
                dow_wildcard,
                tz,
            } => {
                // Start at the next whole minute strictly after `after`
                let secs = unix_floor(after, 1_000_000_000)?;
                let base_ts = secs - secs.rem_euclid(60) + 60;
                let mut candidate = Timestamp::from_second(base_ts).ok()?;
                let zone = tz.clone().unwrap_or(TimeZone::UTC);
                let plus =
                    |t: Timestamp, mins: i64| t.checked_add(SignedDuration::from_mins(mins)).ok();

                // Search up to 5 years ahead (2,629,440 minutes)
                for _ in 0..(366 * 5 * 24 * 60) {
                    let z = candidate.to_zoned(zone.clone());
                    let (m, d, h, min, dow) = (
                        z.month(),
                        z.day(),
                        z.hour(),
                        z.minute(),
                        z.weekday().to_sunday_zero_offset(),
                    );

                    if (months & (1 << m)) == 0 {
                        // Advance to the next hour to skip non-matching months quickly
                        candidate = plus(candidate, (60 - i64::from(min)).max(1))?;
                        continue;
                    }

                    let dom_match = (days_of_month & (1 << d)) != 0;
                    let dow_match = (days_of_week & (1 << dow)) != 0;
                    let day_ok = match (*dom_wildcard, *dow_wildcard) {
                        (true, true) => true,
                        (true, false) => dow_match,
                        (false, true) => dom_match,
                        // Standard cron rule: when both DOM and DOW are restricted, either matching triggers
                        (false, false) => dom_match || dow_match,
                    };

                    if !day_ok {
                        candidate = plus(candidate, (60 - i64::from(min)).max(1))?;
                        continue;
                    }

                    if (hours & (1 << h)) == 0 {
                        candidate = plus(candidate, (60 - i64::from(min)).max(1))?;
                        continue;
                    }

                    if (minutes & (1 << min)) != 0 {
                        return Some(candidate);
                    }

                    candidate = plus(candidate, 1)?;
                }
                None
            }
        }
    }
}

fn parse_cron_field_u64(
    field: &str,
    min: u32,
    max: u32,
    names: &[(&str, u32)],
) -> Result<u64, CoreError> {
    let mut mask = 0u64;
    for part in field.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((r, s)) => {
                let st: u32 = s
                    .parse()
                    .map_err(|_| CoreError::config(format!("invalid cron step '{s}'")))?;
                if st == 0 {
                    return Err(CoreError::config("cron step cannot be 0"));
                }
                (r, st)
            }
            None => (part, 1),
        };

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let sv = parse_cron_atom(a, min, max, names)?;
            let ev = parse_cron_atom(b, min, max, names)?;
            if sv > ev {
                return Err(CoreError::config(format!(
                    "invalid cron range '{range_part}'"
                )));
            }
            (sv, ev)
        } else {
            let v = parse_cron_atom(range_part, min, max, names)?;
            if part.contains('/') {
                (v, max)
            } else {
                (v, v)
            }
        };

        let mut v = start;
        while v <= end {
            mask |= 1u64 << v;
            v += step;
        }
    }
    // Every part sets at least its start bit, so the mask is never empty.
    Ok(mask)
}

fn parse_cron_atom(s: &str, min: u32, max: u32, names: &[(&str, u32)]) -> Result<u32, CoreError> {
    let upper = s.to_ascii_uppercase();
    for (name, val) in names {
        if upper == *name {
            return Ok(*val);
        }
    }
    let val: u32 = s
        .parse()
        .map_err(|_| CoreError::config(format!("invalid cron value '{s}'")))?;
    if val < min || val > max {
        return Err(CoreError::config(format!(
            "cron value {val} out of bounds ({min}..={max})"
        )));
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(expr: &str, tz: &str, from: &str) -> Option<String> {
        let after = jc_time::parse_rfc3339(from)?;
        let schedule = CronSchedule::parse(expr, Some(tz)).ok()?;
        Some(jc_time::TimestampTz(schedule.next_after(after)?).to_rfc3339())
    }

    #[test]
    fn a_zoned_schedule_follows_dst_in_bratislava_and_helsinki() {
        // Daily 09:00 local: 08:00Z in CET, 07:00Z in CEST; 07:00Z in EET, 06:00Z in EEST.
        for (tz, winter, summer) in [
            (
                "Europe/Bratislava",
                "2026-03-28T08:00:00+00:00",
                "2026-03-30T07:00:00+00:00",
            ),
            (
                "Europe/Helsinki",
                "2026-03-28T07:00:00+00:00",
                "2026-03-30T06:00:00+00:00",
            ),
        ] {
            assert_eq!(
                next("0 9 * * *", tz, "2026-03-28T00:00:00Z").as_deref(),
                Some(winter)
            );
            assert_eq!(
                next("0 9 * * *", tz, "2026-03-29T12:00:00Z").as_deref(),
                Some(summer)
            );
        }
        assert!(CronSchedule::parse("0 9 * * *", Some("Mars/Olympus")).is_err());
    }

    fn err(expr: &str) -> String {
        CronSchedule::parse(expr, None).expect_err(expr).to_string()
    }

    #[test]
    fn every_adds_its_period_and_refuses_zero_or_garbage() {
        assert_eq!(
            next("@every 90s", "UTC", "2026-03-28T10:00:00Z").as_deref(),
            Some("2026-03-28T10:01:30+00:00")
        );
        assert!(err("@every 0s").contains("greater than zero"));
        assert!(CronSchedule::parse("@every soon", None).is_err());
        // A period past the timestamp range has no next run.
        let huge = CronSchedule::Every(Duration::from_secs(u64::MAX));
        assert!(huge.next_after(Timestamp::UNIX_EPOCH).is_none());
    }

    #[test]
    fn aliases_and_empty_or_utc_zones_run_in_utc() {
        let from = "2026-03-28T10:30:00Z"; // a Saturday
        for (expr, want) in [
            ("@hourly", "2026-03-28T11:00:00+00:00"),
            ("@daily", "2026-03-29T00:00:00+00:00"),
            ("@midnight", "2026-03-29T00:00:00+00:00"),
            ("@weekly", "2026-03-29T00:00:00+00:00"),
            ("@monthly", "2026-04-01T00:00:00+00:00"),
            ("@yearly", "2027-01-01T00:00:00+00:00"),
            ("@annually", "2027-01-01T00:00:00+00:00"),
        ] {
            assert_eq!(next(expr, "", from).as_deref(), Some(want), "{expr}");
            assert_eq!(next(expr, "utc", from).as_deref(), Some(want), "{expr}");
        }
    }

    #[test]
    fn fields_steps_ranges_and_names_pick_the_next_minute() {
        let from = "2026-03-28T10:07:30Z"; // a Saturday
        for (expr, want) in [
            ("*/15 * * * *", "2026-03-28T10:15:00+00:00"),
            ("5/20 * * * *", "2026-03-28T10:25:00+00:00"),
            ("0,50 * * * *", "2026-03-28T10:50:00+00:00"),
            ("0 0 * * MON-FRI", "2026-03-30T00:00:00+00:00"),
            ("0 0 * * 7", "2026-03-29T00:00:00+00:00"),
            ("0 0 * * sun", "2026-03-29T00:00:00+00:00"),
            ("0 0 1 jan *", "2027-01-01T00:00:00+00:00"),
            ("0 12 15 * *", "2026-04-15T12:00:00+00:00"),
            ("30 9-17/4 * * *", "2026-03-28T13:30:00+00:00"),
            // Day of month and day of week both restricted: either one fires.
            ("0 0 13 * FRI", "2026-04-03T00:00:00+00:00"),
        ] {
            assert_eq!(next(expr, "UTC", from).as_deref(), Some(want), "{expr}");
        }
    }

    #[test]
    fn a_date_that_never_comes_has_no_next_run() {
        assert_eq!(next("0 0 30 2 *", "UTC", "2026-03-28T00:00:00Z"), None);
    }

    #[test]
    fn malformed_fields_are_refused_with_the_reason() {
        assert!(err("* * * *").contains("must have 5 fields"));
        assert!(err("* * * * * *").contains("must have 5 fields"));
        assert!(err("*/0 * * * *").contains("step cannot be 0"));
        assert!(err("*/x * * * *").contains("invalid cron step 'x'"));
        assert!(err("5-3 * * * *").contains("invalid cron range '5-3'"));
        assert!(err("60 * * * *").contains("out of bounds (0..=59)"));
        assert!(err("* 24 * * *").contains("out of bounds (0..=23)"));
        assert!(err("* * 0 * *").contains("out of bounds (1..=31)"));
        assert!(err("* * * 13 *").contains("out of bounds (1..=12)"));
        assert!(err("* * * * 8").contains("out of bounds (0..=7)"));
        assert!(err("x * * * *").contains("invalid cron value 'x'"));
        assert!(err("1-x * * * *").contains("invalid cron value 'x'"));
        assert!(err("x-1 * * * *").contains("invalid cron value 'x'"));
        assert!(err("1, * * * *").contains("invalid cron value ''"));
        assert!(err("* * * FOO *").contains("invalid cron value 'FOO'"));
    }
}
