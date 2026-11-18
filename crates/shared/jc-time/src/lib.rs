#![forbid(unsafe_code)]
//! [`TimestampTz`]: an instant on [`jiff`] that keeps chrono's wire formats byte for byte
//! (T-7233 plan, RUSTSEC-2026-0543).
//!
//! - JSON: RFC 3339 in UTC with `Z` and 0, 3, 6 or 9 fraction digits, as chrono's
//!   `DateTime<Utc>` serialized (`SecondsFormat::AutoSi`).
//! - [`format_rfc3339`], [`TimestampTz::to_rfc3339`], [`TimestampTz::to_rfc3339_opts`]: chrono's
//!   methods of the same names, same digits.
//! - [`parse_rfc3339`]: chrono's `DateTime::parse_from_rfc3339` grammar, no wider.
//! - feature `schemars`: chrono's JSON Schema (`string`, `date-time`).
//! - feature `sqlx`: Postgres `timestamptz` (sqlx 0.9 has no jiff feature), plus [`PgTimestamp`]
//!   and [`PgDate`] for `timestamp` and `date`.

use jiff::fmt::temporal::DateTimePrinter;
use jiff::tz::Offset;
use jiff::{SignedDuration, Timestamp, civil};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// An instant in UTC, stored and serialized as chrono's `DateTime<Utc>` was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimestampTz(pub Timestamp);

impl TimestampTz {
    /// The 1970-01-01T00:00:00Z instant (chrono's `DateTime::<Utc>::default()`).
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    /// The current system time.
    #[must_use]
    pub fn now() -> Self {
        Self(Timestamp::now())
    }

    /// The instant `ms` milliseconds after the Unix epoch; `None` outside jiff's range
    /// (years -9999..=9999).
    #[must_use]
    pub fn from_millis(ms: i64) -> Option<Self> {
        Timestamp::from_millisecond(ms).ok().map(Self)
    }

    /// The instant `s` whole seconds after the Unix epoch; `None` outside jiff's range.
    #[must_use]
    pub fn from_second(s: i64) -> Option<Self> {
        Timestamp::from_second(s).ok().map(Self)
    }

    /// Milliseconds since the Unix epoch (chrono's `timestamp_millis`).
    #[must_use]
    pub fn as_millis(self) -> i64 {
        self.0.as_millisecond()
    }

    /// Whole seconds since the Unix epoch (chrono's `timestamp`).
    #[must_use]
    pub fn as_second(self) -> i64 {
        self.0.as_second()
    }

    /// The instant at midnight UTC of `date` (chrono's `and_hms(0, 0, 0).and_utc()`).
    #[must_use]
    pub fn from_date(date: civil::Date) -> Option<Self> {
        Offset::UTC
            .to_timestamp(date.to_datetime(civil::Time::midnight()))
            .ok()
            .map(Self)
    }

    /// The calendar date and wall time in UTC (chrono's `Datelike`/`Timelike` on `DateTime<Utc>`).
    #[must_use]
    pub fn utc(self) -> civil::DateTime {
        Offset::UTC.to_datetime(self.0)
    }

    /// `self + d`; `None` outside jiff's range (chrono's `checked_add_signed`).
    #[must_use]
    pub fn checked_add(self, d: SignedDuration) -> Option<Self> {
        self.0.checked_add(d).ok().map(Self)
    }

    /// `self - d`; `None` outside jiff's range (chrono's `checked_sub_signed`).
    #[must_use]
    pub fn checked_sub(self, d: SignedDuration) -> Option<Self> {
        self.0.checked_sub(d).ok().map(Self)
    }

    /// RFC 3339 in UTC with `+00:00` and `AutoSi` digits (chrono's `to_rfc3339()`).
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        format_rfc3339(self.0, SecondsFormat::AutoSi, false)
    }

    /// RFC 3339 in UTC, `Z` when `use_z` (chrono's `to_rfc3339_opts(digits, use_z)`).
    #[must_use]
    pub fn to_rfc3339_opts(self, digits: SecondsFormat, use_z: bool) -> String {
        format_rfc3339(self.0, digits, use_z)
    }
}

