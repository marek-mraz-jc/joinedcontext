//! Time series processing and Europe/Helsinki calendar arithmetic (T-3337):
//! RFC 3339 parsing without an external date crate, EU daylight-saving rules,
//! hourly bucketing, and alignment of weather observations to bike history.

use std::collections::{BTreeMap, HashMap};

/// Hourly aggregated bike availability point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HourlyPoint {
    pub utc_hour: i64,
    pub value: f64,
}

/// Hourly aggregated weather observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HourlyWeather {
    pub utc_hour: i64,
    pub temperature: Option<f64>,
    pub precipitation: Option<f64>,
}

/// Weather values matched to an hourly bike availability point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherObservation {
    pub temperature: f64,
    pub precipitation: f64,
}

/// One bike availability hour paired with its aligned weather (if readable).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignedHour {
    pub utc_hour: i64,
    pub bikes: f64,
    pub weather: Option<WeatherObservation>,
}

/// Integer floor division for negative and positive integers.
pub fn div_floor(a: i64, b: i64) -> i64 {
    let res = a / b;
    let rem = a % b;
    if rem != 0 && (a ^ b) < 0 {
        res - 1
    } else {
        res
    }
}

/// Converts civil date (year, month 1..=12, day 1..=31) to days since Unix epoch (1970-01-01).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let days_in_m = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if d > days_in_m {
        return None;
    }
    let m_days: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_of_year = m_days[(m - 1) as usize] + (d as i64 - 1);
    if m > 2 && leap {
        day_of_year += 1;
    }
    let y_prev = y - 1;
    let era_days =
        y_prev * 365 + div_floor(y_prev, 4) - div_floor(y_prev, 100) + div_floor(y_prev, 400);
    let epoch_days = 719_162;
    Some(era_days - epoch_days + day_of_year)
}

/// Converts days since Unix epoch (1970-01-01) to civil date (year, month 1..=12, day 1..=31).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Parses an RFC 3339 timestamp (e.g. 2026-03-29T00:30:00Z or with offsets) to Unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;

    let sep = bytes[10];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return None;
    }

    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let hour: u32 = s[11..13].parse().ok()?;
    let minute: u32 = s[14..16].parse().ok()?;
    let second: u32 = s[17..19].parse().ok()?;

    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut idx = 19;
    if idx < s.len() && bytes[idx] == b'.' {
        idx += 1;
        let frac_start = idx;
        while idx < s.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == frac_start {
            return None;
        }
    }

    if idx >= s.len() {
        return None;
    }

    let offset_secs: i64 = match bytes[idx] {
        b'Z' | b'z' => {
            if idx + 1 != s.len() {
                return None;
            }
            0
        }
        b'+' | b'-' => {
            let sign = if bytes[idx] == b'+' { 1 } else { -1 };
            let rest = &s[idx + 1..];
            let (oh, om) = if rest.len() == 5 && rest.as_bytes()[2] == b':' {
                let h: u32 = rest[0..2].parse().ok()?;
                let m: u32 = rest[3..5].parse().ok()?;
                (h, m)
            } else if rest.len() == 4 {
                let h: u32 = rest[0..2].parse().ok()?;
                let m: u32 = rest[2..4].parse().ok()?;
                (h, m)
            } else {
                return None;
            };
            if oh > 23 || om > 59 {
                return None;
            }
            sign * (oh as i64 * 3600 + om as i64 * 60)
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day)?;
    let time_secs = hour as i64 * 3600 + minute as i64 * 60 + second as i64;
    let local_secs = days.checked_mul(86_400)?.checked_add(time_secs)?;
    local_secs.checked_sub(offset_secs)
}

