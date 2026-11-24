//! Go's `fmt.Sprintf`, which Bento's `format` method calls with the mapping's values as Go sees
//! them: int64, float64, string, bool, `json.Number`, `time.Time`, `[]interface {}`,
//! `map[string]interface {}` and nil. Ported from `src/fmt/print.go` and `format.go` (flags, width,
//! precision, `*`, bad verbs, missing and extra operands) with float text from `strconv/ftoa.go`.

use crate::value::Value;

#[derive(Default, Clone, Copy)]
struct Flags {
    plus: bool,
    minus: bool,
    sharp: bool,
    space: bool,
    zero: bool,
    wid: Option<usize>,
    prec: Option<usize>,
}

/// Formats `args` by `format` as Go's `fmt.Sprintf` does.
pub fn sprintf(format: &str, args: &[Value]) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let bytes = format.as_bytes();
    let mut i = 0;
    let mut arg = 0;
    let mut reordered = false;
    // Ends at the `i >= bytes.len()` break below, after the last literal run.
    loop {
        let start = i;
        while i < bytes.len() && bytes[i] != b'%' {
            i += 1;
        }
        out.push_str(&format[start..i]);
        if i >= bytes.len() {
            break;
        }
        i += 1;
        let mut f = Flags::default();
        while i < bytes.len() {
            match bytes[i] {
                b'#' => f.sharp = true,
                b'0' => f.zero = true,
                b'+' => f.plus = true,
                b'-' => f.minus = true,
                b' ' => f.space = true,
                _ => break,
            }
            i += 1;
        }
        let mut good = arg_number(bytes, &mut i, &mut arg, args.len(), &mut reordered);
        if i < bytes.len() && bytes[i] == b'*' {
            i += 1;
            match args.get(arg).and_then(star) {
                Some(w) => {
                    if w.is_negative() {
                        f.minus = true;
                        f.zero = false;
                    }
                    f.wid = Some(w.unsigned_abs() as usize);
                }
                None => out.push_str("%!(BADWIDTH)"),
            }
            arg += 1;
        } else {
            f.wid = Some(number(bytes, &mut i));
        }
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            good &= arg_number(bytes, &mut i, &mut arg, args.len(), &mut reordered);
            if i < bytes.len() && bytes[i] == b'*' {
                i += 1;
                match args.get(arg).and_then(star) {
                    Some(p) if p >= 0 => f.prec = Some(p as usize),
                    Some(_) => {}
                    None => out.push_str("%!(BADPREC)"),
                }
                arg += 1;
            } else {
                f.prec = Some(number(bytes, &mut i));
            }
        }
        if f.minus {
            f.zero = false;
        }
        good &= arg_number(bytes, &mut i, &mut arg, args.len(), &mut reordered);
        let Some(verb) = format[i..].chars().next() else {
            out.push_str("%!(NOVERB)");
            break;
        };
        i += verb.len_utf8();
        if verb == '%' {
            out.push('%');
            continue;
        }
        if !good {
            out.push_str(&format!("%!{verb}(BADINDEX)"));
            continue;
        }
        let Some(v) = args.get(arg) else {
            out.push_str(&format!("%!{verb}(MISSING)"));
            continue;
        };
        arg += 1;
        print_arg(&mut out, v, verb, f);
    }
    if !reordered && arg < args.len() {
        out.push_str("%!(EXTRA ");
        for (n, v) in args[arg..].iter().enumerate() {
            if n > 0 {
                out.push_str(", ");
            }
            if is_nil(v) {
                out.push_str("<nil>");
            } else {
                out.push_str(type_name(v));
                out.push('=');
                print_arg(&mut out, v, 'v', Flags::default());
            }
        }
        out.push(')');
    }
    out
}

/// Go's argNumber: an explicit `[n]` moves to argument n (1-based); false on a bad index.
fn arg_number(
    b: &[u8],
    i: &mut usize,
    arg: &mut usize,
    nargs: usize,
    reordered: &mut bool,
) -> bool {
    if b.get(*i) != Some(&b'[') {
        return true;
    }
    *reordered = true;
    let Some(close) = b[*i..].iter().position(|c| *c == b']') else {
        *i += 1;
        return false;
    };
    let digits = &b[*i + 1..*i + close];
    *i += close + 1;
    match std::str::from_utf8(digits)
        .ok()
        .and_then(|d| d.parse::<usize>().ok())
    {
        Some(n) if (1..=nargs).contains(&n) && digits.iter().all(u8::is_ascii_digit) => {
            *arg = n - 1;
            true
        }
        _ => false,
    }
}