/// The Unix epoch, as chrono's `DateTime::<Utc>::default()`.
impl Default for TimestampTz {
    fn default() -> Self {
        Self::UNIX_EPOCH
    }
}

impl From<Timestamp> for TimestampTz {
    fn from(ts: Timestamp) -> Self {
        Self(ts)
    }
}

impl From<TimestampTz> for Timestamp {
    fn from(ts: TimestampTz) -> Self {
        ts.0
    }
}

/// Fraction digits of the seconds, as chrono's `SecondsFormat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecondsFormat {
    Secs,
    Millis,
    Micros,
    Nanos,
    /// 0, 3, 6 or 9 digits: the fewest that hold the fraction exactly.
    AutoSi,
}

/// RFC 3339 in UTC, ending in `Z` when `use_z`, else `+00:00`; the digits as chrono's
/// `to_rfc3339_opts(digits, use_z)` (`to_rfc3339()` is `AutoSi` without `Z`).
#[must_use]
pub fn format_rfc3339(ts: Timestamp, digits: SecondsFormat, use_z: bool) -> String {
    let precision = match digits {
        SecondsFormat::Secs => 0,
        SecondsFormat::Millis => 3,
        SecondsFormat::Micros => 6,
        SecondsFormat::Nanos => 9,
        SecondsFormat::AutoSi => match ts.subsec_nanosecond() {
            0 => 0,
            n if n % 1_000_000 == 0 => 3,
            n if n % 1_000 == 0 => 6,
            _ => 9,
        },
    };
    let printer = DateTimePrinter::new().precision(Some(precision));
    if use_z {
        printer.timestamp_to_string(&ts)
    } else {
        printer.timestamp_with_offset_to_string(&ts, Offset::UTC)
    }
}

/// Parses RFC 3339 exactly as chrono's `DateTime::parse_from_rfc3339` did:
/// `YYYY-MM-DD`, `T`/`t`/space, `HH:MM:SS`, an optional `.` fraction (digits past the ninth
/// dropped), then `Z`/`z` or `±HH:MM`. Second 60 (a leap second) counts as the next second,
/// as chrono's millisecond value did. Anything else, such as a missing second, a basic
/// format or an RFC 9557 annotation (all of which jiff accepts), is `None`.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<Timestamp> {
    let b = s.as_bytes();
    let num = |at: usize, len: usize| -> Option<i32> {
        let digits = b.get(at..at + len)?;
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| digits.iter().fold(0, |n, d| n * 10 + i32::from(d - b'0')))
    };
    let shape = b.get(4) == Some(&b'-')
        && b.get(7) == Some(&b'-')
        && matches!(b.get(10), Some(b'T' | b't' | b' '))
        && b.get(13) == Some(&b':')
        && b.get(16) == Some(&b':');
    if !shape {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hour, minute, second) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut at = 19;
    let mut nanos = 0;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            if at - start < 9 {
                nanos = nanos * 10 + i32::from(b[at] - b'0');
            }
            at += 1;
        }
        if at == start {
            return None;
        }
        for _ in (at - start).min(9)..9 {
            nanos *= 10;
        }
    }
    let offset = match b.get(at..)? {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let (h, m) = (num(at + 1, 2)?, num(at + 4, 2)?);
            if h > 23 || m > 59 {
                return None;
            }
            let secs = h * 3600 + m * 60;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };
    let leap = second == 60;
    let dt = civil::DateTime::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        if leap { 59 } else { i8::try_from(second).ok()? },
        nanos,
    )
    .ok()?;
    let ts = Offset::from_seconds(offset).ok()?.to_timestamp(dt).ok()?;
    if leap {
        ts.checked_add(SignedDuration::from_secs(1)).ok()
    } else {
        Some(ts)
    }
}

