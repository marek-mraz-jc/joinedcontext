//! JSON text as Go's `encoding/json` writes it (what Bento prints through gabs): sorted
//! keys, HTML characters and U+2028/U+2029 escaped, Go float formatting, RFC 3339 nano
//! timestamps, base64 bytes, and `null` for a value Go refuses (NaN, infinity).

use crate::time_format::format_rfc3339_nano;
use crate::value::Value;
use base64::Engine;
use std::fmt::Write;

impl Value {
    /// `gabs.Wrap(v).String()`: compact, HTML escaped, `null` when Go's marshal fails.
    pub fn to_go_json(&self) -> String {
        self.to_go_json_with(true, None)
            .unwrap_or_else(|| "null".to_string())
    }

    /// Like [`Value::to_go_json`] without the HTML escapes: the bytes Bento writes for a message
    /// whose content a mapping set (its encoder runs with `SetEscapeHTML(false)`).
    pub fn to_go_json_unescaped(&self) -> String {
        self.to_go_json_with(false, None)
            .unwrap_or_else(|| "null".to_string())
    }

    /// `json.Marshal` (indent `None`) or `json.MarshalIndent(v, "", indent)`; `None` where
    /// Go answers `json: unsupported value` (NaN or infinity anywhere in the value).
    pub fn to_go_json_with(&self, escape_html: bool, indent: Option<&str>) -> Option<String> {
        let mut out = String::new();
        write_value(
            &mut out,
            self,
            escape_html,
            indent.filter(|i| !i.is_empty()),
            0,
        )?;
        Some(out)
    }
}

fn newline(out: &mut String, indent: &str, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str(indent);
    }
}

fn write_value(
    out: &mut String,
    v: &Value,
    html: bool,
    indent: Option<&str>,
    depth: usize,
) -> Option<()> {
    match v {
        Value::Null | Value::Delete | Value::Nothing => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Uint(u) => {
            let _ = write!(out, "{u}");
        }
        Value::Float(f) => out.push_str(&go_json_float(*f)?),
        // json.Number marshals as its own text.
        Value::Number(t) => out.push_str(t),
        Value::String(s) => write_string(out, s, html),
        Value::Bytes(b) => write_string(
            out,
            &base64::engine::general_purpose::STANDARD.encode(b),
            html,
        ),
        Value::Timestamp(ts) => write_string(out, &format_rfc3339_nano(*ts), html),
        Value::Array(items) => {
            let items: Vec<&Value> = items
                .iter()
                .filter(|v| !matches!(v, Value::Delete | Value::Nothing))
                .collect();
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                if let Some(ind) = indent {
                    newline(out, ind, depth + 1);
                }
                write_value(out, item, html, indent, depth + 1)?;
            }
            if let (Some(ind), false) = (indent, items.is_empty()) {
                newline(out, ind, depth);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut first = true;
            for (k, item) in map {
                if matches!(item, Value::Delete | Value::Nothing) {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                if let Some(ind) = indent {
                    newline(out, ind, depth + 1);
                }
                write_string(out, k, html);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, html, indent, depth + 1)?;
            }
            if let (Some(ind), false) = (indent, first) {
                newline(out, ind, depth);
            }
            out.push('}');
        }
    }
    Some(())
}

fn write_string(out: &mut String, s: &str, html: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' if html => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `encoding/json` float64: shortest digits, plain between 1e-6 and 1e21, else Go's `e`
/// form with `e-09` cleaned to `e-9`; `None` for NaN and infinities, which Go refuses.
fn go_json_float(f: f64) -> Option<String> {
    f.is_finite().then(|| crate::value::format_json_float(f))
}

/// Decodes one JSON document as Bento decodes message content (`json.Decoder` with `UseNumber`):
/// a number keeps its text as [`Value::Number`] when Go would print the parsed value differently.
/// Answers `None` for anything it does not take (a syntax error, invalid UTF-8, nesting past
/// serde_json's 128 levels, trailing data), so the caller falls back to serde_json and its errors.
pub fn decode_with_numbers(raw: &[u8]) -> Option<Value> {
    let mut p = Decoder { raw, at: 0 };
    let v = p.value(0)?;
    p.ws();
    (p.at == raw.len()).then_some(v)
}

/// serde_json's default recursion limit; deeper input takes the serde_json path and its error.
const MAX_DEPTH: usize = 128;

struct Decoder<'a> {
    raw: &'a [u8],
    at: usize,
}

