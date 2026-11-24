//! Port of `internal/impl/sql/util.go` (Bento 1.22.0): a row becomes an object keyed by column
//! name, each value as the Go driver hands it to `database/sql` (`[]byte` turned into a string).
//! Beside it, what the Go drivers do with the arguments of a query: lib/pq sends each argument
//! as text in the type the server gives the parameter, go-sql-driver and modernc bind Go values.

use std::collections::BTreeMap;

use crate::sqlx::mysql::types::MySqlTime;
use crate::sqlx::mysql::{MySql, MySqlArguments, MySqlRow};
use crate::sqlx::postgres::{
    PgArgumentBuffer, PgArguments, PgRow, PgTypeInfo, PgValueFormat, Postgres,
};
use crate::sqlx::{Arguments, Column, Decode, Executor, Row, Statement, TypeInfo, ValueRef};
use ekiben_bloblang::time_format::fixed;
use ekiben_bloblang::Value;
use futures::TryStreamExt;
use jiff::civil::{self, Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Timestamp, Zoned};

use crate::conn_fields::{Db, Driver};

/// An error as its text (one function for every driver error ekiben hands on as it reads).
pub(crate) fn err_text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The message text of a driver error, as the Go drivers print theirs (`pq: ...`, `Error 1146 (42S02): ...`).
pub fn db_error_text(e: &crate::sqlx::Error) -> String {
    match e {
        crate::sqlx::Error::Database(d) => d.message().to_string(),
        // ekiben's own texts (argument conversion, the sqlite driver) pass as they are.
        crate::sqlx::Error::Protocol(m) => m.clone(),
        other => other.to_string(),
    }
}

/// The Go `%T` of a structured value (`mapping returned non-array result: %T`).
pub fn go_type(v: &Value) -> &'static str {
    match v {
        Value::Null | Value::Nothing | Value::Delete => "<nil>",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int64",
        Value::Uint(_) => "uint64",
        Value::Float(_) => "float64",
        Value::Number(_) => "json.Number",
        Value::String(_) => "string",
        Value::Bytes(_) => "[]uint8",
        Value::Timestamp(_) => "time.Time",
        Value::Array(_) => "[]interface {}",
        Value::Object(_) => "map[string]interface {}",
    }
}

/// The arguments an `args_mapping` result holds, or Go's error for a non-array.
pub fn args_from_value(v: Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Array(a) => Ok(Vec::clone(&a)),
        other => Err(format!(
            "mapping returned non-array result: {}",
            go_type(&other)
        )),
    }
}

/// `database/sql`'s default argument conversion, for the error it gives a value no driver takes.
fn check_arg(i: usize, v: &Value) -> Result<(), String> {
    let n = i + 1;
    match v {
        Value::Array(_) => Err(format!(
            "sql: converting argument ${n} type: unsupported type []interface {{}}, a slice of interface"
        )),
        Value::Object(_) => Err(format!(
            "sql: converting argument ${n} type: unsupported type map[string]interface {{}}, a map from string to interface"
        )),
        Value::Uint(u) if *u >= 1 << 63 => Err(format!(
            "sql: converting argument ${n} type: uint64 values with high bit set are not supported"
        )),
        _ => Ok(()),
    }
}

/// lib/pq's `encode` of a Go value into the text a parameter carries; `None` is SQL NULL.
fn pq_text(v: &Value) -> Option<Vec<u8>> {
    Some(match v {
        Value::Null | Value::Nothing | Value::Delete => return None,
        Value::Bool(b) => b.to_string().into_bytes(),
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Uint(u) => u.to_string().into_bytes(),
        Value::Float(f) => {
            // lib/pq: strconv.AppendFloat(nil, v, 'f', -1, 64)
            let s = format!("{f}");
            if f.is_infinite() {
                if *f > 0.0 {
                    "Infinity".into()
                } else {
                    "-Infinity".into()
                }
            } else if f.is_nan() {
                "NaN".into()
            } else {
                s
            }
            .into_bytes()
        }
        Value::Number(s) | Value::String(s) => s.clone().into_bytes(),
        Value::Bytes(b) => b.clone(),
        // lib/pq formatTs: 2006-01-02 15:04:05.999999999Z07:00
        Value::Timestamp(t) => format_pq_ts(t).into_bytes(),
        Value::Array(_) | Value::Object(_) => return None,
    })
}

fn format_pq_ts(t: &Zoned) -> String {
    let mut s = t.strftime("%Y-%m-%d %H:%M:%S").to_string();
    let nanos = t.subsec_nanosecond();
    if nanos > 0 {
        let frac = format!("{nanos:09}");
        s.push('.');
        s.push_str(frac.trim_end_matches('0'));
    }
    let off = t.offset().seconds();
    if off == 0 {
        s.push('Z');
    } else {
        let sign = if off < 0 { '-' } else { '+' };
        let a = off.abs();
        s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60));
        if a % 60 != 0 {
            s.push_str(&format!(":{:02}", a % 60));
        }
    }
    s
}

/// A postgres parameter already encoded in the binary form of the type the server gave it.
struct PgRaw {
    oid: u32,
    bytes: Option<Vec<u8>>,
}

impl crate::sqlx::Type<Postgres> for PgRaw {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(crate::sqlx::postgres::types::Oid(0))
    }
}

impl crate::sqlx::Encode<'_, Postgres> for PgRaw {
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<crate::sqlx::encode::IsNull, crate::sqlx::error::BoxDynError> {
        match &self.bytes {
            None => Ok(crate::sqlx::encode::IsNull::Yes),
            Some(b) => {
                buf.extend_from_slice(b);
                Ok(crate::sqlx::encode::IsNull::No)
            }
        }
    }
    fn produces(&self) -> Option<PgTypeInfo> {
        Some(PgTypeInfo::with_oid(crate::sqlx::postgres::types::Oid(
            self.oid,
        )))
    }
}

fn pg_invalid(type_name: &str, text: &str) -> String {
    format!("pq: invalid input syntax for type {type_name}: \"{text}\"")
}

/// The server's text input of `oid` for `text`, encoded in the type's binary form: what postgres
/// does with the text parameters lib/pq sends.
fn pg_text_to_binary(
    oid: u32,
    type_name: &str,
    text: &[u8],
    is_bytes: bool,
) -> Result<Vec<u8>, String> {
    let s = || String::from_utf8_lossy(text).to_string();
    let int = |bits: u32| -> Result<i64, String> {
        let t = s();
        let v: i64 = t.trim().parse().map_err(|_| pg_invalid(type_name, &t))?;
        if bits < 64 && (v < -(1i64 << (bits - 1)) || v >= 1i64 << (bits - 1)) {
            return Err(format!(
                "pq: value \"{t}\" is out of range for type {type_name}"
            ));
        }
        Ok(v)
    };
    let float = || -> Result<f64, String> {
        let t = s();
        match t.trim().to_ascii_lowercase().as_str() {
            "nan" => Ok(f64::NAN),
            "infinity" | "inf" | "+infinity" => Ok(f64::INFINITY),
            "-infinity" | "-inf" => Ok(f64::NEG_INFINITY),
            x => x.parse().map_err(|_| pg_invalid(type_name, &t)),
        }
    };
    Ok(match oid {
        16 => match s().trim().to_ascii_lowercase().as_str() {
            "t" | "true" | "y" | "yes" | "on" | "1" => vec![1],
            "f" | "false" | "n" | "no" | "off" | "0" => vec![0],
            _ => return Err(pg_invalid("boolean", &s())),
        },
        21 => (int(16)? as i16).to_be_bytes().to_vec(),
        23 => (int(32)? as i32).to_be_bytes().to_vec(),
        20 => int(64)?.to_be_bytes().to_vec(),
        26 => u32::try_from(int(64)?)
            .map_err(|_| pg_invalid("oid", &s()))?
            .to_be_bytes()
            .to_vec(),
        700 => (float()? as f32).to_be_bytes().to_vec(),
        701 => float()?.to_be_bytes().to_vec(),
        17 if is_bytes => text.to_vec(),
        17 => decode_bytea_text(text).ok_or_else(|| pg_invalid("bytea", &s()))?,
        // text, varchar, bpchar, name, json, xml, unknown and char: the binary form is the text.
        25 | 1043 | 1042 | 19 | 114 | 142 | 705 | 18 => text.to_vec(),
        3802 => {
            let mut v = vec![1u8];
            v.extend_from_slice(text);
            v
        }
        2950 => {
            let t = s();
            let hex: String = t
                .trim()
                .trim_matches(['{', '}'])
                .chars()
                .filter(|c| *c != '-')
                .collect();
            // Nibbles of bytes, not slices of the text: a multibyte character is no digit.
            let nibble = |b: u8| char::from(b).to_digit(16);
            let bytes: Option<Vec<u8>> = hex
                .as_bytes()
                .chunks(2)
                .map(|p| Some((nibble(p[0])? << 4 | nibble(*p.get(1)?)?) as u8))
                .collect();
            bytes
                .filter(|b| b.len() == 16)
                .ok_or_else(|| pg_invalid("uuid", &t))?
        }
        1700 => encode_numeric(&s()).ok_or_else(|| pg_invalid("numeric", &s()))?,
        1114 | 1184 => {
            let t = s();
            let micros = parse_pg_timestamp(&t).ok_or_else(|| pg_invalid(type_name, &t))?;
            micros.to_be_bytes().to_vec()
        }
        1082 => {
            let t = s();
            let days = pg_date_days(&t).ok_or_else(|| pg_invalid("date", &t))?;
            days.to_be_bytes().to_vec()
        }
        _ => {
            return Err(format!(
                "ekiben does not encode an argument for a postgres parameter of type {type_name}"
            ))
        }
    })
}

