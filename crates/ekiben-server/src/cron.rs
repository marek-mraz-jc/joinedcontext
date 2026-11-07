use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;
use ekiben_core::config::parse_duration_str;
use ekiben_core::CoreError;

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
        tz: Option<Tz>,
    },
}

impl CronSchedule {
    pub fn parse(expr: &str, tz_name: Option<&str>) -> Result<Self, CoreError> {
        let trimmed = expr.trim();
        let tz = match tz_name.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("UTC")) {
            Some(name) => Some(
                name.parse::<Tz>()
                    .map_err(|_| CoreError::config(format!("unknown timezone '{name}'")))?,
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
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            CronSchedule::Every(dur) => {
                let delta = chrono::Duration::from_std(*dur).ok()?;
                Some(after + delta)
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
                let base_ts = after.timestamp() - (after.timestamp().rem_euclid(60)) + 60;
                let mut candidate = chrono::TimeZone::timestamp_opt(&Utc, base_ts, 0).single()?;

                // Search up to 5 years ahead (2,629,440 minutes)
                for _ in 0..(366 * 5 * 24 * 60) {
                    let (m, d, h, min, dow) = match tz {
                        Some(zone) => {
                            let z = candidate.with_timezone(zone);
                            (
                                z.month(),
                                z.day(),
                                z.hour(),
                                z.minute(),
                                z.weekday().num_days_from_sunday(),
                            )
                        }
                        None => (
                            candidate.month(),
                            candidate.day(),
                            candidate.hour(),
                            candidate.minute(),
                            candidate.weekday().num_days_from_sunday(),
                        ),
                    };

                    if (months & (1 << m)) == 0 {
                        // Advance to the next hour to skip non-matching months quickly
                        candidate += chrono::Duration::minutes((60 - min as i64).max(1));
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
                        candidate += chrono::Duration::minutes((60 - min as i64).max(1));
                        continue;
                    }

                    if (hours & (1 << h)) == 0 {
                        candidate += chrono::Duration::minutes((60 - min as i64).max(1));
                        continue;
                    }

                    if (minutes & (1 << min)) != 0 {
                        return Some(candidate);
                    }

                    candidate += chrono::Duration::minutes(1);
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
    if mask == 0 {
        return Err(CoreError::config(format!("empty cron field '{field}'")));
    }
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