/// The decimal at `i`, 0 when there is none: a missing width or precision acts as 0.
fn number(b: &[u8], i: &mut usize) -> usize {
    let mut n: usize = 0;
    while *i < b.len() && b[*i].is_ascii_digit() {
        n = n
            .saturating_mul(10)
            .saturating_add(usize::from(b[*i] - b'0'));
        *i += 1;
    }
    n.min(1_000_000)
}

fn star(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) if i.unsigned_abs() <= 1_000_000 => Some(*i),
        Value::Uint(u) if *u <= 1_000_000 => Some(*u as i64),
        _ => None,
    }
}

fn is_nil(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Nothing | Value::Delete)
}

fn type_name(v: &Value) -> &'static str {
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

fn print_arg(out: &mut String, v: &Value, verb: char, f: Flags) {
    if verb == 'T' {
        pad(out, type_name(v), f);
        return;
    }
    print_value(out, v, verb, f, 0);
}

fn bad_verb(out: &mut String, v: &Value, verb: char) {
    out.push_str("%!");
    out.push(verb);
    out.push('(');
    if is_nil(v) {
        out.push_str("<nil>");
    } else {
        out.push_str(type_name(v));
        out.push('=');
        print_value(out, v, 'v', Flags::default(), 0);
    }
    out.push(')');
}

fn print_value(out: &mut String, v: &Value, verb: char, f: Flags, depth: usize) {
    match v {
        Value::Null | Value::Nothing | Value::Delete => {
            if verb == 'v' || depth > 0 {
                pad(out, "<nil>", f);
            } else {
                bad_verb(out, v, verb);
            }
        }
        Value::Bool(b) => match verb {
            't' | 'v' => pad(out, if *b { "true" } else { "false" }, f),
            _ => bad_verb(out, v, verb),
        },
        Value::Int(i) => fmt_integer(out, v, i.unsigned_abs(), *i < 0, verb, f),
        Value::Uint(u) => fmt_integer(out, v, *u, false, verb, f),
        Value::Float(x) => fmt_float(out, v, *x, verb, f),
        // json.Number and time.Time are Stringers: v, s, x, X and q print their String().
        Value::Number(t) => fmt_string(out, v, t, verb, f),
        Value::Timestamp(_) => fmt_string(out, v, &crate::value::format_go_v(v), verb, f),
        Value::String(s) => fmt_string(out, v, s, verb, f),
        Value::Bytes(b) => match verb {
            's' => pad(out, &truncate(&String::from_utf8_lossy(b), f), f),
            'x' | 'X' => fmt_hex_bytes(out, b, verb == 'X', f),
            'q' => fmt_q(out, &String::from_utf8_lossy(b), f),
            _ => {
                out.push('[');
                for (n, c) in b.iter().enumerate() {
                    if n > 0 {
                        out.push(' ');
                    }
                    fmt_integer(out, v, u64::from(*c), false, verb, f);
                }
                out.push(']');
            }
        },
        Value::Array(items) => {
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(' ');
                }
                print_value(out, item, verb, f, depth + 1);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("map[");
            for (n, (k, item)) in map.iter().enumerate() {
                if n > 0 {
                    out.push(' ');
                }
                fmt_string(out, &Value::String(k.clone()), k, verb, f);
                out.push(':');
                print_value(out, item, verb, f, depth + 1);
            }
            out.push(']');
        }
    }
}

/// Go's `fmt.pad`: width counts runes; zeros only on the left.
fn pad(out: &mut String, s: &str, f: Flags) {
    let fill = f.wid.unwrap_or(0).saturating_sub(s.chars().count());
    let c = if f.zero { '0' } else { ' ' };
    if f.minus {
        out.push_str(s);
        out.extend(std::iter::repeat_n(' ', fill));
    } else {
        out.extend(std::iter::repeat_n(c, fill));
        out.push_str(s);
    }
}

fn truncate(s: &str, f: Flags) -> String {
    match f.prec {
        Some(p) => s.chars().take(p).collect(),
        None => s.to_string(),
    }
}

fn fmt_string(out: &mut String, v: &Value, s: &str, verb: char, f: Flags) {
    match verb {
        'v' | 's' => pad(out, &truncate(s, f), f),
        'x' | 'X' => fmt_hex_bytes(out, s.as_bytes(), verb == 'X', f),
        'q' => fmt_q(out, s, f),
        _ => bad_verb(out, v, verb),
    }
}

