use jiff::civil::{self, Weekday};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone, TimeZoneDatabase};
use jiff::{SignedDuration, Timestamp, Zoned};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

/// Translates a Go `time` reference layout (`Mon Jan 2 15:04:05 -0700 MST 2006`)
/// into tokens and formats or parses timestamps accordingly.
///
/// Supports all layouts used across Bento docs and PART B seed files, including:
/// - `2006-01-02T15:04:05Z07:00`
/// - `2006-01-02T15:04:05Z`
/// - `2006-01-02T15:04:05`
/// - `2006-Jan-02 15:04:05.999999`
/// - `2006-Jan-02 15:04:05`
/// - `2006-Jan-02`
/// - `2006-01-02`
/// - `2006-01`
/// - `2.1.2006`
/// - `-07:00`, `-0700`, `Z07:00`, `Z0700`
/// - `Mon Jan 2 15:04:05 -0700 MST 2006`
/// - `RFC3339` / `RFC3339Nano` (`2006-01-02T15:04:05.999999999Z07:00`)
pub const GO_RFC3339_NANO: &str = "2006-01-02T15:04:05.999999999Z07:00";

/// An IANA time zone by its exact name (`Europe/Bratislava`), from jiff's bundled database and
/// never the host's: /usr/share/zoneinfo differs between images (Debian moves `US/Eastern` and
/// the other backward links to tzdata-legacy). Names are case-sensitive, as in chrono-tz (jiff
/// itself matches them case-insensitively). ponytail: the kept set is bounded by the database
/// (about 600 names, and only names that resolve are kept).
pub fn zone(name: &str) -> Result<TimeZone, String> {
    static KEPT: OnceLock<Mutex<HashMap<String, TimeZone>>> = OnceLock::new();
    static BUNDLED: OnceLock<TimeZoneDatabase> = OnceLock::new();
    let mut kept = KEPT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(tz) = kept.get(name) {
        return Ok(tz.clone());
    }
    let unknown = || format!("unknown time zone {name}");
    let tz = BUNDLED
        .get_or_init(TimeZoneDatabase::bundled)
        .get(name)
        .map_err(|_| unknown())?;
    if tz.iana_name() != Some(name) {
        return Err(unknown());
    }
    kept.insert(name.to_owned(), tz.clone());
    Ok(tz)
}

/// `ts` in the fixed offset `offset_secs`, as chrono's `DateTime<FixedOffset>` held it (an
/// offset of a day or more is refused, as chrono's `FixedOffset` refused it).
pub fn fixed(ts: Timestamp, offset_secs: i32) -> Result<Zoned, String> {
    let offset = Offset::from_seconds(offset_secs)
        .ok()
        .filter(|_| offset_secs.abs() < 86_400)
        .ok_or_else(|| format!("invalid timezone offset {offset_secs}"))?;
    Ok(ts.to_zoned(TimeZone::fixed(offset)))
}

/// `ts` in UTC.
pub fn utc(ts: Timestamp) -> Zoned {
    ts.to_zoned(TimeZone::fixed(Offset::UTC))
}

/// The civil date-time `dt` read in the fixed offset `offset_secs`.
fn fixed_civil(dt: civil::DateTime, offset_secs: i32) -> Result<Zoned, String> {
    let ts = Offset::from_seconds(offset_secs)
        .and_then(|o| o.to_timestamp(dt))
        .map_err(|_| format!("invalid timezone offset {offset_secs}"))?;
    fixed(ts, offset_secs)
}

/// The earliest instant the wall time `dt` names in `tz` (`None` in a gap the zone skips), as
/// chrono's `from_local_datetime(..).earliest()`.
pub fn earliest_local(tz: &TimeZone, dt: civil::DateTime) -> Option<Zoned> {
    let offset = match tz.to_ambiguous_timestamp(dt).offset() {
        AmbiguousOffset::Unambiguous { offset } => offset,
        AmbiguousOffset::Gap { .. } => return None,
        AmbiguousOffset::Fold { before, .. } => before,
    };
    offset
        .to_timestamp(dt)
        .ok()
        .map(|ts| ts.to_zoned(tz.clone()))
}

/// The offset in seconds `tz_name` has at `ts`; `UTC`, `Z` and `Local` are UTC.
pub fn resolve_tz_offset(tz_name: &str, ts: Timestamp) -> Result<i32, String> {
    if tz_name.eq_ignore_ascii_case("UTC")
        || tz_name == "Z"
        || tz_name.eq_ignore_ascii_case("Local")
    {
        return Ok(0);
    }
    let tz = zone(tz_name).map_err(|e| format!("failed to parse timezone location name: {e}"))?;
    Ok(tz.to_offset(ts).seconds())
}

pub fn apply_timezone(dt: Zoned, tz_name: Option<&str>) -> Result<Zoned, String> {
    let Some(tz_str) = tz_name else {
        return Ok(dt);
    };
    let offset = resolve_tz_offset(tz_str, dt.timestamp())?;
    fixed(dt.timestamp(), offset)
}