/// Parses as chrono's `DateTime<FixedOffset>: FromStr` (its serde read path,
/// `parse_rfc3339_relaxed` in chrono 0.4.45) did: [`parse_rfc3339`] plus spaces around and
/// inside the fields, one-digit fields, a signed year, `UTC`, `+HHMM` or `+HH MM` and U+2212 as
/// the minus of an offset. A missing second, `+HH` alone, a comma fraction, a basic format and an
/// RFC 9557 annotation stay `None`, as in chrono.
#[must_use]
pub fn parse_rfc3339_relaxed(s: &str) -> Option<Timestamp> {
    // chrono's `scan::number`: `min..=max` ASCII digits, after the spaces a numeric field skips.
    fn number(s: &mut &str, max: usize) -> Option<i64> {
        *s = s.trim_start();
        let len = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
        let n = s.get(..len).filter(|d| !d.is_empty())?.parse().ok()?;
        *s = &s[len..];
        Some(n)
    }
    fn literal(s: &mut &str, lit: char) -> Option<()> {
        *s = s.trim_start().strip_prefix(lit)?;
        Some(())
    }
    let mut s = s;
    let year = {
        let t = s.trim_start();
        let signed = |r: &str| r.starts_with(|c: char| c.is_ascii_digit());
        if let Some(rest) = t.strip_prefix('-').filter(|r| signed(r)) {
            s = rest;
            -number(&mut s, usize::MAX)?
        } else if let Some(rest) = t.strip_prefix('+').filter(|r| signed(r)) {
            s = rest;
            number(&mut s, usize::MAX)?
        } else {
            number(&mut s, 4)?
        }
    };
    literal(&mut s, '-')?;
    let month = number(&mut s, 2)?;
    literal(&mut s, '-')?;
    let day = number(&mut s, 2)?;
    s = s.strip_prefix(['T', 't', ' '])?;
    let hour = number(&mut s, 2)?;
    literal(&mut s, ':')?;
    let minute = number(&mut s, 2)?;
    literal(&mut s, ':')?;
    let second = number(&mut s, 2)?;
    let mut nanos = 0;
    if let Some(rest) = s.strip_prefix('.') {
        s = rest;
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        let kept = digits.min(9);
        let frac: i64 = s.get(..kept).filter(|d| !d.is_empty())?.parse().ok()?;
        nanos = frac * 10_i64.pow(u32::try_from(9 - kept).ok()?);
        s = &s[digits..];
    }
    s = s.trim_start();
    let offset = if s.get(..3).is_some_and(|z| z.eq_ignore_ascii_case("UTC")) {
        s = &s[3..];
        0
    } else if let Some(rest) = s.strip_prefix(['Z', 'z']) {
        s = rest;
        0
    } else {
        let (negative, rest) = match s.chars().next()? {
            '+' => (false, &s[1..]),
            '-' => (true, &s[1..]),
            '\u{2212}' => (true, &s['\u{2212}'.len_utf8()..]),
            _ => return None,
        };
        let two = |t: &str| {
            t.get(..2)
                .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|d| d.parse::<i32>().ok())
        };
        let hours = two(rest)?;
        let rest = rest[2..].trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        let minutes = two(rest).filter(|m| *m < 60)?;
        s = &rest[2..];
        let secs = hours * 3600 + minutes * 60;
        if negative { -secs } else { secs }
    };
    if !s.trim_start().is_empty() || offset.abs() >= 86_400 {
        return None;
    }
    let small = |n: i64| i8::try_from(n).ok();
    let date = civil::Date::new(i16::try_from(year).ok()?, small(month)?, small(day)?).ok()?;
    // A leap second (60) is the next second, as chrono's instant for it was.
    let leap = second == 60;
    let time = civil::Time::new(
        small(hour)?,
        small(minute)?,
        small(if leap { 59 } else { second })?,
        i32::try_from(nanos).ok()?,
    )
    .ok()?;
    let ts = Offset::from_seconds(offset)
        .ok()?
        .to_timestamp(date.to_datetime(time))
        .ok()?;
    if leap {
        ts.checked_add(SignedDuration::from_secs(1)).ok()
    } else {
        Some(ts)
    }
}