impl Decoder<'_> {
    fn ws(&mut self) {
        while matches!(self.raw.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> Option<()> {
        self.raw
            .get(self.at..)?
            .starts_with(lit)
            .then(|| self.at += lit.len())
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth >= MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.raw.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut map = std::collections::BTreeMap::new();
                self.ws();
                if self.eat(b"}").is_some() {
                    return Some(Value::Object(map.into()));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    self.eat(b":")?;
                    let v = self.value(depth + 1)?;
                    map.insert(key, v);
                    self.ws();
                    if self.eat(b",").is_none() {
                        self.eat(b"}")?;
                        return Some(Value::Object(map.into()));
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat(b"]").is_some() {
                    return Some(Value::Array(items.into()));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b",").is_none() {
                        self.eat(b"]")?;
                        return Some(Value::Array(items.into()));
                    }
                }
            }
            b'"' => self.string().map(Value::String),
            b't' => self.eat(b"true").map(|()| Value::Bool(true)),
            b'f' => self.eat(b"false").map(|()| Value::Bool(false)),
            b'n' => self.eat(b"null").map(|()| Value::Null),
            _ => self.number(),
        }
    }

    fn string(&mut self) -> Option<String> {
        let start = self.at;
        if self.raw.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut escaped = false;
        loop {
            match *self.raw.get(self.at)? {
                b'"' => break,
                b'\\' => {
                    escaped = true;
                    self.at += 2;
                }
                _ => self.at += 1,
            }
        }
        self.at += 1;
        let lit = self.raw.get(start..self.at)?;
        if escaped {
            serde_json::from_slice::<String>(lit).ok()
        } else {
            let body = std::str::from_utf8(lit.get(1..lit.len() - 1)?).ok()?;
            // A raw control character is a syntax error in JSON.
            (!body.bytes().any(|b| b < 0x20)).then(|| body.to_string())
        }
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.at;
        self.eat(b"-");
        let digits = |p: &mut Self| {
            let s = p.at;
            while p.raw.get(p.at).is_some_and(u8::is_ascii_digit) {
                p.at += 1;
            }
            p.at - s
        };
        let int_start = self.at;
        let n = digits(self);
        if n == 0 || (n > 1 && self.raw[int_start] == b'0') {
            return None;
        }
        if self.eat(b".").is_some() && digits(self) == 0 {
            return None;
        }
        if matches!(self.raw.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.raw.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if digits(self) == 0 {
                return None;
            }
        }
        let text = std::str::from_utf8(&self.raw[start..self.at]).ok()?;
        Some(number_from_text(text))
    }
}

/// The value a JSON number text decodes to: `Int` or `Float` when Go prints that value back as the
/// same text, otherwise the text itself as [`Value::Number`].
pub fn number_from_text(text: &str) -> Value {
    if let Ok(i) = text.parse::<i64>() {
        if i.to_string() == text {
            return Value::Int(i);
        }
    } else if let Ok(f) = text.parse::<f64>() {
        if !text.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            && go_json_float(f).as_deref() == Some(text)
        {
            return Value::Float(f);
        }
    }
    Value::Number(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_the_text_go_would_not_print() {
        let v = decode_with_numbers(
            br#"{"a":5.0,"b":1E2,"c":10,"d":5.5,"e":-0,"f":12345678901234567890123,"g":[0.10]}"#,
        )
        .expect("decodes");
        assert_eq!(
            v.to_go_json(),
            r#"{"a":5.0,"b":1E2,"c":10,"d":5.5,"e":-0,"f":12345678901234567890123,"g":[0.10]}"#
        );
        let Value::Object(m) = &v else {
            panic!("object")
        };
        assert!(matches!(m["c"], Value::Int(10)));
        assert!(matches!(m["d"], Value::Float(f) if f == 5.5));
        assert!(matches!(m["a"].sanitized(), Value::Float(f) if f == 5.0));
        assert!(matches!(m["f"].sanitized(), Value::Float(_)));
        for bad in [
            &b"{\"a\":01}"[..],
            b"[1,]",
            b"{} {}",
            b"\"a\tb\"",
            b"1.",
            b"-",
        ] {
            assert!(
                decode_with_numbers(bad).is_none(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        let text = "{\"s\":\"\\u00e9\\n\",\"t\":\"\u{e9}\"}";
        assert_eq!(
            decode_with_numbers(text.as_bytes()).map(|v| v.to_go_json()),
            Some("{\"s\":\"\u{e9}\\n\",\"t\":\"\u{e9}\"}".to_string())
        );
    }

    #[test]
    fn go_floats_and_escapes() {
        assert_eq!(go_json_float(2.0).as_deref(), Some("2"));
        assert_eq!(go_json_float(1e21).as_deref(), Some("1e+21"));
        assert_eq!(go_json_float(1.5e-7).as_deref(), Some("1.5e-7"));
        assert_eq!(go_json_float(0.000001).as_deref(), Some("0.000001"));
        assert_eq!(go_json_float(f64::NAN), None);
        assert_eq!(
            Value::String("a<&>\u{2028}\u{1}".into()).to_go_json(),
            r#""a\u003c\u0026\u003e\u2028\u0001""#
        );
    }
}