/// Go's `fmtSbx`.
fn fmt_hex_bytes(out: &mut String, b: &[u8], upper: bool, f: Flags) {
    let digits: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let x = if upper { 'X' } else { 'x' };
    let length = f.prec.map_or(b.len(), |p| p.min(b.len()));
    if length == 0 {
        out.extend(std::iter::repeat_n(' ', f.wid.unwrap_or(0)));
        return;
    }
    let mut width = 2 * length;
    if f.space {
        if f.sharp {
            width *= 2;
        }
        width += length - 1;
    } else if f.sharp {
        width += 2;
    }
    let fill = f.wid.unwrap_or(0).saturating_sub(width);
    let c = if f.zero { '0' } else { ' ' };
    if !f.minus {
        out.extend(std::iter::repeat_n(c, fill));
    }
    if f.sharp {
        out.push('0');
        out.push(x);
    }
    for (i, byte) in b[..length].iter().enumerate() {
        if f.space && i > 0 {
            out.push(' ');
            if f.sharp {
                out.push('0');
                out.push(x);
            }
        }
        out.push(char::from(digits[usize::from(byte >> 4)]));
        out.push(char::from(digits[usize::from(byte & 0xF)]));
    }
    if f.minus {
        out.extend(std::iter::repeat_n(' ', fill));
    }
}

/// Go's `fmtQ`: `strconv.Quote` (`%+q` ASCII only, `%#q` a raw string where it can be one).
fn fmt_q(out: &mut String, s: &str, f: Flags) {
    let s = truncate(s, f);
    if f.sharp
        && s.chars()
            .all(|c| c == '\t' || (c != '`' && c != '\u{FEFF}' && !c.is_control()))
    {
        pad(out, &format!("`{s}`"), f);
        return;
    }
    pad(out, &quote(&s, '"', f.plus), f);
}

