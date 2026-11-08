#![forbid(unsafe_code)]
//! Self-contained indentation-based YAML subset parser for Grafana provisioning YAML files
//! (`datasources.yaml`, `dashboardproviders.yaml` v0 and v1).

use daiya_model::{CrError, CrResult};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
struct Line<'a> {
    indent: usize,
    content: &'a str,
}

fn preprocess_lines(input: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for raw in input.lines() {
        let stripped = strip_yaml_comment(raw).trim_end();
        if stripped.trim().is_empty() || stripped.trim() == "---" {
            continue;
        }
        let indent = stripped.len() - stripped.trim_start().len();
        out.push(Line {
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
    parse_block(&lines, &mut pos, lines[0].indent)
}

fn parse_block(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    if *pos >= lines.len() {
        return Ok(Value::Null);
    }
    if lines[*pos].content.starts_with("- ") || lines[*pos].content == "-" {
        parse_sequence(lines, pos, base_indent)
    } else {
        parse_mapping(lines, pos, base_indent)
    }
}

fn parse_sequence(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    let mut arr = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < base_indent {
            break;
        }
        if !line.content.starts_with('-') {
            break;
        }
        let after_dash = line.content[1..].trim_start();
        let item_indent = line.indent + 2;
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
            let val = if v.is_empty() {
                if *pos < lines.len() && lines[*pos].indent >= item_indent {
                    let child_indent = lines[*pos].indent;
                    parse_block(lines, pos, child_indent)?
                } else {
                    Value::Null
                }
            } else {
                parse_scalar(v)
            };
            map.insert(unquote(k), val);

            while *pos < lines.len() && lines[*pos].indent >= item_indent {
                if lines[*pos].indent == line.indent && lines[*pos].content.starts_with('-') {
                    break;
                }
                let sub_line = &lines[*pos];
                let Some((sk, sv)) = split_key_value(sub_line.content) else {
                    break;
                };
                let sub_indent = sub_line.indent;
                *pos += 1;
                let sub_val = if sv.is_empty() {
                    if *pos < lines.len() && lines[*pos].indent > sub_indent {
                        let next_indent = lines[*pos].indent;
                        parse_block(lines, pos, next_indent)?
                    } else {
                        Value::Object(Map::new())
                    }
                } else {
                    parse_scalar(sv)
                };
                map.insert(unquote(sk), sub_val);
            }
            arr.push(Value::Object(map));
        } else {
            arr.push(parse_scalar(after_dash));
        }
    }
    Ok(Value::Array(arr))
}

fn parse_mapping(lines: &[Line<'_>], pos: &mut usize, base_indent: usize) -> CrResult<Value> {
    let mut map = Map::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < base_indent {
            break;
        }
        if line.content.starts_with('-') {
            break;
        }
        let Some((k, v)) = split_key_value(line.content) else {
            return Err(CrError::BadRequest(format!(
                "invalid YAML mapping line: '{}'",
                line.content
            )));
        };
        let cur_indent = line.indent;
        *pos += 1;
        let val = if v.is_empty() {
            if *pos < lines.len() && lines[*pos].indent > cur_indent {
                let child_indent = lines[*pos].indent;
                parse_block(lines, pos, child_indent)?
            } else {
                Value::Object(Map::new())
            }
        } else {
            parse_scalar(v)
        };
        map.insert(unquote(k), val);
    }
    Ok(Value::Object(map))
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

fn parse_scalar(s: &str) -> Value {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')) {
        return Value::String(unquote(t));
    }
    if (t.starts_with('[') || t.starts_with('{'))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return v;
    }
    match t {
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
    }
}