fn decode_bytea_text(text: &[u8]) -> Option<Vec<u8>> {
    if let Some(hex) = text.strip_prefix(b"\\x") {
        if hex.len() % 2 != 0 || !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        return hex
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
            .collect();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'\\' {
            if text.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            let oct = text.get(i + 1..i + 4)?;
            if !oct.iter().all(|b| (b'0'..=b'7').contains(b)) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(oct).ok()?, 8).ok()?);
            i += 4;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Postgres counts timestamps and dates from 2000-01-01.
const PG_EPOCH: DateTime = civil::date(2000, 1, 1).at(0, 0, 0, 0);

/// Days since 2000-01-01 of the `YYYY-MM-DD` a postgres date text starts with.
fn pg_date_days(text: &str) -> Option<i32> {
    let d = Date::strptime("%Y-%m-%d", text.trim().get(..10)?).ok()?;
    let days = PG_EPOCH.date().until(d).ok()?.get_days();
    Some(days)
}

/// The instant `local` names in UTC.
fn utc(local: DateTime) -> Option<Timestamp> {
    Offset::UTC.to_timestamp(local).ok()
}

/// Microseconds since 2000-01-01 of a postgres timestamp text (ISO forms, `T` or space).
fn parse_pg_timestamp(text: &str) -> Option<i64> {
    let t = text.trim();
    match t.to_ascii_lowercase().as_str() {
        "infinity" => return Some(i64::MAX),
        "-infinity" => return Some(i64::MIN),
        _ => {}
    }
    let norm = t.replacen(' ', "T", 1);
    let ts = if let Some(ts) = jc_time::parse_rfc3339(&norm) {
        ts
    } else if let Some(ts) = parse_with_offset(&norm, "%Y-%m-%dT%H:%M:%S%.f") {
        ts
    } else if let Ok(n) = DateTime::strptime("%Y-%m-%dT%H:%M:%S%.f", &norm) {
        utc(n)?
    } else {
        utc(Date::strptime("%Y-%m-%d", &norm)
            .ok()?
            .to_datetime(Time::midnight()))?
    };
    // ponytail: a text without an offset is read as UTC for timestamp and timestamptz alike (the
    // session TimeZone of a stock postgres image); a non-UTC session TimeZone is a listed gap.
    let since = ts.as_nanosecond() - utc(PG_EPOCH)?.as_nanosecond();
    // whole microseconds, toward zero
    i64::try_from(since / 1_000).ok()
}

/// `text` as `format` followed by an offset of `+HH`, `+HHMM` or `+HH:MM` (chrono's `%#z`).
fn parse_with_offset(text: &str, format: &str) -> Option<Timestamp> {
    let at = text.rfind(['+', '-']).filter(|i| *i > 10)?;
    let (local, offset) = text.split_at(at);
    let digits: String = offset[1..].chars().filter(|c| *c != ':').collect();
    let colon_ok = matches!(offset.len(), 3 | 5) || offset.as_bytes().get(3) == Some(&b':');
    if !colon_ok || !matches!(digits.len(), 2 | 4) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits
        .get(2..)
        .filter(|m| !m.is_empty())
        .map_or(Ok(0), str::parse)
        .ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let secs = (hours * 3600 + minutes * 60) * if offset.starts_with('-') { -1 } else { 1 };
    let local = DateTime::strptime(format, local).ok()?;
    Offset::from_seconds(secs).ok()?.to_timestamp(local).ok()
}

/// postgres numeric binary form of a decimal text.
fn encode_numeric(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    let mut out = Vec::new();
    let special = |sign: u16| {
        let mut v = vec![0, 0, 0, 0];
        v.extend_from_slice(&sign.to_be_bytes());
        v.extend_from_slice(&[0, 0]);
        Some(v)
    };
    match t.to_ascii_lowercase().as_str() {
        "nan" => return special(0xC000),
        "infinity" | "+infinity" | "inf" => return special(0xD000),
        "-infinity" | "-inf" => return special(0xF000),
        _ => {}
    }
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], body[i + 1..].parse::<i32>().ok()?),
        None => (body, 0),
    };
    let (ip, fp) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (ip.is_empty() && fp.is_empty()) || !ip.chars().chain(fp.chars()).all(|c| c.is_ascii_digit())
    {
        return None;
    }
    // postgres' limits: 131072 digits before the point, 16383 after it; past them the text is
    // refused before a digit is written (an exponent would otherwise size the buffer).
    let point = i64::try_from(ip.len()).ok()? + i64::from(exp);
    let scale = (i64::try_from(fp.len()).ok()? - i64::from(exp)).max(0);
    if point > 131_072 || scale > 16_383 {
        return None;
    }
    let (mut point, dscale) = (point as i32, scale as u16);
    let mut digits: String = format!("{ip}{fp}");
    // Left-pad so the decimal point sits on a 4-digit group boundary.
    let lead = (4 - point.rem_euclid(4)) % 4;
    digits.insert_str(0, &"0".repeat(lead as usize));
    point += lead;
    // Whole groups; the weight carries the magnitude, so no zero group pads up to the point.
    while !digits.len().is_multiple_of(4) {
        digits.push('0');
    }
    let groups: Vec<i16> = digits
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).ok().and_then(|s| s.parse().ok()))
        .collect::<Option<_>>()?;
    let mut weight = point / 4 - 1;
    let mut start = 0;
    while start < groups.len() && groups[start] == 0 {
        start += 1;
        weight -= 1;
    }
    let mut end = groups.len();
    while end > start && groups[end - 1] == 0 {
        end -= 1;
    }
    let g = &groups[start..end];
    if g.is_empty() {
        weight = 0;
    }
    out.extend_from_slice(&(g.len() as i16).to_be_bytes());
    out.extend_from_slice(&(weight as i16).to_be_bytes());
    out.extend_from_slice(&(if neg && !g.is_empty() { 0x4000u16 } else { 0 }).to_be_bytes());
    out.extend_from_slice(&dscale.to_be_bytes());
    for d in g {
        out.extend_from_slice(&d.to_be_bytes());
    }
    Some(out)
}

/// postgres numeric binary form back to text, as postgres prints it (lib/pq hands it on as text).
fn decode_numeric(b: &[u8]) -> Option<String> {
    let rd = |i: usize| -> Option<u16> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?])) };
    let ndigits = rd(0)? as usize;
    let weight = rd(2)? as i16 as i32;
    let sign = rd(4)?;
    let dscale = rd(6)? as usize;
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let digits: Vec<u16> = (0..ndigits).map(|i| rd(8 + i * 2)).collect::<Option<_>>()?;
    let digit = |i: i32| -> u16 {
        if i >= 0 && (i as usize) < digits.len() {
            digits[i as usize]
        } else {
            0
        }
    };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    } else {
        for i in 0..=weight {
            let d = digit(i);
            if i == 0 {
                s.push_str(&d.to_string());
            } else {
                s.push_str(&format!("{d:04}"));
            }
        }
    }
    if dscale > 0 {
        s.push('.');
        let mut frac = String::new();
        let mut i = weight + 1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", digit(i)));
            i += 1;
        }
        frac.truncate(dscale);
        s.push_str(&frac);
    }
    Some(s)
}

/// A float32 as the shortest float64 that prints the same.
fn f32_as_go(f: f32) -> f64 {
    format!("{f}").parse().unwrap_or(f as f64)
}

fn pg_time_of_day(micros: i64) -> Option<Time> {
    let secs = u32::try_from(micros.div_euclid(1_000_000))
        .ok()
        .filter(|s| *s < 86_400)?;
    let nanos = i32::try_from(micros.rem_euclid(1_000_000) * 1000).ok()?;
    Time::new(
        (secs / 3600) as i8,
        (secs % 3600 / 60) as i8,
        (secs % 60) as i8,
        nanos,
    )
    .ok()
}

fn utc_fixed(n: DateTime) -> Option<Value> {
    utc(n).map(|ts| Value::Timestamp(ts.to_zoned(TimeZone::UTC)))
}

/// A postgres `timestamp`/`timestamptz` of `micros` since 2000-01-01, in UTC.
fn pg_timestamp(micros: i64) -> Option<Value> {
    utc_fixed(
        PG_EPOCH
            .checked_add(SignedDuration::from_micros(micros))
            .ok()?,
    )
}

/// A postgres `date` of `days` since 2000-01-01, at midnight UTC.
fn pg_date(days: i32) -> Option<Value> {
    utc_fixed(
        PG_EPOCH
            .checked_add(SignedDuration::from_hours(i64::from(days) * 24))
            .ok()?,
    )
}

/// A postgres `time` (`zone` None) or `timetz` (`zone` seconds west of UTC) on 0000-01-01.
fn pg_time(micros: i64, zone: Option<i32>) -> Option<Value> {
    let local = civil::date(0, 1, 1).to_datetime(pg_time_of_day(micros)?);
    match zone {
        None => utc_fixed(local),
        Some(west) => {
            let east = west.checked_neg()?;
            let ts = Offset::from_seconds(east).ok()?.to_timestamp(local).ok()?;
            fixed(ts, east).ok().map(Value::Timestamp)
        }
    }
}