impl Serialize for TimestampTz {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_rfc3339(self.0, SecondsFormat::AutoSi, true))
    }
}

impl<'de> Deserialize<'de> for TimestampTz {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = TimestampTz;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an RFC 3339 formatted date and time string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<TimestampTz, E> {
                parse_rfc3339_relaxed(v)
                    .map(TimestampTz)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for TimestampTz {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "DateTime".to_owned()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            format: Some("date-time".to_owned()),
            ..Default::default()
        }
        .into()
    }
}

/// A calendar date that writes and reads JSON `YYYY-MM-DD` as chrono's `NaiveDate` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub civil::Date);

impl Date {
    /// The date `year-month-day`; `None` when it does not exist (chrono's `from_ymd_opt`).
    #[must_use]
    pub fn new(year: i16, month: i8, day: i8) -> Option<Self> {
        civil::Date::new(year, month, day).ok().map(Self)
    }
}

/// `YYYY-MM-DD`, the year in at least four digits and `-0001` before year 0, as chrono printed.
impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = (self.0.year(), self.0.month(), self.0.day());
        if y < 0 {
            write!(f, "{y:05}-{m:02}-{d:02}")
        } else {
            write!(f, "{y:04}-{m:02}-{d:02}")
        }
    }
}

/// Parses `%Y-%m-%d` as chrono's `NaiveDate::parse_from_str(s, "%Y-%m-%d")` did: leading
/// spaces, a sign and one-digit months and days pass; a time, a trailing character or a day
/// that does not exist is `None`.
#[must_use]
pub fn parse_date(s: &str) -> Option<civil::Date> {
    jiff::fmt::strtime::parse("%Y-%m-%d", s)
        .and_then(|t| t.to_date())
        .ok()
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = Date;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a formatted date string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Date, E> {
                // chrono's serde read through `FromStr`, which also took trailing spaces.
                parse_date(v.trim_end())
                    .map(Date)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Date {
    fn is_referenceable() -> bool {
        false
    }

    fn schema_name() -> String {
        "Date".to_owned()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            format: Some("date".to_owned()),
            ..Default::default()
        }
        .into()
    }
}

/// A Postgres `timestamp` (no zone) read or written through sqlx.
#[cfg(feature = "sqlx")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PgTimestamp(pub civil::DateTime);

/// A Postgres `date` read or written through sqlx.
#[cfg(feature = "sqlx")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PgDate(pub civil::Date);

#[cfg(feature = "sqlx")]
mod pg {
    //! Postgres binary formats: `timestamp(tz)` is i64 microseconds and `date` i32 days since
    //! 2000-01-01; text values (simple queries) are parsed as Postgres prints them.
    use super::{PgDate, PgTimestamp, TimestampTz};
    use jiff::{SignedDuration, Timestamp, civil};
    use sqlx::encode::IsNull;
    use sqlx::error::BoxDynError;
    use sqlx::postgres::types::Oid;
    use sqlx::postgres::{
        PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueFormat, PgValueRef, Postgres,
    };
    use sqlx::{Decode, Encode, Type};

    const EPOCH: civil::DateTime = civil::date(2000, 1, 1).at(0, 0, 0, 0);
    const MICROS_PER_DAY: i64 = 86_400_000_000;

    fn to_micros(dt: civil::DateTime) -> Result<i64, BoxDynError> {
        let us = dt.duration_since(EPOCH).as_micros();
        Ok(i64::try_from(us)?)
    }

    fn from_micros(us: i64) -> Result<civil::DateTime, BoxDynError> {
        // Postgres `infinity` / `-infinity` are i64::MAX / MIN: outside jiff's range, an error.
        Ok(EPOCH.checked_add(SignedDuration::from_micros(us))?)
    }

    fn utc(ts: Timestamp) -> civil::DateTime {
        jiff::tz::Offset::UTC.to_datetime(ts)
    }

    macro_rules! pg_type {
        ($ty:ty, $oid:expr, $array_oid:expr) => {
            impl Type<Postgres> for $ty {
                fn type_info() -> PgTypeInfo {
                    PgTypeInfo::with_oid(Oid($oid))
                }
            }
            impl PgHasArrayType for $ty {
                fn array_type_info() -> PgTypeInfo {
                    PgTypeInfo::with_oid(Oid($array_oid))
                }
            }
        };
    }
    pg_type!(TimestampTz, 1184, 1185);
    pg_type!(PgTimestamp, 1114, 1115);
    pg_type!(PgDate, 1082, 1182);

    impl Encode<'_, Postgres> for TimestampTz {
        fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
            Encode::<Postgres>::encode(to_micros(utc(self.0))?, buf)
        }
    }

    impl Decode<'_, Postgres> for TimestampTz {
        fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
            Ok(match value.format() {
                PgValueFormat::Binary => {
                    let dt = from_micros(Decode::<Postgres>::decode(value)?)?;
                    Self(dt.to_zoned(jiff::tz::TimeZone::UTC)?.timestamp())
                }
                // "2026-11-18 16:20:00.123456+00" (the session's TimeZone may add an offset).
                PgValueFormat::Text => Self(value.as_str()?.parse::<Timestamp>()?),
            })
        }
    }

    impl Encode<'_, Postgres> for PgTimestamp {
        fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
            Encode::<Postgres>::encode(to_micros(self.0)?, buf)
        }
    }

    impl Decode<'_, Postgres> for PgTimestamp {
        fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
            Ok(Self(match value.format() {
                PgValueFormat::Binary => from_micros(Decode::<Postgres>::decode(value)?)?,
                PgValueFormat::Text => value.as_str()?.parse::<civil::DateTime>()?,
            }))
        }
    }

    impl Encode<'_, Postgres> for PgDate {
        fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
            let days = to_micros(self.0.to_datetime(civil::Time::midnight()))? / MICROS_PER_DAY;
            Encode::<Postgres>::encode(i32::try_from(days)?, buf)
        }
    }

    impl Decode<'_, Postgres> for PgDate {
        fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
            Ok(Self(match value.format() {
                PgValueFormat::Binary => {
                    let days: i32 = Decode::<Postgres>::decode(value)?;
                    from_micros(i64::from(days) * MICROS_PER_DAY)?.date()
                }
                PgValueFormat::Text => value.as_str()?.parse::<civil::Date>()?,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2025-11-18T16:20:00Z with the fractions chrono 0.4.45 printed for them (pinned from chrono
    // before the switch, T-7234).
    const SECS: i64 = 1_763_482_800;

    fn at(nanos: i32) -> Timestamp {
        Timestamp::new(SECS, nanos).expect("in range")
    }

    // What chrono 0.4.45's serde (`DateTime<FixedOffset>: FromStr`) read, compared case by case
    // against chrono itself before the switch (T-7239): jiff's own parser took the missing
    // second, `+HH`, the comma, the basic format and annotations and refused the rest.
    #[test]
    fn relaxed_parse_matches_chrono_from_str() {
        for (input, want) in [
            ("2026-03-29T03:30:00Z", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29 03:30:00Z", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29t03:30:00z", Some("2026-03-29T03:30:00+00:00")),
            (
                "2026-03-29T03:30:00+0200",
                Some("2026-03-29T01:30:00+00:00"),
            ),
            (
                "2026-03-29T03:30:00+02 30",
                Some("2026-03-29T01:00:00+00:00"),
            ),
            (
                "2026-03-29T03:30:00 +02:00",
                Some("2026-03-29T01:30:00+00:00"),
            ),
            (" 2026-03-29T03:30:00Z ", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29  03:30:00Z", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29T03:30:00\tZ", Some("2026-03-29T03:30:00+00:00")),
            ("2026 - 3 - 9T3 : 3 : 3Z", Some("2026-03-09T03:03:03+00:00")),
            ("2026-03-29T03:30:00UTC", Some("2026-03-29T03:30:00+00:00")),
            ("2026-03-29T03:30:00utc", Some("2026-03-29T03:30:00+00:00")),
            (
                "2026-03-29T03:30:00\u{2212}02:00",
                Some("2026-03-29T05:30:00+00:00"),
            ),
            (
                "2026-03-29T03:30:00+23:59",
                Some("2026-03-28T03:31:00+00:00"),
            ),
            (
                "2026-03-29T03:30:00.1234567891234Z",
                Some("2026-03-29T03:30:00.123456789+00:00"),
            ),
            ("+002026-03-29T03:30:00Z", Some("2026-03-29T03:30:00+00:00")),
            (
                "2024-02-29T23:59:60+01:00",
                Some("2024-02-29T23:00:00+00:00"),
            ),
            (
                "2026-10-25T02:30:00+03:00",
                Some("2026-10-24T23:30:00+00:00"),
            ),
            ("2026-03-29T03:30Z", None),
            ("2026-03-29T03:30:00+02", None),
            ("2026-03-29T03:30:00,5Z", None),
            ("2026-03-29T03:30:00Z[Europe/Bratislava]", None),
            ("20260329T033000Z", None),
            ("2026-03-29T03:30:00+02:00:00", None),
            ("2026-03-29T03:30:00.Z", None),
            ("2026-03-29T24:00:00Z", None),
            ("2026-03-29T03:30:00+2:00", None),
            ("2026-03-29T03:30:00UT", None),
            ("2026-03-29T03:30:00+02:60", None),
            ("2026-03-29T03:30:00+99:00", None),
            ("02026-03-29T03:30:00Z", None),
            ("- 1-01-01T00:00:00Z", None),
            ("2026-02-29T00:00:00Z", None),
            ("2026-03-29T03:30:00Zjunk", None),
        ] {
            let got = parse_rfc3339_relaxed(input).map(|t| TimestampTz(t).to_rfc3339());
            assert_eq!(got.as_deref(), want, "{input:?}");
            let json = serde_json::from_value::<TimestampTz>(serde_json::json!(input)).ok();
            assert_eq!(
                json.map(|t| t.to_rfc3339()).as_deref(),
                want,
                "serde {input:?}"
            );
        }
    }

    // chrono 0.4.45's NaiveDate parse_from_str and serde for the same strings (T-7239).
    #[test]
    fn date_matches_chrono_naive_date() {
        let json = |s: &str| serde_json::from_value::<Date>(serde_json::json!(s)).ok();
        for (input, parsed, read) in [
            ("2024-03-01", Some("2024-03-01"), Some("2024-03-01")),
            ("2024-3-1", Some("2024-03-01"), Some("2024-03-01")),
            ("  2024-03-01", Some("2024-03-01"), Some("2024-03-01")),
            ("2024-03-01 ", None, Some("2024-03-01")),
            ("+2024-03-01", Some("2024-03-01"), Some("2024-03-01")),
            ("24-03-01", Some("0024-03-01"), Some("0024-03-01")),
            ("-0001-03-01", Some("-0001-03-01"), Some("-0001-03-01")),
            ("2024-02-29", Some("2024-02-29"), Some("2024-02-29")),
            ("2024-02-30", None, None),
            ("2024-03-01T00:00", None, None),
            ("02024-03-01", None, None),
            ("2024-03-01x", None, None),
        ] {
            let p = parse_date(input).map(|d| Date(d).to_string());
            assert_eq!(p.as_deref(), parsed, "parse {input:?}");
            let j = json(input).map(|d| serde_json::to_value(d).expect("json"));
            assert_eq!(j, read.map(serde_json::Value::from), "serde {input:?}");
        }
        assert_eq!(
            Date::new(33, 1, 2).map(|d| d.to_string()).as_deref(),
            Some("0033-01-02")
        );
        assert_eq!(Date::new(2026, 2, 29), None);
    }

    #[test]
    fn utc_calendar_and_arithmetic() {
        // 2026-10-25T00:30:00.5Z: 02:30 in Bratislava and 03:30 in Helsinki, both still summer time.
        let t = TimestampTz(Timestamp::new(1_792_888_200, 500_000_000).expect("in range"));
        let dt = t.utc();
        assert_eq!(
            (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute()),
            (2026, 10, 25, 0, 30)
        );
        let zoned = |tz: &str| t.0.in_tz(tz).expect("zone").datetime().to_string();
        assert_eq!(zoned("Europe/Bratislava"), "2026-10-25T02:30:00.5");
        assert_eq!(zoned("Europe/Helsinki"), "2026-10-25T03:30:00.5");
        let later = t
            .checked_add(SignedDuration::from_hours(2))
            .expect("in range");
        assert_eq!(
            later.to_rfc3339_opts(SecondsFormat::Millis, true),
            "2026-10-25T02:30:00.500Z"
        );
        assert_eq!(later.checked_sub(SignedDuration::from_hours(2)), Some(t));
        assert_eq!(t.checked_add(SignedDuration::MAX), None);
        let midnight = TimestampTz::from_date(civil::date(2026, 3, 29)).expect("in range");
        assert_eq!(midnight.to_rfc3339(), "2026-03-29T00:00:00+00:00");
        assert_eq!(
            t.0.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "2026-10-25T00:30:00Z"
        );
        let early = TimestampTz::from_date(civil::date(24, 3, 1)).expect("in range");
        assert_eq!(early.0.strftime("%Y-%m-%d").to_string(), "0024-03-01");
    }

    #[test]
    fn json_matches_chrono_auto_si() {
        for (nanos, want) in [
            (0, "\"2025-11-18T16:20:00Z\""),
            (120_000_000, "\"2025-11-18T16:20:00.120Z\""),
            (123_456_000, "\"2025-11-18T16:20:00.123456Z\""),
            (123_456_789, "\"2025-11-18T16:20:00.123456789Z\""),
            (1_000, "\"2025-11-18T16:20:00.000001Z\""),
        ] {
            let json = serde_json::to_string(&TimestampTz(at(nanos))).expect("serialize");
            assert_eq!(json, want);
            let back: TimestampTz = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.0, at(nanos));
        }
        assert_eq!(
            serde_json::to_string(&TimestampTz::UNIX_EPOCH).expect("serialize"),
            "\"1970-01-01T00:00:00Z\""
        );
    }

    #[test]
    fn rfc3339_opts_match_chrono() {
        let t = at(123_456_000);
        assert_eq!(
            format_rfc3339(t, SecondsFormat::Millis, true),
            "2025-11-18T16:20:00.123Z"
        );
        assert_eq!(
            format_rfc3339(t, SecondsFormat::Nanos, true),
            "2025-11-18T16:20:00.123456000Z"
        );
        assert_eq!(
            format_rfc3339(t, SecondsFormat::Secs, true),
            "2025-11-18T16:20:00Z"
        );
        assert_eq!(
            format_rfc3339(t, SecondsFormat::AutoSi, false),
            "2025-11-18T16:20:00.123456+00:00"
        );
        assert_eq!(
            format_rfc3339(at(0), SecondsFormat::Millis, true),
            "2025-11-18T16:20:00.000Z"
        );
        assert_eq!(
            TimestampTz(t).to_rfc3339(),
            "2025-11-18T16:20:00.123456+00:00"
        );
        assert_eq!(
            TimestampTz(t).to_rfc3339_opts(SecondsFormat::Millis, true),
            "2025-11-18T16:20:00.123Z"
        );
        assert_eq!(TimestampTz::default(), TimestampTz::UNIX_EPOCH);
        assert_eq!(
            format_rfc3339(at(0), SecondsFormat::AutoSi, false),
            "2025-11-18T16:20:00+00:00"
        );
        assert_eq!(
            format_rfc3339(at(120_000_000), SecondsFormat::Nanos, true),
            "2025-11-18T16:20:00.120000000Z"
        );
    }

    #[test]
    fn parse_takes_chrono_grammar_only() {
        // Milliseconds chrono's parse_from_rfc3339 returned; None where chrono refused.
        for (s, want) in [
            ("2026-11-18T16:20:00Z", Some(1_795_018_800_000)),
            ("2026-11-18t16:20:00z", Some(1_795_018_800_000)),
            ("2026-11-18 16:20:00+02:00", Some(1_795_011_600_000)),
            ("2026-11-18T16:20:00.5-01:30", Some(1_795_024_200_500)),
            ("2026-11-18T16:20:60Z", Some(1_795_018_860_000)),
            ("2026-11-18T16:20:00.1234567891Z", Some(1_795_018_800_123)),
            ("2026-11-18T16:20Z", None),
            ("20261118T162000Z", None),
            ("2026-11-18T16:20:00", None),
            ("2026-11-18T16:20:00+0200", None),
            ("2026-11-18T16:20:00Z[Europe/Bratislava]", None),
            (" 2026-11-18T16:20:00Z", None),
            ("2026-11-18T16:20:00,5Z", None),
            ("2026-11-18", None),
            ("2026-02-30T00:00:00Z", None),
            ("2026-11-18T16:20:00.Z", None),
        ] {
            assert_eq!(parse_rfc3339(s).map(Timestamp::as_millisecond), want, "{s}");
        }
    }

    #[test]
    fn json_reads_what_chrono_relaxed_parser_read() {
        for (s, ms) in [
            ("2026-11-18T16:20:00+0200", 1_795_011_600_000),
            (" 2026-11-18T16:20:00Z", 1_795_018_800_000),
        ] {
            let t: TimestampTz = serde_json::from_str(&format!("{s:?}")).expect(s);
            assert_eq!(t.as_millis(), ms);
        }
        assert!(serde_json::from_str::<TimestampTz>("\"2026-11-18\"").is_err());
        assert!(serde_json::from_str::<TimestampTz>("1795018800").is_err());
    }

    #[test]
    fn offsets_in_dst_zones_name_the_same_instant() {
        // Bratislava and Helsinki on both sides of the 2026-03-29 spring change.
        for (s, ms, utc) in [
            (
                "2026-03-29T01:59:59+01:00",
                1_774_745_999_000,
                "2026-03-29T00:59:59Z",
            ),
            (
                "2026-03-29T03:00:00+02:00",
                1_774_746_000_000,
                "2026-03-29T01:00:00Z",
            ),
            (
                "2026-03-29T02:59:59+02:00",
                1_774_745_999_000,
                "2026-03-29T00:59:59Z",
            ),
            (
                "2026-03-29T04:00:00+03:00",
                1_774_746_000_000,
                "2026-03-29T01:00:00Z",
            ),
            (
                "2026-10-25T02:30:00+02:00",
                1_792_888_200_000,
                "2026-10-25T00:30:00Z",
            ),
            (
                "2026-10-25T02:30:00+01:00",
                1_792_891_800_000,
                "2026-10-25T01:30:00Z",
            ),
        ] {
            let t = parse_rfc3339(s).expect(s);
            assert_eq!(t.as_millisecond(), ms, "{s}");
            assert_eq!(format_rfc3339(t, SecondsFormat::Secs, true), utc);
        }
    }

    #[cfg(feature = "schemars")]
    #[test]
    fn schema_matches_chrono() {
        let schema = serde_json::to_string(&schemars::schema_for!(TimestampTz)).expect("schema");
        assert_eq!(
            schema,
            r#"{"$schema":"http://json-schema.org/draft-07/schema#","title":"DateTime","type":"string","format":"date-time"}"#
        );
    }
}