fn quote(s: &str, q: char, ascii: bool) -> String {
    let mut r = String::with_capacity(s.len() + 2);
    r.push(q);
    for c in s.chars() {
        match c {
            _ if c == q || c == '\\' => {
                r.push('\\');
                r.push(c);
            }
            '\u{7}' => r.push_str("\\a"),
            '\u{8}' => r.push_str("\\b"),
            '\u{c}' => r.push_str("\\f"),
            '\n' => r.push_str("\\n"),
            '\r' => r.push_str("\\r"),
            '\t' => r.push_str("\\t"),
            '\u{b}' => r.push_str("\\v"),
            _ if c.is_ascii() && !c.is_ascii_control() => r.push(c),
            _ if !ascii && !c.is_ascii() && is_go_print(c) => r.push(c),
            _ if (c as u32) < 0x80 => r.push_str(&format!("\\x{:02x}", c as u32)),
            _ if (c as u32) < 0x10000 => r.push_str(&format!("\\u{:04x}", c as u32)),
            _ => r.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    r.push(q);
    r
}

/// An approximation of Go's `strconv.IsPrint` beyond ASCII: letters, marks, numbers, punctuation
/// and symbols print; controls, format characters, separators other than U+0020 and unassigned
/// code points do not.
fn is_go_print(c: char) -> bool {
    !(c.is_control()
        || c.is_whitespace()
        || (c as u32 & 0xFFFE) == 0xFFFE
        || matches!(c as u32, 0xF0000..=0x10FFFF | 0xAD | 0x600..=0x605 | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE000..=0xF8FF | 0xFFFE | 0xFFFF))
}

fn fmt_integer(out: &mut String, v: &Value, u: u64, negative: bool, verb: char, f: Flags) {
    let (base, upper) = match verb {
        'v' | 'd' => (10, false),
        'b' => (2, false),
        'o' | 'O' => (8, false),
        'x' => (16, false),
        'X' => (16, true),
        'c' => {
            let c = if negative {
                None
            } else {
                u32::try_from(u).ok().and_then(char::from_u32)
            };
            let mut buf = [0u8; 4];
            pad(out, c.unwrap_or('\u{FFFD}').encode_utf8(&mut buf), f);
            return;
        }
        'q' => {
            let c = if negative {
                None
            } else {
                u32::try_from(u).ok().and_then(char::from_u32)
            };
            let c = c.unwrap_or('\u{FFFD}');
            if f.sharp && c != '\'' && is_go_print(c) {
                pad(out, &format!("'{c}'"), f);
            } else {
                pad(out, &quote(&c.to_string(), '\'', f.plus), f);
            }
            return;
        }
        'U' => {
            // Go prints a negative rune as its uint64 bit pattern.
            let u = if negative { u.wrapping_neg() } else { u };
            let mut s = format!("U+{:0width$X}", u, width = f.prec.map_or(4, |p| p.max(4)));
            if f.sharp {
                if let Some(c) = u32::try_from(u)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|c| is_go_print(*c) || *c == ' ')
                {
                    s.push_str(&format!(" '{c}'"));
                }
            }
            pad(out, &s, Flags { zero: false, ..f });
            return;
        }
        _ => {
            bad_verb(out, v, verb);
            return;
        }
    };
    if f.prec == Some(0) && u == 0 {
        out.extend(std::iter::repeat_n(' ', f.wid.unwrap_or(0)));
        return;
    }
    let mut digits = match base {
        2 => format!("{u:b}"),
        8 => format!("{u:o}"),
        16 if upper => format!("{u:X}"),
        16 => format!("{u:x}"),
        _ => u.to_string(),
    };
    let min = match (f.prec, f.zero, f.wid) {
        (Some(p), _, _) => p,
        // sprintf clears zero under minus, as Go does.
        (None, true, Some(w)) => w.saturating_sub(usize::from(negative || f.plus || f.space)),
        _ => 0,
    };
    digits.insert_str(0, &"0".repeat(min.saturating_sub(digits.len())));
    if f.sharp {
        match base {
            2 => digits.insert_str(0, "0b"),
            8 if !digits.starts_with('0') => digits.insert(0, '0'),
            16 => digits.insert_str(0, if upper { "0X" } else { "0x" }),
            _ => {}
        }
    }
    if verb == 'O' {
        digits.insert_str(0, "0o");
    }
    if negative {
        digits.insert(0, '-');
    } else if f.plus {
        digits.insert(0, '+');
    } else if f.space {
        digits.insert(0, ' ');
    }
    pad(out, &digits, Flags { zero: false, ..f });
}

fn fmt_float(out: &mut String, v: &Value, x: f64, verb: char, f: Flags) {
    let (verb, default_prec) = match verb {
        'v' => ('g', None),
        'g' | 'G' => (verb, None),
        'e' | 'E' | 'f' => (verb, Some(6)),
        'F' => ('f', Some(6)),
        // ponytail: %b, %x and %X of a float print as %g; Bento's pipelines use none of them.
        'b' | 'x' | 'X' => ('g', None),
        _ => {
            bad_verb(out, v, verb);
            return;
        }
    };
    let prec = f.prec.or(default_prec);
    let mut num = format_float(x, verb, prec);
    if !num.starts_with(['-', '+']) {
        num.insert(0, '+');
    }
    if f.space && num.starts_with('+') && !f.plus {
        num.replace_range(0..1, " ");
    }
    if num[1..].starts_with(['I', 'N']) {
        if num[1..].starts_with('N') && !f.space && !f.plus {
            num.remove(0);
        }
        pad(out, &num, Flags { zero: false, ..f });
        return;
    }
    if f.sharp {
        sharp_float(&mut num, verb, prec);
    }
    if f.plus || !num.starts_with('+') {
        let w = f.wid.unwrap_or(0);
        if f.zero && !f.minus && w > num.chars().count() {
            out.push_str(&num[..1]);
            out.extend(std::iter::repeat_n('0', w - num.chars().count()));
            out.push_str(&num[1..]);
            return;
        }
        pad(out, &num, f);
        return;
    }
    pad(out, &num[1..], f);
}

/// `%#g` keeps trailing zeros and `%#e`/`%#f` a decimal point (`format.go` fmtFloat, sharp).
fn sharp_float(num: &mut String, verb: char, prec: Option<usize>) {
    let mut digits: i64 = if matches!(verb, 'g' | 'G') {
        prec.map_or(6, |p| p as i64)
    } else {
        0
    };
    let (head, tail) = match num.find(['e', 'E']) {
        Some(at) => (num[..at].to_string(), num[at..].to_string()),
        None => (num.clone(), String::new()),
    };
    let mut head = head;
    let mut seen = false;
    for c in head[1..].chars() {
        if c == '.' {
            continue;
        }
        if c != '0' {
            seen = true;
        }
        if seen {
            digits -= 1;
        }
    }
    if !head.contains('.') {
        if head.len() == 2 && head.ends_with('0') {
            digits -= 1;
        }
        head.push('.');
    }
    while digits > 0 {
        head.push('0');
        digits -= 1;
    }
    *num = head + &tail;
}

/// `strconv.FormatFloat(x, verb, prec, 64)` for `e`, `E`, `f`, `g` and `G`; `None` is the
/// shortest text that reads back as `x`.
fn format_float(x: f64, verb: char, prec: Option<usize>) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x.is_sign_positive() { "+Inf" } else { "-Inf" }.to_string();
    }
    let neg = x.is_sign_negative();
    let (digs, dp) = decimal_digits(x.abs(), verb, prec);
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    let nd = digs.len();
    match verb {
        // fmt_float gives %e and %f a precision (6 by default); only %g and %v come shortest.
        'e' | 'E' => {
            let p = prec.unwrap_or(6);
            fmt_e(&mut s, &digs, dp, p, verb);
        }
        'f' => {
            let p = prec.unwrap_or(6);
            fmt_f(&mut s, &digs, dp, p);
        }
        _ => {
            let shortest = prec.is_none();
            let mut p = prec.map_or(nd, |p| if p == 0 { 1 } else { p });
            // Go also lowers eprec to nd when nd >= dp; exp = dp - 1 is below nd then, so the
            // %e/%f choice below is the same either way.
            let mut eprec = p as i64;
            if shortest {
                eprec = 6;
            }
            let exp = dp - 1;
            let e = if verb == 'G' { 'E' } else { 'e' };
            if exp < -4 || exp >= eprec {
                p = p.min(nd);
                fmt_e(&mut s, &digs, dp, p.saturating_sub(1), e);
            } else {
                if p as i64 > dp {
                    p = nd;
                }
                fmt_f(&mut s, &digs, dp, (p as i64 - dp).max(0) as usize);
            }
        }
    }
    s
}