/// One postgres column value as lib/pq decodes it (text results, `[]byte` read as a string).
fn pg_value(row: &PgRow, i: usize) -> Result<Value, String> {
    let raw = row.try_get_raw(i).map_err(err_text)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let ti = raw.type_info().into_owned();
    let b = raw.as_bytes().map_err(err_text)?.to_vec();
    pg_decode(
        ti.oid().map(|o| o.0).unwrap_or(0),
        ti.name(),
        b,
        raw.format() == PgValueFormat::Text,
    )
}

/// A postgres column value of type `oid` from its bytes (the binary form unless `text_format`).
fn pg_decode(oid: u32, name: &str, b: Vec<u8>, text_format: bool) -> Result<Value, String> {
    if text_format {
        return Ok(Value::String(String::from_utf8_lossy(&b).into_owned()));
    }
    let be8 = |b: &[u8]| -> Option<[u8; 8]> { b.try_into().ok() };
    let bad = || format!("unexpected {name} value of {} bytes", b.len());
    let text = |b: Vec<u8>| Value::String(String::from_utf8_lossy(&b).into_owned());
    Ok(match oid {
        16 => Value::Bool(b.first().copied().ok_or_else(bad)? != 0),
        21 => Value::Int(i16::from_be_bytes(b.as_slice().try_into().map_err(|_| bad())?) as i64),
        23 => Value::Int(i32::from_be_bytes(b.as_slice().try_into().map_err(|_| bad())?) as i64),
        20 => Value::Int(i64::from_be_bytes(be8(&b).ok_or_else(bad)?)),
        26 => Value::String(
            u32::from_be_bytes(b.as_slice().try_into().map_err(|_| bad())?).to_string(),
        ),
        // lib/pq parses the float4 text as a float64: 1.1, not 1.100000023841858.
        700 => Value::Float(f32_as_go(f32::from_be_bytes(
            b.as_slice().try_into().map_err(|_| bad())?,
        ))),
        701 => Value::Float(f64::from_be_bytes(be8(&b).ok_or_else(bad)?)),
        3802 => text(b.get(1..).unwrap_or_default().to_vec()),
        1700 => Value::String(decode_numeric(&b).ok_or_else(bad)?),
        2950 => {
            let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
            if h.len() != 32 {
                return Err(bad());
            }
            Value::String(format!(
                "{}-{}-{}-{}-{}",
                &h[..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..]
            ))
        }
        1114 | 1184 => {
            let m = i64::from_be_bytes(be8(&b).ok_or_else(bad)?);
            match m {
                i64::MAX => Value::String("infinity".into()),
                i64::MIN => Value::String("-infinity".into()),
                m => pg_timestamp(m).ok_or_else(bad)?,
            }
        }
        1082 => {
            let d = i32::from_be_bytes(b.as_slice().try_into().map_err(|_| bad())?);
            match d {
                i32::MAX => Value::String("infinity".into()),
                i32::MIN => Value::String("-infinity".into()),
                d => pg_date(d).ok_or_else(bad)?,
            }
        }
        1083 => pg_time(i64::from_be_bytes(be8(&b).ok_or_else(bad)?), None).ok_or_else(bad)?,
        1266 => {
            let m = i64::from_be_bytes(b.get(..8).and_then(be8).ok_or_else(bad)?);
            let zone = i32::from_be_bytes(
                b.get(8..12)
                    .and_then(|x| x.try_into().ok())
                    .ok_or_else(bad)?,
            );
            pg_time(m, Some(zone)).ok_or_else(bad)?
        }
        // text, varchar, bpchar, name, char, bytea, json, xml, unknown: the bytes as a string.
        25 | 1043 | 1042 | 19 | 18 | 17 | 114 | 142 | 705 => text(b),
        _ => {
            return Err(format!(
                "ekiben does not decode postgres columns of type {name}"
            ))
        }
    })
}

/// One mysql column value as go-sql-driver 1.7.1 decodes it: the text protocol (no arguments)
/// gives every value as bytes; the binary protocol gives integers and floats typed, `DATETIME`
/// as text unless `parseTime`.
fn mysql_value(
    row: &MySqlRow,
    i: usize,
    parse_time: bool,
    text_protocol: bool,
) -> Result<Value, String> {
    let raw = row.try_get_raw(i).map_err(err_text)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let name = raw.type_info().name().to_string();
    let bytes = <&[u8] as Decode<MySql>>::decode(raw.clone()).map_err(err_text)?;
    let text = |b: &[u8]| Value::String(String::from_utf8_lossy(b).into_owned());
    let base = name.trim_end_matches(" UNSIGNED");
    let unsigned = name.ends_with("UNSIGNED");
    let time_kind = matches!(base, "DATETIME" | "TIMESTAMP" | "DATE");
    match text_protocol {
        true => {
            if parse_time && time_kind {
                return mysql_text_time(&String::from_utf8_lossy(bytes));
            }
            Ok(text(bytes))
        }
        false => Ok(match base {
            "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" | "YEAR" => {
                if unsigned {
                    let v = <u64 as Decode<MySql>>::decode(raw).map_err(err_text)?;
                    // go-sql-driver hands an unsigned value past int64 on as text.
                    i64::try_from(v)
                        .map(Value::Int)
                        .unwrap_or_else(|_| Value::String(v.to_string()))
                } else {
                    Value::Int(<i64 as Decode<MySql>>::decode(raw).map_err(err_text)?)
                }
            }
            "FLOAT" => {
                let f = <f32 as Decode<MySql>>::decode(raw).map_err(err_text)?;
                // Go marshals a float32 with 32-bit precision: 1.1, not 1.100000023841858.
                Value::Float(f32_as_go(f))
            }
            "DOUBLE" => Value::Float(<f64 as Decode<MySql>>::decode(raw).map_err(err_text)?),
            "DATETIME" | "TIMESTAMP" | "DATE" => {
                let t = mysql_binary_datetime(bytes)?;
                if parse_time {
                    utc_fixed(t).ok_or_else(|| format!("{name} {t} is out of range"))?
                } else if base == "DATE" {
                    Value::String(t.strftime("%Y-%m-%d").to_string())
                } else {
                    Value::String(mysql_datetime_text(t))
                }
            }
            "TIME" => {
                let t = <MySqlTime as Decode<MySql>>::decode(raw).map_err(err_text)?;
                if !t.is_valid_time_of_day() {
                    return Err(format!("TIME value {t} is not a time of day"));
                }
                Value::String(format!(
                    "{:02}:{:02}:{:02}",
                    t.hours(),
                    t.minutes(),
                    t.seconds()
                ))
            }
            _ => text(bytes),
        }),
    }
}

/// A binary-protocol `DATE`, `DATETIME` or `TIMESTAMP` as sqlx's chrono decoder read it: a length
/// byte, the year (u16 LE), month and day, then hour, minute, second and microseconds (LE).
fn mysql_binary_datetime(buf: &[u8]) -> Result<DateTime, String> {
    let (&len, rest) = buf.split_first().ok_or("empty buffer")?;
    if rest.is_empty() {
        return Err("unexpected null".to_string());
    }
    let [y0, y1, month, day, time @ ..] = rest else {
        return Err(format!(
            "expected at least 4 bytes for date, got {}",
            rest.len()
        ));
    };
    let year = u16::from_le_bytes([*y0, *y1]);
    let date = i16::try_from(year)
        .ok()
        .and_then(|y| Date::new(y, *month as i8, *day as i8).ok())
        .ok_or_else(|| format!("server returned invalid date: {year}/{month}/{day}"))?;
    if len <= 4 {
        return Ok(date.to_datetime(Time::midnight()));
    }
    let [hour, minute, second, micros @ ..] = time else {
        return Err("expected 3 bytes for time".to_string());
    };
    let mut le = [0u8; 8];
    le[..micros.len().min(8)].copy_from_slice(&micros[..micros.len().min(8)]);
    let micros = u64::from_le_bytes(le);
    let invalid = || {
        format!("server returned invalid time: {hour:02}:{minute:02}:{second:02}; micros: {micros}")
    };
    let nanos = i32::try_from(micros)
        .ok()
        .filter(|m| *m < 1_000_000)
        .ok_or_else(invalid)?
        * 1000;
    let time =
        Time::new(*hour as i8, *minute as i8, *second as i8, nanos).map_err(|_| invalid())?;
    Ok(date.to_datetime(time))
}

fn mysql_datetime_text(t: DateTime) -> String {
    let base = t.strftime("%Y-%m-%d %H:%M:%S").to_string();
    let micros = t.subsec_nanosecond() / 1000;
    if micros == 0 {
        base
    } else {
        format!("{base}.{micros:06}")
    }
}

fn mysql_text_time(s: &str) -> Result<Value, String> {
    if s.starts_with("0000-00-00") {
        return Ok(Value::Timestamp(
            Timestamp::UNIX_EPOCH.to_zoned(TimeZone::UTC),
        ));
    }
    let t = DateTime::strptime("%Y-%m-%d %H:%M:%S%.f", s)
        .or_else(|_| Date::strptime("%Y-%m-%d", s).map(|d| d.to_datetime(Time::midnight())))
        .map_err(|_| format!("parsing time {s:?}: {}", ymd_error(s)))?;
    utc_fixed(t).ok_or_else(|| format!("parsing time {s:?}: input is out of range"))
}

