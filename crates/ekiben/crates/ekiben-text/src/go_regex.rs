//! Copied from marek-mraz-jc/ekiben crates/ekiben-bloblang/src/go_regex.rs (cf2b0ff), which this
//! workspace's ekiben-bloblang does not carry yet.
//!
//! Go `regexp.Compile` (RE2 Perl syntax) on top of the `regex` crate: the pattern is read with Go's
//! grammar and rewritten in Rust syntax, so it means what it means in Bento.
//!
//! What differs between the two grammars: in Go a `{` that starts no valid repeat is a literal,
//! `\d \s \w \b \B` are ASCII-only, `\Q..\E` quotes, `\123` is octal, `[a&&b]`, `[a~~b]` and a
//! nested `[` are literals inside a class, stacked repeats (`a**`, `a{2}{3}`) are errors, and
//! escapes Rust adds (`\<`, `\e`) are errors.
//! ponytail: a `\p{Name}` Rust knows and Go does not (`\p{Alphabetic}`) compiles here; carry Go's
//! unicode table names if a mapping ever depends on that refusal.

use regex::Regex;

const PERL_CLASSES: [(char, &str); 6] = [
    ('d', "0-9"),
    ('s', "\\t\\n\\x0C\\r "),
    ('w', "0-9A-Za-z_"),
    ('D', "^0-9"),
    ('S', "^\\t\\n\\x0C\\r "),
    ('W', "^0-9A-Za-z_"),
];

