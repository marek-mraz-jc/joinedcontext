// SPDX-License-Identifier: EUPL-1.2
//! Dates, times of day and date-times: ISO 8601 text (the RFC 3339 profile NGSI-LD, `OData` and
//! CQL2 share) to integers, and back to the civil fields the date and time functions read.
//!
//! Years run from 0000 to 9999 (four digits, as all three standards write them), precision is
//! the microsecond: digits past the sixth fractional one are read and dropped.

const MICROS_PER_SECOND: i64 = 1_000_000;
const MICROS_PER_MINUTE: i64 = 60 * MICROS_PER_SECOND;
const MICROS_PER_DAY: i64 = 86_400 * MICROS_PER_SECOND;
/// 1970-01-01 counted from 0000-03-01, the epoch of [`days_from_civil`].
const UNIX_EPOCH_DAYS: i64 = 719_468;

/// A point in time with the offset it was written in, to the microsecond.
///
/// Two values are compared by the instant they name, whatever their offsets; the offset only
/// decides what `year`, `hour` and the other field functions read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    utc: i64,
    offset: i64,
}

/// A calendar date, as days since 1970-01-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date(i64);

/// A time of day, as microseconds since midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(i64);

impl DateTime {
    /// The earliest value: `0000-01-01T00:00:00Z`.
    pub const MIN: Self = Self {
        utc: -62_167_219_200 * MICROS_PER_SECOND,
        offset: 0,
    };
    /// The latest value: `9999-12-31T23:59:59.999999Z`.
    pub const MAX: Self = Self {
        utc: 253_402_300_800 * MICROS_PER_SECOND - 1,
        offset: 0,
    };

    /// `YYYY-MM-DDThh:mm[:ss[.f]]` followed by `Z` or `±hh:mm`; `None` for anything else,
    /// a value without an offset included.
    ///
    /// ```
    /// use ngsild_residual::DateTime;
    ///
    /// let a = DateTime::parse("2026-10-11T12:00:00+02:00").ok_or("a")?;
    /// let b = DateTime::parse("2026-10-11T10:00:00Z").ok_or("b")?;
    /// assert_eq!(a.unix_micros(), b.unix_micros());
    /// assert!(DateTime::parse("2026-10-11T10:00:00").is_none());
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut t = Text::new(text);
        let day = t.date()?;
        if !(t.eat(b'T') || t.eat(b't')) {
            return None;
        }
        let tod = t.time()?;
        let offset = t.offset()?;
        if !t.done() {
            return None;
        }
        let local = day.checked_mul(MICROS_PER_DAY)?.checked_add(tod)?;
        Self::from_parts(
            local.checked_sub(offset.checked_mul(MICROS_PER_MINUTE)?)?,
            offset,
        )
    }

    /// The instant `micros` after 1970-01-01T00:00:00Z, written in UTC; `None` outside the
    /// years 0000 to 9999.
    #[must_use]
    pub fn from_unix_micros(micros: i64) -> Option<Self> {
        Self::from_parts(micros, 0)
    }

    fn from_parts(utc: i64, offset: i64) -> Option<Self> {
        (Self::MIN.utc..=Self::MAX.utc)
            .contains(&utc)
            .then_some(Self { utc, offset })
    }

    /// Microseconds since 1970-01-01T00:00:00Z.
    #[must_use]
    pub fn unix_micros(self) -> i64 {
        self.utc
    }

    /// The offset it was written in, in minutes east of UTC.
    #[must_use]
    pub fn offset_minutes(self) -> i64 {
        self.offset
    }

    /// The wall-clock date and time of day in its own offset.
    pub(crate) fn local(self) -> Option<(Date, Time)> {
        let local = self
            .utc
            .checked_add(self.offset.checked_mul(MICROS_PER_MINUTE)?)?;
        Some((
            Date(local.div_euclid(MICROS_PER_DAY)),
            Time(local.rem_euclid(MICROS_PER_DAY)),
        ))
    }
}

impl Date {
    /// `YYYY-MM-DD`; `None` for anything else or a day the month does not have.
    ///
    /// ```
    /// use ngsild_residual::Date;
    ///
    /// assert!(Date::parse("2024-02-29").is_some());
    /// assert!(Date::parse("2023-02-29").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut t = Text::new(text);
        let day = t.date()?;
        t.done().then_some(Self(day))
    }

    /// Year, month and day.
    pub(crate) fn civil(self) -> Option<(i64, i64, i64)> {
        civil_from_days(self.0)
    }
}

impl Time {
    /// `hh:mm[:ss[.f]]`; `None` for anything else.
    ///
    /// ```
    /// use ngsild_residual::Time;
    ///
    /// assert!(Time::parse("23:59:59.5").is_some());
    /// assert!(Time::parse("24:00").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut t = Text::new(text);
        let tod = t.time()?;
        t.done().then_some(Self(tod))
    }

    /// Hour, minute, second and the microseconds past it.
    pub(crate) fn fields(self) -> Option<(i64, i64, i64, i64)> {
        let seconds = self.0.checked_div(MICROS_PER_SECOND)?;
        Some((
            seconds.checked_div(3600)?,
            seconds.checked_div(60)?.checked_rem(60)?,
            seconds.checked_rem(60)?,
            self.0.checked_rem(MICROS_PER_SECOND)?,
        ))
    }
}