/// Why `s` is no `%Y-%m-%d`, in the words of chrono's parser (go-sql-driver's error text named
/// it since the chrono build).
fn ymd_error(s: &str) -> &'static str {
    let mut rest = s;
    let mut fields = [0u32; 3];
    for (i, width) in [4, 2, 2].into_iter().enumerate() {
        if i > 0 {
            match rest.strip_prefix('-') {
                Some(r) => rest = r,
                None if rest.is_empty() => return "premature end of input",
                None => return "input contains invalid characters",
            }
        }
        let n = rest
            .bytes()
            .take(width)
            .take_while(u8::is_ascii_digit)
            .count();
        if n == 0 {
            return if rest.is_empty() {
                "premature end of input"
            } else {
                "input contains invalid characters"
            };
        }
        fields[i] = rest[..n].parse().unwrap_or(0);
        rest = &rest[n..];
    }
    if !rest.is_empty() {
        "trailing input"
    } else if !(1..=12).contains(&fields[1]) || !(1..=31).contains(&fields[2]) {
        "input is out of range"
    } else {
        "no possible date and time matching input"
    }
}

fn row_object<R: Row>(
    row: &R,
    f: impl Fn(&R, usize) -> Result<Value, String>,
) -> Result<Value, String> {
    let mut obj = BTreeMap::new();
    for (i, c) in row.columns().iter().enumerate() {
        obj.insert(c.name().to_string(), f(row, i)?);
    }
    Ok(Value::Object(obj.into()))
}

/// `sqlRowToMap` of one driver's row.
pub enum AnyRow {
    Postgres(PgRow),
    /// The row, `parseTime`, and whether it came over the text protocol.
    MySql(MySqlRow, bool, bool),
}

pub fn row_to_map(row: &AnyRow) -> Result<Value, String> {
    match row {
        AnyRow::Postgres(r) => row_object(r, pg_value),
        AnyRow::MySql(r, pt, text) => row_object(r, |r, i| mysql_value(r, i, *pt, *text)),
    }
}

fn mysql_args(args: &[Value]) -> Result<MySqlArguments, String> {
    let mut a = MySqlArguments::default();
    for (i, v) in args.iter().enumerate() {
        check_arg(i, v)?;
        let r = match v {
            // check_arg refused arrays and objects above.
            Value::Null | Value::Nothing | Value::Delete | Value::Array(_) | Value::Object(_) => {
                a.add(None::<String>)
            }
            Value::Bool(b) => a.add(*b),
            Value::Int(n) => a.add(*n),
            Value::Uint(n) => a.add(*n as i64),
            Value::Float(f) => a.add(*f),
            Value::Number(s) | Value::String(s) => a.add(s.clone()),
            Value::Bytes(b) => a.add(b.clone()),
            // go-sql-driver sends a time.Time as text in cfg.Loc (UTC).
            Value::Timestamp(t) => a.add(mysql_datetime_text(
                t.with_time_zone(TimeZone::UTC).datetime(),
            )),
        };
        r.map_err(err_text)?;
    }
    Ok(a)
}

async fn pg_args(
    conn: &mut crate::sqlx::postgres::PgConnection,
    sql: &str,
    args: &[Value],
) -> Result<PgArguments, crate::sqlx::Error> {
    let stmt = conn
        .prepare(crate::sqlx::SqlSafeStr::into_sql_str(
            crate::sqlx::AssertSqlSafe(sql.to_string()),
        ))
        .await?;
    // postgres always describes its parameters (the Right arm is mysql's count).
    let types: &[PgTypeInfo] = stmt
        .parameters()
        .and_then(crate::sqlx::Either::left)
        .unwrap_or_default();
    if types.len() != args.len() {
        return Err(crate::sqlx::Error::Protocol(format!(
            "pq: got {} parameters but the statement requires {}",
            args.len(),
            types.len()
        )));
    }
    let mut a = PgArguments::default();
    for (i, (v, t)) in args.iter().zip(types).enumerate() {
        check_arg(i, v).map_err(crate::sqlx::Error::Protocol)?;
        let oid = t.oid().map(|o| o.0).unwrap_or(0);
        let bytes = match pq_text(v) {
            None => None,
            Some(text) => Some(
                pg_text_to_binary(oid, t.name(), &text, matches!(v, Value::Bytes(_)))
                    .map_err(crate::sqlx::Error::Protocol)?,
            ),
        };
        a.add(PgRaw { oid, bytes })
            .map_err(crate::sqlx::Error::Encode)?;
    }
    Ok(a)
}

fn arg_err(e: String) -> crate::sqlx::Error {
    crate::sqlx::Error::Protocol(e)
}