pub fn format_go_layout(dt: &Zoned, layout: &str) -> String {
    if layout == GO_RFC3339_NANO {
        return format_rfc3339_nano(dt);
    }
    let mut out = String::with_capacity(layout.len() + 8);
    let bytes = layout.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rem = &layout[i..];
        if rem.starts_with("2006") {
            out.push_str(&format!("{:04}", dt.year()));
            i += 4;
        } else if rem.starts_with("January") {
            out.push_str(month_long_name(dt.month()));
            i += 7;
        } else if rem.starts_with("Jan") {
            out.push_str(month_short_name(dt.month()));
            i += 3;
        } else if rem.starts_with("Monday") {
            out.push_str(weekday_long_name(dt.weekday()));
            i += 6;
        } else if rem.starts_with("Mon") {
            out.push_str(weekday_short_name(dt.weekday()));
            i += 3;
        } else if rem.starts_with("01") {
            out.push_str(&format!("{:02}", dt.month()));
            i += 2;
        } else if rem.starts_with("02") {
            out.push_str(&format!("{:02}", dt.day()));
            i += 2;
        } else if rem.starts_with("_2") {
            out.push_str(&format!("{:2}", dt.day()));
            i += 2;
        } else if rem.starts_with("15") {
            out.push_str(&format!("{:02}", dt.hour()));
            i += 2;
        } else if rem.starts_with("03") {
            let h = match dt.hour() % 12 {
                0 => 12,
                x => x,
            };
            out.push_str(&format!("{:02}", h));
            i += 2;
        } else if rem.starts_with("04") {
            out.push_str(&format!("{:02}", dt.minute()));
            i += 2;
        } else if rem.starts_with("05") {
            out.push_str(&format!("{:02}", dt.second()));
            i += 2;
        } else if rem.starts_with(".999999999")
            || rem.starts_with(".999999")
            || rem.starts_with(".999")
        {
            let dots = rem[1..].bytes().take_while(|b| *b == b'9').count();
            let nanos = dt.subsec_nanosecond() % 1_000_000_000;
            if nanos > 0 {
                let s = format!("{:09}", nanos);
                let trimmed = s[..dots.min(9)].trim_end_matches('0');
                if !trimmed.is_empty() {
                    out.push('.');
                    out.push_str(trimmed);
                }
            }
            i += 1 + dots;
        } else if rem.starts_with(".000000000")
            || rem.starts_with(".000000")
            || rem.starts_with(".000")
        {
            let zeros = rem[1..].bytes().take_while(|b| *b == b'0').count();
            let nanos = dt.subsec_nanosecond() % 1_000_000_000;
            let s = format!("{:09}", nanos);
            out.push('.');
            out.push_str(&s[..zeros.min(9)]);
            i += 1 + zeros;
        } else if rem.starts_with("Z07:00") {
            let secs = dt.offset().seconds();
            if secs == 0 {
                out.push('Z');
            } else {
                out.push_str(&format_offset_colon(secs));
            }
            i += 6;
        } else if rem.starts_with("Z0700") {
            let secs = dt.offset().seconds();
            if secs == 0 {
                out.push('Z');
            } else {
                out.push_str(&format_offset_nocolon(secs));
            }
            i += 5;
        } else if rem.starts_with("-07:00") {
            out.push_str(&format_offset_colon(dt.offset().seconds()));
            i += 6;
        } else if rem.starts_with("-0700") {
            out.push_str(&format_offset_nocolon(dt.offset().seconds()));
            i += 5;
        } else if rem.starts_with("Z07") || rem.starts_with("-07") {
            // Go's stdNumShortTZ (`-07`, an hour offset) and stdISO8601ShortTZ (`Z07`, `Z` at UTC).
            let secs = dt.offset().seconds();
            if secs == 0 && rem.starts_with('Z') {
                out.push('Z');
            } else {
                out.push_str(&format_offset_colon(secs)[..3]);
            }
            i += 3;
        } else if rem.starts_with("MST") {
            if dt.offset().seconds() == 0 {
                out.push_str("UTC");
            } else {
                out.push_str(&format_offset_colon(dt.offset().seconds()));
            }
            i += 3;
        } else if rem.starts_with("PM") {
            out.push_str(if dt.hour() >= 12 { "PM" } else { "AM" });
            i += 2;
        } else if rem.starts_with("pm") {
            out.push_str(if dt.hour() >= 12 { "pm" } else { "am" });
            i += 2;
        } else if rem.starts_with("06") {
            out.push_str(&format!("{:02}", dt.year() % 100));
            i += 2;
        } else if rem.starts_with('2') && !rem[1..].starts_with(|c: char| c.is_ascii_digit()) {
            out.push_str(&dt.day().to_string());
            i += 1;
        } else if rem.starts_with('1') && !rem[1..].starts_with(|c: char| c.is_ascii_digit()) {
            out.push_str(&dt.month().to_string());
            i += 1;
        } else {
            let ch = rem.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

pub fn format_rfc3339_nano(dt: &Zoned) -> String {
    let base = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    let nanos = dt.subsec_nanosecond() % 1_000_000_000;
    let frac = if nanos > 0 {
        let s = format!("{:09}", nanos);
        format!(".{}", s.trim_end_matches('0'))
    } else {
        String::new()
    };
    let secs = dt.offset().seconds();
    let tz = if secs == 0 {
        "Z".to_string()
    } else {
        format_offset_colon(secs)
    };
    format!("{base}{frac}{tz}")
}

/// Whole `unit_nanos` since the Unix epoch, rounded down as chrono's `timestamp()`,
/// `timestamp_millis()` and `timestamp_micros()` did (jiff's `as_*` truncate toward zero).
/// `None` when the count does not fit an i64 (only nanoseconds can overflow).
pub fn unix_floor(ts: Timestamp, unit_nanos: i128) -> Option<i64> {
    i64::try_from(ts.as_nanosecond().div_euclid(unit_nanos)).ok()
}

/// Rounds `dt` to a multiple of `span_nanos` of its wall time, a half up, as chrono's
/// `duration_round` on a `DateTime<FixedOffset>` did (errors in its words).
pub fn round_local(dt: &Zoned, span_nanos: i64) -> Result<Zoned, String> {
    if span_nanos <= 0 {
        return Err("duration exceeds num_nanoseconds limit".to_string());
    }
    let local = dt.timestamp().as_nanosecond() + i128::from(dt.offset().seconds()) * 1_000_000_000;
    let stamp =
        i64::try_from(local).map_err(|_| "timestamp exceeds num_nanoseconds limit".to_string())?;
    let delta_down = stamp % span_nanos;
    if delta_down == 0 {
        return Ok(dt.clone());
    }
    let (up, down) = if delta_down < 0 {
        (delta_down.abs(), span_nanos - delta_down.abs())
    } else {
        (span_nanos - delta_down, delta_down)
    };
    let shift = if up <= down { up } else { -down };
    dt.timestamp()
        .checked_add(SignedDuration::from_nanos(shift))
        .map(|ts| ts.to_zoned(dt.time_zone().clone()))
        .map_err(|e| e.to_string())
}

/// RFC 3339 with 0, 3, 6 or 9 fraction digits, as chrono's `to_rfc3339_opts(SecondsFormat::AutoSi,
/// use_z)` printed it: `Z` for a zero offset when `use_z`, else `+00:00` (chrono's `to_rfc3339()`).
pub fn format_rfc3339_auto_si(dt: &Zoned, use_z: bool) -> String {
    let digits = match dt.subsec_nanosecond() {
        0 => 0,
        n if n % 1_000_000 == 0 => 3,
        n if n % 1_000 == 0 => 6,
        _ => 9,
    };
    let printer = jiff::fmt::temporal::DateTimePrinter::new().precision(Some(digits));
    match dt.offset() {
        Offset::UTC if use_z => printer.timestamp_to_string(&dt.timestamp()),
        offset => printer.timestamp_with_offset_to_string(&dt.timestamp(), offset),
    }
}

fn format_offset_colon(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.abs();
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    format!("{sign}{h:02}:{m:02}")
}

fn format_offset_nocolon(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.abs();
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    format!("{sign}{h:02}{m:02}")
}

fn month_short_name(m: i8) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    }
}

