//! Clamping a caller's temporal window to what the grants permit (T-0150, GW26).
//!
//! A grant like "the last 24 hours" is a moving window, so it is written relative
//! (`timerel=after;timeAt=P-1D`) and resolved against the same `now` the rest of the
//! decision uses. The caller's window is intersected with it, never unioned: asking for a
//! year of history under a 24-hour grant returns 24 hours, and asking for last week under
//! a grant that ends last month returns an empty list rather than a refusal, because the
//! request was well formed and the answer is genuinely nothing (GW26).
//!
//! Several grants with several windows cannot be one interval. Their union is forwarded
//! as its hull, which is a superset the broker can execute, and the gateway then drops
//! the instances that fall in the gaps — the hull is never the answer, only the query.

use jc_time::{parse_rfc3339, SecondsFormat, TimestampTz};
use jiff::SignedDuration;

/// A half-open interval `[from, to)`; an open end is the beginning or the end of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Window {
    /// The earliest instant the window admits.
    pub from: Option<TimestampTz>,
    /// The first instant it no longer admits.
    pub to: Option<TimestampTz>,
}

impl Window {
    /// Whether the window admits nothing at all.
    pub fn is_empty(&self) -> bool {
        matches!((self.from, self.to), (Some(from), Some(to)) if from >= to)
    }

    /// Whether an instant falls inside the window.
    pub fn admits(&self, at: TimestampTz) -> bool {
        self.from.is_none_or(|from| at >= from) && self.to.is_none_or(|to| at < to)
    }

    /// The overlap of two windows, which is what a clamp is.
    pub fn intersect(self, other: Window) -> Window {
        Window {
            from: max_bound(self.from, other.from),
            to: min_bound(self.to, other.to),
        }
    }

    /// The window as a `temporalQ`, or `None` when it constrains nothing.
    pub fn to_temporal_q(self) -> Option<String> {
        match (self.from, self.to) {
            (Some(from), Some(to)) => Some(format!(
                "timerel=between;timeAt={};endTimeAt={}",
                stamp(from),
                stamp(to)
            )),
            (Some(from), None) => Some(format!("timerel=after;timeAt={}", stamp(from))),
            (None, Some(to)) => Some(format!("timerel=before;timeAt={}", stamp(to))),
            (None, None) => None,
        }
    }
}

/// What the clamp decided (GW26).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Clamped {
    /// The `temporalQ` to forward, `None` when nothing constrains the query.
    pub temporal_q: Option<String>,
    /// The windows an instance must fall into, when the forwarded hull is wider than the
    /// union it stands for. Empty means the broker's answer needs no filtering.
    pub windows: Vec<Window>,
    /// The caller asked for a period the grants do not reach at all: the answer is an
    /// empty list, not a refusal (GW26).
    pub empty: bool,
    /// Whether the caller's own window was narrowed (R22).
    pub restricted: bool,
}

/// Intersects the caller's window with each grant's, resolved against `now` (GW26).
pub fn clamp(caller: Option<&str>, grants: &[&str], now: TimestampTz) -> Clamped {
    let asked = caller.and_then(|q| parse(q, now)).unwrap_or_default();

    // No grant narrows time, so the caller's own window stands as it was written: a
    // relative `timeAt` the caller sent is the caller's business, not ours to resolve.
    if grants.is_empty() {
        return Clamped {
            temporal_q: caller.map(str::to_owned),
            ..Clamped::default()
        };
    }

    let mut windows: Vec<Window> = grants
        .iter()
        .filter_map(|grant| parse(grant, now))
        .map(|granted| granted.intersect(asked))
        .filter(|window| !window.is_empty())
        .collect();
    windows.sort_by_key(|window| (window.from, window.to));
    windows.dedup();

    // Every grant window ends before the caller's begins, or begins after it ends. The
    // request is legal and its answer is nothing.
    if windows.is_empty() {
        return Clamped {
            empty: true,
            restricted: true,
            ..Clamped::default()
        };
    }

    // One window is exactly forwardable. Several are not: their hull is the query and the
    // windows themselves are the filter.
    let hull = Window {
        from: windows.iter().map(|window| window.from).min().flatten(),
        to: to_bound(windows.iter().map(|window| window.to)),
    };
    Clamped {
        temporal_q: hull.to_temporal_q(),
        windows: if windows.len() > 1 {
            windows
        } else {
            Vec::new()
        },
        empty: false,
        restricted: hull != asked,
    }
}

/// The window of a `temporalQ`, resolved against `now` (CIM 009 clause 4.11).
///
/// `timeAt` is either an absolute instant or an ISO 8601 duration, which a grant uses to
/// say "the last day" without being rewritten every day.
pub fn parse(temporal_q: &str, now: TimestampTz) -> Option<Window> {
    let mut timerel = None;
    let mut time_at = None;
    let mut end_time_at = None;
    for part in temporal_q.split([';', '&']) {
        match part.split_once('=') {
            Some(("timerel", value)) => timerel = Some(value.trim()),
            Some(("timeAt", value)) => time_at = instant(value.trim(), now),
            Some(("endTimeAt", value)) => end_time_at = instant(value.trim(), now),
            _ => {}
        }
    }

    match timerel? {
        "after" => Some(Window {
            from: Some(time_at?),
            to: None,
        }),
        "before" => Some(Window {
            from: None,
            to: Some(time_at?),
        }),
        "between" => Some(Window {
            from: Some(time_at?),
            to: Some(end_time_at?),
        }),
        // A relation the vocabulary does not know constrains nothing that can be checked,
        // and a window nobody can check must not be treated as one.
        _ => None,
    }
}