impl std::fmt::Display for Date {
    /// `YYYY-MM-DD`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (year, month, day) = self.civil().ok_or(std::fmt::Error)?;
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

impl std::fmt::Display for Time {
    /// `hh:mm:ss`, and the fraction to the microsecond when there is one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (hour, minute, second, micros) = self.fields().ok_or(std::fmt::Error)?;
        write!(f, "{hour:02}:{minute:02}:{second:02}")?;
        if micros > 0 {
            let fraction = format!("{micros:06}");
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        Ok(())
    }
}

impl std::fmt::Display for DateTime {
    /// RFC 3339 in the offset it was written in, `Z` for UTC.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (date, time) = self.local().ok_or(std::fmt::Error)?;
        write!(f, "{date}T{time}")?;
        let minutes = self.offset.unsigned_abs();
        match self.offset.signum() {
            0 => f.write_str("Z"),
            sign => write!(
                f,
                "{}{:02}:{:02}",
                if sign < 0 { '-' } else { '+' },
                minutes.checked_div(60).ok_or(std::fmt::Error)?,
                minutes.checked_rem(60).ok_or(std::fmt::Error)?,
            ),
        }
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let shifted = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let doy = shifted
        .checked_mul(153)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe.checked_div(4)?)?
        .checked_sub(yoe.checked_div(100)?)?
        .checked_add(doy)?;
    era.checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(UNIX_EPOCH_DAYS)
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> Option<(i64, i64, i64)> {
    let z = days.checked_add(UNIX_EPOCH_DAYS)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = doe
        .checked_sub(doe.checked_div(1460)?)?
        .checked_add(doe.checked_div(36_524)?)?
        .checked_sub(doe.checked_div(146_096)?)?
        .checked_div(365)?;
    let years = yoe
        .checked_mul(365)?
        .checked_add(yoe.checked_div(4)?)?
        .checked_sub(yoe.checked_div(100)?)?;
    let doy = doe.checked_sub(years)?;
    let mp = doy.checked_mul(5)?.checked_add(2)?.checked_div(153)?;
    let day = doy
        .checked_sub(mp.checked_mul(153)?.checked_add(2)?.checked_div(5)?)?
        .checked_add(1)?;
    let month = if mp < 10 {
        mp.checked_add(3)?
    } else {
        mp.checked_sub(9)?
    };
    let year = era
        .checked_mul(400)?
        .checked_add(yoe)?
        .checked_add(i64::from(month <= 2))?;
    Some((year, month, day))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A cursor over ASCII date and time text.
struct Text<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Text<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let hit = self.peek() == Some(byte);
        if hit {
            self.at = self.at.saturating_add(1);
        }
        hit
    }

    fn digit(&mut self) -> Option<i64> {
        let d = char::from(self.peek()?).to_digit(10)?;
        self.at = self.at.saturating_add(1);
        Some(i64::from(d))
    }

    fn digits(&mut self, n: usize) -> Option<i64> {
        (0..n).try_fold(0i64, |v, _| v.checked_mul(10)?.checked_add(self.digit()?))
    }