fn month_long_name(m: i8) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "January",
    }
}

fn weekday_short_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

fn weekday_long_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

fn parse_month_name(s: &str) -> Option<(u32, usize)> {
    let names = [
        ("January", 1),
        ("February", 2),
        ("March", 3),
        ("April", 4),
        ("May", 5),
        ("June", 6),
        ("July", 7),
        ("August", 8),
        ("September", 9),
        ("October", 10),
        ("November", 11),
        ("December", 12),
        ("Jan", 1),
        ("Feb", 2),
        ("Mar", 3),
        ("Apr", 4),
        ("Jun", 6),
        ("Jul", 7),
        ("Aug", 8),
        ("Sep", 9),
        ("Oct", 10),
        ("Nov", 11),
        ("Dec", 12),
    ];
    for (name, num) in names {
        if starts_with_ignore_ascii_case(s, name) {
            return Some((num, name.len()));
        }
    }
    None
}

/// Parses a timestamp string according to a Go `time.Parse` layout.
pub fn parse_go_layout(input: &str, layout: &str) -> Result<Zoned, String> {
    let mut year: i32 = 0;
    let mut month: u32 = 1;
    let mut day: u32 = 1;
    let mut hour: u32 = 0;
    let mut minute: u32 = 0;
    let mut second: u32 = 0;
    let mut nanosecond: u32 = 0;
    let mut offset_secs: i32 = 0;
    let mut is_pm: Option<bool> = None;

    let mut li = 0usize;
    let mut si = 0usize;

    while li < layout.len() {
        let lrem = &layout[li..];
        let srem = &input[si..];

        if lrem.starts_with("2006") {
            let (v, n) = take_digits(srem, 4, 4)?;
            year = v as i32;
            li += 4;
            si += n;
        } else if lrem.starts_with("06") {
            let (v, n) = take_digits(srem, 2, 2)?;
            year = if v >= 69 {
                1900 + v as i32
            } else {
                2000 + v as i32
            };
            li += 2;
            si += n;
        } else if lrem.starts_with("January") {
            let (m, n) = parse_month_name(srem)
                .ok_or_else(|| format!("cannot parse '{input}' with layout '{layout}'"))?;
            month = m;
            li += 7;
            si += n;
        } else if lrem.starts_with("Jan") {
            let (m, n) = parse_month_name(srem)
                .ok_or_else(|| format!("cannot parse '{input}' with layout '{layout}'"))?;
            month = m;
            li += 3;
            si += n;
        } else if lrem.starts_with("Monday") || lrem.starts_with("Mon") {
            let skip_l = if lrem.starts_with("Monday") { 6 } else { 3 };
            let alpha_len = srem.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
            if alpha_len == 0 {
                return Err(format!("expected weekday in '{input}'"));
            }
            li += skip_l;
            si += alpha_len;
        } else if lrem.starts_with("01") {
            let (v, n) = take_digits(srem, 2, 2)?;
            month = v;
            li += 2;
            si += n;
        } else if lrem.starts_with("02") {
            let (v, n) = take_digits(srem, 2, 2)?;
            day = v;
            li += 2;
            si += n;
        } else if lrem.starts_with("_2") {
            let trimmed = srem.strip_prefix(' ').unwrap_or(srem);
            let space_consumed = srem.len() - trimmed.len();
            let (v, n) = take_digits(trimmed, 1, 2)?;
            day = v;
            li += 2;
            si += space_consumed + n;
        } else if lrem.starts_with("15") || lrem.starts_with("03") {
            let (v, n) = take_digits(srem, 2, 2)?;
            hour = v;
            li += 2;
            si += n;
        } else if lrem.starts_with("04") {
            let (v, n) = take_digits(srem, 2, 2)?;
            minute = v;
            li += 2;
            si += n;
        } else if let Some(after_05) = lrem.strip_prefix("05") {
            let (v, n) = take_digits(srem, 2, 2)?;
            second = v;
            li += 2;
            si += n;
            // In Go's time.Parse, fractional seconds after 05 are parsed even if not in layout when followed by .999...
            if !after_05.starts_with('.') && input[si..].starts_with('.') {
                let (ns, consumed) = parse_fractional_nanos(&input[si..])?;
                nanosecond = ns;
                si += consumed;
            }
        } else if lrem.starts_with(".999") || lrem.starts_with(".000") {
            let pat_ch = lrem.as_bytes()[1];
            let pat_len = 1 + lrem[1..].bytes().take_while(|b| *b == pat_ch).count();
            if srem.starts_with('.') {
                let (ns, consumed) = parse_fractional_nanos(srem)?;
                nanosecond = ns;
                si += consumed;
            } else if pat_ch == b'0' {
                return Err(format!("expected fractional seconds in '{input}'"));
            }
            li += pat_len;
        } else if lrem.starts_with("Z07:00") || lrem.starts_with("-07:00") {
            let pat_len = 6;
            let (ofs, consumed) = parse_tz_offset_token(srem, true)?;
            offset_secs = ofs;
            li += pat_len;
            si += consumed;
        } else if lrem.starts_with("Z0700") || lrem.starts_with("-0700") {
            let pat_len = 5;
            let (ofs, consumed) = parse_tz_offset_token(srem, false)?;
            offset_secs = ofs;
            li += pat_len;
            si += consumed;
        } else if lrem.starts_with("Z07") || lrem.starts_with("-07") {
            if lrem.starts_with('Z') && srem.starts_with('Z') {
                offset_secs = 0;
                si += 1;
            } else {
                let sign = match srem.as_bytes().first() {
                    Some(b'+') => 1,
                    Some(b'-') => -1,
                    _ => {
                        return Err(format!(
                        "parsing time {input:?} as {layout:?}: cannot parse {srem:?} as \"-07\""
                    ))
                    }
                };
                let (h, n) = take_digits(&srem[1..], 2, 2)?;
                offset_secs = sign * (h as i32) * 3600;
                si += 1 + n;
            }
            li += 3;
        } else if lrem.starts_with("MST") {
            let alpha_len = srem.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
            if alpha_len == 0 {
                return Err(format!("expected timezone name in '{input}'"));
            }
            li += 3;
            si += alpha_len;
        } else if lrem.starts_with("PM") || lrem.starts_with("pm") {
            if starts_with_ignore_ascii_case(srem, "pm") {
                is_pm = Some(true);
                si += 2;
            } else if starts_with_ignore_ascii_case(srem, "am") {
                is_pm = Some(false);
                si += 2;
            } else {
                return Err(format!("expected AM/PM in '{input}'"));
            }
            li += 2;
        } else if lrem.starts_with('2') && !lrem[1..].starts_with(|c: char| c.is_ascii_digit()) {
            let (v, n) = take_digits(srem, 1, 2)?;
            day = v;
            li += 1;
            si += n;
        } else if lrem.starts_with('1') && !lrem[1..].starts_with(|c: char| c.is_ascii_digit()) {
            let (v, n) = take_digits(srem, 1, 2)?;
            month = v;
            li += 1;
            si += n;
        } else {
            let lch = lrem.chars().next().unwrap();
            let sch = srem
                .chars()
                .next()
                .ok_or_else(|| format!("unexpected end of timestamp '{input}'"))?;
            if lch != sch {
                return Err(format!(
                    "parsing time {input:?} as {layout:?}: cannot parse {srem:?} as {lch:?}"
                ));
            }
            li += lch.len_utf8();
            si += sch.len_utf8();
        }
    }

    if si != input.len() {
        return Err(format!(
            "parsing time {input:?}: extra text {:?}",
            &input[si..]
        ));
    }

    if let Some(pm) = is_pm {
        if pm && hour < 12 {
            hour += 12;
        } else if !pm && hour == 12 {
            hour = 0;
        }
    }

    // take_digits reads at most 4 digits for a year and 2 for the rest: the casts are exact
    fixed(Timestamp::UNIX_EPOCH, offset_secs)?;
    let date = civil::Date::new(year as i16, month as i8, day as i8)
        .map_err(|_| format!("invalid date {year:04}-{month:02}-{day:02}"))?;
    let time = civil::Time::new(hour as i8, minute as i8, second as i8, nanosecond as i32)
        .map_err(|_| format!("invalid time {hour:02}:{minute:02}:{second:02}"))?;
    fixed_civil(date.to_datetime(time), offset_secs)
}

