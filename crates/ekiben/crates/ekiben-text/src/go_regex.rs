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
                    return char::from_u32(r).map(Escape::Char).ok_or_else(|| bad(self));
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
                let hi = match self.class_char()? {
                    Ok(ch) => ch,
                    Err(_) => {
                        return Err(err(
                            "invalid character class range",
                            &self.src[item_start..self.pos],
                        ))
                    }
                };
                if hi < lo {
                    return Err(err(
                        "invalid character class range",
                        &self.src[item_start..self.pos],
                    ));
                }
                push_literal(out, lo);
                out.push('-');
                push_literal(out, hi);
            } else {
                push_literal(out, lo);
            }
        }
        out.push(']');
        Ok(())
    }

    /// One class member: a character, or (as `Err`) a nested set already in Rust syntax.
    fn class_char(&mut self) -> Result<Result<char, String>, String> {
        let Some(c) = self.bump() else {
            return Err(err("missing closing ]", self.src));
        };
        if c != '\\' {
            return Ok(Ok(c));
        }
        Ok(match self.escape(true)? {
            Escape::Char(ch) => Ok(ch),
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
                        if (min >= 2 || max.unwrap_or(u32::MAX) >= 2) && weight > 1000 {
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
                '}' => {
                    self.bump();
                    fold(&mut frames);
                    out.push_str("\\}");
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

    /// Go's `ReplaceAllString(src, template)`: `$1`, `${1}`, `$name`, `${name}` expand as
    /// `Regexp.Expand` does, `$$` is a `$`, a malformed `$` stays.
    pub fn replace_all_go(&self, src: &str, template: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut last = 0;
        for caps in self.re.captures_iter(src) {
            let Some(m) = caps.get(0) else { continue };
            out.push_str(&src[last..m.start()]);
            self.expand(&mut out, template, &caps);
            last = m.end();
        }
        out.push_str(&src[last..]);
        out
    }

    fn expand(&self, out: &mut String, template: &str, caps: &regex::Captures<'_>) {
        let mut t = template;
        while let Some(i) = t.find('$') {
            out.push_str(&t[..i]);
            t = &t[i + 1..];
            if let Some(rest) = t.strip_prefix('$') {
                out.push('$');
                t = rest;
                continue;
            }
            let Some((name, rest)) = extract(t) else {
                out.push('$');
                continue;
            };
            t = rest;
            let group = match group_number(name) {
                Some(n) => caps.get(n),
                None => self
                    .names
                    .iter()
                    .position(|n| n.as_deref() == Some(name))
                    .and_then(|i| caps.get(i)),
            };
            if let Some(g) = group {
                out.push_str(g.as_str());
            }
        }
        out.push_str(t);
    }
}

/// Go's `extract`: the name after a `$` (letters, digits, `_`; in braces or bare) and the rest.
fn extract(t: &str) -> Option<(&str, &str)> {
    let (body, brace) = match t.strip_prefix('{') {
        Some(b) => (b, true),
        None => (t, false),
    };
    let len = body
        .char_indices()
        .find(|&(_, c)| !(c.is_alphabetic() || c.is_numeric() || c == '_'))
        .map_or(body.len(), |(i, _)| i);
    if len == 0 {
        return None;
    }
    let name = &body[..len];
    let rest = &body[len..];
    if brace {
        return rest.strip_prefix('}').map(|r| (name, r));
    }
    Some((name, rest))
}

/// A group number as `extract` reads it: digits only, no leading zero, below 1e8.
fn group_number(name: &str) -> Option<usize> {
    if !name.bytes().all(|b| b.is_ascii_digit()) || (name.len() > 1 && name.starts_with('0')) {
        return None;
    }
    name.parse::<usize>().ok().filter(|&n| n < 100_000_000)
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
}
