// SPDX-License-Identifier: EUPL-1.2
//! A feed's IANA zone and its `VTIMEZONE` (RFC 5545 3.6.5), both from the IANA database jiff
//! bundles. A [`Zone`] is `Copy`: each name is looked up once and kept for the process.
//! ponytail: the kept set is bounded by the database (about 600 names, and only names that
//! resolve are kept).
//!
//! The `VTIMEZONE` holds every offset change from the year before `year` to three years after
//! it, each as the `DTSTART` or an `RDATE` of its `STANDARD` or `DAYLIGHT` observance. A zone
//! without a change in that span is one `STANDARD` from 1970. The feed writes its instants in
//! UTC, so the block names the zone a calendar shows them in (`X-WR-TIMEZONE` says the same to
//! the clients that read only that) and keeps an empty calendar a valid iCalendar object (3.4:
//! one component at least).

use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone as JiffZone};
use jiff::{SignedDuration, Timestamp};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock, PoisonError};

/// A zone and the name it was asked by.
#[derive(Debug)]
pub struct Entry {
    name: String,
    tz: JiffZone,
}

static UTC_ENTRY: OnceLock<Entry> = OnceLock::new();

/// An IANA time zone.
#[derive(Clone, Copy)]
pub struct Zone(&'static Entry);

/// A name the IANA database does not hold (names are case-sensitive, as in chrono-tz).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownZone(pub String);

impl fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown time zone {}", self.0)
    }
}

impl std::error::Error for UnknownZone {}

impl Zone {
    /// UTC.
    #[must_use]
    pub fn utc() -> Self {
        Self(UTC_ENTRY.get_or_init(|| Entry {
            name: "UTC".to_owned(),
            tz: JiffZone::UTC,
        }))
    }

    /// The IANA name, as written when parsed (`Europe/Bratislava`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        &self.0.name
    }

    /// The UTC wall time of the local wall time `local`, the earlier of a doubled hour;
    /// `None` for a time the zone skips (chrono's `from_local_datetime(..).earliest()`).
    #[must_use]
    pub fn earliest_utc(self, local: DateTime) -> Option<DateTime> {
        let offset = match self.0.tz.to_ambiguous_timestamp(local).offset() {
            AmbiguousOffset::Unambiguous { offset } => offset,
            AmbiguousOffset::Fold { before, .. } => before,
            AmbiguousOffset::Gap { .. } => return None,
        };
        local
            .checked_sub(SignedDuration::from_secs(i64::from(offset.seconds())))
            .ok()
    }

    /// The local wall time at the UTC wall time `utc`. jiff covers the years -9999 to 9999;
    /// an instant outside them takes the offset of the nearest end.
    #[must_use]
    pub fn local(self, utc: DateTime) -> DateTime {
        let at = Offset::UTC.to_timestamp(utc).unwrap_or(if utc.year() < 0 {
            Timestamp::MIN
        } else {
            Timestamp::MAX
        });
        let seconds = self.0.tz.to_offset(at).seconds();
        utc.checked_add(SignedDuration::from_secs(i64::from(seconds)))
            .unwrap_or(utc)
    }
}

impl FromStr for Zone {
    type Err = UnknownZone;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        static KEPT: OnceLock<Mutex<HashMap<String, &'static Entry>>> = OnceLock::new();
        // the bundled database, never the host's: /usr/share/zoneinfo differs between images
        // (Debian moves `US/Eastern` and the other backward links to tzdata-legacy)
        static BUNDLED: OnceLock<jiff::tz::TimeZoneDatabase> = OnceLock::new();
        let unknown = || UnknownZone(name.to_owned());
        let mut kept = KEPT
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = kept.get(name) {
            return Ok(Self(entry));
        }
        let tz = BUNDLED
            .get_or_init(jiff::tz::TimeZoneDatabase::bundled)
            .get(name)
            .map_err(|_| unknown())?;
        // jiff matches names case-insensitively; chrono-tz did not, and neither does this.
        if tz.iana_name() != Some(name) {
            return Err(unknown());
        }
        let entry: &'static Entry = Box::leak(Box::new(Entry {
            name: name.to_owned(),
            tz,
        }));
        kept.insert(name.to_owned(), entry);
        Ok(Self(entry))
    }
}

