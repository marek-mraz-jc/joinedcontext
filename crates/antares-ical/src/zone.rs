// SPDX-License-Identifier: EUPL-1.2
//! The `VTIMEZONE` of a feed's zone (RFC 5545 3.6.5), read from the IANA database chrono-tz
//! carries: every offset change from the year before `year` to three years after it, each as
//! the `DTSTART` or an `RDATE` of its `STANDARD` or `DAYLIGHT` observance. A zone without a
//! change in that span is one `STANDARD` from 1970. The feed writes its instants in UTC, so the
//! block names the zone a calendar shows them in (`X-WR-TIMEZONE` says the same to the clients
//! that read only that) and keeps an empty calendar a valid iCalendar object (3.4: one
//! component at least).

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Offset as _, TimeZone as _, Utc};
use chrono_tz::{OffsetComponents as _, OffsetName as _, Tz};
use std::collections::BTreeMap;

/// One observance: its kind, its offsets before and after, and its name.
type Observance = (&'static str, i32, i32, String);

/// What is in force at `t`: the offset in seconds, the daylight flag and the abbreviation.
fn state(zone: Tz, t: NaiveDateTime) -> (i32, bool, String) {
    let o = zone.offset_from_utc_datetime(&t);
    (
        o.fix().local_minus_utc(),
        o.dst_offset() != Duration::zero(),
        o.abbreviation().unwrap_or_default().to_owned(),
    )
}

fn at(seconds: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp(seconds, 0).map(|t| t.naive_utc())
}

/// `+0100`, `-0330`, `+053328`: RFC 5545 3.3.14 `utc-offset`.
fn offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let s = seconds.unsigned_abs();
    let (h, m, r) = (s / 3600, s % 3600 / 60, s % 60);
    if r == 0 {
        format!("{sign}{h:02}{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}{r:02}")
    }
}

/// The first second at which `zone` leaves `before`, between `lo` and `hi` (Unix seconds).
fn change(zone: Tz, before: &(i32, bool, String), mut lo: i64, mut hi: i64) -> i64 {
    while hi.saturating_sub(lo) > 1 {
        let mid = lo.saturating_add(hi.saturating_sub(lo).checked_div(2).unwrap_or(0));
        match at(mid) {
            Some(t) if state(zone, t) == *before => lo = mid,
            _ => hi = mid,
        }
    }
    hi
}

/// The `VTIMEZONE` lines of `zone`, unfolded.
#[must_use]
pub fn vtimezone(zone: Tz, year: i32) -> Vec<String> {
    let first = NaiveDate::from_ymd_opt(year.saturating_sub(1), 1, 1).unwrap_or_default();
    let days = NaiveDate::from_ymd_opt(year.saturating_add(3), 1, 1)
        .map_or(0, |last| last.signed_duration_since(first).num_days());
    // each observance: its first onset and the later ones
    let mut onsets: BTreeMap<Observance, (NaiveDateTime, Vec<NaiveDateTime>)> = BTreeMap::new();
    let start = first.and_hms_opt(0, 0, 0).unwrap_or_default();
    let mut now = state(zone, start);
    let mut add = |(off, dst, name): &(i32, bool, String), from: i32, utc: NaiveDateTime| {
        let kind = if *dst { "DAYLIGHT" } else { "STANDARD" };
        let local = utc
            .checked_add_signed(Duration::seconds(i64::from(from)))
            .unwrap_or(utc);
        onsets
            .entry((kind, from, *off, name.clone()))
            .and_modify(|(_, later)| later.push(local))
            .or_insert((local, Vec::new()));
    };
    // what holds when the span opens, as an onset at its first second
    add(&now, now.0, start);
    let mut day = start.and_utc().timestamp();
    for _ in 0..days {
        let next = day.saturating_add(86_400);
        let then = state(zone, at(next).unwrap_or_default());
        if then != now {
            let utc = at(change(zone, &now, day, next)).unwrap_or_default();
            let to = state(zone, utc);
            add(&to, now.0, utc);
            now = to;
        }
        day = next;
    }
    let mut groups: Vec<_> = onsets.iter().collect();
    groups.sort_by_key(|(_, (head, _))| *head);
    let stamp = |t: &NaiveDateTime| t.format("%Y%m%dT%H%M%S").to_string();
    let mut lines = vec![
        "BEGIN:VTIMEZONE".to_owned(),
        format!("TZID:{}", zone.name()),
    ];
    for ((kind, from, to, name), (head, rest)) in groups {
        lines.push(format!("BEGIN:{kind}"));
        lines.push(format!("DTSTART:{}", stamp(head)));
        if !rest.is_empty() {
            let all: Vec<String> = rest.iter().map(stamp).collect();
            lines.push(format!("RDATE:{}", all.join(",")));
        }
        lines.push(format!("TZOFFSETFROM:{}", offset(*from)));
        lines.push(format!("TZOFFSETTO:{}", offset(*to)));
        // chrono-tz names every offset (`CET`, `+03`)
        lines.push(format!("TZNAME:{}", crate::mapping::escape(name)));
        lines.push(format!("END:{kind}"));
    }
    lines.push("END:VTIMEZONE".to_owned());
    lines
}

/// The year `now` falls in, for [`vtimezone`].
#[must_use]
pub fn year(now: DateTime<Utc>) -> i32 {
    use chrono::Datelike as _;
    now.year()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bratislava_changes_twice_a_year_on_the_last_sundays() {
        let lines = vtimezone(chrono_tz::Europe::Bratislava, 2026);
        let text = lines.join("\n");
        assert!(text.contains("TZID:Europe/Bratislava"), "{text}");
        // 2026-03-29 01:00Z: 02:00 CET becomes 03:00 CEST
        assert!(text.contains("20260329T020000"), "{text}");
        // 2026-10-25 01:00Z: 03:00 CEST becomes 02:00 CET
        assert!(text.contains("20261025T030000"), "{text}");
        assert!(
            text.contains("TZOFFSETFROM:+0100\nTZOFFSETTO:+0200\nTZNAME:CEST"),
            "{text}"
        );
        assert!(
            text.contains("TZOFFSETFROM:+0200\nTZOFFSETTO:+0100\nTZNAME:CET"),
            "{text}"
        );
    }

    #[test]
    fn a_zone_without_changes_is_one_standard_observance() {
        let lines = vtimezone(chrono_tz::UTC, 2026);
        assert_eq!(
            lines,
            [
                "BEGIN:VTIMEZONE",
                "TZID:UTC",
                "BEGIN:STANDARD",
                "DTSTART:20250101T000000",
                "TZOFFSETFROM:+0000",
                "TZOFFSETTO:+0000",
                "TZNAME:UTC",
                "END:STANDARD",
                "END:VTIMEZONE"
            ]
        );
        assert_eq!(offset(-12_600), "-0330");
        assert_eq!(offset(19_808), "+053008");
    }
}