/// Runs `sql` with `args` for its rows, sending each converted row (or the first error) to `tx`;
/// stops when `tx` is closed.
pub async fn stream_rows(
    db: Db,
    sql: String,
    args: Vec<Value>,
    parse_time: bool,
    tx: tokio::sync::mpsc::Sender<Result<Value, String>>,
) {
    let res: Result<(), crate::sqlx::Error> = async {
        macro_rules! pump {
            ($stream:expr, $wrap:expr) => {{
                let mut s = $stream;
                while let Some(row) = s.try_next().await? {
                    let v = row_to_map(&$wrap(row));
                    if tx.send(v).await.is_err() {
                        break;
                    }
                }
            }};
        }
        match &db {
            Db::Postgres(p) => {
                let mut conn = p.acquire().await?;
                let a = pg_args(&mut conn, &sql, &args).await?;
                pump!(
                    crate::sqlx::query_with(crate::sqlx::AssertSqlSafe(sql.clone()), a)
                        .fetch(&mut *conn),
                    AnyRow::Postgres
                )
            }
            Db::MySql(p) if args.is_empty() => {
                pump!(
                    crate::sqlx::raw_sql(crate::sqlx::AssertSqlSafe(sql.clone())).fetch(p),
                    |r| AnyRow::MySql(r, parse_time, true)
                )
            }
            Db::MySql(p) => {
                let a = mysql_args(&args).map_err(arg_err)?;
                pump!(
                    crate::sqlx::query_with(crate::sqlx::AssertSqlSafe(sql.clone()), a).fetch(p),
                    |r| AnyRow::MySql(r, parse_time, false)
                )
            }
            Db::Sqlite(d) => {
                for (i, v) in args.iter().enumerate() {
                    check_arg(i, v).map_err(arg_err)?;
                }
                let (sql, args, tx) = (sql.clone(), args.clone(), tx.clone());
                d.with(move |c| {
                    crate::sqlite::query(c, &sql, &args, |row| tx.blocking_send(Ok(row)).is_ok())
                })
                .await
                .map_err(arg_err)?
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = res {
        let _ = tx.send(Err(db_error_text(&e))).await;
    }
}

/// `db.ExecContext(ctx, sql, args...)`: without arguments a simple query, as the Go drivers run it.
pub async fn exec(db: &Db, sql: &str, args: &[Value]) -> Result<(), crate::sqlx::Error> {
    if args.is_empty() {
        return db.exec_raw(sql).await;
    }
    let q = crate::sqlx::AssertSqlSafe(sql.to_string());
    match db {
        Db::Postgres(p) => {
            let mut conn = p.acquire().await?;
            let a = pg_args(&mut conn, sql, args).await?;
            crate::sqlx::query_with(q, a)
                .execute(&mut *conn)
                .await
                .map(drop)
        }
        Db::MySql(p) => crate::sqlx::query_with(q, mysql_args(args).map_err(arg_err)?)
            .execute(p)
            .await
            .map(drop),
        Db::Sqlite(d) => {
            for (i, v) in args.iter().enumerate() {
                check_arg(i, v).map_err(arg_err)?;
            }
            let (sql, args) = (sql.to_string(), args.to_vec());
            d.with(move |c| crate::sqlite::exec(c, &sql, &args))
                .await
                .map_err(arg_err)
        }
    }
}

/// squirrel's placeholder formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placeholder {
    Question,
    Dollar,
}

impl Placeholder {
    /// `postgres` and `clickhouse` take `$1`, the rest `?` (`oracle`/`gocosmos` colons are refused drivers).
    pub fn for_driver(d: Driver) -> Self {
        match d {
            Driver::Postgres => Placeholder::Dollar,
            _ => Placeholder::Question,
        }
    }

    /// squirrel `replacePositionalPlaceholders`: `?` becomes `$n`, `??` an escaped `?`.
    pub fn replace(self, sql: &str) -> String {
        if self == Placeholder::Question {
            return sql.to_string();
        }
        let mut out = String::with_capacity(sql.len() + 8);
        let mut n = 0;
        let mut chars = sql.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '?' {
                if chars.peek() == Some(&'?') {
                    chars.next();
                    out.push('?');
                } else {
                    n += 1;
                    out.push_str(&format!("${n}"));
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// The entry of `fuzz/fuzz_targets/values.rs`: `which` picks the parser `data` goes to (postgres
/// text input and binary columns, mysql binary and text datetimes, sqlite time text).
#[cfg(feature = "fuzzing")]
pub fn fuzz_values(which: u8, data: &[u8]) {
    match which % 5 {
        0 => {
            if let [o1, o2, flag, text @ ..] = data {
                let oid = u32::from(u16::from_be_bytes([*o1, *o2]));
                if let Ok(b) = pg_text_to_binary(oid, "t", text, flag & 1 == 1) {
                    if oid == 1700 {
                        assert!(decode_numeric(&b).is_some(), "an encoded numeric decodes");
                    }
                    let _ = pg_decode(oid, "t", b, false);
                }
            }
        }
        1 => {
            if let [o1, o2, rest @ ..] = data {
                let oid = u32::from(u16::from_be_bytes([*o1, *o2]));
                let _ = pg_decode(oid, "t", rest.to_vec(), false);
            }
        }
        2 => {
            if let Ok(t) = mysql_binary_datetime(data) {
                let _ = mysql_datetime_text(t);
            }
        }
        3 => {
            let _ = mysql_text_time(&String::from_utf8_lossy(data));
        }
        _ => {
            if let Some(t) = crate::sqlite::parse_time(&String::from_utf8_lossy(data)) {
                let _ = crate::sqlite::time_text(&t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_round_trips_through_its_binary_form() {
        for s in [
            "0",
            "1",
            "-1",
            "12345.678",
            "0.0001",
            "100000000",
            "-0.5",
            "3.14159265358979",
            "10000.0000",
        ] {
            let b = encode_numeric(s).unwrap();
            assert_eq!(decode_numeric(&b).unwrap(), s, "{s}");
        }
        assert_eq!(
            decode_numeric(&encode_numeric("1e3").unwrap()).unwrap(),
            "1000"
        );
        assert!(encode_numeric("abc").is_none());
        // The wire form postgres reads: leading and trailing zero groups dropped, the weight
        // counting groups of four digits from the point.
        for (t, b) in [
            ("0.0001", vec![0, 1, 0xff, 0xff, 0, 0, 0, 4, 0, 1]),
            ("10000", vec![0, 1, 0, 1, 0, 0, 0, 0, 0, 1]),
            ("1e8", vec![0, 1, 0, 2, 0, 0, 0, 0, 0, 1]),
            ("-12.5", vec![0, 2, 0, 0, 0x40, 0, 0, 1, 0, 12, 0x13, 0x88]),
            ("0", vec![0, 0, 0, 0, 0, 0, 0, 0]),
        ] {
            assert_eq!(encode_numeric(t).unwrap(), b, "{t}");
        }
    }

    #[test]
    fn numeric_text_past_postgres_limits_is_refused_without_allocating() {
        // postgres: at most 131072 digits before the point and 16383 after it.
        for t in [
            "1e999999999",
            "1e2147483647",
            "1e-2147483648",
            "1e-16384",
            "0.1e-16383",
            "1e131072",
        ] {
            assert!(encode_numeric(t).is_none(), "{t}");
        }
        for (t, back) in [
            ("1e131071", None),
            ("1e-16383", None),
            ("NaN", Some("NaN")),
            ("inf", Some("Infinity")),
            ("+Infinity", Some("Infinity")),
            ("-inf", Some("-Infinity")),
            ("-0", Some("0")),
            ("+.5", Some("0.5")),
        ] {
            let b = encode_numeric(t).expect(t);
            if let Some(back) = back {
                assert_eq!(decode_numeric(&b).unwrap(), back, "{t}");
            }
        }
        for bad in ["", ".", "-", "1e", "1ex", "1.2.3", "١"] {
            assert!(encode_numeric(bad).is_none(), "{bad:?}");
        }
        assert!(
            decode_numeric(&[0, 1, 0, 0, 0, 0, 0, 0]).is_none(),
            "a digit short"
        );
        assert!(decode_numeric(&[0, 0]).is_none());
    }

    #[test]
    fn uuid_and_bytea_hex_take_hex_digits_only() {
        let uuid = |t: &str| pg_text_to_binary(2950, "uuid", t.as_bytes(), false);
        // 16 two-byte characters are 32 bytes: refused, not sliced mid-character.
        assert!(uuid(&format!("a{}a", "é".repeat(15))).is_err());
        assert!(uuid("+a+a+a+a+a+a+a+a+a+a+a+a+a+a+a+a").is_err());
        assert_eq!(
            uuid("{A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11}").unwrap()[..2],
            [0xa0, 0xee]
        );
        assert_eq!(decode_bytea_text(b"\\x+f"), None);
        assert_eq!(decode_bytea_text(b"\\+77"), None);
        assert_eq!(decode_bytea_text(b"\\x0aFF"), Some(vec![0x0a, 0xff]));
    }

    #[test]
    fn dollar_placeholders() {
        assert_eq!(
            Placeholder::Dollar.replace("a = ? and b = ?? or c = ?"),
            "a = $1 and b = ? or c = $2"
        );
        assert_eq!(Placeholder::Question.replace("a = ?"), "a = ?");
    }

    #[test]
    fn non_array_args_error_names_the_go_type() {
        assert_eq!(
            args_from_value(Value::String("x".into())).unwrap_err(),
            "mapping returned non-array result: string"
        );
        assert!(check_arg(0, &Value::Object(Default::default()))
            .unwrap_err()
            .contains("map[string]interface {}"));
        assert!(check_arg(1, &Value::Uint(u64::MAX))
            .unwrap_err()
            .contains("$2"));
    }

    #[test]
    fn postgres_binary_columns_of_the_wrong_size_are_named() {
        let short = |oid: u32, name: &str| pg_decode(oid, name, vec![1], false).unwrap_err();
        for (oid, name) in [
            (21, "int2"),
            (23, "int4"),
            (20, "int8"),
            (26, "oid"),
            (700, "float4"),
            (701, "float8"),
            (1700, "numeric"),
            (2950, "uuid"),
            (1114, "timestamp"),
            (1082, "date"),
            (1083, "time"),
            (1266, "timetz"),
        ] {
            assert_eq!(
                short(oid, name),
                format!("unexpected {name} value of 1 bytes")
            );
        }
        assert!(pg_decode(16, "bool", Vec::new(), false).is_err());
        // timetz: eight bytes of time but no zone.
        assert!(pg_decode(1266, "timetz", vec![0; 8], false).is_err());
        let infinity = |b: Vec<u8>| pg_decode(1082, "date", b, false).unwrap();
        assert_eq!(
            infinity(i32::MAX.to_be_bytes().to_vec()),
            Value::String("infinity".into())
        );
        assert_eq!(
            infinity(i32::MIN.to_be_bytes().to_vec()),
            Value::String("-infinity".into())
        );
        assert_eq!(
            pg_decode(1186, "interval", vec![0; 16], false).unwrap_err(),
            "ekiben does not decode postgres columns of type interval"
        );
        // A text-format value (a simple query's rows) is its text, whatever the type.
        assert_eq!(
            pg_decode(23, "int4", b"42".to_vec(), true).unwrap(),
            Value::String("42".into())
        );
        assert_eq!(
            pg_decode(3802, "jsonb", b"\x01{}".to_vec(), false).unwrap(),
            Value::String("{}".into())
        );
        // float4 reads as the shortest float64 that prints the same, as lib/pq parses its text.
        assert_eq!(
            pg_decode(700, "float4", 1.1f32.to_be_bytes().to_vec(), false).unwrap(),
            Value::Float(1.1)
        );
        assert_eq!(f32_as_go(-2.5), -2.5);
        assert_eq!(
            pg_decode(26, "oid", 7u32.to_be_bytes().to_vec(), false).unwrap(),
            Value::String("7".into())
        );
    }

    #[test]
    fn mysql_binary_datetimes_refuse_what_sqlx_refused() {
        let e = |b: &[u8]| mysql_binary_datetime(b).unwrap_err();
        assert_eq!(e(&[]), "empty buffer");
        assert_eq!(e(&[0]), "unexpected null");
        assert_eq!(
            e(&[4, 0xe8, 0x07, 1]),
            "expected at least 4 bytes for date, got 3"
        );
        assert_eq!(
            e(&[4, 0xe8, 0x07, 13, 1]),
            "server returned invalid date: 2024/13/1"
        );
        assert_eq!(
            e(&[4, 0xff, 0xff, 1, 1]),
            "server returned invalid date: 65535/1/1"
        );
        assert_eq!(e(&[7, 0xe8, 0x07, 1, 2, 3, 4]), "expected 3 bytes for time");
        assert_eq!(
            e(&[11, 0xe8, 0x07, 1, 2, 25, 0, 0, 0, 0, 0, 0]),
            "server returned invalid time: 25:00:00; micros: 0"
        );
        assert_eq!(
            e(&[11, 0xe8, 0x07, 1, 2, 3, 4, 5, 0x40, 0x42, 0x0f, 0]),
            "server returned invalid time: 03:04:05; micros: 1000000"
        );
        let t = mysql_binary_datetime(&[7, 0xe8, 0x07, 1, 2, 3, 4, 5]).unwrap();
        assert_eq!(mysql_datetime_text(t), "2024-01-02 03:04:05");
        let d = mysql_binary_datetime(&[4, 0xe8, 0x07, 1, 2]).unwrap();
        assert_eq!(mysql_datetime_text(d), "2024-01-02 00:00:00");
    }

    #[test]
    fn mysql_text_times_name_why_a_date_is_refused() {
        let e = |s: &str| mysql_text_time(s).unwrap_err();
        assert_eq!(e("2024"), "parsing time \"2024\": premature end of input");
        assert_eq!(
            e("2024/01/02"),
            "parsing time \"2024/01/02\": input contains invalid characters"
        );
        assert_eq!(e(""), "parsing time \"\": premature end of input");
        assert_eq!(
            e("x"),
            "parsing time \"x\": input contains invalid characters"
        );
        assert_eq!(
            e("2024-01-"),
            "parsing time \"2024-01-\": premature end of input"
        );
        assert_eq!(
            e("2024-1x-02"),
            "parsing time \"2024-1x-02\": input contains invalid characters"
        );
        assert_eq!(
            e("2024-01-02x"),
            "parsing time \"2024-01-02x\": trailing input"
        );
        assert_eq!(
            e("2024-13-02"),
            "parsing time \"2024-13-02\": input is out of range"
        );
        assert_eq!(
            e("2023-02-29"),
            "parsing time \"2023-02-29\": no possible date and time matching input"
        );
        assert_eq!(
            e("9999-12-31 23:00:00"),
            "parsing time \"9999-12-31 23:00:00\": input is out of range"
        );
        assert!(matches!(
            mysql_text_time("0000-00-00 00:00:00"),
            Ok(Value::Timestamp(_))
        ));
        assert!(matches!(
            mysql_text_time("2024-01-02"),
            Ok(Value::Timestamp(_))
        ));
    }

    #[test]
    fn postgres_offsets_past_a_clock_are_not_offsets() {
        for t in [
            "2024-01-02T03:04:05+2:00",
            "2024-01-02T03:04:05+24:00",
            "2024-01-02T03:04:05+05:60",
            "2024-01-02T03:04:05+0a",
            "2024-01-02T03:04:05+24",
            "2024-01-02T03:04:05+123",
        ] {
            assert!(
                parse_with_offset(t, "%Y-%m-%dT%H:%M:%S%.f").is_none(),
                "{t}"
            );
        }
        // The widest offsets a clock has are offsets.
        for (t, secs) in [("+23:59", 86_340), ("-23:59", -86_340), ("+23", 82_800)] {
            let at = parse_with_offset(&format!("2024-01-02T00:00:00{t}"), "%Y-%m-%dT%H:%M:%S%.f")
                .unwrap_or_else(|| panic!("{t}"));
            assert_eq!(at.as_second(), 1_704_153_600 - secs, "{t}");
        }
        assert_eq!(
            db_error_text(&crate::sqlx::Error::RowNotFound),
            crate::sqlx::Error::RowNotFound.to_string()
        );
        assert_eq!(
            db_error_text(&crate::sqlx::Error::Protocol("p".into())),
            "p"
        );
        // lib/pq's parameters carry no type: the server infers it from the statement.
        assert_eq!(
            <PgRaw as crate::sqlx::Type<Postgres>>::type_info().oid(),
            Some(crate::sqlx::postgres::types::Oid(0))
        );
    }

    /// The pool of the CI database `dsn` names.
    async fn env_db(dsn: &str, driver: Driver) -> Db {
        let (t, _) = crate::conn_fields::parse_dsn(driver, dsn, None).unwrap();
        let quotas = ekiben_core::OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        };
        let org = ekiben_core::OrgContext::new(ekiben_core::OrgId::new("o"), quotas);
        let s =
            crate::conn_fields::ConnSettings::from_config(&serde_json::Map::new(), None).unwrap();
        crate::conn_fields::open(&t, &s, &org).await.unwrap()
    }

    async fn rows(
        db: &Db,
        sql: &str,
        args: Vec<Value>,
        parse_time: bool,
    ) -> Vec<Result<Value, String>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(stream_rows(db.clone(), sql.into(), args, parse_time, tx));
        let mut out = Vec::new();
        while let Some(r) = rx.recv().await {
            out.push(r);
        }
        out
    }

    #[tokio::test]
    async fn postgres_arguments_nulls_counts_and_rows() {
        if let Some(dsn) = crate::testing::env_dsn("PG_DSN") {
            postgres_arguments(&dsn).await;
        }
    }

    async fn postgres_arguments(dsn: &str) {
        let db = env_db(dsn, Driver::Postgres).await;
        let got = rows(
            &db,
            "SELECT $1::int4 IS NULL AS n",
            vec![Value::Null],
            false,
        )
        .await;
        assert_eq!(got[0].as_ref().unwrap().to_go_json(), r#"{"n":true}"#);
        let got = rows(&db, "SELECT $1::int4 AS a", vec![], false).await;
        assert_eq!(
            got,
            vec![Err(
                "pq: got 0 parameters but the statement requires 1".to_string()
            )]
        );
        let e = exec(&db, "SELECT $1::int4, $2::int4", &[Value::Int(1)])
            .await
            .unwrap_err();
        assert_eq!(
            db_error_text(&e),
            "pq: got 1 parameters but the statement requires 2"
        );
        let got = rows(
            &db,
            "SELECT $1::text AS a",
            vec![Value::Array(vec![].into())],
            false,
        )
        .await;
        assert!(got[0]
            .as_ref()
            .unwrap_err()
            .contains("unsupported type []interface {}"));
        let p = crate::sqlx::postgres::PgPool::connect_lazy_with(
            crate::conn_fields::parse_postgres_dsn(dsn).unwrap().opts,
        );
        // A simple query's row comes back as text; a column past the row's end is an error.
        let row = crate::sqlx::raw_sql(crate::sqlx::AssertSqlSafe("SELECT 7 AS a".to_string()))
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(pg_value(&row, 0).unwrap(), Value::String("7".into()));
        assert!(pg_value(&row, 9).unwrap_err().contains("out of bounds"));
        db.close().await;
    }

    #[tokio::test]
    async fn mysql_arguments_of_every_type_and_time_edges() {
        if let Some(dsn) = crate::testing::env_dsn("MY_DSN") {
            mysql_arguments(&dsn).await;
        }
    }

    async fn mysql_arguments(dsn: &str) {
        let db = env_db(dsn, Driver::MySql).await;
        let t = crate::sqlite::parse_time("2024-01-02T03:04:05.25+01:00").unwrap();
        let args = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(-2),
            Value::Uint(7),
            Value::Float(1.5),
            Value::String("s".into()),
            Value::Bytes(b"b".to_vec()),
            Value::Timestamp(t),
        ];
        let got = rows(
            &db,
            "SELECT ? AS a, ? AS b, ? AS c, ? AS d, ? AS e, ? AS f, ? AS g, CAST(? AS DATETIME(6)) AS h, CAST(18446744073709551615 AS UNSIGNED) AS u, CAST(5 AS UNSIGNED) AS v, CAST(1.1 AS FLOAT) AS w",
            args,
            false,
        )
        .await;
        assert_eq!(
            got[0].as_ref().unwrap().to_go_json(),
            r#"{"a":null,"b":1,"c":-2,"d":7,"e":1.5,"f":"s","g":"b","h":"2024-01-02 02:04:05.250000","u":"18446744073709551615","v":5,"w":1.1}"#
        );
        let got = rows(
            &db,
            "SELECT ? AS a",
            vec![Value::Object(Default::default())],
            false,
        )
        .await;
        assert!(got[0]
            .as_ref()
            .unwrap_err()
            .contains("map[string]interface {}"));
        // A TIME past a day, and a DATETIME past what a time.Time in UTC holds.
        let got = rows(
            &db,
            "SELECT CAST('25:00:00' AS TIME) AS t, ? AS x",
            vec![Value::Int(1)],
            false,
        )
        .await;
        assert!(
            got[0]
                .as_ref()
                .unwrap_err()
                .contains("is not a time of day"),
            "{got:?}"
        );
        let got = rows(
            &db,
            "SELECT CAST(? AS DATETIME) AS t",
            vec![Value::String("9999-12-31 23:00:00".into())],
            true,
        )
        .await;
        assert!(
            got[0].as_ref().unwrap_err().contains("is out of range"),
            "{got:?}"
        );
        let p = crate::sqlx::mysql::MySqlPool::connect_lazy_with(
            crate::conn_fields::parse_mysql_dsn(dsn).unwrap().0,
        );
        let row = crate::sqlx::raw_sql(crate::sqlx::AssertSqlSafe("SELECT 7 AS a".to_string()))
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(
            mysql_value(&row, 0, false, true).unwrap(),
            Value::String("7".into())
        );
        // parseTime reads only time columns; a number stays the text the protocol gave.
        assert_eq!(
            mysql_value(&row, 0, true, true).unwrap(),
            Value::String("7".into())
        );
        // Without arguments go-sql-driver runs a text-protocol query: every value is text.
        let got = rows(&db, "SELECT 7 AS a", vec![], false).await;
        assert_eq!(got[0].as_ref().unwrap().to_go_json(), r#"{"a":"7"}"#);
        assert!(mysql_value(&row, 9, false, true)
            .unwrap_err()
            .contains("out of bounds"));
        db.close().await;
    }

    #[test]
    fn lib_pq_text_into_binary() {
        assert_eq!(
            pg_text_to_binary(23, "integer", b"5", false).unwrap(),
            5i32.to_be_bytes()
        );
        assert_eq!(
            pg_text_to_binary(23, "integer", b"x", false).unwrap_err(),
            "pq: invalid input syntax for type integer: \"x\""
        );
        assert_eq!(
            pg_text_to_binary(16, "boolean", b"true", false).unwrap(),
            vec![1]
        );
        assert_eq!(
            pg_text_to_binary(17, "bytea", b"\\x0aff", false).unwrap(),
            vec![10, 255]
        );
        let micros = parse_pg_timestamp("2000-01-01T00:00:01Z").unwrap();
        assert_eq!(micros, 1_000_000);
        assert_eq!(
            pg_text_to_binary(2950, "uuid", b"a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11", false)
                .unwrap()[0],
            0xa0
        );
        assert!(pg_text_to_binary(600, "point", b"(1,2)", false)
            .unwrap_err()
            .contains("point"));
    }

    #[test]
    fn go_type_names_every_value_as_go_prints_it() {
        let cases = [
            (Value::Null, "<nil>"),
            (Value::Nothing, "<nil>"),
            (Value::Delete, "<nil>"),
            (Value::Bool(true), "bool"),
            (Value::Int(1), "int64"),
            (Value::Uint(1), "uint64"),
            (Value::Float(1.0), "float64"),
            (Value::Number("1".into()), "json.Number"),
            (Value::String("s".into()), "string"),
            (Value::Bytes(b"b".to_vec()), "[]uint8"),
            (Value::Array(vec![].into()), "[]interface {}"),
            (Value::Object(Default::default()), "map[string]interface {}"),
        ];
        for (v, want) in cases {
            assert_eq!(go_type(&v), want, "{v:?}");
        }
        assert_eq!(
            go_type(&Value::Timestamp(
                crate::sqlite::parse_time("2024-01-02T03:04:05Z").expect("a time")
            )),
            "time.Time"
        );
        assert_eq!(
            args_from_value(Value::Array(vec![Value::Int(1)].into())).unwrap(),
            vec![Value::Int(1)]
        );
    }

    #[test]
    fn database_sql_refuses_slices_maps_and_high_uints_only() {
        assert_eq!(
            check_arg(2, &Value::Array(vec![].into())).unwrap_err(),
            "sql: converting argument $3 type: unsupported type []interface {}, a slice of interface"
        );
        assert!(check_arg(0, &Value::Uint((1 << 63) - 1)).is_ok());
        assert!(check_arg(0, &Value::Uint(1 << 63)).is_err());
        assert!(check_arg(0, &Value::String("x".into())).is_ok());
    }

    #[test]
    fn lib_pq_text_of_every_scalar() {
        let text = |v: Value| pq_text(&v).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(text(Value::Bool(false)).as_deref(), Some("false"));
        assert_eq!(text(Value::Int(-3)).as_deref(), Some("-3"));
        assert_eq!(
            text(Value::Uint(u64::MAX)).as_deref(),
            Some("18446744073709551615")
        );
        assert_eq!(
            text(Value::Float(f64::INFINITY)).as_deref(),
            Some("Infinity")
        );
        assert_eq!(
            text(Value::Float(f64::NEG_INFINITY)).as_deref(),
            Some("-Infinity")
        );
        assert_eq!(text(Value::Float(f64::NAN)).as_deref(), Some("NaN"));
        assert_eq!(text(Value::Float(-0.25)).as_deref(), Some("-0.25"));
        assert_eq!(text(Value::Number("1e3".into())).as_deref(), Some("1e3"));
        assert_eq!(text(Value::Bytes(b"raw".to_vec())).as_deref(), Some("raw"));
        assert_eq!(text(Value::Nothing), None);
        assert_eq!(text(Value::Delete), None);
        assert_eq!(text(Value::Array(vec![].into())), None);
        assert_eq!(text(Value::Object(Default::default())), None);
        let t = crate::sqlite::parse_time("2024-01-02T03:04:05Z").expect("a time");
        assert_eq!(
            text(Value::Timestamp(t)).as_deref(),
            Some("2024-01-02 03:04:05Z")
        );
    }

    #[test]
    fn pq_timestamps_carry_negative_and_second_offsets() {
        let t = crate::sqlite::parse_time("2024-01-02T03:04:05-05:30").expect("a time");
        assert_eq!(format_pq_ts(&t), "2024-01-02 03:04:05-05:30");
        let odd = jiff::Timestamp::from_second(0)
            .unwrap()
            .to_zoned(TimeZone::fixed(Offset::from_seconds(3600 + 125).unwrap()));
        assert_eq!(format_pq_ts(&odd), "1970-01-01 01:02:05+01:02:05");
    }

    #[test]
    fn postgres_text_input_of_every_encoded_type() {
        let ok =
            |oid, name, text: &str| pg_text_to_binary(oid, name, text.as_bytes(), false).unwrap();
        let err = |oid, name, text: &str| {
            pg_text_to_binary(oid, name, text.as_bytes(), false).unwrap_err()
        };
        for t in ["t", "TRUE", " yes ", "on", "1", "y"] {
            assert_eq!(ok(16, "boolean", t), vec![1], "{t}");
        }
        for t in ["f", "false", "n", "no", "OFF", "0"] {
            assert_eq!(ok(16, "boolean", t), vec![0], "{t}");
        }
        assert_eq!(
            err(16, "boolean", "maybe"),
            "pq: invalid input syntax for type boolean: \"maybe\""
        );
        assert_eq!(ok(21, "smallint", "-32768"), (-32768i16).to_be_bytes());
        assert_eq!(ok(21, "smallint", "32767"), 32767i16.to_be_bytes());
        assert_eq!(
            err(21, "smallint", "32768"),
            "pq: value \"32768\" is out of range for type smallint"
        );
        assert_eq!(
            err(21, "smallint", "-32769"),
            "pq: value \"-32769\" is out of range for type smallint"
        );
        assert_eq!(ok(23, "integer", " 7 "), 7i32.to_be_bytes());
        assert_eq!(
            err(23, "integer", "2147483648"),
            "pq: value \"2147483648\" is out of range for type integer"
        );
        assert_eq!(
            ok(20, "bigint", "-9223372036854775808"),
            i64::MIN.to_be_bytes()
        );
        assert_eq!(ok(26, "oid", "4294967295"), u32::MAX.to_be_bytes());
        assert_eq!(
            err(26, "oid", "-1"),
            "pq: invalid input syntax for type oid: \"-1\""
        );
        assert_eq!(ok(700, "real", "1.5"), 1.5f32.to_be_bytes());
        assert_eq!(
            ok(701, "double precision", "-2.25"),
            (-2.25f64).to_be_bytes()
        );
        assert!(
            f64::from_be_bytes(ok(701, "double precision", "NaN").try_into().unwrap()).is_nan()
        );
        for (t, want) in [
            ("Infinity", f64::INFINITY),
            ("inf", f64::INFINITY),
            ("+infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("-inf", f64::NEG_INFINITY),
        ] {
            assert_eq!(ok(701, "double precision", t), want.to_be_bytes(), "{t}");
        }
        assert_eq!(
            err(701, "double precision", "x"),
            "pq: invalid input syntax for type double precision: \"x\""
        );
        assert_eq!(
            pg_text_to_binary(17, "bytea", b"\\x00", true).unwrap(),
            b"\\x00"
        );
        assert_eq!(
            err(17, "bytea", "\\x0"),
            "pq: invalid input syntax for type bytea: \"\\x0\""
        );
        for oid in [25, 1043, 1042, 19, 114, 142, 705, 18] {
            assert_eq!(ok(oid, "text", "abc"), b"abc", "{oid}");
        }
        assert_eq!(ok(3802, "jsonb", "{}"), b"\x01{}");
        assert_eq!(
            ok(2950, "uuid", "{a0eebc999c0b4ef8bb6d6bb9bd380a11}").len(),
            16
        );
        assert_eq!(
            err(2950, "uuid", "a0ee"),
            "pq: invalid input syntax for type uuid: \"a0ee\""
        );
        assert_eq!(
            err(2950, "uuid", "g0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"),
            "pq: invalid input syntax for type uuid: \"g0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11\""
        );
        assert_eq!(ok(1700, "numeric", "1.5"), encode_numeric("1.5").unwrap());
        assert_eq!(
            err(1700, "numeric", "one"),
            "pq: invalid input syntax for type numeric: \"one\""
        );
        assert_eq!(
            ok(1114, "timestamp", "2000-01-01 00:00:02"),
            2_000_000i64.to_be_bytes()
        );
        assert_eq!(
            err(1184, "timestamp with time zone", "soon"),
            "pq: invalid input syntax for type timestamp with time zone: \"soon\""
        );
        assert_eq!(ok(1082, "date", "2000-01-03"), 2i32.to_be_bytes());
        assert_eq!(ok(1082, "date", "1999-12-31 BC?"), (-1i32).to_be_bytes());
        assert_eq!(
            err(1082, "date", "2000"),
            "pq: invalid input syntax for type date: \"2000\""
        );
    }

    #[test]
    fn bytea_escape_text_decodes_octets_and_backslashes() {
        assert_eq!(
            decode_bytea_text(b"a\\\\b\\001"),
            Some(b"a\\b\x01".to_vec())
        );
        assert_eq!(decode_bytea_text(b"\\x"), Some(vec![]));
        assert_eq!(
            decode_bytea_text(b"a\\001b\\002"),
            Some(b"a\x01b\x02".to_vec())
        );
        assert_eq!(decode_bytea_text(b"\\xzz"), None);
        assert_eq!(decode_bytea_text(b"\\01"), None, "a cut octal escape");
        assert_eq!(decode_bytea_text(b"\\999"), None, "no octal digits");
    }

    #[test]
    fn pq_text_formats() {
        let t = crate::sqlite::parse_time("2024-01-02T03:04:05.5+02:00").expect("a time");
        assert_eq!(format_pq_ts(&t), "2024-01-02 03:04:05.5+02:00");
        assert_eq!(pq_text(&Value::Float(1.5)).unwrap(), b"1.5");
        assert!(pq_text(&Value::Null).is_none());
    }

    /// The time texts of every driver, pinned from the chrono build before the move to jiff
    /// (T-7245): the helpers above the loop differ by build, the expected text does not.
    #[test]
    fn time_wire_formats_match_the_chrono_build() {
        let ts = |s: &str| crate::sqlite::parse_time(s).expect("a time");
        let show = |v: Result<Value, String>| match v {
            Ok(Value::Timestamp(t)) => ekiben_bloblang::value::format_rfc3339_nano(&t),
            other => format!("{other:?}"),
        };
        let some = |v: Option<Value>| v.ok_or("none").map_err(String::from);
        let pg_ts = |m: i64| some(pg_timestamp(m));
        let pg_date = |d: i32| some(pg_date(d));
        let pg_time = |m: i64, zone: Option<i32>| some(pg_time(m, zone));
        let mysql_text =
            |s: &str| mysql_datetime_text(ts(s).with_time_zone(TimeZone::UTC).datetime());
        let mut got = Vec::new();
        for t in [
            "2024-01-02T03:04:05Z",
            "2024-01-02T03:04:05.000000001-05:30",
            "2024-02-29T23:59:59.120+14:00",
            "0001-01-01T00:00:00.5Z",
        ] {
            got.push(format!("pq {}", format_pq_ts(&ts(t))));
            got.push(format!("sqlite {}", crate::sqlite::time_text(&ts(t))));
            got.push(format!("mysql {}", mysql_text(t)));
        }
        for s in [
            "2024-01-02 03:04:05.123456+02",
            "2024-01-02 03:04:05.5-0330",
            "2024-01-02T03:04:05Z",
            "2024-01-02 03:04:05+05:30",
            "2024-01-02 03:04:05",
            "2024-01-02 03:04",
            "2024-01-02",
            "1999-12-31 23:59:59.999999",
            "INFINITY",
            "-infinity",
            "2024-13-02",
            "junk",
        ] {
            got.push(format!("pg {s:?} {:?}", parse_pg_timestamp(s)));
            got.push(format!("pg date {s:?} {:?}", pg_date_days(s)));
            got.push(format!(
                "sqlite parse {s:?} {:?}",
                crate::sqlite::parse_time(s).map(|t| show(Ok(Value::Timestamp(t))))
            ));
            got.push(format!("mysql parse {s:?} {}", show(mysql_text_time(s))));
        }
        for s in [
            "2024-01-02 03:04:05.5+02:00",
            "2024-01-02T03:04:05.5+02:00Z",
            "2024-01-02T03:04Z",
            "0000-00-00 00:00:00",
        ] {
            got.push(format!(
                "sqlite parse {s:?} {:?}",
                crate::sqlite::parse_time(s).map(|t| show(Ok(Value::Timestamp(t))))
            ));
            got.push(format!("mysql parse {s:?} {}", show(mysql_text_time(s))));
        }
        for m in [0, -1, 1_234_567_890_123_456, -63_082_281_600_000_000] {
            got.push(format!("pg timestamp {m} {}", show(pg_ts(m))));
        }
        for d in [0, -1, 9_000, -730_119] {
            got.push(format!("pg date {d} {}", show(pg_date(d))));
        }
        for (m, z) in [
            (0, None),
            (45_296_000_001, None),
            (45_296_500_000, Some(-7_200)),
            (1, Some(19_800)),
        ] {
            got.push(format!("pg time {m} {z:?} {}", show(pg_time(m, z))));
        }
        let got = got.join("\n");
        assert_eq!(got, TIME_GOLDEN.trim(), "\n{got}");
    }

    const TIME_GOLDEN: &str = r#"
pq 2024-01-02 03:04:05Z
sqlite 2024-01-02 03:04:05+00:00
mysql 2024-01-02 03:04:05
pq 2024-01-02 03:04:05.000000001-05:30
sqlite 2024-01-02 03:04:05.000000001-05:30
mysql 2024-01-02 08:34:05
pq 2024-02-29 23:59:59.12+14:00
sqlite 2024-02-29 23:59:59.12+14:00
mysql 2024-02-29 09:59:59.120000
pq 0001-01-01 00:00:00.5Z
sqlite 0001-01-01 00:00:00.5+00:00
mysql 0001-01-01 00:00:00.500000
pg "2024-01-02 03:04:05.123456+02" Some(757472645123456)
pg date "2024-01-02 03:04:05.123456+02" Some(8767)
sqlite parse "2024-01-02 03:04:05.123456+02" None
mysql parse "2024-01-02 03:04:05.123456+02" Err("parsing time \"2024-01-02 03:04:05.123456+02\": trailing input")
pg "2024-01-02 03:04:05.5-0330" Some(757492445500000)
pg date "2024-01-02 03:04:05.5-0330" Some(8767)
sqlite parse "2024-01-02 03:04:05.5-0330" Some("2024-01-02T03:04:05.5-03:30")
mysql parse "2024-01-02 03:04:05.5-0330" Err("parsing time \"2024-01-02 03:04:05.5-0330\": trailing input")
pg "2024-01-02T03:04:05Z" Some(757479845000000)
pg date "2024-01-02T03:04:05Z" Some(8767)
sqlite parse "2024-01-02T03:04:05Z" Some("2024-01-02T03:04:05Z")
mysql parse "2024-01-02T03:04:05Z" Err("parsing time \"2024-01-02T03:04:05Z\": trailing input")
pg "2024-01-02 03:04:05+05:30" Some(757460045000000)
pg date "2024-01-02 03:04:05+05:30" Some(8767)
sqlite parse "2024-01-02 03:04:05+05:30" Some("2024-01-02T03:04:05+05:30")
mysql parse "2024-01-02 03:04:05+05:30" Err("parsing time \"2024-01-02 03:04:05+05:30\": trailing input")
pg "2024-01-02 03:04:05" Some(757479845000000)
pg date "2024-01-02 03:04:05" Some(8767)
sqlite parse "2024-01-02 03:04:05" Some("2024-01-02T03:04:05Z")
mysql parse "2024-01-02 03:04:05" 2024-01-02T03:04:05Z
pg "2024-01-02 03:04" None
pg date "2024-01-02 03:04" Some(8767)
sqlite parse "2024-01-02 03:04" Some("2024-01-02T03:04:00Z")
mysql parse "2024-01-02 03:04" Err("parsing time \"2024-01-02 03:04\": trailing input")
pg "2024-01-02" Some(757468800000000)
pg date "2024-01-02" Some(8767)
sqlite parse "2024-01-02" Some("2024-01-02T00:00:00Z")
mysql parse "2024-01-02" 2024-01-02T00:00:00Z
pg "1999-12-31 23:59:59.999999" Some(-1)
pg date "1999-12-31 23:59:59.999999" Some(-1)
sqlite parse "1999-12-31 23:59:59.999999" Some("1999-12-31T23:59:59.999999Z")
mysql parse "1999-12-31 23:59:59.999999" 1999-12-31T23:59:59.999999Z
pg "INFINITY" Some(9223372036854775807)
pg date "INFINITY" None
sqlite parse "INFINITY" None
mysql parse "INFINITY" Err("parsing time \"INFINITY\": input contains invalid characters")
pg "-infinity" Some(-9223372036854775808)
pg date "-infinity" None
sqlite parse "-infinity" None
mysql parse "-infinity" Err("parsing time \"-infinity\": input contains invalid characters")
pg "2024-13-02" None
pg date "2024-13-02" None
sqlite parse "2024-13-02" None
mysql parse "2024-13-02" Err("parsing time \"2024-13-02\": input is out of range")
pg "junk" None
pg date "junk" None
sqlite parse "junk" None
mysql parse "junk" Err("parsing time \"junk\": input contains invalid characters")
sqlite parse "2024-01-02 03:04:05.5+02:00" Some("2024-01-02T03:04:05.5+02:00")
mysql parse "2024-01-02 03:04:05.5+02:00" Err("parsing time \"2024-01-02 03:04:05.5+02:00\": trailing input")
sqlite parse "2024-01-02T03:04:05.5+02:00Z" Some("2024-01-02T03:04:05.5+02:00")
mysql parse "2024-01-02T03:04:05.5+02:00Z" Err("parsing time \"2024-01-02T03:04:05.5+02:00Z\": trailing input")
sqlite parse "2024-01-02T03:04Z" Some("2024-01-02T03:04:00Z")
mysql parse "2024-01-02T03:04Z" Err("parsing time \"2024-01-02T03:04Z\": trailing input")
sqlite parse "0000-00-00 00:00:00" None
mysql parse "0000-00-00 00:00:00" 1970-01-01T00:00:00Z
pg timestamp 0 2000-01-01T00:00:00Z
pg timestamp -1 1999-12-31T23:59:59.999999Z
pg timestamp 1234567890123456 2039-02-13T23:31:30.123456Z
pg timestamp -63082281600000000 0001-01-01T00:00:00Z
pg date 0 2000-01-01T00:00:00Z
pg date -1 1999-12-31T00:00:00Z
pg date 9000 2024-08-22T00:00:00Z
pg date -730119 0001-01-01T00:00:00Z
pg time 0 None 0000-01-01T00:00:00Z
pg time 45296000001 None 0000-01-01T12:34:56.000001Z
pg time 45296500000 Some(-7200) 0000-01-01T12:34:56.5+02:00
pg time 1 Some(19800) 0000-01-01T00:00:00.000001-05:30
"#;
}
