//! go-syslog v3.0.0 `common/functions.go`, plus the parts of Go's `time` package the machines call:
//! `time.Parse` for the layouts the grammars admit, `Time.AddDate`, `Time.In`, `time.LoadLocation`
//! and `Time.Format(time.RFC3339Nano)`. The grammars have already checked each timestamp's shape, so
//! only Go's range checks remain to be reproduced, with Go's error texts.

use std::str::FromStr;

use chrono::{
    DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike,
};
use chrono_tz::Tz;

/// A Go `time.Time` reduced to what the machines keep: the instant and the zone offset it reads in.
pub type GoTime = DateTime<FixedOffset>;

/// `common.UnsafeUTF8DecimalCodePointsToInt`.
pub fn decimal(chars: &[u8]) -> i64 {
    chars.iter().fold(0i64, |acc, c| {
        acc.wrapping_mul(10).wrapping_add(i64::from(*c) - 48)
    })
}

/// `common.RemoveBytes`: drops the bytes at absolute `positions` from `data`, which starts at `offset`.
pub fn remove_bytes(data: &[u8], positions: &[isize], offset: isize) -> Vec<u8> {
    let mut cp = data.to_vec();
    for (i, pos) in positions.iter().enumerate() {
        let at = pos - i as isize - offset;
        if let Ok(at) = usize::try_from(at) {
            if at < cp.len() {
                cp.remove(at);
            }
        }
    }
    cp
}

/// Go's `string([]byte)` as it later reaches JSON: each byte that does not start a valid UTF-8
/// sequence becomes one U+FFFD, as `encoding/json` writes it.
pub fn go_string(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut rest = b;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (ok, bad) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(ok).unwrap_or_default());
                out.push('\u{FFFD}');
                rest = &bad[1..];
            }
        }
    }
}

/// `fmt.Errorf(msg + " [col %d]", m.p)`.
pub fn col(msg: &str, p: isize) -> String {
    format!("{msg} [col {p}]")
}

fn day_out_of_range(text: &[u8]) -> String {
    format!("parsing time {:?}: day out of range", go_string(text))
}

fn num(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, c| {
        a.wrapping_mul(10)
            .wrapping_add(u32::from(c.wrapping_sub(b'0')))
    })
}

/// The digits at `text[a..b]`; the grammar has checked they are there.
fn at(text: &[u8], a: usize, b: usize) -> u32 {
    num(text.get(a..b).unwrap_or_default())
}

fn date(text: &[u8], year: i32, month: u32, day: u32) -> Result<NaiveDate, String> {
    NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| day_out_of_range(text))
}