/// Parses a timestamp string using `strptime` specifiers, matching `itchyny/timefmt-go`
/// including day rollover (e.g. `31.02.2026` -> `2026-03-03` as noted in PART B traps!).
pub fn parse_strptime(input: &str, format: &str) -> Result<Zoned, String> {
    let mut year: i32 = 1970;
    let mut month: i32 = 1;
    let mut day: i32 = 1;
    let mut hour: u32 = 0;
    let mut minute: u32 = 0;
    let mut second: u32 = 0;
    let mut nanosecond: u32 = 0;
    let mut offset_secs: i32 = 0;
    let mut is_pm: Option<bool> = None;

    let mut fi = 0usize;
    let mut si = 0usize;
    let fbytes = format.as_bytes();

    while fi < fbytes.len() {
        if fbytes[fi] == b'%' {
            fi += 1;
            if fi >= fbytes.len() {
                return Err("trailing '%' in strptime format".to_string());
            }
            let spec = fbytes[fi] as char;
            fi += 1;
            let srem = &input[si..];
            match spec {
                'Y' => {
                    let (v, n) = take_digits(srem, 1, 4)?;
                    year = v as i32;
                    si += n;
                }
                'y' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    year = if v >= 69 {
                        1900 + v as i32
                    } else {
                        2000 + v as i32
                    };
                    si += n;
                }
                'm' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    if !(1..=12).contains(&v) {
                        return Err(format!("month {v} out of range"));
                    }
                    month = v as i32;
                    si += n;
                }
                'b' | 'B' | 'h' => {
                    let (m, n) = parse_month_name(srem)
                        .ok_or_else(|| format!("invalid month name in '{input}'"))?;
                    month = m as i32;
                    si += n;
                }
                'd' | 'e' => {
                    let trimmed = if spec == 'e' {
                        srem.strip_prefix(' ').unwrap_or(srem)
                    } else {
                        srem
                    };
                    let sp = srem.len() - trimmed.len();
                    let (v, n) = take_digits(trimmed, 1, 2)?;
                    if !(1..=31).contains(&v) {
                        return Err(format!("day {v} out of range"));
                    }
                    day = v as i32;
                    si += sp + n;
                }
                'H' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    if v > 23 {
                        return Err(format!("hour {v} out of range"));
                    }
                    hour = v;
                    si += n;
                }
                'I' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    if !(1..=12).contains(&v) {
                        return Err(format!("hour12 {v} out of range"));
                    }
                    hour = v;
                    si += n;
                }
                'M' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    if v > 59 {
                        return Err(format!("minute {v} out of range"));
                    }
                    minute = v;
                    si += n;
                }
                'S' => {
                    let (v, n) = take_digits(srem, 1, 2)?;
                    if v > 60 {
                        return Err(format!("second {v} out of range"));
                    }
                    second = v.min(59);
                    si += n;
                }
                'f' => {
                    let (v, n) = take_digits(srem, 1, 9)?;
                    let scale = 9u32.saturating_sub(n as u32);
                    nanosecond = v * 10u32.pow(scale);
                    si += n;
                }
                'p' | 'P' => {
                    if starts_with_ignore_ascii_case(srem, "pm") {
                        is_pm = Some(true);
                        si += 2;
                    } else if starts_with_ignore_ascii_case(srem, "am") {
                        is_pm = Some(false);
                        si += 2;
                    } else {
                        return Err(format!("expected AM/PM in '{input}'"));
                    }
                }
                'z' => {
                    let (ofs, consumed) = parse_tz_offset_token(srem, true)?;
                    offset_secs = ofs;
                    si += consumed;
                }
                'Z' => {
                    if srem.starts_with('Z') {
                        offset_secs = 0;
                        si += 1;
                    } else {
                        let alpha_len =
                            srem.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
                        si += alpha_len;
                    }
                }
                '%' => {
                    if !srem.starts_with('%') {
                        return Err("expected literal '%'".to_string());
                    }
                    si += 1;
                }
                other => {
                    return Err(format!("unsupported strptime directive '%{other}'"));
                }
            }
        } else {
            let fch = format[fi..].chars().next().unwrap();
            let sch = input[si..]
                .chars()
                .next()
                .ok_or_else(|| format!("unexpected end of input '{input}'"))?;
            if fch != sch {
                return Err(format!(
                    "mismatch in strptime: expected {fch:?}, got {sch:?}"
                ));
            }
            fi += fch.len_utf8();
            si += sch.len_utf8();
        }
    }

    if si != input.len() {
        return Err(format!("extra unparsed text in '{input}'"));
    }

    if let Some(pm) = is_pm {
        if pm && hour < 12 {
            hour += 12;
        } else if !pm && hour == 12 {
            hour = 0;
        }
    }

    // Match Go's `time.Date(year, month, day, ...)` rollover behavior!
    // In Go, `time.Date(2026, 2, 31, 0, 0, 0, 0, UTC)` adds `(day - 1)` days to the 1st of the month.
    // the fields are range-checked above (a year has at most 4 digits): the casts are exact
    let first_of_month = civil::Date::new(year as i16, month as i8, 1)
        .map_err(|_| format!("invalid year/month {year}-{month}"))?;
    let rolled_date = first_of_month
        .checked_add(SignedDuration::from_hours(i64::from(day - 1) * 24))
        .map_err(|_| format!("invalid year/month {year}-{month}"))?;
    let time = civil::Time::new(hour as i8, minute as i8, second as i8, nanosecond as i32)
        .map_err(|_| "invalid time".to_string())?;
    let dt = rolled_date.to_datetime(time);
    fixed_civil(dt, offset_secs).or_else(|_| fixed_civil(dt, 0))
}