impl PartialEq for Zone {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for Zone {}

impl fmt::Debug for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One observance: its kind, its offsets before and after, and its name.
type Observance = (&'static str, i32, i32, String);

/// What is in force at `t`: the offset in seconds, the daylight flag and the abbreviation.
fn state(zone: Zone, seconds: i64) -> (i32, bool, String) {
    let instant = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    let o = zone.0.tz.to_offset_info(instant);
    (
        o.offset().seconds(),
        o.dst().is_dst(),
        o.abbreviation().to_owned(),
    )
}

/// The UTC wall time `seconds` after the Unix epoch.
fn at(seconds: i64) -> DateTime {
    Offset::UTC.to_datetime(Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH))
}

/// Seconds after the Unix epoch of the UTC wall time `t`.
fn seconds(t: DateTime) -> i64 {
    Offset::UTC
        .to_timestamp(t)
        .map_or(0, Timestamp::as_second)
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
fn change(zone: Zone, before: &(i32, bool, String), mut lo: i64, mut hi: i64) -> i64 {
    while hi.saturating_sub(lo) > 1 {
        let mid = lo.saturating_add(hi.saturating_sub(lo).checked_div(2).unwrap_or(0));
        if state(zone, mid) == *before {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// The `VTIMEZONE` lines of `zone`, unfolded.
#[must_use]
pub fn vtimezone(zone: Zone, year: i32) -> Vec<String> {
    let january = |y: i32| {
        i16::try_from(y)
            .ok()
            .and_then(|y| Date::new(y, 1, 1).ok())
            .map(|d| d.to_datetime(jiff::civil::Time::midnight()))
    };
    let start =
        january(year.saturating_sub(1)).unwrap_or(DateTime::constant(1970, 1, 1, 0, 0, 0, 0));
    let days = january(year.saturating_add(3)).map_or(0, |last| {
        seconds(last).saturating_sub(seconds(start)) / 86_400
    });
    // each observance: its first onset and the later ones
    let mut onsets: BTreeMap<Observance, (DateTime, Vec<DateTime>)> = BTreeMap::new();
    let mut now = state(zone, seconds(start));
    let mut add = |(off, dst, name): &(i32, bool, String), from: i32, utc: DateTime| {
        let kind = if *dst { "DAYLIGHT" } else { "STANDARD" };
        let local = utc
            .checked_add(SignedDuration::from_secs(i64::from(from)))
            .unwrap_or(utc);
        onsets
            .entry((kind, from, *off, name.clone()))
            .and_modify(|(_, later)| later.push(local))
            .or_insert((local, Vec::new()));
    };
    // what holds when the span opens, as an onset at its first second
    add(&now, now.0, start);
    let mut day = seconds(start);
    for _ in 0..days {
        let next = day.saturating_add(86_400);
        let then = state(zone, next);
        if then != now {
            let onset = change(zone, &now, day, next);
            let utc = at(onset);
            let to = state(zone, onset);
            add(&to, now.0, utc);
            now = to;
        }
        day = next;
    }
    let mut groups: Vec<_> = onsets.iter().collect();
    groups.sort_by_key(|(_, (head, _))| *head);
    let stamp = |t: &DateTime| t.strftime("%Y%m%dT%H%M%S").to_string();
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
        // the IANA database names every offset (`CET`, `+03`)
        lines.push(format!("TZNAME:{}", crate::mapping::escape(name)));
        lines.push(format!("END:{kind}"));
    }
    lines.push("END:VTIMEZONE".to_owned());
    lines
}

/// The year `now` falls in, for [`vtimezone`].
#[must_use]
pub fn year(now: Timestamp) -> i32 {
    i32::from(Offset::UTC.to_datetime(now).year())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_iana_and_case_sensitive() -> Result<(), UnknownZone> {
        let z: Zone = "Europe/Bratislava".parse()?;
        assert_eq!(z.name(), "Europe/Bratislava");
        assert_eq!(z, "Europe/Bratislava".parse()?);
        assert!("europe/bratislava".parse::<Zone>().is_err());
        assert!("Mars/Olympus".parse::<Zone>().is_err());
        assert_eq!("US/Eastern".parse::<Zone>()?.name(), "US/Eastern");
        assert_eq!(Zone::utc().name(), "UTC");
        Ok(())
    }

    #[test]
    fn default_zone_is_bratislava() {
        assert_eq!(crate::mapping::default_zone().name(), "Europe/Bratislava");
    }

    /// DST in both test zones: the spring gap has no time, the autumn fold reads as its
    /// earlier instant, and a UTC wall time shows in the offset in force at it.
    #[test]
    fn dst_crossings_in_bratislava_and_helsinki() -> Result<(), UnknownZone> {
        let t = |s: &str| s.parse::<DateTime>().unwrap_or_default();
        for (name, gap, fold, fold_utc, summer_utc, summer_local) in [
            (
                "Europe/Bratislava",
                "2026-03-29T02:30",
                "2026-10-25T02:30",
                "2026-10-25T00:30",
                "2026-07-01T12:00",
                "2026-07-01T14:00",
            ),
            (
                "Europe/Helsinki",
                "2026-03-29T03:30",
                "2026-10-25T03:30",
                "2026-10-25T00:30",
                "2026-07-01T12:00",
                "2026-07-01T15:00",
            ),
        ] {
            let z: Zone = name.parse()?;
            assert_eq!(z.earliest_utc(t(gap)), None, "{name}");
            assert_eq!(z.earliest_utc(t(fold)), Some(t(fold_utc)), "{name}");
            assert_eq!(z.local(t(summer_utc)), t(summer_local), "{name}");
            assert_eq!(z.local(t(fold_utc)), t(fold), "{name}");
        }
        Ok(())
    }

    #[test]
    fn bratislava_changes_twice_a_year_on_the_last_sundays() {
        let lines = vtimezone(
            "Europe/Bratislava".parse().unwrap_or_else(|_| Zone::utc()),
            2026,
        );
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
        let lines = vtimezone(Zone::utc(), 2026);
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

#[cfg(test)]
mod wire_golden {
    use super::*;

    /// T-7248: whole `VTIMEZONE` blocks as the chrono build wrote them, for the zones the
    /// platform serves and three odd ones (a half-hour DST, a -03:30 zone, no change).
    #[test]
    fn vtimezone_keeps_the_chrono_wire_format() {
        let got: Vec<String> = [
            ("Europe/Bratislava", 2026),
            ("Europe/Helsinki", 2026),
            ("Australia/Lord_Howe", 2026),
            ("America/St_Johns", 2027),
            ("Asia/Kolkata", 2026),
        ]
        .iter()
        .map(|(name, year)| {
            vtimezone(name.parse().unwrap_or_else(|_| Zone::utc()), *year).join("\n")
        })
        .collect();
        assert_eq!(
            got.join("\n--\n").trim(),
            BLOCKS.trim(),
            "\n{}",
            got.join("\n--\n")
        );
    }

    const BLOCKS: &str = r"
BEGIN:VTIMEZONE
TZID:Europe/Bratislava
BEGIN:STANDARD
DTSTART:20250101T010000
TZOFFSETFROM:+0100
TZOFFSETTO:+0100
TZNAME:CET
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:20250330T020000
RDATE:20260329T020000,20270328T020000,20280326T020000
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
TZNAME:CEST
END:DAYLIGHT
BEGIN:STANDARD
DTSTART:20251026T030000
RDATE:20261025T030000,20271031T030000,20281029T030000
TZOFFSETFROM:+0200
TZOFFSETTO:+0100
TZNAME:CET
END:STANDARD
END:VTIMEZONE
--
BEGIN:VTIMEZONE
TZID:Europe/Helsinki
BEGIN:STANDARD
DTSTART:20250101T020000
TZOFFSETFROM:+0200
TZOFFSETTO:+0200
TZNAME:EET
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:20250330T030000
RDATE:20260329T030000,20270328T030000,20280326T030000
TZOFFSETFROM:+0200
TZOFFSETTO:+0300
TZNAME:EEST
END:DAYLIGHT
BEGIN:STANDARD
DTSTART:20251026T040000
RDATE:20261025T040000,20271031T040000,20281029T040000
TZOFFSETFROM:+0300
TZOFFSETTO:+0200
TZNAME:EET
END:STANDARD
END:VTIMEZONE
--
BEGIN:VTIMEZONE
TZID:Australia/Lord_Howe
BEGIN:DAYLIGHT
DTSTART:20250101T110000
TZOFFSETFROM:+1100
TZOFFSETTO:+1100
TZNAME:+11
END:DAYLIGHT
BEGIN:STANDARD
DTSTART:20250406T020000
RDATE:20260405T020000,20270404T020000,20280402T020000
TZOFFSETFROM:+1100
TZOFFSETTO:+1030
TZNAME:+1030
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:20251005T020000
RDATE:20261004T020000,20271003T020000,20281001T020000
TZOFFSETFROM:+1030
TZOFFSETTO:+1100
TZNAME:+11
END:DAYLIGHT
END:VTIMEZONE
--
BEGIN:VTIMEZONE
TZID:America/St_Johns
BEGIN:STANDARD
DTSTART:20251231T203000
TZOFFSETFROM:-0330
TZOFFSETTO:-0330
TZNAME:NST
END:STANDARD
BEGIN:DAYLIGHT
DTSTART:20260308T020000
RDATE:20270314T020000,20280312T020000,20290311T020000
TZOFFSETFROM:-0330
TZOFFSETTO:-0230
TZNAME:NDT
END:DAYLIGHT
BEGIN:STANDARD
DTSTART:20261101T020000
RDATE:20271107T020000,20281105T020000,20291104T020000
TZOFFSETFROM:-0230
TZOFFSETTO:-0330
TZNAME:NST
END:STANDARD
END:VTIMEZONE
--
BEGIN:VTIMEZONE
TZID:Asia/Kolkata
BEGIN:STANDARD
DTSTART:20250101T053000
TZOFFSETFROM:+0530
TZOFFSETTO:+0530
TZNAME:IST
END:STANDARD
END:VTIMEZONE
";
}
