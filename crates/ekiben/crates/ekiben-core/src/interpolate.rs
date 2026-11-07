use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("missing environment variables: {missing:?}")]
pub struct EnvLintError {
    pub missing: Vec<String>,
    pub best_attempt: String,
}

/// Replaces `${VAR}` and `${VAR:default}` in a configuration string, matching Bento's
/// `internal/config/env_*.go` and `website/docs/configuration/interpolation.md`.
///
/// Rules:
/// 1. `${{foo}}` is unescaped to the literal `${foo}` (and `${{!foo}}` is left as `${{!foo}}` for the Bloblang field parser).
/// 2. `${! ... }` is a Bloblang query interpolation and is left untouched.
/// 3. `${VAR:default}` resolves `VAR` via `lookup`; if not found, substitutes `default` (note: `${VAR:}` substitutes `""` with no lint error).
/// 4. `${VAR}` (without `:`) resolves `VAR` via `lookup`; if not found, records `VAR` as a missing env variable lint error and substitutes `""` in `best_attempt`.
pub fn interpolate_env_str<F>(input: &str, mut lookup: F) -> Result<String, EnvLintError>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = String::with_capacity(input.len());
    let mut missing = Vec::new();
    let mut pos = 0usize;

    while pos < input.len() {
        let rem = &input[pos..];

        // Keep `${{!...}}` for Bloblang field parser
        if let Some(body) = rem.strip_prefix("${{!") {
            if let Some(end_idx) = body.find("}}") {
                let take = 4 + end_idx + 2;
                out.push_str(&rem[..take]);
                pos += take;
                continue;
            }
        }

        // Escaped env var `${{foo}}` -> `${foo}`
        if let Some(body) = rem.strip_prefix("${{") {
            if let Some(end_idx) = body.find("}}") {
                out.push_str("${");
                out.push_str(&body[..end_idx]);
                out.push('}');
                pos += 3 + end_idx + 2;
                continue;
            }
        }

        // Bloblang query `${! ... }` -> leave untouched, respecting nested braces `{ ... }`
        if rem.starts_with("${!") {
            let mut depth = 1usize;
            let mut i = 3usize;
            let mut in_quotes = false;
            let mut escaped = false;
            let bytes = rem.as_bytes();
            while i < bytes.len() && depth > 0 {
                let b = bytes[i];
                if in_quotes {
                    if b == b'"' && !escaped {
                        in_quotes = false;
                    } else if b == b'\\' {
                        escaped = !escaped;
                    } else {
                        escaped = false;
                    }
                } else {
                    match b {
                        b'"' => in_quotes = true,
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                }
                i += 1;
            }
            out.push_str(&rem[..i]);
            pos += i;
            continue;
        }

        // Env var `${VAR}` or `${VAR:default}`
        if let Some(body) = rem.strip_prefix("${") {
            if let Some(close_idx) = body.find('}') {
                let inner = &body[..close_idx];
                let (var_name, default_opt) = match inner.split_once(':') {
                    Some((v, d)) => (v, Some(d)),
                    None => (inner, None),
                };
                if is_valid_env_var_name(var_name) {
                    if let Some(val) = lookup(var_name) {
                        out.push_str(&val);
                    } else if let Some(def) = default_opt {
                        out.push_str(def);
                    } else {
                        if !missing.contains(&var_name.to_string()) {
                            missing.push(var_name.to_string());
                        }
                    }
                    pos += 2 + close_idx + 1;
                    continue;
                }
            }
        }

        let ch = rem.chars().next().unwrap();
        out.push(ch);
        pos += ch.len_utf8();
    }

    if !missing.is_empty() {
        Err(EnvLintError {
            missing,
            best_attempt: out,
        })
    } else {
        Ok(out)
    }
}

pub fn interpolate_env_bytes<F>(input: &[u8], lookup: F) -> Result<Vec<u8>, EnvLintError>
where
    F: FnMut(&str) -> Option<String>,
{
    let s = String::from_utf8_lossy(input);
    interpolate_env_str(&s, lookup).map(String::into_bytes)
}

fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
