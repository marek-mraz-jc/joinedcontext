//! Helsinki's wall clock from a UTC instant, without a time zone database: EET (UTC+2) and EEST
//! (UTC+3) between the last Sundays of March and October at 01:00 UTC, the EU rule Finland keeps.

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_HOUR: i64 = 3_600_000;

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01: (year, month 1..=12, day 1..=31).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The weekday of a day count, Monday 0 to Sunday 6 (1970-01-01 was a Thursday).
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

/// The day count of the last Sunday of a month.
fn last_sunday(year: i64, month: u32) -> i64 {
    let first_of_next = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    let last = first_of_next - 1;
    last - i64::from((weekday(last) + 1) % 7)
}

/// Helsinki's offset from UTC at an instant, in hours: 3 in summer time, 2 otherwise.
pub fn helsinki_offset_hours(utc_ms: i64) -> i64 {
    let (year, _, _) = civil_from_days(utc_ms.div_euclid(MS_PER_DAY));
    let start = last_sunday(year, 3) * MS_PER_DAY + MS_PER_HOUR;
    let end = last_sunday(year, 10) * MS_PER_DAY + MS_PER_HOUR;
    if utc_ms >= start && utc_ms < end {
        3
    } else {
        2
    }
}

/// The weekday (Monday 0) and hour (0..=23) on Helsinki's wall clock at a UTC instant.
pub fn helsinki_weekday_hour(utc_ms: i64) -> (u32, u32) {
    let local = utc_ms + helsinki_offset_hours(utc_ms) * MS_PER_HOUR;
    let days = local.div_euclid(MS_PER_DAY);
    let hour = (local.rem_euclid(MS_PER_DAY) / MS_PER_HOUR) as u32;
    (weekday(days), hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(year: i64, month: u32, day: u32, hour: i64, minute: i64) -> i64 {
        days_from_civil(year, month, day) * MS_PER_DAY + hour * MS_PER_HOUR + minute * 60_000
    }

    #[test]
    fn days_and_civil_dates_round_trip_around_the_epoch_and_leap_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for days in [-1, 59, 60, 10_957, 19_782, 20_369] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn weekdays_start_on_monday() {
        assert_eq!(weekday(0), 3); // Thursday 1970-01-01
        assert_eq!(weekday(days_from_civil(2026, 10, 5)), 0); // a Monday
        assert_eq!(weekday(days_from_civil(2026, 10, 11)), 6); // a Sunday
    }

    #[test]
    fn summer_time_turns_at_one_utc_on_the_last_sundays() {
        // 2026: 29 March and 25 October.
        assert_eq!(helsinki_offset_hours(utc(2026, 3, 29, 0, 59)), 2);
        assert_eq!(helsinki_offset_hours(utc(2026, 3, 29, 1, 0)), 3);
        assert_eq!(helsinki_offset_hours(utc(2026, 10, 25, 0, 59)), 3);
        assert_eq!(helsinki_offset_hours(utc(2026, 10, 25, 1, 0)), 2);
        assert_eq!(helsinki_offset_hours(utc(2026, 1, 15, 12, 0)), 2);
        assert_eq!(helsinki_offset_hours(utc(2026, 7, 15, 12, 0)), 3);
    }

    #[test]
    fn the_wall_clock_crosses_midnight_in_local_time() {
        // Tuesday 2026-10-06 21:30 UTC is Wednesday 00:30 in Helsinki (UTC+3).
        assert_eq!(helsinki_weekday_hour(utc(2026, 10, 6, 21, 30)), (2, 0));
        // In winter the same UTC time is still Tuesday, 23:30.
        assert_eq!(helsinki_weekday_hour(utc(2026, 12, 8, 21, 30)), (1, 23));
    }
}