    fn date(&mut self) -> Option<i64> {
        let year = self.digits(4)?;
        let month = self.eat(b'-').then(|| self.digits(2))??;
        let day = self.eat(b'-').then(|| self.digits(2))??;
        if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
            return None;
        }
        days_from_civil(year, month, day)
    }

    fn time(&mut self) -> Option<i64> {
        let hour = self.digits(2)?;
        let minute = self.eat(b':').then(|| self.digits(2))??;
        let (second, micros) = if self.eat(b':') {
            (self.digits(2)?, self.fraction()?)
        } else {
            (0, 0)
        };
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        hour.checked_mul(60)?
            .checked_add(minute)?
            .checked_mul(60)?
            .checked_add(second)?
            .checked_mul(MICROS_PER_SECOND)?
            .checked_add(micros)
    }

    /// `.ddd…` as microseconds; digits past the sixth are read and dropped.
    fn fraction(&mut self) -> Option<i64> {
        if !self.eat(b'.') {
            return Some(0);
        }
        let mut micros = 0i64;
        let mut scale = MICROS_PER_SECOND;
        let mut read = 0usize;
        while let Some(d) = self.digit() {
            scale = scale.checked_div(10)?;
            micros = micros.checked_add(d.checked_mul(scale)?)?;
            read = read.saturating_add(1);
        }
        (read > 0).then_some(micros)
    }

    /// `Z` or `±hh:mm`, in minutes.
    fn offset(&mut self) -> Option<i64> {
        if self.eat(b'Z') || self.eat(b'z') {
            return Some(0);
        }
        let negative = match self.peek()? {
            b'+' => false,
            b'-' => true,
            _ => return None,
        };
        self.at = self.at.saturating_add(1);
        let hours = self.digits(2)?;
        let minutes = self.eat(b':').then(|| self.digits(2))??;
        if hours > 23 || minutes > 59 {
            return None;
        }
        let total = hours.checked_mul(60)?.checked_add(minutes)?;
        if negative {
            total.checked_neg()
        } else {
            Some(total)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil, Date, DateTime, Time};

    #[test]
    fn civil_round_trips_at_the_edges_and_the_leap_days() {
        for (y, m, d) in [
            (0, 1, 1),
            (1970, 1, 1),
            (2000, 2, 29),
            (1900, 3, 1),
            (2024, 12, 31),
            (9999, 12, 31),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(days.and_then(civil_from_days), Some((y, m, d)));
        }
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(1969, 12, 31), Some(-1));
    }

    #[test]
    fn date_time_reads_offsets_fractions_and_refuses_the_rest() {
        let utc = |t| DateTime::parse(t).map(DateTime::unix_micros);
        assert_eq!(utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(utc("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(utc("1969-12-31T23:00:00-01:00"), Some(0));
        assert_eq!(utc("1970-01-01T00:00:00.1234567z"), Some(123_456));
        assert_eq!(utc("1970-01-01t00:01Z"), Some(60_000_000));
        assert_eq!(
            DateTime::parse("2026-10-11T12:00:00+02:30").map(DateTime::offset_minutes),
            Some(150)
        );
        for bad in [
            "",
            "2026-10-11",
            "2026-10-11T12:00:00",
            "2026-10-11 12:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-11T24:00:00Z",
            "2026-10-11T12:60:00Z",
            "2026-10-11T12:00:60Z",
            "2026-10-11T12:00:00.Z",
            "2026-10-11T12:00:00+24:00",
            "2026-10-11T12:00:00+02:60",
            "2026-10-11T12:00:00+0200",
            "2026-10-11T12:00:00Zx",
            "0000-01-01T00:00:00+00:01",
            "9999-12-31T23:59:59-00:01",
            "2026-1O-11T12:00:00Z",
            "2026-10-11T12:00:00x",
        ] {
            assert_eq!(DateTime::parse(bad), None, "{bad}");
        }
        assert_eq!(DateTime::parse("0000-01-01T00:00:00Z"), Some(DateTime::MIN));
        assert_eq!(
            DateTime::parse("9999-12-31T23:59:59.999999Z"),
            Some(DateTime::MAX)
        );
        assert_eq!(
            DateTime::from_unix_micros(DateTime::MAX.unix_micros() + 1),
            None
        );
        assert_eq!(DateTime::from_unix_micros(i64::MIN), None);
    }

    #[test]
    fn local_fields_follow_the_written_offset() {
        let t = DateTime::parse("2026-01-01T00:30:00+01:00").and_then(DateTime::local);
        let (date, time) = t.unwrap_or((Date(0), Time(0)));
        assert_eq!(date.civil(), Some((2026, 1, 1)));
        assert_eq!(time.fields(), Some((0, 30, 0, 0)));
        let before_epoch = DateTime::parse("1969-12-31T23:59:59.5Z").and_then(DateTime::local);
        let (date, time) = before_epoch.unwrap_or((Date(0), Time(0)));
        assert_eq!(date.civil(), Some((1969, 12, 31)));
        assert_eq!(time.fields(), Some((23, 59, 59, 500_000)));
    }

    #[test]
    fn each_writes_back_the_text_it_reads() {
        for text in [
            "2026-10-11T12:00:00Z",
            "2026-10-11T12:00:00.5+02:30",
            "0000-01-01T00:00:00Z",
            "9999-12-31T23:59:59.999999+11:45",
            "1969-12-31T23:59:59.000001Z",
            "2026-10-11T07:00:00-05:30",
        ] {
            assert_eq!(
                DateTime::parse(text).map(|d| d.to_string()),
                Some(text.to_owned())
            );
        }
        // seconds always, the fraction trimmed
        assert_eq!(
            DateTime::parse("2026-10-11T12:00+01:00").map(|d| d.to_string()),
            Some("2026-10-11T12:00:00+01:00".to_owned())
        );
        assert_eq!(
            Time::parse("01:02:03.120").map(|t| t.to_string()),
            Some("01:02:03.12".to_owned())
        );
        assert_eq!(
            Date::parse("0042-02-28").map(|d| d.to_string()),
            Some("0042-02-28".to_owned())
        );
    }

    #[test]
    fn date_and_time_alone() {
        assert_eq!(Date::parse("1970-01-02"), Some(Date(1)));
        assert_eq!(Date::parse("1900-02-29"), None);
        assert!(Date::parse("2024-02-29").is_some());
        assert!(Date::parse("2000-02-29").is_some());
        assert_eq!(Date::parse("2026-04-31"), None);
        assert_eq!(Date::parse("2026-00-10"), None);
        assert_eq!(Date::parse("2026-10-11T"), None);
        assert_eq!(Time::parse("00:00"), Some(Time(0)));
        assert_eq!(
            Time::parse("12:34:56.000001").and_then(Time::fields),
            Some((12, 34, 56, 1))
        );
        assert_eq!(Time::parse("12:34:56x"), None);
        assert_eq!(Time::parse("1:00"), None);
    }
}
