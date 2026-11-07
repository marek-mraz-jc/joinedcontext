use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

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

pub fn resolve_tz_offset(tz_name: &str, utc_dt: DateTime<Utc>) -> Result<FixedOffset, String> {
    if tz_name.eq_ignore_ascii_case("UTC") || tz_name == "Z" {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }
    if tz_name.eq_ignore_ascii_case("Local") {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }
    let tz: Tz = tz_name.parse().map_err(|_| {
        format!("failed to parse timezone location name: unknown time zone {tz_name}")
    })?;
    use chrono::Offset;
    let zoned = utc_dt.with_timezone(&tz);
    Ok(zoned.offset().fix())
}

pub fn apply_timezone(
    dt: DateTime<FixedOffset>,
    tz_name: Option<&str>,
) -> Result<DateTime<FixedOffset>, String> {
    let Some(tz_str) = tz_name else {
        return Ok(dt);
    };
    let offset = resolve_tz_offset(tz_str, dt.with_timezone(&Utc))?;
    Ok(dt.with_timezone(&offset))
}

pub fn format_go_layout(dt: DateTime<FixedOffset>, layout: &str) -> String {
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
            let nanos = dt.nanosecond() % 1_000_000_000;
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
            let nanos = dt.nanosecond() % 1_000_000_000;
            let s = format!("{:09}", nanos);
            out.push('.');
            out.push_str(&s[..zeros.min(9)]);
            i += 1 + zeros;
        } else if rem.starts_with("Z07:00") {
            let secs = dt.offset().local_minus_utc();
            if secs == 0 {
                out.push('Z');
            } else {
                out.push_str(&format_offset_colon(secs));
            }
            i += 6;
        } else if rem.starts_with("Z0700") {
            let secs = dt.offset().local_minus_utc();
            if secs == 0 {
                out.push('Z');
            } else {
                out.push_str(&format_offset_nocolon(secs));
            }
            i += 5;
        } else if rem.starts_with("-07:00") {
            out.push_str(&format_offset_colon(dt.offset().local_minus_utc()));
            i += 6;
        } else if rem.starts_with("-0700") {
            out.push_str(&format_offset_nocolon(dt.offset().local_minus_utc()));
            i += 5;
        } else if rem.starts_with("Z07") || rem.starts_with("-07") {
            // Go's stdNumShortTZ (`-07`, an hour offset) and stdISO8601ShortTZ (`Z07`, `Z` at UTC).
            let secs = dt.offset().local_minus_utc();
            if secs == 0 && rem.starts_with('Z') {
                out.push('Z');
            } else {
                out.push_str(&format_offset_colon(secs)[..3]);
            }
            i += 3;
        } else if rem.starts_with("MST") {
            if dt.offset().local_minus_utc() == 0 {
                out.push_str("UTC");
            } else {
                out.push_str(&format_offset_colon(dt.offset().local_minus_utc()));
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

pub fn format_rfc3339_nano(dt: DateTime<FixedOffset>) -> String {
    let base = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    let nanos = dt.nanosecond() % 1_000_000_000;
    let frac = if nanos > 0 {
        let s = format!("{:09}", nanos);
        format!(".{}", s.trim_end_matches('0'))
    } else {
        String::new()
    };
    let secs = dt.offset().local_minus_utc();
    let tz = if secs == 0 {
        "Z".to_string()
    } else {
        format_offset_colon(secs)
    };
    format!("{base}{frac}{tz}")
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

fn month_short_name(m: u32) -> &'static str {
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

fn month_long_name(m: u32) -> &'static str {
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

fn weekday_short_name(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

fn weekday_long_name(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
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
        if s.len() >= name.len() && s[..name.len()].eq_ignore_ascii_case(name) {
            return Some((num, name.len()));
        }
    }
    None
}

/// Parses a timestamp string according to a Go `time.Parse` layout.
pub fn parse_go_layout(input: &str, layout: &str) -> Result<DateTime<FixedOffset>, String> {
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
            if srem.len() >= 2 && srem[..2].eq_ignore_ascii_case("pm") {
                is_pm = Some(true);
                si += 2;
            } else if srem.len() >= 2 && srem[..2].eq_ignore_ascii_case("am") {
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

    let offset = FixedOffset::east_opt(offset_secs)
        .ok_or_else(|| format!("invalid timezone offset {offset_secs}"))?;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| format!("invalid date {year:04}-{month:02}-{day:02}"))?;
    let time = chrono::NaiveTime::from_hms_nano_opt(hour, minute, second, nanosecond)
        .ok_or_else(|| format!("invalid time {hour:02}:{minute:02}:{second:02}"))?;
    offset
        .from_local_datetime(&NaiveDateTime::new(date, time))
        .single()
        .ok_or_else(|| "ambiguous local datetime".to_string())
}

/// Parses a timestamp string using `strptime` specifiers, matching `itchyny/timefmt-go`
/// including day rollover (e.g. `31.02.2026` -> `2026-03-03` as noted in PART B traps!).
pub fn parse_strptime(input: &str, format: &str) -> Result<DateTime<FixedOffset>, String> {
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
                    if srem.len() >= 2 && srem[..2].eq_ignore_ascii_case("pm") {
                        is_pm = Some(true);
                        si += 2;
                    } else if srem.len() >= 2 && srem[..2].eq_ignore_ascii_case("am") {
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
    let first_of_month = chrono::NaiveDate::from_ymd_opt(year, month as u32, 1)
        .ok_or_else(|| format!("invalid year/month {year}-{month}"))?;
    let rolled_date = first_of_month + Duration::days((day - 1) as i64);
    let time = chrono::NaiveTime::from_hms_nano_opt(hour, minute, second, nanosecond)
        .ok_or_else(|| "invalid time".to_string())?;
    let offset =
        FixedOffset::east_opt(offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    offset
        .from_local_datetime(&NaiveDateTime::new(rolled_date, time))
        .single()
        .ok_or_else(|| "ambiguous datetime".to_string())
}

/// Formats a timestamp using `strftime` specifiers (`itchyny/timefmt-go` compatible, including `%f` for 6-digit microseconds).
pub fn format_strftime(dt: DateTime<FixedOffset>, format: &str) -> String {
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
                    let micros = (dt.nanosecond() % 1_000_000_000) / 1_000;
                    out.push_str(&format!("{:06}", micros));
                }
                'a' => out.push_str(weekday_short_name(dt.weekday())),
                'A' => out.push_str(weekday_long_name(dt.weekday())),
                'p' => out.push_str(if dt.hour() >= 12 { "PM" } else { "AM" }),
                'P' => out.push_str(if dt.hour() >= 12 { "pm" } else { "am" }),
                'z' => out.push_str(&format_offset_nocolon(dt.offset().local_minus_utc())),
                'Z' => {
                    if dt.offset().local_minus_utc() == 0 {
                        out.push_str("UTC");
                    } else {
                        out.push_str(&format_offset_colon(dt.offset().local_minus_utc()));
                    }
                }
                's' => out.push_str(&dt.timestamp().to_string()),
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

    pub fn add_to(&self, dt: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
        let mut total_months =
            (dt.year() * 12 + (dt.month() as i32 - 1)) + self.years * 12 + self.months;
        let new_year = total_months.div_euclid(12);
        total_months = total_months.rem_euclid(12);
        let new_month = (total_months + 1) as u32;
        let max_day = days_in_month(new_year, new_month);
        let new_day = dt.day().min(max_day);
        let base_date = chrono::NaiveDate::from_ymd_opt(new_year, new_month, new_day)
            .unwrap_or_else(|| dt.date_naive());
        let naive = NaiveDateTime::new(base_date, dt.time())
            + Duration::days(self.weeks * 7 + self.days)
            + Duration::nanoseconds(self.nanos);
        dt.offset()
            .from_local_datetime(&naive)
            .single()
            .unwrap_or(dt)
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