const POSIX_NAMES: [&str; 14] = [
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// Go's `canonicalName`: separators dropped, the first letter upper case, the rest lower case.
fn canonical_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().filter(|c| !matches!(c, '_' | '-' | ' ')) {
        if out.is_empty() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// Go's `unicodeTable`: the Unicode table a `\p{name}` selects, by the name Rust knows it by.
fn unicode_table(name: &str) -> Option<&'static str> {
    let key = canonical_name(name);
    let names = &crate::go_unicode_names::NAMES;
    names
        .binary_search_by(|(k, _)| (*k).cmp(key.as_str()))
        .ok()
        .map(|i| names[i].1)
}

fn err(code: &str, expr: &str) -> String {
    format!("error parsing regexp: {code}: `{expr}`")
}

/// One literal character, escaped for Rust inside or outside a class.
fn push_literal(out: &mut String, c: char) {
    if "\\.+*?()|[]{}^$#&-~".contains(c) {
        out.push('\\');
        out.push(c);
    } else if c.is_control() {
        out.push_str(&format!("\\x{{{:X}}}", c as u32));
    } else {
        out.push(c);
    }
}

/// A class no character is in: what a surrogate (`\x{D800}`, which Go reads) matches, since no
/// text holds one.
const NO_CHAR: &str = "[^\\x{0}-\\x{10FFFF}]";

/// The code points `lo..=hi` as class members, without the surrogates Rust has no `char` for.
fn push_range(out: &mut String, lo: u32, hi: u32) {
    let surrogate = |c: u32| (0xD800..0xE000).contains(&c);
    let lo = if surrogate(lo) { 0xE000 } else { lo };
    let hi = if surrogate(hi) { 0xD7FF } else { hi };
    match (char::from_u32(lo), char::from_u32(hi)) {
        (Some(l), Some(h)) if l < h => {
            push_literal(out, l);
            out.push('-');
            push_literal(out, h);
        }
        (Some(l), Some(h)) if l == h => push_literal(out, l),
        _ => out.push_str(NO_CHAR),
    }
}

/// Ends the open group's last operand for the nested repeat limit: a new one starts.
fn fold(frames: &mut [(u64, u64)]) {
    if let Some(f) = frames.last_mut() {
        f.0 = f.0.max(f.1);
        f.1 = 1;
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    /// Go's `SubexpNames`: one entry per group, index 0 the whole match.
    names: Vec<Option<String>>,
}

/// What one escape stands for.
enum Escape {
    Char(char),
    /// A surrogate code point: Go takes `\x{D800}`, and no text holds it.
    Surrogate(u32),
    /// Rust text for a class (`[0-9]`) or an assertion, already complete.
    Class(String),
    Assert(String),
    Quote(String),
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Reads the escape after a `\` (already consumed), Go's `parseEscape` plus the class and
    /// assertion escapes `parse` handles itself.
    fn escape(&mut self, in_class: bool) -> Result<Escape, String> {
        let start = self.pos - 1;
        let Some(c) = self.bump() else {
            return Err("error parsing regexp: trailing backslash at end of expression: ``".into());
        };
        let bad = |p: &Self| err("invalid escape sequence", &p.src[start..p.pos]);
        match c {
            '1'..='7' | '0' => {
                if c != '0' && !matches!(self.peek(), Some('0'..='7')) {
                    return Err(bad(self));
                }
                let mut r = c as u32 - '0' as u32;
                for _ in 1..3 {
                    match self.peek() {
                        Some(d @ '0'..='7') => {
                            r = r * 8 + (d as u32 - '0' as u32);
                            self.bump();
                        }
                        _ => break,
                    }
                }
                Ok(Escape::Char(char::from_u32(r).unwrap_or('\u{FFFD}')))
            }
            'x' => {
                let hex = |p: &mut Self| p.bump().and_then(|h| h.to_digit(16));
                if self.peek() == Some('{') {
                    self.bump();
                    let mut r: u32 = 0;
                    let mut n = 0;
                    loop {
                        match self.bump() {
                            Some('}') if n > 0 => break,
                            Some(h) if h.is_ascii_hexdigit() => {
                                r = r
                                    .saturating_mul(16)
                                    .saturating_add(h.to_digit(16).unwrap_or(0));
                                n += 1;
                                if r > 0x10FFFF {
                                    return Err(bad(self));
                                }
                            }
                            _ => return Err(bad(self)),
                        }
                    }
                    return Ok(char::from_u32(r).map_or(Escape::Surrogate(r), Escape::Char));
                }
                match (hex(self), hex(self)) {
                    (Some(a), Some(b)) => Ok(Escape::Char(
                        char::from_u32(a * 16 + b).unwrap_or('\u{FFFD}'),
                    )),
                    _ => Err(bad(self)),
                }
            }
            'a' => Ok(Escape::Char('\x07')),
            'f' => Ok(Escape::Char('\x0C')),
            'n' => Ok(Escape::Char('\n')),
            'r' => Ok(Escape::Char('\r')),
            't' => Ok(Escape::Char('\t')),
            'v' => Ok(Escape::Char('\x0B')),
            'd' | 's' | 'w' | 'D' | 'S' | 'W' => {
                let body = PERL_CLASSES
                    .iter()
                    .find(|(k, _)| *k == c)
                    .map_or("", |(_, b)| b);
                Ok(Escape::Class(format!("[{body}]")))
            }
            'p' | 'P' => {
                let mut neg = c == 'P';
                let name = match self.bump() {
                    Some('{') => {
                        let end = self.rest().find('}').ok_or_else(|| {
                            err("invalid character class range", &self.src[start..])
                        })?;
                        let name = &self.rest()[..end];
                        self.pos += end + 1;
                        name.to_string()
                    }
                    Some(l) => l.to_string(),
                    None => return Err(err("invalid character class range", &self.src[start..])),
                };
                let name = match name.strip_prefix('^') {
                    Some(n) => {
                        neg = !neg;
                        n.to_string()
                    }
                    None => name,
                };
                let Some(table) = unicode_table(&name) else {
                    return Err(err(
                        "invalid character class range",
                        &self.src[start..self.pos],
                    ));
                };
                Ok(Escape::Class(format!(
                    "\\{}{{{table}}}",
                    if neg { 'P' } else { 'p' }
                )))
            }
            'A' | 'z' if !in_class => Ok(Escape::Assert(format!("\\{c}"))),
            'b' | 'B' if !in_class => Ok(Escape::Assert(format!("(?-u:\\{c})"))),
            'Q' if !in_class => {
                let rest = self.rest();
                let (lit, skip) = match rest.find("\\E") {
                    Some(i) => (&rest[..i], i + 2),
                    None => (rest, rest.len()),
                };
                self.pos += skip;
                Ok(Escape::Quote(lit.to_string()))
            }
            c if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok(Escape::Char(c)),
            _ => Err(bad(self)),
        }
    }

    /// `{n}`, `{n,}` or `{n,m}` at the cursor; `None` (cursor unchanged) when it is a literal `{`.
    fn repeat(&mut self) -> Option<(u32, Option<u32>, &'a str)> {
        let rest = self.rest();
        let b = rest.as_bytes();
        let digits = |i: usize| b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        let num = |s: &str| s.parse::<u32>().unwrap_or(u32::MAX);
        let mut i = 1;
        let n1 = digits(i);
        if n1 == 0 {
            return None;
        }
        let min = num(&rest[i..i + n1]);
        i += n1;
        let max = match b.get(i) {
            Some(b'}') => Some(min),
            Some(b',') => {
                i += 1;
                let n2 = digits(i);
                let max = (n2 > 0).then(|| num(&rest[i..i + n2]));
                i += n2;
                max
            }
            _ => return None,
        };
        if b.get(i) != Some(&b'}') {
            return None;
        }
        self.pos += i + 1;
        Some((min, max, &rest[..=i]))
    }

    fn class(&mut self, out: &mut String) -> Result<(), String> {
        let start = self.pos - 1;
        out.push('[');
        if self.peek() == Some('^') {
            self.bump();
            out.push('^');
        }
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err(err("missing closing ]", &self.src[start..]));
            };
            if c == ']' && !first {
                self.bump();
                break;
            }
            first = false;
            if c == '[' && self.rest().starts_with("[:") {
                if let Some(end) = self.rest()[2..].find(":]") {
                    let name = &self.rest()[2..2 + end];
                    if !POSIX_NAMES.contains(&name.strip_prefix('^').unwrap_or(name)) {
                        return Err(err(
                            "invalid character class range",
                            &self.rest()[..end + 4],
                        ));
                    }
                    out.push_str(&self.rest()[..end + 4]);
                    self.pos += end + 4;
                    continue;
                }
            }
            let item_start = self.pos;
            let lo = match self.class_char()? {
                Ok(ch) => ch,
                Err(set) => {
                    out.push_str(&set);
                    continue;
                }
            };
            let rest = self.rest();
            if rest.starts_with('-') && !rest.starts_with("-]") && rest.len() > 1 {
                self.bump();
                let hi_start = self.pos;
                let hi = match self.class_char()? {
                    Ok(ch) => ch,
                    // Go reads a class escape (`\d`) as no range end at all
                    Err(_) => {
                        return Err(err(
                            "invalid escape sequence",
                            &self.src[hi_start..self.pos],
                        ))
                    }
                };
                if hi < lo {
                    return Err(err(
                        "invalid character class range",
                        &self.src[item_start..self.pos],
                    ));
                }
                push_range(out, lo, hi);
            } else {
                push_range(out, lo, lo);
            }
        }
        out.push(']');
        Ok(())
    }

    /// One class member: a code point, or (as `Err`) a nested set already in Rust syntax.
    fn class_char(&mut self) -> Result<Result<u32, String>, String> {
        let Some(c) = self.bump() else {
            return Err(err("missing closing ]", self.src));
        };
        if c != '\\' {
            return Ok(Ok(u32::from(c)));
        }
        Ok(match self.escape(true)? {
            Escape::Char(ch) => Ok(u32::from(ch)),
            Escape::Surrogate(point) => Ok(point),
            Escape::Class(set) => Err(set),
            Escape::Assert(a) | Escape::Quote(a) => Err(a),
        })
    }

    fn translate(&mut self) -> Result<String, String> {
        let mut out = String::with_capacity(self.src.len() + 8);
        // Where the last repeat operator started, to refuse a stacked one as Go does.
        let mut last_repeat: Option<usize> = None;
        // Whether something a repeat can apply to stands before the cursor (Go's operand stack).
        let mut operand = false;
        // Where a run of flag groups (`(?i)`) starts in `out` when it directly follows an operand:
        // Go repeats that operand (`a(?i)*` is `a*`), Rust refuses, so the repeat moves before it.
        let mut flags_at: Option<usize> = None;
        // Per open group: the largest `{n,m}` product of its finished operands and of its last
        // one, for Go's nested repeat limit (`repeatIsValid`, 1000).
        let mut frames: Vec<(u64, u64)> = vec![(1, 1)];
        while let Some(c) = self.peek() {
            let start = self.pos;
            let mut repeat = false;
            let mut next_operand = true;
            let mut next_flags_at = None;
            let frame = frames.len() - 1;
            match c {
                '*' | '+' | '?' | '{' => {
                    let mut count = None;
                    if c == '{' {
                        match self.repeat() {
                            Some((min, max, text)) => {
                                if min > 1000 || max.is_some_and(|m| m > 1000 || m < min) {
                                    return Err(err("invalid repeat count", text));
                                }
                                count = Some((min, max));
                            }
                            None => {
                                self.bump();
                                fold(&mut frames);
                                out.push_str("\\{");
                                last_repeat = None;
                                operand = true;
                                flags_at = None;
                                continue;
                            }
                        }
                    } else {
                        self.bump();
                    }
                    if self.peek() == Some('?') {
                        self.bump();
                    }
                    let op = &self.src[start..self.pos];
                    if let Some(prev) = last_repeat {
                        return Err(err(
                            "invalid nested repetition operator",
                            &self.src[prev..self.pos],
                        ));
                    }
                    if !operand {
                        return Err(err("missing argument to repetition operator", op));
                    }
                    if let Some((min, max)) = count {
                        let m = u64::from(max.unwrap_or(min));
                        let weight = if m == 0 { 1 } else { m * frames[frame].1 };
                        // max is at least min here: a repeat of at most one copy never counts
                        if max.unwrap_or(u32::MAX) >= 2 && weight > 1000 {
                            return Err(err("invalid repeat count", op));
                        }
                        frames[frame].1 = weight;
                    }
                    match flags_at {
                        Some(at) => {
                            let flags = out.split_off(at);
                            out.push_str(op);
                            out.push_str(&flags);
                        }
                        None => out.push_str(op),
                    }
                    repeat = true;
                }
                '\\' => {
                    self.bump();
                    let esc = self.escape(false)?;
                    if !matches!(&esc, Escape::Quote(l) if l.is_empty()) {
                        fold(&mut frames);
                    }
                    match esc {
                        Escape::Char(ch) => push_literal(&mut out, ch),
                        Escape::Surrogate(_) => out.push_str(NO_CHAR),
                        Escape::Class(set) | Escape::Assert(set) => out.push_str(&set),
                        // A quoted run is a concatenation: a repeat after it binds its last char.
                        Escape::Quote(lit) if lit.is_empty() => {
                            next_operand = operand;
                            next_flags_at = flags_at;
                        }
                        Escape::Quote(lit) => lit.chars().for_each(|ch| push_literal(&mut out, ch)),
                    }
                }
                '[' => {
                    self.bump();
                    fold(&mut frames);
                    self.class(&mut out)?;
                }
                '(' => {
                    self.bump();
                    let opens = if self.peek() == Some('?') {
                        let at = out.len();
                        let (text, closes) = self.group_flags()?;
                        if let Some(name) =
                            text.strip_prefix("?P<").and_then(|t| t.strip_suffix('>'))
                        {
                            // Every group is unnamed in Rust: Go allows a name twice and names Rust
                            // refuses (`1a`), so `names` keeps them.
                            out.push('(');
                            self.names.push(Some(name.to_string()));
                        } else if text != "?)" {
                            // `(?)` changes nothing; `(?i)` sets flags without opening a group.
                            out.push('(');
                            out.push_str(&text);
                        }
                        if closes {
                            next_operand = operand;
                            next_flags_at = if operand { flags_at.or(Some(at)) } else { None };
                        }
                        !closes
                    } else {
                        out.push('(');
                        self.names.push(None);
                        true
                    };
                    if opens {
                        next_operand = false;
                        fold(&mut frames);
                        frames.push((1, 1));
                    }
                }
                ')' => {
                    self.bump();
                    if frames.len() == 1 {
                        return Err(err("unexpected )", self.src));
                    }
                    let (done, last) = frames.pop().unwrap_or((1, 1));
                    let parent = frames.len() - 1;
                    frames[parent].1 = done.max(last);
                    out.push(')');
                }
                _ => {
                    self.bump();
                    fold(&mut frames);
                    next_operand = c != '|';
                    match c {
                        '.' | '^' | '$' | '|' => out.push(c),
                        _ => push_literal(&mut out, c),
                    }
                }
            }
            last_repeat = repeat.then_some(start);
            operand = next_operand;
            flags_at = next_flags_at;
        }
        if frames.len() > 1 {
            return Err(err("missing closing )", self.src));
        }
        Ok(out)
    }

    /// `(?flags)`, `(?flags:`, `(?P<name>` or `(?<name>` after the `(` (the `?` is next).
    /// Returns the Rust text after the `(` and whether it closes itself (`(?i)`).
    fn group_flags(&mut self) -> Result<(String, bool), String> {
        let start = self.pos - 1;
        self.bump();
        let rest = self.rest();
        let name_start = if rest.starts_with("P<") {
            Some(2)
        } else if rest.starts_with('<') {
            Some(1)
        } else {
            None
        };
        if let Some(skip) = name_start {
            let Some(end) = rest.find('>') else {
                return Err(err("invalid named capture", &self.src[start..]));
            };
            let name = &rest[skip..end];
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(err(
                    "invalid named capture",
                    &self.src[start..self.pos + end + 1],
                ));
            }
            self.pos += end + 1;
            return Ok((format!("?P<{name}>"), false));
        }
        let mut text = String::from("?");
        let mut sign_seen = false;
        let mut any_flag = false;
        loop {
            let Some(c) = self.bump() else {
                return Err(err(
                    "invalid or unsupported Perl syntax",
                    &self.src[start..],
                ));
            };
            match c {
                'i' | 'm' | 's' | 'U' => {
                    text.push(c);
                    any_flag = true;
                }
                '-' if !sign_seen => {
                    sign_seen = true;
                    any_flag = false;
                    text.push(c);
                }
                ':' | ')' if !(sign_seen && !any_flag) => {
                    text.push(c);
                    return Ok((text, c == ')'));
                }
                _ => {
                    return Err(err(
                        "invalid or unsupported Perl syntax",
                        &self.src[start..self.pos],
                    ))
                }
            }
        }
    }
}

/// A compiled Go pattern: the Rust regex and Go's group names (`SubexpNames`).
pub struct GoRegex {
    re: Regex,
    names: Vec<Option<String>>,
}

impl std::ops::Deref for GoRegex {
    type Target = Regex;
    fn deref(&self) -> &Regex {
        &self.re
    }
}

impl GoRegex {
    /// Each group's name as Go reports it, index 0 the whole match.
    pub fn group_names(&self) -> impl Iterator<Item = Option<&str>> {
        self.names.iter().map(Option::as_deref)
    }
}

/// Go's `regexp.Compile(pattern)`.
pub fn compile(pattern: &str) -> Result<GoRegex, String> {
    let mut p = Parser {
        src: pattern,
        pos: 0,
        names: vec![None],
    };
    let rust = p.translate()?;
    let re = Regex::new(&rust).map_err(|e| {
        let msg = e.to_string();
        let last = msg
            .lines()
            .last()
            .unwrap_or(&msg)
            .trim_start_matches("error: ");
        format!("error parsing regexp: {last}: `{pattern}`")
    })?;
    Ok(GoRegex { re, names: p.names })
}

#[cfg(test)]
mod tests {
    use super::compile;

    #[test]
    fn reads_go_syntax() {
        let m = |p: &str, s: &str| compile(p).map(|r| r.is_match(s));
        assert_eq!(m(r#"{"k":[1,2]}"#, r#"{"k":1}"#), Ok(true));
        assert_eq!(m(r"\s", "\u{2028}"), Ok(false));
        assert_eq!(m(r"\w", "é"), Ok(false));
        assert_eq!(m(r"\Qa.b\E", "axb"), Ok(false));
        assert_eq!(m(r"[a&&b]", "&"), Ok(true));
        assert_eq!(m(r"\101", "A"), Ok(true));
        assert_eq!(
            m("a**", ""),
            Err("error parsing regexp: invalid nested repetition operator: `**`".into())
        );
        assert_eq!(m(r"\<", "<"), Ok(true));
        assert_eq!(
            m(r"\e", ""),
            Err("error parsing regexp: invalid escape sequence: `\\e`".into())
        );
        let r = compile("(?P<a>x)(?P<a>y)").map(|r| {
            r.group_names()
                .map(|n| n.map(str::to_string))
                .collect::<Vec<_>>()
        });
        assert_eq!(r, Ok(vec![None, Some("a".into()), Some("a".into())]));
    }

    /// Each pattern against Go 1.25's `regexp.Compile` and `FindStringIndex`, recorded with the
    /// golang:1.25-alpine image: the compile error text, or where the first match lies.
    #[test]
    fn compiles_and_matches_as_go_does() {
        type Expected = Result<Option<(usize, usize)>, &'static str>;
        let cases: &[(&str, &str, Expected)] = &[
            (r#"\0"#, "\u{0}", Ok(Some((0, 1)))),
            (r#"\012"#, "\u{a}", Ok(Some((0, 1)))),
            (
                r#"\1"#,
                "1",
                Err(r#"error parsing regexp: invalid escape sequence: `\1`"#),
            ),
            (
                r#"\18"#,
                "\u{1}8",
                Err(r#"error parsing regexp: invalid escape sequence: `\1`"#),
            ),
            (r#"\177"#, "\u{7f}", Ok(Some((0, 1)))),
            (r#"\400"#, "\u{100}", Ok(Some((0, 2)))),
            (r#"\0123"#, "\u{a}3", Ok(Some((0, 2)))),
            (r#"\x41"#, "A", Ok(Some((0, 1)))),
            (r#"\x{41}"#, "A", Ok(Some((0, 1)))),
            (
                r#"\x{}"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\x{}`"#),
            ),
            (
                r#"\x{110000}"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\x{110000`"#),
            ),
            (r#"\x{D800}"#, "", Ok(None)),
            (
                r#"\xZ1"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\xZ1`"#),
            ),
            (
                r#"\x4"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\x4`"#),
            ),
            (
                r#"\x{4G}"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\x{4G`"#),
            ),
            (
                r#"\x{41"#,
                "A",
                Err(r#"error parsing regexp: invalid escape sequence: `\x{41`"#),
            ),
            (r#"\a"#, "\u{7}", Ok(Some((0, 1)))),
            (r#"\f"#, "\u{c}", Ok(Some((0, 1)))),
            (r#"\n"#, "\u{a}", Ok(Some((0, 1)))),
            (r#"\r"#, "\u{d}", Ok(Some((0, 1)))),
            (r#"\t"#, "\u{9}", Ok(Some((0, 1)))),
            (r#"\v"#, "\u{b}", Ok(Some((0, 1)))),
            (r#"\d\D\s\S\w\W"#, "1a b_.", Ok(Some((0, 6)))),
            (r#"[\d\D]"#, "x", Ok(Some((0, 1)))),
            (r#"[\s]"#, "\u{2028}", Ok(None)),
            (r#"[\W]"#, "\u{e9}", Ok(Some((0, 2)))),
            (r#"\pL"#, "\u{e9}", Ok(Some((0, 2)))),
            (r#"\p{Greek}"#, "\u{3b1}", Ok(Some((0, 2)))),
            (r#"\p{^Greek}"#, "\u{3b1}", Ok(None)),
            (r#"\PL"#, "1", Ok(Some((0, 1)))),
            (r#"\P{^L}"#, "a", Ok(Some((0, 1)))),
            (
                r#"\p{Foo}"#,
                "",
                Err(r#"error parsing regexp: invalid character class range: `\p{Foo}`"#),
            ),
            (
                r#"\p{L"#,
                "L",
                Err(r#"error parsing regexp: invalid character class range: `\p{L`"#),
            ),
            (
                r#"\p"#,
                "",
                Err(r#"error parsing regexp: invalid character class range: `\p`"#),
            ),
            (r#"[\p{Lu}]"#, "A", Ok(Some((0, 1)))),
            (r#"\p{greek}"#, "\u{3b1}", Ok(Some((0, 2)))),
            (r#"\Aa\z"#, "a", Ok(Some((0, 1)))),
            (r#"a\b"#, "a!", Ok(Some((0, 1)))),
            (r#"a\Bb"#, "ab", Ok(Some((0, 2)))),
            (
                r#"[\b]"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\b`"#),
            ),
            (
                r#"[\A]"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\A`"#),
            ),
            (
                r#"[\Q]"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\Q`"#),
            ),
            (r#"\Qab"#, "ab", Ok(Some((0, 2)))),
            (
                r#"\Q\E*"#,
                "",
                Err("error parsing regexp: missing argument to repetition operator: `*`"),
            ),
            (r#"a\Q\E*"#, "aa", Ok(Some((0, 2)))),
            (r#"\Qa.\E+"#, "a..", Ok(Some((0, 3)))),
            (r#"\."#, ".", Ok(Some((0, 1)))),
            (
                "\\\u{e9}",
                "",
                Err("error parsing regexp: invalid escape sequence: `\\\u{e9}`"),
            ),
            (
                r#"\"#,
                "",
                Err("error parsing regexp: trailing backslash at end of expression: ``"),
            ),
            (
                r#"\8"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\8`"#),
            ),
            ("a{2}", "aa", Ok(Some((0, 2)))),
            ("a{2,}", "aaa", Ok(Some((0, 3)))),
            ("a{2,3}", "aaaa", Ok(Some((0, 3)))),
            ("a{,3}", "a{,3}", Ok(Some((0, 5)))),
            ("a{2", "a{2", Ok(Some((0, 3)))),
            ("a{2,3", "a{2,3", Ok(Some((0, 5)))),
            ("a{x}", "a{x}", Ok(Some((0, 4)))),
            (
                "{2}",
                "",
                Err("error parsing regexp: missing argument to repetition operator: `{2}`"),
            ),
            (
                "a{1001}",
                "",
                Err("error parsing regexp: invalid repeat count: `{1001}`"),
            ),
            (
                "a{3,2}",
                "",
                Err("error parsing regexp: invalid repeat count: `{3,2}`"),
            ),
            (
                "a{2}{3}",
                "",
                Err("error parsing regexp: invalid nested repetition operator: `{2}{3}`"),
            ),
            ("a*?", "aa", Ok(Some((0, 0)))),
            (
                "a**",
                "",
                Err("error parsing regexp: invalid nested repetition operator: `**`"),
            ),
            (
                "*",
                "",
                Err("error parsing regexp: missing argument to repetition operator: `*`"),
            ),
            (
                "(a{500}){3}",
                "",
                Err("error parsing regexp: invalid repeat count: `{3}`"),
            ),
            ("a{0}", "a", Ok(Some((0, 0)))),
            ("(a{2}){0}", "aa", Ok(Some((0, 0)))),
            (
                "a{99999999999}",
                "",
                Err("error parsing regexp: invalid repeat count: `{99999999999}`"),
            ),
            (
                "a{2,1001}",
                "",
                Err("error parsing regexp: invalid repeat count: `{2,1001}`"),
            ),
            ("(a{10}b{100}){10}", "", Ok(None)),
            ("(a{10}|b{200}){5}", "", Ok(None)),
            ("((a{10}){10}){10}", "", Ok(None)),
            (
                "((a{10}){10}){11}",
                "",
                Err("error parsing regexp: invalid repeat count: `{11}`"),
            ),
            ("(a{2})(b{2}){500}", "", Ok(None)),
            ("a{1000}", "", Ok(None)),
            ("a{1,1000}", "", Ok(None)),
            (
                "(?:a{0}){1001}",
                "",
                Err("error parsing regexp: invalid repeat count: `{1001}`"),
            ),
            (
                "(a)*{2}",
                "",
                Err("error parsing regexp: invalid nested repetition operator: `*{2}`"),
            ),
            ("a{2}?", "aaa", Ok(Some((0, 2)))),
            (
                "a{2}+",
                "",
                Err("error parsing regexp: invalid nested repetition operator: `{2}+`"),
            ),
            ("[^a]", "b", Ok(Some((0, 1)))),
            ("[]a]", "]", Ok(Some((0, 1)))),
            ("[a-]", "-", Ok(Some((0, 1)))),
            ("[a-z]", "q", Ok(Some((0, 1)))),
            (
                "[z-a]",
                "",
                Err("error parsing regexp: invalid character class range: `z-a`"),
            ),
            (
                r#"[a-\d]"#,
                "",
                Err(r#"error parsing regexp: invalid escape sequence: `\d`"#),
            ),
            ("[[:alpha:]]", "q", Ok(Some((0, 1)))),
            ("[[:^alpha:]]", "1", Ok(Some((0, 1)))),
            (
                "[[:foo:]]",
                "",
                Err("error parsing regexp: invalid character class range: `[:foo:]`"),
            ),
            ("[[a:]]", "a]", Ok(Some((0, 2)))),
            ("[a-a]", "a", Ok(Some((0, 1)))),
            (
                "x[a-",
                "",
                Err("error parsing regexp: missing closing ]: `[a-`"),
            ),
            (
                "(a{0,500}){0,3}",
                "",
                Err("error parsing regexp: invalid repeat count: `{0,3}`"),
            ),
            (
                "(a{0,500}){3,}",
                "",
                Err("error parsing regexp: invalid repeat count: `{3,}`"),
            ),
            (r"a{100}\x41{100}", "", Ok(None)),
            ("[[:]]", "x", Ok(None)),
            ("[[:alpha]", "[", Ok(Some((0, 1)))),
            (
                "[abc",
                "",
                Err("error parsing regexp: missing closing ]: `[abc`"),
            ),
            ("[", "", Err("error parsing regexp: missing closing ]: `[`")),
            ("[a&&b]", "&", Ok(Some((0, 1)))),
            ("[~~]", "~", Ok(Some((0, 1)))),
            ("[-a]", "-", Ok(Some((0, 1)))),
            (
                r#"[a\"#,
                "",
                Err("error parsing regexp: trailing backslash at end of expression: ``"),
            ),
            (
                "[a-",
                "",
                Err("error parsing regexp: missing closing ]: `[a-`"),
            ),
            (r#"[a-\x7A]"#, "z", Ok(Some((0, 1)))),
            (r#"[\n]"#, "\u{a}", Ok(Some((0, 1)))),
            (r#"[\x{41}-\x{43}]"#, "B", Ok(Some((0, 1)))),
            (
                "[^]",
                "]",
                Err("error parsing regexp: missing closing ]: `[^]`"),
            ),
            (
                "[]",
                "]",
                Err("error parsing regexp: missing closing ]: `[]`"),
            ),
            ("[a-b-c]", "-", Ok(Some((0, 1)))),
            ("[.+*?(){}^$#&~|]", "#", Ok(Some((0, 1)))),
            (r#"[\]]"#, "]", Ok(Some((0, 1)))),
            ("(a)", "a", Ok(Some((0, 1)))),
            ("(?:a)", "a", Ok(Some((0, 1)))),
            ("(?i)a", "A", Ok(Some((0, 1)))),
            ("(?i:a)b", "Ab", Ok(Some((0, 2)))),
            ("(?P<n>a)", "a", Ok(Some((0, 1)))),
            ("(?<n>a)", "a", Ok(Some((0, 1)))),
            (
                "(?P<>a)",
                "",
                Err("error parsing regexp: invalid named capture: `(?P<>`"),
            ),
            ("(?P<1a>a)", "a", Ok(Some((0, 1)))),
            (
                "(?P<n",
                "",
                Err("error parsing regexp: invalid named capture: `(?P<n`"),
            ),
            (
                "(?P<n-x>a)",
                "",
                Err("error parsing regexp: invalid named capture: `(?P<n-x>`"),
            ),
            (
                "(?-)",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?-)`"),
            ),
            ("(?i-m)a", "A", Ok(Some((0, 1)))),
            (
                "(?x)",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?x`"),
            ),
            (
                "(?",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?`"),
            ),
            ("(?)a", "a", Ok(Some((0, 1)))),
            ("a(?i)*", "aa", Ok(Some((0, 2)))),
            ("a(?i)(?s)+", "aa", Ok(Some((0, 2)))),
            (")", "", Err("error parsing regexp: unexpected ): `)`")),
            ("(", "", Err("error parsing regexp: missing closing ): `(`")),
            (
                "(?i)*",
                "",
                Err("error parsing regexp: missing argument to repetition operator: `*`"),
            ),
            ("(?-i:a)", "a", Ok(Some((0, 1)))),
            (
                "(?i-:a)",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?i-:`"),
            ),
            (
                "(?--i)",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?--`"),
            ),
            ("(?U)a+", "aa", Ok(Some((0, 1)))),
            ("(?s:.)", "\u{a}", Ok(Some((0, 1)))),
            ("(?m)^a$", "b\u{a}a", Ok(Some((2, 3)))),
            (
                "(?i",
                "",
                Err("error parsing regexp: invalid or unsupported Perl syntax: `(?i`"),
            ),
            ("}", "}", Ok(Some((0, 1)))),
            (
                "a|*",
                "",
                Err("error parsing regexp: missing argument to repetition operator: `*`"),
            ),
            ("^a.c$|x", "abc", Ok(Some((0, 3)))),
            ("a\u{1}", "a\u{1}", Ok(Some((0, 2)))),
            ("a#&~", "a#&~", Ok(Some((0, 4)))),
            ("(|a)*", "a", Ok(Some((0, 0)))),
            ("x(?i)", "", Ok(None)),
            (r#"\pN{1000}"#, "", Ok(None)),
            ("a|b{3}", "bbb", Ok(Some((0, 3)))),
            (r#"\Q\E"#, "x", Ok(Some((0, 0)))),
            (r#"a\Q\E(?i)*"#, "aa", Ok(Some((0, 2)))),
            (r#"[\x{D800}]"#, "a", Ok(None)),
            (r#"[a\x{D800}]"#, "a", Ok(Some((0, 1)))),
            (r#"[^\x{D800}]"#, "a", Ok(Some((0, 1)))),
            (r#"[\x{D7FF}-\x{E000}]"#, "\u{e000}", Ok(Some((0, 3)))),
            (r#"[\x{D800}-\x{DFFF}]"#, "-", Ok(None)),
            (r#"[\x{D800}-\x{DFFF}]"#, "\u{e000}", Ok(None)),
            (r#"[\x{D800}-\x{DFFF}]"#, "\u{d7ff}", Ok(None)),
            (
                r#"[\x{D800}-a]"#,
                "",
                Err(r#"error parsing regexp: invalid character class range: `\x{D800}-a`"#),
            ),
            (r#"a\x{DFFF}*"#, "a", Ok(Some((0, 1)))),
            (r#"[\x{DC00}-\x{E001}]"#, "\u{e001}", Ok(Some((0, 3)))),
            (r#"[\x{10FFFF}]"#, "\u{10ffff}", Ok(Some((0, 4)))),
        ];
        for (pattern, input, want) in cases {
            let got = compile(pattern).map(|r| r.find(input).map(|m| (m.start(), m.end())));
            assert_eq!(
                got,
                want.map_err(str::to_string),
                "pattern {pattern:?} on {input:?}"
            );
        }
        // Go compiles this; the regex crate's 10 MiB program limit stays, since a pattern from a
        // config file may not grow a pipeline's memory without bound: the refusal names the pattern.
        let big = r"(?i)\p{L}{1000}";
        assert_eq!(
            compile(big).err(),
            Some(format!(
                "error parsing regexp: Compiled regex exceeds size limit of 10485760 bytes.: `{big}`"
            ))
        );
    }
}