/// The decimal digits of `x` (no leading or trailing zeros; "0" for zero) and the place of the
/// decimal point, rounded as `strconv` rounds for `verb` and `prec`.
fn decimal_digits(x: f64, verb: char, prec: Option<usize>) -> (Vec<u8>, i64) {
    // An f64's exact decimal has at most 1074 places and 767 significant digits, so rounding at
    // 1100 is exact and the zeros past it are trimmed anyway; Rust's format! refuses a precision
    // past u16::MAX that Go's 1e6 limit allows.
    let prec = prec.map(|p| p.min(1100));
    let sci = match (verb, prec) {
        (_, None) => format!("{x:e}"),
        ('e' | 'E', Some(p)) => format!("{x:.p$e}"),
        ('g' | 'G', Some(p)) => format!("{:.*e}", p.max(1) - 1, x),
        (_, Some(p)) => {
            // %f: round at `p` places after the point.
            let fixed = format!("{x:.p$}");
            let (int, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
            let all: String = format!("{int}{frac}");
            let lead = all.len() - all.trim_start_matches('0').len();
            let digs: Vec<u8> = all
                .trim_start_matches('0')
                .trim_end_matches('0')
                .bytes()
                .collect();
            if digs.is_empty() {
                return (vec![b'0'], 1);
            }
            return (digs, int.len() as i64 - lead as i64);
        }
    };
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let digs: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).collect();
    let trimmed: Vec<u8> = {
        let end = digs.iter().rposition(|d| *d != b'0').map_or(0, |p| p + 1);
        digs[..end].to_vec()
    };
    if trimmed.is_empty() {
        return (vec![b'0'], 1);
    }
    (trimmed, exp + 1)
}

fn fmt_e(s: &mut String, digs: &[u8], dp: i64, prec: usize, e: char) {
    let zero = digs == b"0";
    s.push(char::from(digs[0]));
    if prec > 0 {
        s.push('.');
        for i in 1..=prec {
            s.push(char::from(*digs.get(i).unwrap_or(&b'0')));
        }
    }
    s.push(e);
    let exp = if zero { 0 } else { dp - 1 };
    s.push(if exp < 0 { '-' } else { '+' });
    let a = exp.unsigned_abs();
    if a < 10 {
        s.push('0');
    }
    s.push_str(&a.to_string());
}