/// `time.Parse(time.Stamp, ..)` on `Jan _2 15:04:05`, which reads as year 0 in UTC.
pub fn parse_stamp(text: &[u8]) -> Result<GoTime, String> {
    const MONTHS: [&[u8]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
    let month = MONTHS
        .iter()
        .position(|m| text.get(..3) == Some(*m))
        .map(|i| i as u32 + 1)
        .unwrap_or(1);
    let day = num(text.get(4..6).unwrap_or(b"01").trim_ascii_start());
    let d = date(text, 0, month, day)?;
    Ok(utc(d.and_hms_opt(
        at(text, 7, 9),
        at(text, 10, 12),
        at(text, 13, 15),
    )))
}

fn utc(t: Option<NaiveDateTime>) -> GoTime {
    t.unwrap_or_default().and_utc().fixed_offset()
}

/// `time.Parse(time.RFC3339, ..)` and `time.Parse("2006-01-02T15:04:05.999999Z07:00", ..)`.
pub fn parse_rfc3339(text: &[u8]) -> Result<GoTime, String> {
    let d = date(text, at(text, 0, 4) as i32, at(text, 5, 7), at(text, 8, 10))?;
    let mut i = 19;
    let mut nanos = 0u32;
    if text.get(i) == Some(&b'.') {
        let digits = text
            .get(i + 1..)
            .unwrap_or_default()
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        nanos = at(text, i + 1, i + 1 + digits) * 10u32.pow(9 - digits.min(9) as u32);
        i += 1 + digits;
    }
    let naive = d
        .and_hms_nano_opt(at(text, 11, 13), at(text, 14, 16), at(text, 17, 19), nanos)
        .unwrap_or_default();
    let offset = match text.get(i) {
        Some(b'+' | b'-') => {
            let secs = (at(text, i + 1, i + 3) * 3600 + at(text, i + 4, i + 6) * 60) as i32;
            if text.get(i) == Some(&b'-') {
                -secs
            } else {
                secs
            }
        }
        _ => 0,
    };
    let tz = FixedOffset::east_opt(offset).unwrap_or(chrono::Utc.fix());
    Ok(tz.from_utc_datetime(&(naive - chrono::Duration::seconds(i64::from(offset)))))
}

/// `Time.AddDate(years, 0, 0)`: Go normalises Feb 29 in a common year to Mar 1.
pub fn add_years(t: GoTime, years: i64) -> GoTime {
    let y = i64::from(t.year()) + years;
    let y = i32::try_from(y).unwrap_or(i32::MAX);
    let n = t.naive_local();
    let d = NaiveDate::from_ymd_opt(y, n.month(), n.day())
        .or_else(|| NaiveDate::from_ymd_opt(y, 3, 1))
        .unwrap_or(n.date());
    let naive = d.and_time(n.time());
    t.offset().from_local_datetime(&naive).single().unwrap_or(t)
}

/// A location `time.LoadLocation` returned.
#[derive(Clone, Debug)]
pub enum Location {
    Utc,
    Zone(Tz),
}

/// `time.LoadLocation`. `Local` reads as UTC: ekiben's container runs in UTC.
pub fn load_location(name: &str) -> Result<Location, String> {
    match name {
        "" | "UTC" | "Local" => return Ok(Location::Utc),
        _ => {}
    }
    if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
        return Err("time: invalid location name".to_string());
    }
    Tz::from_str(name)
        .map(Location::Zone)
        .map_err(|_| format!("unknown time zone {name}"))
}

/// `Time.In(loc)`.
pub fn in_location(t: GoTime, loc: &Location) -> GoTime {
    match loc {
        Location::Utc => t.with_timezone(&chrono::Utc).fixed_offset(),
        Location::Zone(tz) => t.with_timezone(tz).fixed_offset(),
    }
}

/// `Time.Format(time.RFC3339Nano)`.
pub fn format_rfc3339_nano(t: &GoTime) -> String {
    let year = t.year();
    let mut s = if year < 0 {
        format!("-{:04}", -i64::from(year))
    } else {
        format!("{year:04}")
    };
    s.push_str(&format!(
        "-{:02}-{:02}T{:02}:{:02}:{:02}",
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    ));
    let ns = t.nanosecond();
    if ns != 0 {
        let frac = format!("{ns:09}");
        s.push('.');
        s.push_str(frac.trim_end_matches('0'));
    }
    let off = t.offset().local_minus_utc();
    if off == 0 {
        s.push('Z');
    } else {
        let sign = if off < 0 { '-' } else { '+' };
        let a = off.abs();
        s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, a % 3600 / 60));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_time_helpers_match_go() {
        let t = parse_rfc3339(b"2003-10-11T22:14:15.003000-07:00").unwrap();
        assert_eq!(format_rfc3339_nano(&t), "2003-10-11T22:14:15.003-07:00");
        assert_eq!(
            parse_rfc3339(b"2003-02-30T22:14:15Z").unwrap_err(),
            "parsing time \"2003-02-30T22:14:15Z\": day out of range"
        );
        let s = parse_stamp(b"Feb 29 01:02:03").unwrap();
        assert_eq!(
            format_rfc3339_nano(&add_years(s, 2023)),
            "2023-03-01T01:02:03Z"
        );
        assert_eq!(format_rfc3339_nano(&s), "0000-02-29T01:02:03Z");
        assert!(parse_stamp(b"Feb 30 01:02:03").is_err());
        let z = load_location("Europe/Bratislava").unwrap();
        assert_eq!(
            format_rfc3339_nano(&in_location(add_years(s, 2024), &z)),
            "2024-02-29T02:02:03+01:00"
        );
        assert_eq!(
            load_location("Mars/Base").unwrap_err(),
            "unknown time zone Mars/Base"
        );
        assert_eq!(go_string(b"a\xe0\xa0b"), "a\u{FFFD}\u{FFFD}b");
        assert_eq!(remove_bytes(b"a\\]b", &[11], 10), b"a]b");
    }
}