/// Formats a Unix timestamp to RFC 3339 UTC string (YYYY-MM-DDTHH:MM:SSZ).
pub fn format_rfc3339_utc(unix_secs: i64) -> String {
    let days = div_floor(unix_secs, 86_400);
    let rem_secs = unix_secs.rem_euclid(86400);
    let hour = rem_secs / 3600;
    let minute = (rem_secs % 3600) / 60;
    let second = rem_secs % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Checks whether Europe/Helsinki is in summer time (EEST, UTC+3) at `utc_secs`.
/// EU DST switch: last Sunday of March 01:00 UTC to last Sunday of October 01:00 UTC.
pub fn is_helsinki_dst(utc_secs: i64) -> bool {
    let days = div_floor(utc_secs, 86_400);
    let (year, _, _) = civil_from_days(days);

    // Last Sunday of March at 01:00 UTC
    let march_31_days = match days_from_civil(year, 3, 31) {
        Some(d) => d,
        None => return false,
    };
    let dow_mar = (march_31_days + 3).rem_euclid(7); // 0 = Mon, ..., 6 = Sun
                                                     // Always 25..=31: the last Sunday of a 31-day month.
    let last_sun_mar_day = (31 - (dow_mar + 1) % 7) as u32;
    let dst_start_days = match days_from_civil(year, 3, last_sun_mar_day) {
        Some(d) => d,
        None => return false,
    };
    let dst_start = dst_start_days * 86_400 + 3600;

    // Last Sunday of October at 01:00 UTC
    let oct_31_days = match days_from_civil(year, 10, 31) {
        Some(d) => d,
        None => return false,
    };
    let dow_oct = (oct_31_days + 3).rem_euclid(7);
    let last_sun_oct_day = (31 - (dow_oct + 1) % 7) as u32;
    let dst_end_days = match days_from_civil(year, 10, last_sun_oct_day) {
        Some(d) => d,
        None => return false,
    };
    let dst_end = dst_end_days * 86_400 + 3600;

    utc_secs >= dst_start && utc_secs < dst_end
}

/// Europe/Helsinki UTC offset in seconds (+3h in summer, +2h in winter).
pub fn helsinki_offset_secs(utc_secs: i64) -> i64 {
    if is_helsinki_dst(utc_secs) {
        3 * 3600
    } else {
        2 * 3600
    }
}

/// Computes the Europe/Helsinki hour of week (Monday 00 = 0 … Sunday 23 = 167).
pub fn helsinki_hour_of_week(utc_secs: i64) -> usize {
    let local_secs = utc_secs + helsinki_offset_secs(utc_secs);
    let local_days = div_floor(local_secs, 86_400);
    let dow = (local_days + 3).rem_euclid(7) as usize; // 0 = Mon, ..., 6 = Sun
    let rem_secs = ((local_secs % 86_400) + 86_400) % 86_400;
    let hour = (rem_secs / 3600) as usize;
    (dow * 24 + hour).min(167)
}

/// Groups points by UTC hour bucket and computes their mean values.
/// Bounded linear time over points via BTreeMap of hourly buckets.
pub fn hourly_means(points: &[(i64, f64)]) -> Vec<HourlyPoint> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut map = BTreeMap::<i64, (f64, usize)>::new();
    for &(t, val) in points {
        if !val.is_finite() {
            continue;
        }
        let bucket = div_floor(t, 3600) * 3600;
        let entry = map.entry(bucket).or_insert((0.0, 0));
        entry.0 += val;
        entry.1 += 1;
    }
    map.into_iter()
        .map(|(utc_hour, (sum, count))| HourlyPoint {
            utc_hour,
            value: sum / count as f64,
        })
        .collect()
}

/// Groups weather points by UTC hour bucket and computes temperature and precipitation means.
pub fn hourly_weather(points: &[(i64, Option<f64>, Option<f64>)]) -> Vec<HourlyWeather> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut map = BTreeMap::<i64, (f64, usize, f64, usize)>::new();
    for &(t, temp_opt, precip_opt) in points {
        let bucket = div_floor(t, 3600) * 3600;
        let entry = map.entry(bucket).or_insert((0.0, 0, 0.0, 0));
        if let Some(temp) = temp_opt {
            if temp.is_finite() {
                entry.0 += temp;
                entry.1 += 1;
            }
        }
        if let Some(precip) = precip_opt {
            if precip.is_finite() {
                entry.2 += precip;
                entry.3 += 1;
            }
        }
    }
    map.into_iter()
        .map(
            |(utc_hour, (t_sum, t_count, p_sum, p_count))| HourlyWeather {
                utc_hour,
                temperature: if t_count > 0 {
                    Some(t_sum / t_count as f64)
                } else {
                    None
                },
                precipitation: if p_count > 0 {
                    Some(p_sum / p_count as f64)
                } else {
                    None
                },
            },
        )
        .collect()
}