/// An absolute instant, or one relative to `now` when the grant is written as a duration.
fn instant(value: &str, now: TimestampTz) -> Option<TimestampTz> {
    if let Some(absolute) = parse_rfc3339(value) {
        return Some(TimestampTz(absolute));
    }
    // Checked: an offset past jiff's range (years -9999..=9999) is no instant, and the window
    // that names it is unreadable rather than a panic on the request path (F-PLT2-005).
    duration(value).and_then(|offset| now.checked_add(offset))
}

/// An ISO 8601 duration as a signed offset: `P-1D` and `-P1D` are both "a day ago".
///
/// Only the units a retention window is written in are understood — days, hours, minutes,
/// seconds and weeks. A duration carrying months or years has no fixed length and is
/// refused rather than approximated, because a policy boundary that drifts by three days
/// depending on the month is not a boundary. The sum is counted in milliseconds within `i64`,
/// the range chrono's `Duration` had, so a duration readable before is readable now.
fn duration(value: &str) -> Option<SignedDuration> {
    let (sign, rest) = match value.strip_prefix('-') {
        Some(rest) => (-1i32, rest),
        None => (1i32, value),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };

    let mut total: i64 = 0;
    let mut sign: i32 = sign;
    let mut digits = String::new();
    for (part, units) in [(date, "WD"), (time.unwrap_or_default(), "HMS")] {
        for character in part.chars() {
            match character {
                '-' if digits.is_empty() => sign = -sign,
                '0'..='9' => digits.push(character),
                unit if units.contains(unit) => {
                    let count: i64 = digits.parse().ok()?;
                    digits.clear();
                    let unit_ms: i64 = match unit {
                        'W' => 604_800_000,
                        'D' => 86_400_000,
                        'H' => 3_600_000,
                        'M' => 60_000,
                        _ => 1_000,
                    };
                    total = total.checked_add(count.checked_mul(unit_ms)?)?;
                }
                // A month or a year has no fixed length, and anything else is not a
                // duration at all.
                _ => return None,
            }
        }
    }

    (digits.is_empty() && total != 0).then(|| SignedDuration::from_millis(total) * sign)
}

fn stamp(at: TimestampTz) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The later of two lower bounds; an absent bound is the beginning of time.
fn max_bound(a: Option<TimestampTz>, b: Option<TimestampTz>) -> Option<TimestampTz> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (bound, None) | (None, bound) => bound,
    }
}

/// The earlier of two upper bounds; an absent bound is the end of time.
fn min_bound(a: Option<TimestampTz>, b: Option<TimestampTz>) -> Option<TimestampTz> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (bound, None) | (None, bound) => bound,
    }
}

/// The upper bound of a hull: one open end makes the whole hull open.
fn to_bound(bounds: impl Iterator<Item = Option<TimestampTz>>) -> Option<TimestampTz> {
    let mut highest: Option<TimestampTz> = None;
    for bound in bounds {
        let at = bound?;
        highest = Some(highest.map_or(at, |current| current.max(at)));
    }
    highest
}

#[cfg(test)]
mod overflow_tests {
    use super::*;

    /// The forwarded `temporalQ` reads and writes its instants as the chrono build did
    /// (T-7253): offsets of the DST nights of Bratislava and Helsinki folded to `Z`, to the
    /// second, a relative grant resolved against `now`. Strings pasted from the chrono build.
    #[test]
    fn the_forwarded_window_keeps_the_chrono_wire_format() {
        let now = "2026-10-25T01:30:00.987Z".parse().expect("an instant");
        let after = |at: &str| {
            parse(&format!("timerel=after;timeAt={at}"), now).and_then(Window::to_temporal_q)
        };
        let golden = [
            ("2026-03-29T03:30:00.5+02:00", Some("2026-03-29T01:30:00Z")),
            ("2026-03-29T01:30:00+01:00", Some("2026-03-29T00:30:00Z")),
            ("2026-10-25T02:30:00+03:00", Some("2026-10-24T23:30:00Z")),
            ("2026-10-25T02:30:00+02:00", Some("2026-10-25T00:30:00Z")),
            ("2026-10-25 01:30:00Z", Some("2026-10-25T01:30:00Z")),
            ("2026-01-01T00:00:00+0100", None),
            ("2026-01-01T00:00:00", None),
            // chrono forwarded a leap second as `23:59:60Z`; jiff has none and forwards the
            // next second, the one instant later.
            ("2026-06-30T23:59:60Z", Some("2026-07-01T00:00:00Z")),
            ("-P1D", Some("2026-10-24T01:30:00Z")),
            ("PT90M", Some("2026-10-25T03:00:00Z")),
            ("-P2W", Some("2026-10-11T01:30:00Z")),
            ("P1DT12H", Some("2026-10-26T13:30:00Z")),
            ("P1M", None),
            ("P0D", None),
            // chrono reached year 10240 (`+10240-07-15T01:30:00Z`, which the broker refuses);
            // jiff ends at 9999, so the grant is unreadable and admits nothing.
            ("P3000000D", None),
        ];
        for (at, want) in golden {
            let want = want.map(|w| format!("timerel=after;timeAt={w}"));
            assert_eq!(after(at), want, "{at}");
        }
    }

    /// F-PLT2-005: an offset past jiff's range is no instant, and a window that names one
    /// clamps without a panic.
    #[test]
    fn an_offset_out_of_range_is_no_instant() {
        let now = TimestampTz::now();
        assert_eq!(instant("P100000000D", now), None);
        assert_eq!(instant("P99999999999W", now), None);
        assert!(duration("P15000000000W").is_some());
        assert_eq!(
            duration("P15000000000W10000000000D"),
            None,
            "the sum overflows"
        );
        let _ = clamp(Some("timerel=after;timeAt=P100000000D"), &[], now);
        let _ = clamp(None, &["timerel=after;timeAt=P100000000D"], now);
    }
}
