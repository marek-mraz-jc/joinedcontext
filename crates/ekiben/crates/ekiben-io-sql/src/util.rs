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
            if hex.len() != 32 {
                return Err(pg_invalid("uuid", &t));
            }
            (0..16)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| pg_invalid("uuid", &t))?
        }
        1700 => encode_numeric(&s()).ok_or_else(|| pg_invalid("numeric", &s()))?,
        1114 | 1184 => {
            let t = s();
            let micros =
                parse_pg_timestamp(&t, oid == 1184).ok_or_else(|| pg_invalid(type_name, &t))?;
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
        if hex.len() % 2 != 0 {
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
            let oct = std::str::from_utf8(text.get(i + 1..i + 4)?).ok()?;
            out.push(u8::from_str_radix(oct, 8).ok()?);
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
fn parse_pg_timestamp(text: &str, with_tz: bool) -> Option<i64> {
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
    // ponytail: a text without an offset is read as UTC for timestamptz (the session TimeZone of
    // a stock postgres image); a non-UTC session TimeZone is a listed gap.
    let _ = with_tz;
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
    let mut digits: String = format!("{ip}{fp}");
    let mut point = ip.len() as i32 + exp;
    let dscale = (fp.len() as i32 - exp).max(0) as u16;
    // Left-pad so the decimal point sits on a 4-digit group boundary.
    let lead = (4 - point.rem_euclid(4)) % 4;
    digits.insert_str(0, &"0".repeat(lead as usize));
    point += lead;
    while !digits.len().is_multiple_of(4) || (digits.len() as i32) < point {
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
    let raw = row.try_get_raw(i).map_err(|e| e.to_string())?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let ti = raw.type_info().into_owned();
    let name = ti.name().to_string();
    let oid = ti.oid().map(|o| o.0).unwrap_or(0);
    let b = raw.as_bytes().map_err(|e| e.to_string())?.to_vec();
    if raw.format() == PgValueFormat::Text {
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
    let raw = row.try_get_raw(i).map_err(|e| e.to_string())?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let name = raw.type_info().name().to_string();
    let bytes = <&[u8] as Decode<MySql>>::decode(raw.clone()).map_err(|e| e.to_string())?;
    let text = |b: &[u8]| Value::String(String::from_utf8_lossy(b).into_owned());
    let base = name.trim_end_matches(" UNSIGNED");
    let unsigned = name.ends_with("UNSIGNED");
    let time_kind = matches!(base, "DATETIME" | "TIMESTAMP" | "DATE");
    match text_protocol {
        true => {
            if parse_time && time_kind {
                return mysql_text_time(&String::from_utf8_lossy(bytes)).map_err(|e| e.to_string());
            }
            Ok(text(bytes))
        }
        false => Ok(match base {
            "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" | "YEAR"
                if unsigned =>
            {
                let v = <u64 as Decode<MySql>>::decode(raw).map_err(|e| e.to_string())?;
                // go-sql-driver hands an unsigned value past int64 on as text.
                i64::try_from(v)
                    .map(Value::Int)
                    .unwrap_or_else(|_| Value::String(v.to_string()))
            }
            "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" | "YEAR" => {
                Value::Int(<i64 as Decode<MySql>>::decode(raw).map_err(|e| e.to_string())?)
            }
            "FLOAT" => {
                let f = <f32 as Decode<MySql>>::decode(raw).map_err(|e| e.to_string())?;
                // Go marshals a float32 with 32-bit precision: 1.1, not 1.100000023841858.
                Value::Float(f32_as_go(f))
            }
            "DOUBLE" => {
                Value::Float(<f64 as Decode<MySql>>::decode(raw).map_err(|e| e.to_string())?)
            }
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
                let t = <MySqlTime as Decode<MySql>>::decode(raw).map_err(|e| e.to_string())?;
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
            Value::Null | Value::Nothing | Value::Delete => a.add(None::<String>),
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
            Value::Array(_) | Value::Object(_) => unreachable_arg(),
        };
        r.map_err(|e| e.to_string())?;
    }
    Ok(a)
}

fn unreachable_arg() -> Result<(), crate::sqlx::error::BoxDynError> {
    Err("unsupported argument".into())
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
    let types: Vec<PgTypeInfo> = match stmt.parameters() {
        Some(crate::sqlx::Either::Left(t)) => t.to_vec(),
        _ => Vec::new(),
    };
    if types.len() != args.len() {
        return Err(crate::sqlx::Error::Protocol(format!(
            "pq: got {} parameters but the statement requires {}",
            args.len(),
            types.len()
        )));
    }
    let mut a = PgArguments::default();
    for (i, (v, t)) in args.iter().zip(&types).enumerate() {
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
        let micros = parse_pg_timestamp("2000-01-01T00:00:01Z", true).unwrap();
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
        let some = |v: Option<Value>| v.ok_or_else(|| "none".to_string());
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
            got.push(format!("pg {s:?} {:?}", parse_pg_timestamp(s, true)));
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
