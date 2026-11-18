// SPDX-License-Identifier: EUPL-1.2
//! An IANA time zone from jiff's bundled database, usable as a chrono [`TimeZone`] (T-7233: in
//! place of chrono-tz, RUSTSEC-2026-0555). The rest of the crate keeps its chrono arithmetic.
//! A [`Zone`] is `Copy` like `chrono_tz::Tz`: each name is looked up once and kept for the
//! process. ponytail: the kept set is bounded by the database (about 600 names, and only names
//! that resolve are kept); drop this module when chrono goes (T-7233's plan).

use chrono::{FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone};
use jiff::tz::{AmbiguousOffset, TimeZone as JiffZone};
use std::collections::HashMap;
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

    /// The jiff zone, for what chrono has no word for (abbreviations, the daylight flag).
    #[must_use]
    pub fn jiff(&self) -> &'static JiffZone {
        &self.0.tz
    }

    /// The offset in force at the UTC instant `utc`. jiff covers the years -9999 to 9999; an
    /// instant outside them takes the offset of the nearest end.
    fn fixed_at_utc(self, utc: &NaiveDateTime) -> FixedOffset {
        let ts = jiff::Timestamp::new(utc.and_utc().timestamp(), 0).unwrap_or(
            if utc.and_utc().timestamp() < 0 {
                jiff::Timestamp::MIN
            } else {
                jiff::Timestamp::MAX
            },
        );
        fixed(self.0.tz.to_offset(ts).seconds())
    }
}

fn fixed(seconds: i32) -> FixedOffset {
    // jiff offsets stay within +-25:59:59, chrono's within +-23:59:59; real zones within +-15 h
    FixedOffset::east_opt(seconds).unwrap_or_else(|| chrono::Utc.fix())
}

impl FromStr for Zone {
    type Err = UnknownZone;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        static KEPT: OnceLock<Mutex<HashMap<String, &'static Entry>>> = OnceLock::new();
        let unknown = || UnknownZone(name.to_owned());
        let mut kept = KEPT
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = kept.get(name) {
            return Ok(Self(entry));
        }
        // the bundled database, never the host's: /usr/share/zoneinfo differs between images
        // (Debian moves `US/Eastern` and the other backward links to tzdata-legacy)
        static BUNDLED: OnceLock<jiff::tz::TimeZoneDatabase> = OnceLock::new();
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

/// The offset of a [`Zone`] at one instant.
#[derive(Clone, Copy, Debug)]
pub struct ZoneOffset {
    zone: Zone,
    fix: FixedOffset,
}

impl Offset for ZoneOffset {
    fn fix(&self) -> FixedOffset {
        self.fix
    }
}

impl fmt::Display for ZoneOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.fix, f)
    }
}

/// `local` as a jiff civil time; `None` past jiff's years -9999 to 9999.
fn civil(local: &NaiveDateTime) -> Option<jiff::civil::DateTime> {
    use chrono::{Datelike as _, Timelike as _};
    jiff::civil::DateTime::new(
        i16::try_from(local.year()).ok()?,
        i8::try_from(local.month()).ok()?,
        i8::try_from(local.day()).ok()?,
        i8::try_from(local.hour()).ok()?,
        i8::try_from(local.minute()).ok()?,
        i8::try_from(local.second()).ok()?,
        0,
    )
    .ok()
}

impl TimeZone for Zone {
    type Offset = ZoneOffset;

    fn from_offset(offset: &ZoneOffset) -> Self {
        offset.zone
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<ZoneOffset> {
        self.offset_from_local_datetime(&local.and_time(NaiveTime::MIN))
    }

    /// `None` for a wall time the zone skips, `Ambiguous(earlier, later)` for one it repeats.
    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<ZoneOffset> {
        let Some(civil) = civil(local) else {
            return MappedLocalTime::None;
        };
        let at = |o: jiff::tz::Offset| ZoneOffset {
            zone: *self,
            fix: fixed(o.seconds()),
        };
        match self.0.tz.to_ambiguous_timestamp(civil).offset() {
            AmbiguousOffset::Unambiguous { offset } => MappedLocalTime::Single(at(offset)),
            AmbiguousOffset::Gap { .. } => MappedLocalTime::None,
            AmbiguousOffset::Fold { before, after } => {
                MappedLocalTime::Ambiguous(at(before), at(after))
            }
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> ZoneOffset {
        self.offset_from_utc_datetime(&utc.and_time(NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> ZoneOffset {
        ZoneOffset {
            zone: *self,
            fix: self.fixed_at_utc(utc),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn naive(s: &str) -> Result<NaiveDateTime, String> {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").map_err(|e| e.to_string())
    }

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

    /// DST in both test zones: the spring gap, the autumn fold, the offsets on either side.
    #[test]
    fn dst_crossings_in_bratislava_and_helsinki() -> Result<(), String> {
        for (name, gap, fold, winter, summer) in [
            (
                "Europe/Bratislava",
                "2026-03-29T02:30:00",
                "2026-10-25T02:30:00",
                3600,
                7200,
            ),
            (
                "Europe/Helsinki",
                "2026-03-29T03:30:00",
                "2026-10-25T03:30:00",
                7200,
                10800,
            ),
        ] {
            let z: Zone = name.parse().map_err(|e: UnknownZone| e.to_string())?;
            assert!(
                z.from_local_datetime(&naive(gap)?).earliest().is_none(),
                "{name} gap"
            );
            let MappedLocalTime::Ambiguous(early, late) = z.from_local_datetime(&naive(fold)?)
            else {
                return Err(format!("{name}: the fold is not ambiguous"));
            };
            assert_eq!(
                early.offset().fix().local_minus_utc(),
                summer,
                "{name} fold early"
            );
            assert_eq!(
                late.offset().fix().local_minus_utc(),
                winter,
                "{name} fold late"
            );
            assert_eq!(
                (late.to_utc() - early.to_utc()).num_seconds(),
                3600,
                "{name}"
            );
            let jan = naive("2026-01-15T12:00:00")?.and_utc().with_timezone(&z);
            let jul = naive("2026-07-15T12:00:00")?.and_utc().with_timezone(&z);
            assert_eq!(
                jan.offset().fix().local_minus_utc(),
                winter,
                "{name} January"
            );
            assert_eq!(jul.offset().fix().local_minus_utc(), summer, "{name} July");
            assert_eq!(
                jul.with_timezone(&Utc),
                naive("2026-07-15T12:00:00")?.and_utc()
            );
        }
        Ok(())
    }
}
