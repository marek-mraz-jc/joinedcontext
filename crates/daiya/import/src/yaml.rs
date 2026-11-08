#![forbid(unsafe_code)]
//! Self-contained indentation-based YAML subset parser for Grafana provisioning YAML files
//! (`datasources.yaml`, `dashboardproviders.yaml` v0 and v1).

use daiya_model::{CrError, CrResult};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
struct Line<'a> {
    /// 1-based line number in the file, for errors (which never quote a line: it may hold a
    /// credential).
    number: usize,
    indent: usize,
    content: &'a str,
}

fn preprocess_lines(input: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for (idx, raw) in input.lines().enumerate() {
        let stripped = strip_yaml_comment(raw).trim_end();
        if stripped.trim().is_empty() || stripped.trim() == "---" {
            continue;
        }
        let indent = stripped.len() - stripped.trim_start().len();
        out.push(Line {
            number: idx + 1,
            indent,
            content: stripped.trim_start(),
        });
    }
    out
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single
                && !in_double
                && (idx == 0 || line.as_bytes()[idx - 1].is_ascii_whitespace()) =>
            {
                return &line[..idx];
            }
            _ => {}
        }
    }
    line
}

/// Parses a YAML provisioning document into a `serde_json::Value`.
pub fn parse_yaml_to_json(input: &str) -> CrResult<Value> {
    let trimmed = input.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str(trimmed)
            .map_err(|e| CrError::BadRequest(format!("invalid JSON/YAML: {e}")));
    }
    let lines = preprocess_lines(input);
    if lines.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let mut pos = 0usize;
    let value = parse_block(&lines, &mut pos, lines[0].indent)?;
    // Whatever the grammar did not consume is an error, never silently dropped configuration.
    if let Some(rest) = lines.get(pos) {
        return Err(CrError::BadRequest(format!(
            "invalid YAML: unexpected content at line {} (indentation {})",
            rest.number, rest.indent
        )));
    }
    Ok(value)
}

fn is_sequence_item(content: &str) -> bool {
    content.starts_with("- ") || content == "-"
}

fn parse_block(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    if *pos >= lines.len() {
        return Ok(Value::Null);
    }
    if is_sequence_item(lines[*pos].content) {
        parse_sequence(lines, pos, base_indent)
    } else {
        parse_mapping(lines, pos, base_indent)
    }
}

fn parse_sequence(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    let mut arr = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent != base_indent || !is_sequence_item(line.content) {
            break;
        }
        let after_dash = line.content[1..].trim_start();
        // The column of the item's first key: its sibling keys align with it.
        let item_indent = line.indent + (line.content.len() - after_dash.len());
        *pos += 1;

        if after_dash.is_empty() {
            if *pos < lines.len() && lines[*pos].indent > line.indent {
                let child_indent = lines[*pos].indent;
                arr.push(parse_block(lines, pos, child_indent)?);
            } else {
                arr.push(Value::Null);
            }
        } else if let Some((k, v)) = split_key_value(after_dash) {
            let mut map = Map::new();
            let key = unquote_key(k)?;
            let val = if v.is_empty() {
                nested_value(lines, pos, item_indent)?
            } else {
                parse_scalar(v)?
            };
            map.insert(key, val);

            while *pos < lines.len() && lines[*pos].indent >= item_indent {
                let sub_line = &lines[*pos];
                if sub_line.indent > item_indent {
                    return Err(CrError::BadRequest(format!(
                        "invalid YAML: unexpected indentation at line {}",
                        sub_line.number
                    )));
                }
                if is_sequence_item(sub_line.content) {
                    break;
                }
                let Some((sk, sv)) = split_key_value(sub_line.content) else {
                    break;
                };
                let sub_indent = sub_line.indent;
                let sub_key = unquote_key(sk)?;
                *pos += 1;
                let sub_val = if sv.is_empty() {
                    nested_value(lines, pos, sub_indent)?
                } else {
                    parse_scalar(sv)?
                };
                map.insert(sub_key, sub_val);
            }
            arr.push(Value::Object(map));
        } else {
            arr.push(parse_scalar(after_dash)?);
        }
    }
    Ok(Value::Array(arr))
}