/// Formats a timestamp using `strftime` specifiers (`itchyny/timefmt-go` compatible, including `%f` for 6-digit microseconds).
pub fn format_strftime(dt: &Zoned, format: &str) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let bytes = format.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() {
            let spec = bytes[i + 1] as char;
            i += 2;
            match spec {
                'Y' => out.push_str(&format!("{:04}", dt.year())),
                'y' => out.push_str(&format!("{:02}", dt.year() % 100)),
                'm' => out.push_str(&format!("{:02}", dt.month())),
                'b' | 'h' => out.push_str(month_short_name(dt.month())),
                'B' => out.push_str(month_long_name(dt.month())),
                'd' => out.push_str(&format!("{:02}", dt.day())),
                'e' => out.push_str(&format!("{:2}", dt.day())),
                'H' => out.push_str(&format!("{:02}", dt.hour())),
                'I' => {
                    let h = match dt.hour() % 12 {
                        0 => 12,
                        x => x,
                    };
                    out.push_str(&format!("{:02}", h));
                }
                'M' => out.push_str(&format!("{:02}", dt.minute())),
                'S' => out.push_str(&format!("{:02}", dt.second())),
                'f' => {
                    let micros = (dt.subsec_nanosecond() % 1_000_000_000) / 1_000;
                    out.push_str(&format!("{:06}", micros));
                }
                'a' => out.push_str(weekday_short_name(dt.weekday())),
                'A' => out.push_str(weekday_long_name(dt.weekday())),
                'p' => out.push_str(if dt.hour() >= 12 { "PM" } else { "AM" }),
                'P' => out.push_str(if dt.hour() >= 12 { "pm" } else { "am" }),
                'z' => out.push_str(&format_offset_nocolon(dt.offset().seconds())),
                'Z' => {
                    if dt.offset().seconds() == 0 {
                        out.push_str("UTC");
                    } else {
                        out.push_str(&format_offset_colon(dt.offset().seconds()));
                    }
                }
                's' => out.push_str(&dt.timestamp().as_second().to_string()),
                'F' => out.push_str(&format!(
                    "{:04}-{:02}-{:02}",
                    dt.year(),
                    dt.month(),
                    dt.day()
                )),
                'T' => out.push_str(&format!(
                    "{:02}:{:02}:{:02}",
                    dt.hour(),
                    dt.minute(),
                    dt.second()
                )),
                '%' => out.push('%'),
                other => {
                    out.push('%');
                    out.push(other);
                }
            }
        } else {
            let ch = format[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Compares bytes, so a prefix that ends inside a multibyte character is a mismatch, not a panic.
fn starts_with_ignore_ascii_case(s: &str, prefix: &str) -> bool {
    s.as_bytes()
        .get(..prefix.len())
        .is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn take_digits(s: &str, min_len: usize, max_len: usize) -> Result<(u32, usize), String> {
    let count = s
        .bytes()
        .take(max_len)
        .take_while(|b| b.is_ascii_digit())
        .count();
    if count < min_len {
        return Err(format!("expected {min_len}..{max_len} digits in '{s}'"));
    }
    let val = s[..count]
        .parse::<u32>()
        .map_err(|e| format!("invalid digits: {e}"))?;
    Ok((val, count))
}

fn parse_fractional_nanos(s: &str) -> Result<(u32, usize), String> {
    if !s.starts_with('.') {
        return Ok((0, 0));
    }
    let digits = s[1..].bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return Err("expected digits after '.'".to_string());
    }
    let take = digits.min(9);
    let raw = s[1..1 + take]
        .parse::<u32>()
        .map_err(|e| format!("invalid fractional seconds: {e}"))?;
    let scale = 9u32.saturating_sub(take as u32);
    Ok((raw * 10u32.pow(scale), 1 + digits))
}

fn parse_tz_offset_token(s: &str, allow_colon: bool) -> Result<(i32, usize), String> {
    if s.starts_with('Z') || s.starts_with('z') {
        return Ok((0, 1));
    }
    if s.is_empty() {
        return Err("expected timezone offset".to_string());
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err(format!("expected '+' or '-' in timezone offset '{s}'")),
    };
    let rest = &s[1..];
    let (h, hn) = take_digits(rest, 2, 2)?;
    let after_h = &rest[hn..];
    if allow_colon && after_h.starts_with(':') {
        let (m, mn) = take_digits(&after_h[1..], 2, 2)?;
        Ok((
            sign * ((h as i32) * 3600 + (m as i32) * 60),
            1 + hn + 1 + mn,
        ))
    } else if after_h.len() >= 2 && after_h.as_bytes()[0].is_ascii_digit() {
        let (m, mn) = take_digits(after_h, 2, 2)?;
        Ok((sign * ((h as i32) * 3600 + (m as i32) * 60), 1 + hn + mn))
    } else {
        Ok((sign * (h as i32) * 3600, 1 + hn))
    }
}

/// Parses a Go duration string (`"300ms"`, `"-1.5h"`, `"2h45m"`, `"50us"`, `"10s"`) into nanoseconds (`i64`).
pub fn parse_go_duration_nanos(input: &str) -> Result<i64, String> {
    let mut s = input.trim();
    if s.is_empty() {
        return Err("invalid duration: empty".to_string());
    }
    let neg = if let Some(r) = s.strip_prefix('-') {
        s = r;
        true
    } else if let Some(r) = s.strip_prefix('+') {
        s = r;
        false
    } else {
        false
    };
    if s == "0" {
        return Ok(0);
    }
    let mut total_nanos: f64 = 0.0;
    while !s.is_empty() {
        let num_len = s
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        if num_len == 0 {
            return Err(format!("invalid duration: {input}"));
        }
        let val: f64 = s[..num_len]
            .parse()
            .map_err(|_| format!("invalid number in duration: {input}"))?;
        s = &s[num_len..];
        let unit_end = s
            .char_indices()
            .take_while(|(_, c)| !c.is_ascii_digit() && *c != '.')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .ok_or_else(|| format!("missing unit in duration: {input}"))?;
        let unit = &s[..unit_end];
        s = &s[unit_end..];
        let mult: f64 = match unit {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1_000.0,
            "ms" => 1_000_000.0,
            "s" => 1_000_000_000.0,
            "m" => 60.0 * 1_000_000_000.0,
            "h" => 3600.0 * 1_000_000_000.0,
            _ => return Err(format!("unknown unit {unit:?} in duration {input:?}")),
        };
        total_nanos += val * mult;
    }
    let res = total_nanos.round() as i64;
    Ok(if neg { -res } else { res })
}

/// Parses an ISO 8601 duration (`P3Y6M4DT12H30M5S`, `PT2H`, `PT2.5S`, `P1W`, `P1D`)
/// into `(years, months, weeks, days, nanos)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Iso8601Period {
    pub years: i32,
    pub months: i32,
    pub weeks: i64,
    pub days: i64,
    pub nanos: i64,
}

impl Iso8601Period {
    pub fn parse(input: &str) -> Result<Self, String> {
        let s = input.trim();
        let Some(mut rest) = s.strip_prefix('P') else {
            return Err(format!(
                "invalid ISO-8601 duration '{input}': must start with 'P'"
            ));
        };
        if rest.is_empty() {
            return Err(format!(
                "invalid ISO-8601 duration '{input}': empty after 'P'"
            ));
        }
        let mut period = Self::default();
        let mut in_time = false;
        while !rest.is_empty() {
            if let Some(after_t) = rest.strip_prefix('T') {
                in_time = true;
                rest = after_t;
                if rest.is_empty() {
                    return Err(format!(
                        "invalid ISO-8601 duration '{input}': empty after 'T'"
                    ));
                }
                continue;
            }
            let num_len = rest
                .bytes()
                .take_while(|b| b.is_ascii_digit() || *b == b'.' || *b == b'-')
                .count();
            if num_len == 0 || num_len >= rest.len() {
                return Err(format!("invalid ISO-8601 duration '{input}'"));
            }
            let num_str = &rest[..num_len];
            let unit = rest.as_bytes()[num_len] as char;
            rest = &rest[num_len + 1..];
            let val: f64 = num_str
                .parse()
                .map_err(|_| format!("invalid number in ISO-8601 duration '{input}'"))?;
            if !in_time {
                match unit {
                    'Y' => period.years += val as i32,
                    'M' => period.months += val as i32,
                    'W' => period.weeks += val as i64,
                    'D' => period.days += val as i64,
                    _ => {
                        return Err(format!(
                            "invalid date unit '{unit}' in ISO-8601 duration '{input}'"
                        ))
                    }
                }
            } else {
                match unit {
                    'H' => period.nanos += (val * 3600.0 * 1e9).round() as i64,
                    'M' => period.nanos += (val * 60.0 * 1e9).round() as i64,
                    'S' => {
                        // Truncate beyond 1 decimal place as documented in Bento's parse_duration_iso8601
                        let truncated = (val * 10.0).trunc() / 10.0;
                        period.nanos += (truncated * 1e9).round() as i64;
                    }
                    _ => {
                        return Err(format!(
                            "invalid time unit '{unit}' in ISO-8601 duration '{input}'"
                        ))
                    }
                }
            }
        }
        Ok(period)
    }

    pub fn negate(self) -> Self {
        Self {
            years: -self.years,
            months: -self.months,
            weeks: -self.weeks,
            days: -self.days,
            nanos: -self.nanos,
        }
    }

    /// Approximate duration in nanoseconds matching `rickb777/period.DurationApprox`:
    /// 1 year = 365.2425 days (31556952s), 1 month = 30.436875 days (2629746s), 1 day = 86400s.
    pub fn approx_nanos(&self) -> i64 {
        let year_nanos = (self.years as i64) * 31_556_952_000_000_000;
        let month_nanos = (self.months as i64) * 2_629_746_000_000_000;
        let week_nanos = self.weeks * 7 * 86_400_000_000_000;
        let day_nanos = self.days * 86_400_000_000_000;
        year_nanos + month_nanos + week_nanos + day_nanos + self.nanos
    }

    pub fn add_to(&self, dt: &Zoned) -> Zoned {
        let mut total_months = (i32::from(dt.year()) * 12 + (i32::from(dt.month()) - 1))
            + self.years * 12
            + self.months;
        let new_year = total_months.div_euclid(12);
        total_months = total_months.rem_euclid(12);
        let new_month = (total_months + 1) as u32;
        let max_day = days_in_month(new_year, new_month);
        let new_day = (dt.day() as u32).min(max_day);
        let base_date = i16::try_from(new_year)
            .ok()
            .and_then(|y| civil::Date::new(y, new_month as i8, new_day as i8).ok())
            .unwrap_or_else(|| dt.date());
        let days = self.weeks.saturating_mul(7).saturating_add(self.days);
        base_date
            .to_datetime(dt.time())
            .checked_add(SignedDuration::from_secs(days.saturating_mul(86_400)))
            .and_then(|t| t.checked_add(SignedDuration::from_nanos(self.nanos)))
            .ok()
            .and_then(|t| fixed_civil(t, dt.offset().seconds()).ok())
            .unwrap_or_else(|| dt.clone())
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    #[test]
    fn zone_offsets_change_at_the_dst_instant_in_bratislava_and_helsinki() {
        let utc = |s: &str| jc_time::parse_rfc3339(s).ok_or_else(|| s.to_string());
        for (tz, winter, summer) in [
            ("Europe/Bratislava", 3600, 7200),
            ("Europe/Helsinki", 7200, 10800),
        ] {
            for (at, want) in [
                ("2026-03-29T00:59:59Z", winter),
                ("2026-03-29T01:00:00Z", summer),
                ("2026-10-25T00:59:59Z", summer),
                ("2026-10-25T01:00:00Z", winter),
            ] {
                let got = utc(at).and_then(|t| resolve_tz_offset(tz, t));
                assert_eq!(got, Ok(want), "{tz} at {at}");
            }
        }
        assert!(resolve_tz_offset("Nowhere/Town", Timestamp::now()).is_err());
        assert!(
            zone("europe/bratislava").is_err(),
            "names are case-sensitive"
        );
        assert_eq!(
            zone("US/Eastern").map(|z| z.iana_name().map(str::to_owned)),
            Ok(Some("US/Eastern".to_owned()))
        );
    }

    #[test]
    fn local_wall_times_in_a_dst_gap_and_fold() -> Result<(), String> {
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
            let tz = zone(name)?;
            let civil = |s: &str| s.parse::<civil::DateTime>().map_err(|e| e.to_string());
            assert_eq!(earliest_local(&tz, civil(gap)?), None, "{name} gap");
            let early = earliest_local(&tz, civil(fold)?).ok_or("fold")?;
            assert_eq!(
                early.offset().seconds(),
                summer,
                "{name} fold takes the earlier"
            );
            let jan = earliest_local(&tz, civil("2026-01-15T12:00:00")?).ok_or("jan")?;
            assert_eq!(jan.offset().seconds(), winter, "{name} January");
        }
        Ok(())
    }
}