/// Aligns weather hours to bike hours: the weather mean of the same UTC hour, else the previous hour.
pub fn align_weather(bikes: &[HourlyPoint], weather: &[HourlyWeather]) -> Vec<AlignedHour> {
    if bikes.is_empty() {
        return Vec::new();
    }
    let weather_map: HashMap<i64, &HourlyWeather> =
        weather.iter().map(|w| (w.utc_hour, w)).collect();

    bikes
        .iter()
        .map(|b| {
            let obs = weather_map
                .get(&b.utc_hour)
                .and_then(|w| match (w.temperature, w.precipitation) {
                    (Some(t), Some(p)) => Some(WeatherObservation {
                        temperature: t,
                        precipitation: p,
                    }),
                    _ => None,
                })
                .or_else(|| {
                    weather_map.get(&(b.utc_hour - 3600)).and_then(|w| {
                        match (w.temperature, w.precipitation) {
                            (Some(t), Some(p)) => Some(WeatherObservation {
                                temperature: t,
                                precipitation: p,
                            }),
                            _ => None,
                        }
                    })
                });

            AlignedHour {
                utc_hour: b.utc_hour,
                bikes: b.value,
                weather: obs,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_with_offset_z_and_junk() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01t00:00:00z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01 00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T02:00:00+0200"), Some(0));
        assert_eq!(parse_rfc3339("1969-12-31T19:00:00-05:00"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-03-29T00:30:00.123456Z"),
            Some(1_774_744_200)
        );

        // Format UTC round-trip
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1_774_744_200), "2026-03-29T00:30:00Z");

        // Junk and out-of-range dates
        assert_eq!(parse_rfc3339(""), None);
        assert_eq!(parse_rfc3339("not-a-date"), None);
        assert_eq!(parse_rfc3339("2026-02-29T00:00:00Z"), None); // 2026 is non-leap
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800)); // 2024 is leap
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-03-32T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-03-29T25:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-03-29T00:60:00Z"), None);
        assert_eq!(parse_rfc3339("2026-03-29T00:00:00"), None); // missing offset
        assert_eq!(parse_rfc3339("2026-03-29T00:00:00.Z"), None); // malformed fraction
    }

    #[test]
    fn dst_switches_of_2026() {
        // March 29, 2026: DST start at 01:00 UTC.
        // 00:59:59 UTC is winter time (+2h). Local: 02:59:59 Sunday.
        let before_mar = parse_rfc3339("2026-03-29T00:59:59Z").unwrap();
        assert!(!is_helsinki_dst(before_mar));
        assert_eq!(helsinki_offset_secs(before_mar), 2 * 3600);
        // Sunday (dow = 6), hour = 2 -> 6 * 24 + 2 = 146
        assert_eq!(helsinki_hour_of_week(before_mar), 146);

        // 01:00:00 UTC is summer time (+3h). Local: 04:00:00 Sunday.
        let at_mar = parse_rfc3339("2026-03-29T01:00:00Z").unwrap();
        assert!(is_helsinki_dst(at_mar));
        assert_eq!(helsinki_offset_secs(at_mar), 3 * 3600);
        // Sunday (dow = 6), hour = 4 -> 6 * 24 + 4 = 148 (hour 3 was skipped!)
        assert_eq!(helsinki_hour_of_week(at_mar), 148);

        // October 25, 2026: DST end at 01:00 UTC.
        // 00:59:59 UTC is summer time (+3h). Local: 03:59:59 Sunday.
        let before_oct = parse_rfc3339("2026-10-25T00:59:59Z").unwrap();
        assert!(is_helsinki_dst(before_oct));
        assert_eq!(helsinki_offset_secs(before_oct), 3 * 3600);
        // Sunday (dow = 6), hour = 3 -> 6 * 24 + 3 = 147
        assert_eq!(helsinki_hour_of_week(before_oct), 147);

        // 01:00:00 UTC is winter time (+2h). Local: 03:00:00 Sunday.
        let at_oct = parse_rfc3339("2026-10-25T01:00:00Z").unwrap();
        assert!(!is_helsinki_dst(at_oct));
        assert_eq!(helsinki_offset_secs(at_oct), 2 * 3600);
        // Sunday (dow = 6), hour = 3 -> 6 * 24 + 3 = 147 (hour 3 repeated)
        assert_eq!(helsinki_hour_of_week(at_oct), 147);
    }

    #[test]
    fn hourly_means_with_gaps() {
        let pts = vec![
            (100, 10.0),
            (200, 20.0),
            (3600 + 10, 30.0),
            (3 * 3600 + 50, 40.0),
        ];
        let hourly = hourly_means(&pts);
        assert_eq!(hourly.len(), 3);
        assert_eq!(
            hourly[0],
            HourlyPoint {
                utc_hour: 0,
                value: 15.0
            }
        );
        assert_eq!(
            hourly[1],
            HourlyPoint {
                utc_hour: 3600,
                value: 30.0
            }
        );
        assert_eq!(
            hourly[2],
            HourlyPoint {
                utc_hour: 3 * 3600,
                value: 40.0
            }
        );
    }

    #[test]
    fn aligning_weather_hours() {
        let bikes = vec![
            HourlyPoint {
                utc_hour: 3600,
                value: 5.0,
            },
            HourlyPoint {
                utc_hour: 7200,
                value: 8.0,
            },
            HourlyPoint {
                utc_hour: 10800,
                value: 12.0,
            },
        ];
        let weather = vec![
            HourlyWeather {
                utc_hour: 3600,
                temperature: Some(15.0),
                precipitation: Some(0.0),
            },
            HourlyWeather {
                utc_hour: 3600, // Hour 7200 is missing, so it should fall back to 3600
                temperature: Some(15.0),
                precipitation: Some(0.0),
            },
        ];
        let aligned = align_weather(&bikes, &weather);
        assert_eq!(aligned.len(), 3);
        assert_eq!(
            aligned[0].weather,
            Some(WeatherObservation {
                temperature: 15.0,
                precipitation: 0.0
            })
        );
        assert_eq!(
            aligned[1].weather,
            Some(WeatherObservation {
                temperature: 15.0,
                precipitation: 0.0
            })
        );
        assert_eq!(aligned[2].weather, None);
    }

    #[test]
    fn million_point_series_stays_bounded_in_time() {
        let mut pts = Vec::with_capacity(1_000_000);
        // On an hour boundary, so 168 hours of minutes fill exactly 168 UTC hours.
        let base = 472_222 * 3600;
        for i in 0..1_000_000 {
            // Span 168 hours with dense minute-level points
            let offset = (i % (168 * 60)) * 60;
            pts.push((base + offset as i64, (i % 30) as f64));
        }
        let start = std::time::Instant::now();
        let aggregated = hourly_means(&pts);
        let elapsed = start.elapsed();
        assert_eq!(aggregated.len(), 168);
        // Linear: well under two seconds even unoptimised, as the build lane runs the tests.
        assert!(elapsed.as_millis() < 2000, "1M points took {:?}", elapsed);
    }

    #[test]
    fn floor_division_rounds_down_for_a_negative_dividend() {
        assert_eq!(div_floor(-7, 2), -4);
        assert_eq!(div_floor(-8, 2), -4);
        assert_eq!(div_floor(7, 2), 3);
    }

    #[test]
    fn a_civil_date_that_does_not_exist_is_none() {
        assert_eq!(days_from_civil(2026, 4, 31), None);
        assert_eq!(days_from_civil(2026, 2, 29), None);
        assert_eq!(days_from_civil(2026, 13, 1), None);
        // A leap year counts its 29 February before March.
        assert_eq!(
            days_from_civil(2024, 3, 1),
            Some(days_from_civil(2024, 2, 28).expect("a date") + 2)
        );
    }

    #[test]
    fn a_timestamp_with_the_wrong_shape_is_none() {
        for bad in [
            "2026-03-01",
            "2026/03/01T00:00:00Z",
            "2026-03-01X00:00:00Z",
            "2026-03-01T00-00-00Z",
            "2026-02-30T00:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }
}