fn parse_mapping(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    let mut map = Map::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < base_indent || is_sequence_item(line.content) {
            break;
        }
        if line.indent > base_indent {
            return Err(CrError::BadRequest(format!(
                "invalid YAML: unexpected indentation at line {}",
                line.number
            )));
        }
        let Some((k, v)) = split_key_value(line.content) else {
            return Err(CrError::BadRequest(format!(
                "invalid YAML mapping line {}: expected 'key: value'",
                line.number
            )));
        };
        let cur_indent = line.indent;
        let key = unquote_key(k)?;
        *pos += 1;
        let val = if v.is_empty() {
            nested_value(lines, pos, cur_indent)?
        } else {
            parse_scalar(v)?
        };
        map.insert(key, val);
    }
    Ok(Value::Object(map))
}

/// The value of a key written with nothing after its colon: a block indented below the key,
/// a sequence written at the key's own indentation (`key:\n- a`, as YAML allows), or null.
fn nested_value(lines: &[Line<'_>], pos: &mut usize, key_indent: usize) -> CrResult<Value> {
    match lines.get(*pos) {
        Some(next) if next.indent > key_indent => {
            let child_indent = next.indent;
            parse_block(lines, pos, child_indent)
        }
        Some(next) if next.indent == key_indent && is_sequence_item(next.content) => {
            parse_sequence(lines, pos, key_indent)
        }
        _ => Ok(Value::Null),
    }
}

/// A mapping key; YAML merge keys (`<<`) need anchors, which this subset does not have.
fn unquote_key(k: &str) -> CrResult<String> {
    if k == "<<" {
        return Err(CrError::BadRequest(
            "YAML merge keys ('<<') are not supported in provisioning files".into(),
        ));
    }
    Ok(unquote(k))
}

fn split_key_value(s: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in s.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                let rest = &s[idx + 1..];
                if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                    return Some((s[..idx].trim(), rest.trim()));
                }
            }
            _ => {}
        }
    }
    None
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len() - 1].to_owned()
    } else {
        t.to_owned()
    }
}

/// Decodes a double-quoted YAML scalar's escapes (`\\`, `\"`, `\n`, `\t`, `\r`, `\/`, `\0`,
/// `\uXXXX`).
fn decode_double_quoted(inner: &str) -> CrResult<String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let ch = u32::from_str_radix(&hex, 16)
                    .ok()
                    .filter(|_| hex.len() == 4)
                    .and_then(char::from_u32)
                    .ok_or_else(|| {
                        CrError::BadRequest(format!("invalid YAML escape '\\u{hex}'"))
                    })?;
                out.push(ch);
            }
            other => {
                return Err(CrError::BadRequest(format!(
                    "invalid YAML escape '\\{}'",
                    other.map(String::from).unwrap_or_default()
                )));
            }
        }
    }
    Ok(out)
}

fn parse_scalar(s: &str) -> CrResult<Value> {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return decode_double_quoted(&t[1..t.len() - 1]).map(Value::String);
    }
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        // In single quotes the only escape is a doubled quote.
        return Ok(Value::String(t[1..t.len() - 1].replace("''", "'")));
    }
    if t.starts_with('[') || t.starts_with('{') {
        // Flow collections are read as JSON; anything else would be stored as a string and
        // silently lose its structure (a `secureJsonData: {password: x}` would slip past the
        // raw-credential refusal).
        return serde_json::from_str::<Value>(t).map_err(|e| {
            CrError::BadRequest(format!(
                "invalid YAML: a flow collection must be written as JSON: {e}"
            ))
        });
    }
    if let Some(first) = t.chars().next() {
        let what = match first {
            '&' => Some("anchors"),
            '*' => Some("aliases"),
            '!' => Some("tags"),
            '|' | '>' => Some("block scalars"),
            _ => None,
        };
        if let Some(what) = what {
            return Err(CrError::BadRequest(format!(
                "YAML {what} are not supported in provisioning files"
            )));
        }
    }
    Ok(match t {
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        "null" | "Null" | "~" => Value::Null,
        _ => {
            if let Ok(i) = t.parse::<i64>() {
                Value::from(i)
            } else if let Ok(f) = t.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map_or_else(|| Value::String(t.to_owned()), Value::Number)
            } else {
                Value::String(t.to_owned())
            }
        }
    })
}
