//! The period a history is read by, from the span of its range (T-7526): the chart host, the
//! assistant, the broker's MCP and the gateway's anonymous bound read this one table, so a range
//! of 10 or 100 years stays under [`MAX_POINTS`] points a series wherever it is asked.

pub use jiff::SignedDuration;

/// `[days, period]`: a range of at most `days` days is read by `period`, a longer one by the next
/// row's. `granularity.json` is JSON and a Rust array expression at once: jc serves the same
/// bytes to the chart host as `granularity.js`.
const TABLE: [[&str; 2]; 5] = include!("granularity.json");

/// The most periods one series of one aggregated read answers: a month of hours, two years of days.
pub const MAX_POINTS: i64 = 750;

const DAY: i64 = 86_400;

/// The finest period a range of `span` is read by: `raw`, else an ISO 8601 duration.
#[must_use]
pub fn finest(span: SignedDuration) -> &'static str {
    let within = |days: &str| {
        days.parse::<i64>()
            .is_ok_and(|days| span.as_secs() <= days.saturating_mul(DAY))
    };
    TABLE
        .iter()
        .find(|[days, _]| within(days))
        .map_or("P1Y", |[_, period]| *period)
}

/// The finest period an aggregate over `span` is asked by: hours where a chart shows readings.
#[must_use]
pub fn aggregated(span: SignedDuration) -> &'static str {
    match finest(span) {
        "raw" => "PT1H",
        period => period,
    }
}

/// The nominal seconds of a period (`PT1H`, `P1W`, `P1M`): a month is 30 days, a year 365.
/// `None` for one that is no ISO 8601 duration of whole numbers, or is no time at all.
#[must_use]
pub fn seconds(period: &str) -> Option<i64> {
    let rest = period.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((date, time)) if !time.is_empty() => (date, time),
        Some(_) => return None,
        None => (rest, ""),
    };
    let mut total = 0_i64;
    for (part, units) in [
        (
            date,
            &[
                ('Y', 365 * DAY),
                ('M', 30 * DAY),
                ('W', 7 * DAY),
                ('D', DAY),
            ][..],
        ),
        (time, &[('H', 3600), ('M', 60), ('S', 1)][..]),
    ] {
        let (mut part, mut units) = (part, units);
        while !part.is_empty() {
            let end = part.find(|c: char| !c.is_ascii_digit())?;
            let unit = part[end..].chars().next()?;
            // each unit once, in its order
            let at = units.iter().position(|(known, _)| *known == unit)?;
            let count: i64 = part[..end].parse().ok()?;
            total = total.checked_add(count.checked_mul(units[at].1)?)?;
            part = &part[end + unit.len_utf8()..];
            units = &units[at + 1..];
        }
    }
    (total > 0).then_some(total)
}

/// `asked`, unless it is finer than a range of `span` is read by: then that period.
#[must_use]
pub fn bounded(span: SignedDuration, asked: &str) -> &str {
    let least = aggregated(span);
    match (seconds(asked), seconds(least)) {
        (Some(was), Some(floor)) if was < floor => least,
        _ => asked,
    }
}

/// Whether `period` cuts `span` into at most [`MAX_POINTS`] periods.
#[must_use]
pub fn fits(span: SignedDuration, period: &str) -> bool {
    seconds(period).is_some_and(|secs| span.as_secs() <= secs.saturating_mul(MAX_POINTS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days(n: i64) -> SignedDuration {
        SignedDuration::from_hours(24 * n)
    }

    #[test]
    fn the_range_picks_the_period() {
        for (span, period) in [
            (1, "raw"),
            (7, "raw"),
            (8, "PT1H"),
            (31, "PT1H"),
            (32, "P1D"),
            (732, "P1D"),
            (733, "P1M"),
            (3_653, "P1M"),
            (14_640, "P1M"),
            (14_641, "P1Y"),
            (36_525, "P1Y"),
        ] {
            assert_eq!(finest(days(span)), period, "{span} days");
        }
        assert_eq!(finest(days(7) + SignedDuration::from_secs(1)), "PT1H");
        assert_eq!(aggregated(days(1)), "PT1H");
    }

    #[test]
    fn every_range_up_to_750_years_stays_under_the_point_bound() {
        for span in 1..=750 * 365 {
            let period = aggregated(days(span));
            assert!(fits(days(span), period), "{span} days by {period}");
        }
        assert!(!fits(days(3_653), "P1D"));
        assert!(!fits(days(40 * 365), "P1W"));
        assert!(fits(days(10 * 365), "P1W"));
    }

    #[test]
    fn a_coarser_period_is_kept_and_a_finer_one_is_raised() {
        assert_eq!(bounded(days(3_653), "P1D"), "P1M");
        assert_eq!(bounded(days(3_653), "P1Y"), "P1Y");
        assert_eq!(bounded(days(365), "PT10M"), "P1D");
        assert_eq!(bounded(days(30), "P1W"), "P1W");
        // As fine as the floor, written another way, is kept as asked.
        assert_eq!(bounded(days(365), "PT24H"), "PT24H");
        // not a period: the broker refuses it in its own words
        assert_eq!(bounded(days(3_653), "monthly"), "monthly");
    }

    #[test]
    fn a_period_is_whole_numbers_in_their_order() {
        assert_eq!(seconds("P1Y2M3W4DT5H6M7S"), Some(38_898_367));
        assert_eq!(seconds("P1M"), Some(30 * DAY));
        assert_eq!(seconds("PT1M"), Some(60));
        // A unit twice, or a `T` with no time after it, is no period.
        for bad in ["P1D1D", "PT1H1H", "P1DT"] {
            assert_eq!(seconds(bad), None, "{bad}");
        }
        for bad in ["P", "PT", "PT0S", "P1D1Y", "P1.5D", "1D", "P1X"] {
            assert_eq!(seconds(bad), None, "{bad}");
        }
    }
}