fn fmt_f(s: &mut String, digs: &[u8], dp: i64, prec: usize) {
    let zero = digs == b"0";
    if dp > 0 && !zero {
        for i in 0..dp as usize {
            s.push(char::from(*digs.get(i).unwrap_or(&b'0')));
        }
    } else {
        s.push('0');
    }
    if prec > 0 {
        s.push('.');
        for i in 1..=prec as i64 {
            let at = dp + i - 1;
            let d = if zero || at < 0 {
                b'0'
            } else {
                *digs.get(at as usize).unwrap_or(&b'0')
            };
            s.push(char::from(d));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sprintf;
    use crate::value::Value;

    #[test]
    fn formats_as_go_fmt() {
        // Each expected text is Go 1.27 fmt.Sprintf's (through `bento blobl`).
        let s = |f: &str, v: Vec<Value>| sprintf(f, &v);
        assert_eq!(
            s(
                "%04x|%08x|%x|%02X",
                vec![
                    Value::Int(5),
                    Value::Int(255),
                    Value::Int(255),
                    Value::Int(10)
                ]
            ),
            "0005|000000ff|ff|0A"
        );
        assert_eq!(
            s(
                "%-4d|%+d|%o|%b|%c|%U",
                vec![
                    Value::Int(5),
                    Value::Int(5),
                    Value::Int(8),
                    Value::Int(5),
                    Value::Int(65),
                    Value::Int(65)
                ]
            ),
            "5   |+5|10|101|A|U+0041"
        );
        assert_eq!(
            s(
                "%5.2f|%e|%g|%08.3f",
                vec![
                    Value::Float(3.125),
                    Value::Float(1234.5),
                    Value::Float(0.00001234),
                    Value::Float(-3.5)
                ]
            ),
            " 3.12|1.234500e+03|1.234e-05|-003.500"
        );
        assert_eq!(
            s(
                "%d|%s|%v",
                vec![Value::Float(5.5), Value::Null, Value::Float(1e21)]
            ),
            "%!d(float64=5.5)|%!s(<nil>)|1e+21"
        );
        assert_eq!(
            s(
                "%5s|%.2s|%T|%d",
                vec![
                    Value::String("ab".into()),
                    Value::String("abc".into()),
                    Value::Int(1)
                ]
            ),
            "   ab|ab|int64|%!d(MISSING)"
        );
        assert_eq!(
            s("%v", vec![Value::Int(1), Value::Int(2)]),
            "1%!(EXTRA int64=2)"
        );
    }

    #[test]
    fn a_precision_past_u16_max_pads_zeros_as_go() {
        // Fuzz crash (sprintf target): format! panicked on a precision above 65535.
        let x = [Value::Float(1.5)];
        assert_eq!(
            sprintf("%.70000f", &x),
            format!("1.5{}", "0".repeat(69_999))
        );
        assert_eq!(
            sprintf("%.70000e", &x),
            format!("1.5{}e+00", "0".repeat(69_999))
        );
        assert_eq!(sprintf("%.70000g", &x), "1.5");
        assert_eq!(sprintf("%.1100f", &[Value::Float(5e-324)]).len(), 1102);
        // The smallest subnormal's exact expansion still fits inside 1100 places.
        let tiny = sprintf("%.1100f", &[Value::Float(5e-324)]);
        assert!(
            tiny.starts_with(&format!("0.{}494065645841246544", "0".repeat(323))),
            "{tiny}"
        );
    }

    #[test]
    fn formats_flags_widths_and_every_kind_as_go() {
        use std::collections::BTreeMap;
        let i = Value::Int;
        let fl = Value::Float;
        let st = |t: &str| Value::String(t.into());
        let by = |t: &[u8]| Value::Bytes(t.to_vec());
        let cases: Vec<(&str, Vec<Value>, &str)> = vec![
            // operands: `*`, missing, extra, no verb
            ("%*d|%-*d", vec![i(5), i(42), i(5), i(42)], "   42|42   "),
            ("%*d", vec![i(-4), i(7)], "7   "),
            ("%*d", vec![Value::Uint(3), i(7)], "  7"),
            ("%*d", vec![st("x"), i(42)], "%!(BADWIDTH)42"),
            ("%*d", vec![i(2_000_000), i(1)], "%!(BADWIDTH)1"),
            ("%*d", vec![Value::Uint(2_000_000), i(1)], "%!(BADWIDTH)1"),
            // An index without its closing bracket is a bad index.
            ("%[1d", vec![i(5)], "%!d(BADINDEX)"),
            ("%.[9]d", vec![i(5)], "%!d(BADINDEX)"),
            // A precision turns the zero flag off: Go pads with spaces then.
            (
                "%08.3d|%-08d|%8.2f|%08.2f",
                vec![i(42), i(42), Value::Float(1.5), Value::Float(-1.5)],
                "     042|42      |    1.50|-0001.50",
            ),
            ("%#U|%#U", vec![i(0x20), i(0xE9)], "U+0020 ' '|U+00E9 'é'"),
            // %q escapes what Go's IsPrint refuses, by size; %+q every non-ASCII rune.
            (
                "%q|%q|%+q",
                vec![st("\u{80}"), st("\u{2003}"), st("\u{10000}\u{ffff}")],
                "\"\\u0080\"|\"\\u2003\"|\"\\U00010000\\uffff\"",
            ),
            (
                "%#q|%#q|%#q",
                vec![i(0xE9), i(0x27), i(0x41)],
                "'é'|'\\''|'A'",
            ),
            // Go's fmtSbx: case, the space and # flags, width either side.
            ("%x|%X", vec![st("zz"), st("zz")], "7a7a|7A7A"),
            (
                "% x|%# x|%#x",
                vec![st("zz"), st("zz"), st("zz")],
                "7a 7a|0x7a 0x7a|0x7a7a",
            ),
            (
                "%12x|%-12x|",
                vec![st("zz"), st("zz")],
                "        7a7a|7a7a        |",
            ),
            (
                "%# 14x|%#8x|% 8x|",
                vec![st("zz"), st("zz"), st("zz")],
                "     0x7a 0x7a|  0x7a7a|   7a 7a|",
            ),
            ("%.1x|%08x", vec![st("zz"), st("zz")], "7a|00007a7a"),
            // A nil inside a slice or map prints <nil> under any verb, at the top only under %v.
            (
                "%d",
                vec![Value::Array(vec![Value::Null].into())],
                "[<nil>]",
            ),
            (
                "%d",
                vec![Value::Object(
                    [("a".to_string(), Value::Null)]
                        .into_iter()
                        .collect::<BTreeMap<_, _>>()
                        .into(),
                )],
                "map[%!d(string=a):<nil>]",
            ),
            (
                "%.*f|%.*f",
                vec![i(2), fl(5.12159), i(-1), fl(1.5)],
                "5.12|1.500000",
            ),
            ("%.*d", vec![st("x"), i(5)], "%!(BADPREC)5"),
            ("abc%", vec![], "abc%!(NOVERB)"),
            ("%-", vec![], "%!(NOVERB)"),
            ("%.", vec![i(1)], "%!(NOVERB)%!(EXTRA int64=1)"),
            ("100%%", vec![], "100%"),
            (
                "%d",
                vec![i(1), Value::Null, st("a")],
                "1%!(EXTRA <nil>, string=a)",
            ),
            ("%.f|%.1f", vec![fl(2.7), fl(0.05)], "3|0.1"),
            // nil, bool, %T
            (
                "%v|%7v|%t|%d",
                vec![
                    Value::Null,
                    Value::Null,
                    Value::Bool(true),
                    Value::Bool(false),
                ],
                "<nil>|  <nil>|true|%!d(bool=false)",
            ),
            (
                "%T|%T|%T|%T|%T",
                vec![
                    Value::Null,
                    Value::Bool(true),
                    Value::Uint(1),
                    fl(1.0),
                    Value::Number("1".into()),
                ],
                "<nil>|bool|uint64|float64|json.Number",
            ),
            (
                "%T|%T|%T|%10T|%T",
                vec![
                    by(b"a"),
                    Value::Array(vec![].into()),
                    Value::Object(BTreeMap::new().into()),
                    i(1),
                    Value::Timestamp(crate::time_format::utc(jiff::Timestamp::UNIX_EPOCH)),
                ],
                "[]uint8|[]interface {}|map[string]interface {}|     int64|time.Time",
            ),
            // arrays and maps print each element with the verb
            (
                "%v|%d",
                vec![
                    Value::Array(vec![i(1), Value::Null].into()),
                    Value::Array(vec![i(1), st("a")].into()),
                ],
                "[1 <nil>]|[1 %!d(string=a)]",
            ),
            (
                "%v",
                vec![Value::Object(
                    BTreeMap::from([("a".to_string(), i(1)), ("b".to_string(), st("x"))]).into(),
                )],
                "map[a:1 b:x]",
            ),
            // bytes
            (
                "%v|%s|%.1s|%x|%X|% x|%#x|%# x",
                vec![by(b"hi"); 8],
                "[104 105]|hi|h|6869|6869|68 69|0x6869|0x68 0x69",
            ),
            (
                "%q|%o|%6x|%-6x|%4.0x|%d",
                vec![by(b"hi"); 6],
                "\"hi\"|[150 151]|  6869|6869  |    |[104 105]",
            ),
            // strings
            (
                "%x|%X|%q|%+q|%q|%#q|%#q|%d",
                vec![
                    st("hi"),
                    st("hi"),
                    st("a\"b"),
                    st("é"),
                    st("é"),
                    st("ab"),
                    st("a`b"),
                    st("x"),
                ],
                "6869|6869|\"a\\\"b\"|\"\\u00e9\"|\"é\"|`ab`|\"a`b\"|%!d(string=x)",
            ),
            (
                "%q|%q|%q|%+q|%q",
                vec![
                    st("\u{7}\u{8}\u{c}\n\r\t\u{b}\\"),
                    st("\u{1}"),
                    st("\u{200b}"),
                    st("😀"),
                    st("😀"),
                ],
                "\"\\a\\b\\f\\n\\r\\t\\v\\\\\"|\"\\x01\"|\"\\u200b\"|\"\\U0001f600\"|\"😀\"",
            ),
            (
                "%v|%s|%d",
                vec![
                    Value::Number("1.50".into()),
                    Value::Number("2".into()),
                    Value::Number("3".into()),
                ],
                "1.50|2|%!d(json.Number=3)",
            ),
            // integers
            (
                "%x|%#x|%#X|%#o|%#o|%O|%#b|% x",
                vec![i(-255), i(255), i(255), i(8), i(0), i(8), i(5), i(255)],
                "-ff|0xff|0XFF|010|0|0o10|0b101| ff",
            ),
            (
                "%+d|% d|%.0d|%3.0d|%.3d|%05d|%-05d|%+05d",
                vec![i(0), i(5), i(0), i(0), i(5), i(-42), i(42), i(42)],
                "+0| 5||   |005|-0042|42   |+0042",
            ),
            (
                "%c|%c|%c|%q|%q|%+q|%#q|%q|%q",
                vec![
                    i(0x1F600),
                    i(-1),
                    i(0xD800),
                    i(97),
                    i(39),
                    i(233),
                    i(97),
                    i(0x1F600),
                    i(-1),
                ],
                "😀|\u{FFFD}|\u{FFFD}|'a'|'\\''|'\\u00e9'|'a'|'😀'|'\u{FFFD}'",
            ),
            (
                "%U|%#U|%#U|%.6U|%08U|%z",
                vec![i(0x1F600), i(65), i(10), i(65), i(65), i(5)],
                "U+1F600|U+0041 'A'|U+000A|U+000041|  U+0041|%!z(int64=5)",
            ),
            (
                "%d|%v|%x",
                vec![Value::Uint(u64::MAX), Value::Uint(7), Value::Uint(255)],
                "18446744073709551615|7|ff",
            ),
            // floats
            (
                "%v|%v|%v|%v|%g|%g",
                vec![
                    fl(1.5),
                    fl(1e20),
                    fl(1e6),
                    fl(123456.0),
                    fl(0.0),
                    fl(100000.0),
                ],
                "1.5|1e+20|1e+06|123456|0|100000",
            ),
            (
                "%e|%f|%G|%E|%F|%.3g",
                vec![fl(0.0), fl(-0.0), fl(1e-7), fl(1234.5), fl(1.5), fl(1234.5)],
                "0.000000e+00|-0.000000|1E-07|1.234500E+03|1.500000|1.23e+03",
            ),
            (
                "%.0g|%.3g|%.3g|%.2g|%G|%.10g",
                vec![
                    fl(5.7),
                    fl(100.0),
                    fl(0.0001234),
                    fl(1e-5),
                    fl(1e21),
                    fl(1.0 / 3.0),
                ],
                "6|100|0.000123|1e-05|1E+21|0.3333333333",
            ),
            (
                "%f|%.0f|%.3f|%e|%g|%e",
                vec![
                    fl(1e21),
                    fl(0.4),
                    fl(0.0001),
                    fl(1e-300),
                    fl(1e-300),
                    fl(1e100),
                ],
                "1000000000000000000000.000000|0|0.000|1.000000e-300|1e-300|1.000000e+100",
            ),
            (
                "%#g|%#g|%#.0e|%#.0f|%#g|%#.3g",
                vec![fl(1.0), fl(1e6), fl(1.0), fl(1.0), fl(0.0), fl(1.5)],
                "1.00000|1.00000e+06|1.e+00|1.|0.00000|1.50",
            ),
            (
                "%+f|% f|%+.1f|%08.2f|%+08.2f|%-8.2f|% 08.2f",
                vec![
                    fl(1.0),
                    fl(1.0),
                    fl(-1.0),
                    fl(5.12159),
                    fl(5.12),
                    fl(5.12),
                    fl(5.12),
                ],
                "+1.000000| 1.000000|-1.0|00005.12|+0005.12|5.12    | 0005.12",
            ),
            (
                "%f|%+f|% f|%5f|%05f|%f|%05f",
                vec![
                    fl(f64::NAN),
                    fl(f64::NAN),
                    fl(f64::NAN),
                    fl(f64::NAN),
                    fl(f64::INFINITY),
                    fl(f64::INFINITY),
                    fl(f64::NEG_INFINITY),
                ],
                "NaN|+NaN| NaN|  NaN| +Inf|+Inf| -Inf",
            ),
            (
                "%.2e|%10.3e|%b|%c",
                vec![fl(1234.5), fl(1234.56), fl(1.5), fl(1.5)],
                "1.23e+03| 1.235e+03|1.5|%!c(float64=1.5)",
            ),
        ];
        for (format, args, want) in cases {
            assert_eq!(sprintf(format, &args), want, "{format}");
        }
    }
}
