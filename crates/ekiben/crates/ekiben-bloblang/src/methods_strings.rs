use std::collections::BTreeMap;
use std::hash::Hasher;
use std::io::{Read, Write};

use base64::Engine;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::Compression;
use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use regex::Regex;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use twox_hash::XxHash64;
use url::Url;

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::{get_arg, get_first_arg, get_optional_arg};
use crate::value::Value;

const MAX_REPEAT_BYTES: usize = 64 * 1024 * 1024; // 64 MiB safety cap

pub(crate) fn eval_string_family(
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    match name {
        "uppercase" => Some(match target_val {
            Value::String(s) => Ok(Value::String(s.to_uppercase())),
            Value::Bytes(b) => Ok(Value::Bytes(
                String::from_utf8_lossy(b).to_uppercase().into_bytes(),
            )),
            other => Err(format!("expected string value, got {}", other.type_name())),
        }),
        "lowercase" => Some(match target_val {
            Value::String(s) => Ok(Value::String(s.to_lowercase())),
            Value::Bytes(b) => Ok(Value::Bytes(
                String::from_utf8_lossy(b).to_lowercase().into_bytes(),
            )),
            other => Err(format!("expected string value, got {}", other.type_name())),
        }),
        "capitalize" => Some(match target_val {
            Value::String(s) => Ok(Value::String(capitalize_words(s))),
            Value::Bytes(b) => Ok(Value::Bytes(
                capitalize_words(&String::from_utf8_lossy(b)).into_bytes(),
            )),
            other => Err(format!("expected string value, got {}", other.type_name())),
        }),
        "trim" => Some((|| {
            let cutset = match get_optional_arg(args, "cutset", 0) {
                Some(e) => Some(ctx.eval_expr(e)?.get_string()?),
                None => None,
            };
            match target_val {
                Value::String(s) => Ok(Value::String(match &cutset {
                    Some(cs) => {
                        let chars: Vec<char> = cs.chars().collect();
                        s.trim_matches(&chars[..]).to_string()
                    }
                    None => s.trim().to_string(),
                })),
                Value::Bytes(b) => {
                    let s = String::from_utf8_lossy(b);
                    let trimmed = match &cutset {
                        Some(cs) => {
                            let chars: Vec<char> = cs.chars().collect();
                            s.trim_matches(&chars[..]).to_string()
                        }
                        None => s.trim().to_string(),
                    };
                    Ok(Value::Bytes(trimmed.into_bytes()))
                }
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "trim_prefix" => Some((|| {
            let prefix = ctx
                .eval_expr(get_first_arg(args, "prefix")?)?
                .get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::String(
                    s.strip_prefix(&prefix).unwrap_or(s).to_string(),
                )),
                Value::Bytes(b) => Ok(Value::Bytes(
                    b.strip_prefix(prefix.as_bytes()).unwrap_or(b).to_vec(),
                )),
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "trim_suffix" => Some((|| {
            let suffix = ctx
                .eval_expr(get_first_arg(args, "suffix")?)?
                .get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::String(
                    s.strip_suffix(&suffix).unwrap_or(s).to_string(),
                )),
                Value::Bytes(b) => Ok(Value::Bytes(
                    b.strip_suffix(suffix.as_bytes()).unwrap_or(b).to_vec(),
                )),
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "has_prefix" => Some((|| {
            let prefix = ctx.eval_expr(get_first_arg(args, "value")?)?.get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::Bool(s.starts_with(&prefix))),
                Value::Bytes(b) => Ok(Value::Bool(b.starts_with(prefix.as_bytes()))),
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "has_suffix" => Some((|| {
            let suffix = ctx.eval_expr(get_first_arg(args, "value")?)?.get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::Bool(s.ends_with(&suffix))),
                Value::Bytes(b) => Ok(Value::Bool(b.ends_with(suffix.as_bytes()))),
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "index_of" => Some((|| {
            let sub = ctx.eval_expr(get_first_arg(args, "value")?)?.get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::Int(s.find(&sub).map(|i| i as i64).unwrap_or(-1))),
                Value::Bytes(b) => {
                    let sub_b = sub.as_bytes();
                    let pos = if sub_b.is_empty() {
                        Some(0)
                    } else {
                        b.windows(sub_b.len()).position(|w| w == sub_b)
                    };
                    Ok(Value::Int(pos.map(|i| i as i64).unwrap_or(-1)))
                }
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "repeat" => Some((|| {
            let count = ctx.eval_expr(get_first_arg(args, "count")?)?.get_int()?;
            let n = count.max(0) as usize;
            match target_val {
                Value::String(s) => {
                    if s.len().saturating_mul(n) > MAX_REPEAT_BYTES {
                        return Err("repeat output exceeds maximum size cap (64 MiB)".to_string());
                    }
                    Ok(Value::String(s.repeat(n)))
                }
                Value::Bytes(b) => {
                    if b.len().saturating_mul(n) > MAX_REPEAT_BYTES {
                        return Err("repeat output exceeds maximum size cap (64 MiB)".to_string());
                    }
                    Ok(Value::Bytes(b.repeat(n)))
                }
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "reverse" => Some(match target_val {
            Value::String(s) => Ok(Value::String(s.chars().rev().collect())),
            Value::Bytes(b) => {
                let mut rev = b.clone();
                rev.reverse();
                Ok(Value::Bytes(rev))
            }
            other => Err(format!("expected string value, got {}", other.type_name())),
        }),
        "replace_all" | "replace" => Some((|| {
            let old = ctx.eval_expr(get_arg(args, "old", 0)?)?.get_string()?;
            let new = ctx.eval_expr(get_arg(args, "new", 1)?)?.get_string()?;
            match target_val {
                Value::String(s) => Ok(Value::String(s.replace(&old, &new))),
                Value::Bytes(b) => {
                    let s = String::from_utf8_lossy(b);
                    Ok(Value::Bytes(s.replace(&old, &new).into_bytes()))
                }
                other => Err(format!("expected string value, got {}", other.type_name())),
            }
        })()),
        "replace_all_many" | "replace_many" => Some((|| {
            let arr_val = ctx.eval_expr(get_first_arg(args, "values")?)?;
            let Value::Array(items) = arr_val else {
                return Err(format!("expected array value, got {}", arr_val.type_name()));
            };
            if items.len() % 2 != 0 {
                return Err(
                    "replacements should be in pairs and must therefore be even".to_string()
                );
            }
            let mut s = target_val.get_string()?;
            for [from, to] in items.as_chunks::<2>().0 {
                let from = from.get_string()?;
                let to = to.get_string()?;
                s = s.replace(&from, &to);
            }
            match target_val {
                Value::Bytes(_) => Ok(Value::Bytes(s.into_bytes())),
                _ => Ok(Value::String(s)),
            }
        })()),
        "escape_html" => Some((|| {
            let s = target_val.get_string()?;
            Ok(Value::String(escape_html_str(&s)))
        })()),
        "unescape_html" => Some((|| {
            let s = target_val.get_string()?;
            Ok(Value::String(unescape_html_str(&s)))
        })()),
        "strip_html" => Some((|| {
            let preserve: Vec<String> = match get_optional_arg(args, "preserve", 0) {
                Some(e) => match ctx.eval_expr(e)? {
                    Value::Array(arr) => {
                        arr.into_iter()
                            .map(|v| v.get_string())
                            .collect::<Result<Vec<_>, _>>()?
                    }
                    other => return Err(format!("expected array, got {}", other.type_name())),
                },
                None => Vec::new(),
            };
            let s = target_val.get_string()?;
            let stripped = strip_html_tags(&s, &preserve);
            match target_val {
                Value::Bytes(_) => Ok(Value::Bytes(stripped.into_bytes())),
                _ => Ok(Value::String(stripped)),
            }
        })()),
        "escape_url_query" => Some((|| {
            let s = target_val.get_string()?;
            Ok(Value::String(escape_url_query(&s)))
        })()),
        "unescape_url_query" => Some((|| {
            let s = target_val.get_string()?;
            unescape_url_query(&s).map(Value::String)
        })()),
        "quote" => Some((|| {
            let s = target_val.get_string()?;
            serde_json::to_string(&s)
                .map(Value::String)
                .map_err(|e| e.to_string())
        })()),
        "unquote" => Some((|| {
            let s = target_val.get_string()?;
            serde_json::from_str::<String>(&s)
                .map(Value::String)
                .map_err(|e| format!("invalid quoted string: {e}"))
        })()),
        "format" => Some((|| {
            let fmt_str = target_val.get_string()?;
            let mut vals = Vec::new();
            for e in args.exprs() {
                vals.push(ctx.eval_expr(e)?);
            }
            Ok(Value::String(crate::go_fmt::sprintf(&fmt_str, &vals)))
        })()),
        "re_match" => Some((|| {
            let pat = ctx
                .eval_expr(get_first_arg(args, "pattern")?)?
                .get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            Ok(Value::Bool(re.is_match(&s)))
        })()),
        "re_find_all" => Some((|| {
            let pat = ctx
                .eval_expr(get_first_arg(args, "pattern")?)?
                .get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            let matches: Vec<Value> = re
                .find_iter(&s)
                .map(|m| Value::String(m.as_str().to_string()))
                .collect();
            Ok(Value::Array(matches.into()))
        })()),
        "re_find_all_submatch" => Some((|| {
            let pat = ctx
                .eval_expr(get_first_arg(args, "pattern")?)?
                .get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            let mut out = Vec::new();
            for caps in re.captures_iter(&s) {
                let mut group = Vec::with_capacity(caps.len());
                for i in 0..caps.len() {
                    group.push(Value::String(
                        caps.get(i).map(|m| m.as_str()).unwrap_or("").to_string(),
                    ));
                }
                out.push(Value::Array(group.into()));
            }
            Ok(Value::Array(out.into()))
        })()),
        "re_find_object" => Some((|| {
            let pat = ctx
                .eval_expr(get_first_arg(args, "pattern")?)?
                .get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            let mut obj = BTreeMap::new();
            if let Some(caps) = re.captures(&s) {
                for (i, name_opt) in re.capture_names().enumerate() {
                    if let Some(m) = caps.get(i) {
                        let key = name_opt.map(String::from).unwrap_or_else(|| i.to_string());
                        obj.insert(key, Value::String(m.as_str().to_string()));
                    } else {
                        // In Go's FindStringSubmatch, an unmatched optional group in a matching regex is ""
                        let key = name_opt.map(String::from).unwrap_or_else(|| i.to_string());
                        obj.insert(key, Value::String(String::new()));
                    }
                }
            }
            Ok(Value::Object(obj.into()))
        })()),
        "re_find_all_object" => Some((|| {
            let pat = ctx
                .eval_expr(get_first_arg(args, "pattern")?)?
                .get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            let names: Vec<String> = re
                .capture_names()
                .enumerate()
                .map(|(i, n)| n.map(String::from).unwrap_or_else(|| i.to_string()))
                .collect();
            let mut out = Vec::new();
            for caps in re.captures_iter(&s) {
                let mut obj = BTreeMap::new();
                for (i, key) in names.iter().enumerate() {
                    let val = caps.get(i).map(|m| m.as_str()).unwrap_or("");
                    obj.insert(key.clone(), Value::String(val.to_string()));
                }
                out.push(Value::Object(obj.into()));
            }
            Ok(Value::Array(out.into()))
        })()),
        "re_replace_all" | "re_replace" => Some((|| {
            let pat = ctx.eval_expr(get_arg(args, "pattern", 0)?)?.get_string()?;
            let repl = ctx.eval_expr(get_arg(args, "value", 1)?)?.get_string()?;
            let re = go_regex(&pat)?;
            let s = target_val.get_string()?;
            // Convert Go `$1` syntax carefully so `$1:` doesn't get parsed by `regex` crate as `${1:}`:
            let rust_repl = convert_go_regex_replacement(&repl);
            Ok(Value::String(
                re.replace_all(&s, rust_repl.as_str()).into_owned(),
            ))
        })()),
        "encode" => Some((|| {
            let scheme = ctx
                .eval_expr(get_first_arg(args, "scheme")?)?
                .get_string()?;
            let bytes = match target_val {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Bytes(b) => b.clone(),
                other => return Err(format!("expected string value, got {}", other.type_name())),
            };
            let encoded = match scheme.as_str() {
                "base64" => base64::engine::general_purpose::STANDARD.encode(&bytes),
                "base64url" => base64::engine::general_purpose::URL_SAFE.encode(&bytes),
                "base64rawurl" => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
                "hex" => hex::encode(&bytes),
                "ascii85" => encode_ascii85(&bytes),
                other => return Err(format!("unrecognized encoding type: {other}")),
            };
            Ok(Value::String(encoded))
        })()),
        "decode" => Some((|| {
            let scheme = ctx
                .eval_expr(get_first_arg(args, "scheme")?)?
                .get_string()?;
            let s = target_val.get_string()?;
            let decoded = match scheme.as_str() {
                "base64" => base64::engine::general_purpose::STANDARD
                    .decode(s.trim())
                    .map_err(|e| e.to_string())?,
                "base64url" => base64::engine::general_purpose::URL_SAFE
                    .decode(s.trim())
                    .map_err(|e| e.to_string())?,
                "base64rawurl" => base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(s.trim())
                    .map_err(|e| e.to_string())?,
                "hex" => hex::decode(s.trim()).map_err(|e| e.to_string())?,
                "ascii85" => decode_ascii85(&s)?,
                other => return Err(format!("unrecognized encoding type: {other}")),
            };
            Ok(Value::Bytes(decoded))
        })()),
        "hash" => Some((|| {
            let algo = ctx
                .eval_expr(get_arg(args, "algorithm", 0)?)?
                .get_string()?;
            let key_opt = match get_optional_arg(args, "key", 1) {
                Some(e) => Some(ctx.eval_expr(e)?.get_string()?),
                None => None,
            };
            let poly = match get_optional_arg(args, "polynomial", 2) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => "IEEE".to_string(),
            };
            let bytes = match target_val {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Bytes(b) => b.clone(),
                other => return Err(format!("expected string value, got {}", other.type_name())),
            };
            let out_bytes: Vec<u8> = match algo.as_str() {
                "md5" => Md5::digest(&bytes).to_vec(),
                "sha1" => Sha1::digest(&bytes).to_vec(),
                "sha256" => Sha256::digest(&bytes).to_vec(),
                "sha512" => Sha512::digest(&bytes).to_vec(),
                "hmac_sha1" | "hmac-sha1" => {
                    let key = key_opt
                        .filter(|k| !k.is_empty())
                        .ok_or_else(|| format!("hash algorithm {algo} requires a key argument"))?;
                    let mut mac =
                        Hmac::<Sha1>::new_from_slice(key.as_bytes()).map_err(|e| e.to_string())?;
                    mac.update(&bytes);
                    mac.finalize().into_bytes().to_vec()
                }
                "hmac_sha256" | "hmac-sha256" => {
                    let key = key_opt
                        .filter(|k| !k.is_empty())
                        .ok_or_else(|| format!("hash algorithm {algo} requires a key argument"))?;
                    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
                        .map_err(|e| e.to_string())?;
                    mac.update(&bytes);
                    mac.finalize().into_bytes().to_vec()
                }
                "hmac_sha512" | "hmac-sha512" => {
                    let key = key_opt
                        .filter(|k| !k.is_empty())
                        .ok_or_else(|| format!("hash algorithm {algo} requires a key argument"))?;
                    let mut mac = Hmac::<Sha512>::new_from_slice(key.as_bytes())
                        .map_err(|e| e.to_string())?;
                    mac.update(&bytes);
                    mac.finalize().into_bytes().to_vec()
                }
                "xxhash64" => {
                    let mut h = XxHash64::with_seed(0);
                    h.write(&bytes);
                    h.finish().to_string().into_bytes()
                }
                "crc32" => {
                    let crc = match poly.as_str() {
                        "IEEE" => crc32fast::hash(&bytes),
                        "Castagnoli" => crc32_reflected(0x82f6_3b78, &bytes),
                        "Koopman" => crc32_reflected(0xeb31_d82e, &bytes),
                        _ => return Err(format!("unsupported crc32 polynomial {poly}")),
                    };
                    crc.to_be_bytes().to_vec()
                }
                "fnv32" => {
                    let mut h: u32 = 2166136261;
                    for &b in &bytes {
                        h ^= b as u32;
                        h = h.wrapping_mul(16777619);
                    }
                    h.to_string().into_bytes()
                }
                other => return Err(format!("unrecognized hash type: {other}")),
            };
            Ok(Value::Bytes(out_bytes))
        })()),
        "compress" => Some((|| {
            let algo = ctx
                .eval_expr(get_arg(args, "algorithm", 0)?)?
                .get_string()?;
            let bytes = target_val.to_bytes_coerced();
            let compressed = match algo.as_str() {
                "gzip" | "pgzip" => {
                    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes).map_err(|e| e.to_string())?;
                    enc.finish().map_err(|e| e.to_string())?
                }
                "zlib" => {
                    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes).map_err(|e| e.to_string())?;
                    enc.finish().map_err(|e| e.to_string())?
                }
                "flate" => {
                    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&bytes).map_err(|e| e.to_string())?;
                    enc.finish().map_err(|e| e.to_string())?
                }
                "lz4" => {
                    // pierrec/lz4 v4 writer defaults: 4 MB independent blocks, content checksum.
                    let mut info = lz4_flex::frame::FrameInfo::new();
                    info.block_size = lz4_flex::frame::BlockSize::Max4MB;
                    info.content_checksum = true;
                    let mut enc = lz4_flex::frame::FrameEncoder::with_frame_info(info, Vec::new());
                    enc.write_all(&bytes).map_err(|e| e.to_string())?;
                    enc.finish().map_err(|e| e.to_string())?
                }
                other => return Err(format!("unrecognized compression algorithm: {other}")),
            };
            Ok(Value::Bytes(compressed))
        })()),
        "decompress" => Some((|| {
            let algo = ctx
                .eval_expr(get_arg(args, "algorithm", 0)?)?
                .get_string()?;
            let bytes = target_val.to_bytes_coerced();
            let mut out = Vec::new();
            let max_unpacked = 64 * 1024 * 1024u64;
            match algo.as_str() {
                "gzip" | "pgzip" => {
                    let dec = GzDecoder::new(&bytes[..]);
                    dec.take(max_unpacked + 1)
                        .read_to_end(&mut out)
                        .map_err(|e| e.to_string())?;
                }
                "zlib" => {
                    let dec = ZlibDecoder::new(&bytes[..]);
                    dec.take(max_unpacked + 1)
                        .read_to_end(&mut out)
                        .map_err(|e| e.to_string())?;
                }
                "flate" => {
                    let dec = DeflateDecoder::new(&bytes[..]);
                    dec.take(max_unpacked + 1)
                        .read_to_end(&mut out)
                        .map_err(|e| e.to_string())?;
                }
                "lz4" => {
                    let dec = lz4_flex::frame::FrameDecoder::new(&bytes[..]);
                    dec.take(max_unpacked + 1)
                        .read_to_end(&mut out)
                        .map_err(|e| e.to_string())?;
                }
                other => return Err(format!("unrecognized decompression algorithm: {other}")),
            }
            if out.len() as u64 > max_unpacked {
                return Err("decompressed data exceeds safety limit (64 MiB)".to_string());
            }
            Ok(Value::Bytes(out))
        })()),
        "parse_json" => Some((|| {
            let s = target_val.get_string()?;
            let jv: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| format!("failed to parse value as JSON: {e}"))?;
            Ok(Value::from_json_value(jv))
        })()),
        "format_json" => Some((|| {
            let indent = match get_optional_arg(args, "indent", 0) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => "    ".to_string(),
            };
            let no_indent = match get_optional_arg(args, "no_indent", 1) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => false,
            };
            let escape_html = match get_optional_arg(args, "escape_html", 2) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => true,
            };
            let formatted = target_val
                .to_go_json_with(escape_html, (!no_indent).then_some(indent.as_str()))
                .ok_or_else(|| "json: unsupported value: NaN or Inf".to_string())?
                .into_bytes();
            Ok(Value::Bytes(formatted))
        })()),
        "parse_yaml" => Some((|| {
            let s = target_val.get_string()?;
            let jv: serde_json::Value = serde_norway::from_str(&s)
                .map_err(|e| format!("failed to parse value as YAML: {e}"))?;
            Ok(Value::from_json_value(jv))
        })()),
        "format_yaml" => Some((|| {
            let jv = target_val.to_json_value();
            let s = serde_norway::to_string(&jv).map_err(|e| e.to_string())?;
            Ok(Value::Bytes(s.into_bytes()))
        })()),
        "parse_csv" => Some((|| {
            let parse_header_row = match get_optional_arg(args, "parse_header_row", 0) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => true,
            };
            let delim_str = match get_optional_arg(args, "delimiter", 1) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => ",".to_string(),
            };
            let mut chars = delim_str.chars();
            let delim_ch = chars
                .next()
                .ok_or_else(|| "delimiter value must be exactly one character".to_string())?;
            if chars.next().is_some() || !delim_ch.is_ascii() {
                return Err("delimiter value must be a single ASCII character".to_string());
            }
            let lazy_quotes = match get_optional_arg(args, "lazy_quotes", 2) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => false,
            };
            let s = target_val.get_string()?;
            let mut rdr = csv::ReaderBuilder::new()
                .has_headers(false)
                .delimiter(delim_ch as u8)
                .flexible(lazy_quotes)
                .from_reader(s.as_bytes());
            let mut all_rows: Vec<Vec<String>> = Vec::new();
            for rec in rdr.records() {
                let rec = rec.map_err(|e| e.to_string())?;
                all_rows.push(rec.iter().map(String::from).collect());
            }
            if all_rows.is_empty() {
                return Err("zero records were parsed".to_string());
            }
            if parse_header_row {
                let headers = &all_rows[0];
                if headers.is_empty() {
                    return Err("no headers found on first row".to_string());
                }
                let mut out = Vec::with_capacity(all_rows.len() - 1);
                for (row_idx, row) in all_rows[1..].iter().enumerate() {
                    if row.len() != headers.len() {
                        return Err(format!(
                            "record on line {row_idx}: record mismatch with headers"
                        ));
                    }
                    let mut obj = BTreeMap::new();
                    for (h, val) in headers.iter().zip(row.iter()) {
                        obj.insert(h.clone(), Value::String(val.clone()));
                    }
                    out.push(Value::Object(obj.into()));
                }
                Ok(Value::Array(out.into()))
            } else {
                Ok(Value::Array(
                    all_rows
                        .into_iter()
                        .map(|r| Value::Array(r.into_iter().map(Value::String).collect()))
                        .collect(),
                ))
            }
        })()),
        "parse_xml" => Some((|| {
            let cast = match get_optional_arg(args, "cast", 0) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => false,
            };
            let s = target_val.get_string()?;
            parse_xml_to_value(s.as_bytes(), cast)
        })()),
        "format_xml" => Some((|| {
            let indent = match get_optional_arg(args, "indent", 0) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => "    ".to_string(),
            };
            let no_indent = match get_optional_arg(args, "no_indent", 1) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => false,
            };
            let Value::Object(map) = target_val else {
                return Err(format!(
                    "expected object value, got {}",
                    target_val.type_name()
                ));
            };
            let eff_indent = (!no_indent).then_some(indent.as_str());
            Ok(Value::Bytes(
                format_value_to_xml(map, eff_indent, 0).into_bytes(),
            ))
        })()),
        "parse_url" => Some((|| {
            let s = target_val.get_string()?;
            go_url_escapes_valid(&s)?;
            let parsed = Url::parse(&s).map_err(|e| e.to_string())?;
            let mut obj = BTreeMap::new();
            obj.insert(
                "scheme".to_string(),
                Value::String(parsed.scheme().to_string()),
            );
            obj.insert("opaque".to_string(), Value::String(String::new()));
            let host = match (parsed.host_str(), parsed.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                _ => String::new(),
            };
            obj.insert("host".to_string(), Value::String(host));
            obj.insert("path".to_string(), Value::String(parsed.path().to_string()));
            obj.insert("raw_path".to_string(), Value::String(String::new()));
            obj.insert(
                "raw_query".to_string(),
                Value::String(parsed.query().unwrap_or("").to_string()),
            );
            obj.insert(
                "fragment".to_string(),
                Value::String(parsed.fragment().unwrap_or("").to_string()),
            );
            obj.insert("raw_fragment".to_string(), Value::String(String::new()));
            if !parsed.username().is_empty() || parsed.password().is_some() {
                let mut user = BTreeMap::new();
                user.insert(
                    "name".to_string(),
                    Value::String(parsed.username().to_string()),
                );
                if let Some(pw) = parsed.password() {
                    user.insert("password".to_string(), Value::String(pw.to_string()));
                }
                obj.insert("user".to_string(), Value::Object(user.into()));
            }
            Ok(Value::Object(obj.into()))
        })()),
        "parse_form_url_encoded" => Some((|| {
            let s = target_val.get_string()?;
            let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (k, v) in url::form_urlencoded::parse(s.as_bytes()) {
                groups
                    .entry(k.into_owned())
                    .or_default()
                    .push(v.into_owned());
            }
            let mut out = BTreeMap::new();
            for (k, mut vals) in groups {
                if vals.len() == 1 {
                    out.insert(k, Value::String(vals.remove(0)));
                } else {
                    out.insert(
                        k,
                        Value::Array(vals.into_iter().map(Value::String).collect()),
                    );
                }
            }
            Ok(Value::Object(out.into()))
        })()),
        "filepath_join" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut path = std::path::PathBuf::new();
            for (i, item) in arr.iter().enumerate() {
                let s = item
                    .get_string()
                    .map_err(|e| format!("path element {i}: {e}"))?;
                path.push(s);
            }
            Ok(Value::String(path.to_string_lossy().into_owned()))
        })()),
        "filepath_split" => Some((|| {
            let s = target_val.get_string()?;
            match s.rfind(std::path::MAIN_SEPARATOR) {
                Some(idx) => Ok(Value::Array(
                    vec![
                        Value::String(s[..=idx].to_string()),
                        Value::String(s[idx + 1..].to_string()),
                    ]
                    .into(),
                )),
                None => Ok(Value::Array(
                    vec![Value::String(String::new()), Value::String(s)].into(),
                )),
            }
        })()),
        "slug" => Some((|| {
            let s = target_val.get_string()?;
            let lang = match get_optional_arg(args, "lang", 0) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => "en".to_string(),
            };
            Ok(Value::String(make_slug(&s, &lang)))
        })()),
        _ => None,
    }
}

/// Go `url.Parse` refuses a `%` not followed by two hex digits before the query, with
/// `parse "<url without fragment>": invalid URL escape "<at most 3 bytes>"`.
fn go_url_escapes_valid(raw: &str) -> Result<(), String> {
    let no_frag = raw.split_once('#').map_or(raw, |(u, _)| u);
    let before_query = no_frag.split_once('?').map_or(no_frag, |(u, _)| u);
    let bytes = before_query.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let hex = |j: usize| bytes.get(j).is_some_and(u8::is_ascii_hexdigit);
        if b == b'%' && !(hex(i + 1) && hex(i + 2)) {
            let bad: String = before_query[i..].chars().take(3).collect();
            return Err(format!(
                "parse {}: invalid URL escape {}",
                go_quote(no_frag),
                go_quote(&bad)
            ));
        }
    }
    Ok(())
}

/// Go `strconv.Quote` for printable text: `"` and `\` escaped.
fn go_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// CRC-32 over a reflected polynomial (Go `crc32.MakeTable(poly)` + `crc32.Checksum`).
fn crc32_reflected(poly: u32, data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ poly
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Bento's `capitalize`: `cases.Title(language.English)` of golang.org/x/text. The first cased
/// letter of a word is title-cased, the word's other cased letters lower-cased. Words follow x/text's
/// approximation of Unicode word breaks: letters, digits, `_` and combining marks never break, one
/// apostrophe, `.` or `:` between letters does not ("O'neil", "X.y"), anything else does
/// ("Poppeli/Haapa", "Mustikkamaa-Korkeasaari").
fn capitalize_words(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut mid_word = false;
    for (i, &ch) in chars.iter().enumerate() {
        let kind = word_kind(ch);
        match kind {
            WordKind::Cased if !mid_word => {
                title_case_into(ch, &mut out);
                mid_word = true;
            }
            WordKind::Cased if ch == 'Σ' => {
                // Final sigma: ς unless a cased letter follows.
                let next_cased = chars[i + 1..]
                    .iter()
                    .find(|c| word_kind(**c) != WordKind::Mark)
                    .is_some_and(|c| word_kind(*c) == WordKind::Cased);
                out.push(if next_cased { 'σ' } else { 'ς' });
            }
            WordKind::Cased => out.extend(ch.to_lowercase()),
            WordKind::Break => {
                out.push(ch);
                mid_word = false;
            }
            WordKind::Letter | WordKind::Mark | WordKind::Mid => out.push(ch),
        }
        if kind == WordKind::Mid
            && chars
                .get(i + 1)
                .is_some_and(|c| word_kind(*c) == WordKind::Mid)
        {
            mid_word = false;
        }
    }
    out
}

#[derive(PartialEq, Clone, Copy)]
enum WordKind {
    Cased,
    /// Uncased Word_Break ALetter, Numeric or ExtendNumLet.
    Letter,
    /// Extend and Format: combining marks, joiners.
    Mark,
    /// MidLetter, MidNumLet and Single_Quote.
    Mid,
    Break,
}

fn word_kind(c: char) -> WordKind {
    let cp = c as u32;
    if c.is_lowercase() || c.is_uppercase() || matches!(c, 'ǅ' | 'ǈ' | 'ǋ' | 'ǲ') {
        return WordKind::Cased;
    }
    match cp {
        0x27 | 0x2E | 0x3A | 0xB7 | 0x387 | 0x55F | 0x5F4 | 0x2018 | 0x2019 | 0x2024 | 0x2027
        | 0xFE13 | 0xFE52 | 0xFE55 | 0xFF07 | 0xFF0E | 0xFF1A => WordKind::Mid,
        0x300..=0x36F
        | 0x483..=0x489
        | 0x591..=0x5BD
        | 0x610..=0x61A
        | 0x64B..=0x65F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
        | 0xAD
        | 0x200C
        | 0x200D
        | 0x2060..=0x2064
        | 0xFEFF => WordKind::Mark,
        0x5F | 0x202F | 0x203F | 0x2040 | 0x2054 | 0xFE33 | 0xFE34 | 0xFE4D..=0xFE4F | 0xFF3F => {
            WordKind::Letter
        }
        // Han, kana and Hangul syllables are Word_Break Other or Katakana: a break.
        0x3040..=0x30FF
        | 0x3400..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0x20000..=0x3FFFF => WordKind::Break,
        _ if c.is_alphanumeric() => WordKind::Letter,
        _ => WordKind::Break,
    }
}

fn title_case_into(c: char, out: &mut String) {
    match c {
        'Ǆ' | 'ǅ' | 'ǆ' => out.push('ǅ'),
        'Ǉ' | 'ǈ' | 'ǉ' => out.push('ǈ'),
        'Ǌ' | 'ǋ' | 'ǌ' => out.push('ǋ'),
        'Ǳ' | 'ǲ' | 'ǳ' => out.push('ǲ'),
        'ß' => out.push_str("Ss"),
        'ﬀ' => out.push_str("Ff"),
        'ﬁ' => out.push_str("Fi"),
        'ﬂ' => out.push_str("Fl"),
        _ => out.extend(c.to_uppercase()),
    }
}

pub fn escape_html_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Go's `html.UnescapeString`: every WHATWG named reference (a legacy name may drop its `;`,
/// longest match first), decimal and hex references with Windows-1252 for 0x80..0x9F and U+FFFD
/// for 0, surrogates and past U+10FFFF; anything else stays as written.
pub fn unescape_html_str(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    while let Some(off) = s[at..].find('&') {
        out.push_str(&s[at..at + off]);
        at += off;
        at += unescape_entity(&b[at..], &mut out);
    }
    out.push_str(&s[at..]);
    out
}

fn entity_text(name: &[u8]) -> Option<&'static str> {
    let list = crate::html_entities::ENTITIES;
    list.binary_search_by(|(k, _)| k.as_bytes().cmp(name))
        .ok()
        .map(|i| list[i].1)
}

/// Go's `unescapeEntity` at an `&` (`s[0]`): writes what it decodes and answers the bytes read.
fn unescape_entity(s: &[u8], out: &mut String) -> usize {
    const LONGEST_WITHOUT_SEMICOLON: usize = 6;
    if s.len() <= 1 {
        out.push('&');
        return 1;
    }
    let mut i = 1;
    if s[i] == b'#' {
        if s.len() <= 3 {
            out.push('&');
            return 1;
        }
        i += 1;
        let hex = matches!(s[i], b'x' | b'X');
        if hex {
            i += 1;
        }
        let mut x: u32 = 0;
        while i < s.len() {
            let c = s[i];
            i += 1;
            let digit = match c {
                b'0'..=b'9' => Some(u32::from(c - b'0')),
                b'a'..=b'f' if hex => Some(u32::from(c - b'a') + 10),
                b'A'..=b'F' if hex => Some(u32::from(c - b'A') + 10),
                _ => None,
            };
            if let Some(d) = digit {
                // Go's rune arithmetic wraps; saturating keeps an overlong number past U+10FFFF.
                x = x
                    .saturating_mul(if hex { 16 } else { 10 })
                    .saturating_add(d);
                continue;
            }
            if c != b';' {
                i -= 1;
            }
            break;
        }
        if i <= 3 {
            out.push('&');
            return 1;
        }
        let c = match x {
            0x80..=0x9F => crate::html_entities::WINDOWS_1252[(x - 0x80) as usize],
            _ => char::from_u32(x).filter(|_| x != 0).unwrap_or('\u{FFFD}'),
        };
        out.push(c);
        return i;
    }
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c.is_ascii_alphanumeric() {
            continue;
        }
        if c != b';' {
            i -= 1;
        }
        break;
    }
    let name = &s[1..i];
    if !name.is_empty() {
        if let Some(text) = entity_text(name) {
            out.push_str(text);
            return i;
        }
        let longest = (name.len() - 1).min(LONGEST_WITHOUT_SEMICOLON);
        for j in (2..=longest).rev() {
            if let Some(text) = entity_text(&name[..j]) {
                out.push_str(text);
                return j + 1;
            }
        }
    }
    // Only ASCII was read, so this is a char boundary.
    out.push_str(std::str::from_utf8(&s[..i]).unwrap_or_default());
    i
}

/// Strips HTML tags while matching `bluemonday`'s behavior:
/// preserves tags in `preserve`, strips all other `<...>` tags, and re-escapes bare `<` / `>` / `&` in text!
fn strip_html_tags(input: &str, preserve: &[String]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '<' {
            let rem = &input[idx..];
            if let Some(end_offset) = rem.find('>') {
                let tag_full = &rem[..=end_offset];
                let inner = rem[1..end_offset].trim();
                let inner_no_slash = inner.strip_prefix('/').unwrap_or(inner).trim();
                let tag_name: String = inner_no_slash
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if !tag_name.is_empty() {
                    for _ in 0..tag_full.chars().count() - 1 {
                        chars.next();
                    }
                    if preserve.iter().any(|p| p.eq_ignore_ascii_case(&tag_name)) {
                        out.push_str(tag_full);
                    }
                    continue;
                }
            }
            out.push_str("&lt;");
        } else {
            out.push(ch);
        }
    }
    out
}

/// A pattern compiled as Go's RE2 reads it, from a small per-thread cache: Bento compiles a literal
/// pattern once at parse, so a `re_match` inside a `filter` must not compile per element.
/// A shared handle: cloning a `Regex` starts a fresh match cache, which a pattern with counted
/// repeats rebuilds per call (helsinki/traffic-volumes spent 190 µs a line).
fn go_regex(pattern: &str) -> Result<std::sync::Arc<Regex>, String> {
    // One cache for the process, not one per thread: a `Regex` is shared across threads (it pools
    // its own scratch space), and per-thread copies of every pattern held 109 MiB of compiled
    // programs in jc's runner on the prod rehearsal (jc-memory-peak).
    static CACHE: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Regex>>>,
    > = std::sync::LazyLock::new(Default::default);
    // ponytail: the cache empties when full; an LRU only pays off for mappings with > 256 patterns.
    const MAX_CACHED: usize = 256;
    let cache = || {
        CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    if let Some(re) = cache().get(pattern).cloned() {
        return Ok(re);
    }
    let re = std::sync::Arc::new(Regex::new(&go_regex_syntax(pattern)).map_err(|e| e.to_string())?);
    let mut c = cache();
    if c.len() >= MAX_CACHED {
        c.clear();
    }
    Ok(c.entry(pattern.to_string()).or_insert(re).clone())
}

/// Go's `\d`, `\s`, `\w` and `\b` are ASCII (`\s` is `[\t\n\f\r ]`); the regex crate's are Unicode,
/// so a no-break space matched `\s+`. Rewrites those escapes to the ASCII classes Go means.
fn go_regex_syntax(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('d') => out.push_str(if in_class { "0-9" } else { "[0-9]" }),
                Some('D') => out.push_str("[^0-9]"),
                Some('s') => out.push_str(if in_class {
                    r"\t\n\f\r "
                } else {
                    r"[\t\n\f\r ]"
                }),
                Some('S') => out.push_str(r"[^\t\n\f\r ]"),
                Some('w') => out.push_str(if in_class {
                    "0-9A-Za-z_"
                } else {
                    "[0-9A-Za-z_]"
                }),
                Some('W') => out.push_str("[^0-9A-Za-z_]"),
                Some('b') if !in_class => out.push_str(r"(?-u:\b)"),
                Some('B') if !in_class => out.push_str(r"(?-u:\B)"),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    out.push('^');
                    chars.next();
                }
                // A `]` first in a class is a literal.
                if chars.peek() == Some(&']') {
                    out.push_str(r"\]");
                    chars.next();
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Go's `url.QueryEscape`: letters, digits and `-_.~` stay, a space is `+`, every other byte is
/// `%XX` (form_urlencoded keeps `*` and escapes `~`, the other way round).
fn escape_url_query(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn unescape_url_query(s: &str) -> Result<String, String> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(format!("invalid URL escape {:?}", &s[i..]));
                }
                let hex_slice = &s[i + 1..i + 3];
                let byte = u8::from_str_radix(hex_slice, 16)
                    .map_err(|_| format!("invalid URL escape \"%{}%\"", hex_slice))?;
                out.push(byte);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// Converts Go `regexp.ReplaceAllString` replacement syntax (`$1`, `${1}`, `$$`)
/// into Rust `regex` crate replacement syntax (`${1}`, `$$`) so `$1:` is not misread as `${1:}`.
fn convert_go_regex_replacement(repl: &str) -> String {
    let mut out = String::with_capacity(repl.len() + 8);
    let chars: Vec<char> = repl.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            if chars[i + 1] == '$' {
                out.push_str("$$");
                i += 2;
                continue;
            }
            if chars[i + 1] == '{' {
                out.push('$');
                i += 1;
                continue;
            }
            // Collect alphanumeric group name or number
            let start = i + 1;
            let mut end = start;
            if chars[start].is_ascii_digit() {
                while end < chars.len() && chars[end].is_ascii_digit() {
                    end += 1;
                }
            } else if chars[start].is_ascii_alphabetic() || chars[start] == '_' {
                while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
                {
                    end += 1;
                }
            }
            if end > start {
                let name: String = chars[start..end].iter().collect();
                out.push_str(&format!("${{{name}}}"));
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parses XML bytes into a `Value::Object` as Bento does through mxj v2.7.0 `NewMapXml`
/// (`xmlToMapParser`): every attribute becomes `-<local name>` (`xmlns:gml` is `-gml`), each text
/// token is trimmed of `\t\r\b\n ` and the last non-empty one wins, text beside attributes or
/// children goes under `#text`, an empty element is `""` and repeated children become an array.
/// DTD and external entities (`<!DOCTYPE`, `<!ENTITY`) are refused for security (XXE defense).
pub fn parse_xml_to_value(xml_bytes: &[u8], cast: bool) -> Result<Value, String> {
    let raw_str =
        std::str::from_utf8(xml_bytes).map_err(|e| format!("failed to parse value as XML: {e}"))?;
    if raw_str.contains("<!DOCTYPE") || raw_str.contains("<!ENTITY") {
        return Err(
            "failed to parse value as XML: DTD and external entities are forbidden".to_string(),
        );
    }
    // Pre-quote unquoted attribute values like `<number id=99>` which Go's mxj accepts:
    let normalized = quote_unquoted_xml_attrs(raw_str);
    let mut reader = quick_xml::Reader::from_str(&normalized);
    reader.config_mut().trim_text(false);

    let mut stack: Vec<XmlFrame> = Vec::new();
    // Go's decoder hands text and the entities in it as one CharData token.
    let mut pending = String::new();
    let mut root: Option<(String, Value)> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("failed to parse value as XML: {e}"))?;
        match &event {
            quick_xml::events::Event::Text(e) => {
                pending.push_str(e.as_ref());
                continue;
            }
            quick_xml::events::Event::GeneralRef(r) => {
                let entity = format!("&{};", AsRef::<str>::as_ref(r));
                // A non-strict decoder keeps an entity it does not know as text.
                match quick_xml::escape::unescape(&entity) {
                    Ok(t) => pending.push_str(&t),
                    Err(_) => pending.push_str(&entity),
                }
                continue;
            }
            _ => {}
        }
        if let Some(top) = stack.last_mut() {
            top.char_data(&pending, cast);
        }
        pending.clear();
        let closed = match event {
            quick_xml::events::Event::Start(e) => {
                stack.push(XmlFrame::open(&e, cast));
                None
            }
            quick_xml::events::Event::Empty(e) => Some(XmlFrame::open(&e, cast)),
            quick_xml::events::Event::CData(e) => {
                if let Some(top) = stack.last_mut() {
                    top.char_data(&e.into_inner(), cast);
                }
                None
            }
            quick_xml::events::Event::End(_) => stack.pop(),
            quick_xml::events::Event::DocType(_) => {
                return Err("failed to parse value as XML: DOCTYPE is forbidden".to_string());
            }
            quick_xml::events::Event::Eof => break,
            _ => None,
        };
        if let Some(frame) = closed {
            let (name, val) = frame.close();
            match stack.last_mut() {
                Some(parent) => parent.add_child(name, val),
                None => {
                    root = Some((name, val));
                    break;
                }
            }
        }
    }

    let Some((name, val)) = root else {
        return Err("failed to parse value as XML: unclosed or empty document".to_string());
    };
    let mut root_map = BTreeMap::new();
    root_map.insert(name, val);
    Ok(Value::Object(root_map.into()))
}

/// One element while mxj parses it: `n[skey]` (its own text) and `na` (attributes and children).
struct XmlFrame {
    name: String,
    text: Option<Value>,
    na: BTreeMap<String, Value>,
}

impl XmlFrame {
    fn open(e: &quick_xml::events::BytesStart<'_>, cast: bool) -> Self {
        let mut na = BTreeMap::new();
        for attr in e.attributes().with_checks(false).flatten() {
            let v = quick_xml::escape::unescape(&attr.value)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| attr.value.to_string());
            na.insert(
                format!("-{}", local_xml_name(attr.key.as_ref())),
                cast_xml_scalar(&v, cast),
            );
        }
        XmlFrame {
            name: local_xml_name(e.name().as_ref()),
            text: None,
            na,
        }
    }

    /// One Go `xml.CharData` token.
    fn char_data(&mut self, raw: &str, cast: bool) {
        // Go's decoder reads `\r\n` and a lone `\r` in text as `\n`.
        let raw = if raw.contains('\r') {
            std::borrow::Cow::Owned(raw.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            std::borrow::Cow::Borrowed(raw)
        };
        let t = raw.trim_matches(['\t', '\r', '\u{8}', '\n', ' ']);
        if t.is_empty() {
            return;
        }
        if self.na.is_empty() {
            self.text = Some(cast_xml_scalar(t, cast));
        } else {
            self.na
                .insert("#text".to_string(), cast_xml_scalar(t, cast));
        }
    }

    fn add_child(&mut self, key: String, val: Value) {
        match self.na.remove(&key) {
            None => {
                self.na.insert(key, val);
            }
            Some(Value::Array(mut arr)) => {
                arr.push(val);
                self.na.insert(key, Value::Array(arr));
            }
            Some(existing) => {
                self.na
                    .insert(key, Value::Array(vec![existing, val].into()));
            }
        }
    }

    fn close(self) -> (String, Value) {
        let XmlFrame { name, text, mut na } = self;
        let val = match text {
            None if na.is_empty() => Value::String(String::new()),
            None => Value::Object(na.into()),
            Some(t) if na.is_empty() => t,
            Some(t) => {
                na.insert("#text".to_string(), t);
                Value::Object(na.into())
            }
        };
        (name, val)
    }
}

fn local_xml_name(qname: &str) -> String {
    qname.rsplit(':').next().unwrap_or(qname).to_string()
}

/// mxj `cast` with Bento's defaults (floats and bools, no ints): NaN and infinities stay text.
fn cast_xml_scalar(s: &str, cast: bool) -> Value {
    if !cast || matches!(s.to_ascii_lowercase().as_str(), "nan" | "inf" | "-inf") {
        return Value::String(s.to_string());
    }
    if let Ok(f) = s.parse::<f64>() {
        return Value::Float(f);
    }
    if (1..6).contains(&s.len()) && s.starts_with(['t', 'T', 'f', 'F']) {
        if let Ok(b) = crate::value::parse_go_bool(s) {
            return Value::Bool(b);
        }
    }
    Value::String(s.to_string())
}

fn quote_unquoted_xml_attrs(xml: &str) -> String {
    match go_regex(r#"(\s[a-zA-Z_:][a-zA-Z0-9_.:-]*)=([^"'\s><]+)"#) {
        Ok(re) => re.replace_all(xml, r#"$1="$2""#).into_owned(),
        Err(_) => xml.to_string(),
    }
}

/// mxj `Xml()` (indent `None`) or `XmlIndent("", indent)`; an empty indent still breaks lines.
fn format_value_to_xml(
    map: &BTreeMap<String, Value>,
    indent: Option<&str>,
    depth: usize,
) -> String {
    let mut out = String::new();
    let pad = indent.unwrap_or_default().repeat(depth);
    let nl = if indent.is_some() { "\n" } else { "" };
    for (k, v) in map {
        if k.starts_with('-') || k == "#text" {
            continue;
        }
        match v {
            Value::Object(child_map) => {
                if !out.is_empty() {
                    out.push_str(nl);
                }
                out.push_str(&format!("{pad}<{k}>"));
                let inner = format_value_to_xml(child_map, indent, depth + 1);
                if !inner.is_empty() {
                    out.push_str(nl);
                    out.push_str(&inner);
                    out.push_str(nl);
                    out.push_str(&format!("{pad}</{k}>"));
                } else if let Some(txt) = child_map.get("#text") {
                    out.push_str(&escape_html_str(&txt.to_string_coerced()));
                    out.push_str(&format!("</{k}>"));
                } else {
                    out.push_str(&format!("</{k}>"));
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    let mut single = BTreeMap::new();
                    single.insert(k.clone(), item.clone());
                    let rendered = format_value_to_xml(&single, indent, depth);
                    if !out.is_empty() {
                        out.push_str(nl);
                    }
                    out.push_str(&rendered);
                }
            }
            scalar => {
                if !out.is_empty() {
                    out.push_str(nl);
                }
                out.push_str(&format!(
                    "{pad}<{k}>{}</{k}>",
                    escape_html_str(&scalar.to_string_coerced())
                ));
            }
        }
    }
    out
}

fn encode_ascii85(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(4) {
        let mut word: u32 = 0;
        for i in 0..4 {
            word = (word << 8) | (*chunk.get(i).unwrap_or(&0) as u32);
        }
        if chunk.len() == 4 && word == 0 {
            out.push('z');
            continue;
        }
        let mut digits = [0u8; 5];
        let mut rem = word;
        for i in (0..5).rev() {
            digits[i] = (rem % 85) as u8 + 33;
            rem /= 85;
        }
        for &d in &digits[..chunk.len() + 1] {
            out.push(d as char);
        }
    }
    out
}

fn decode_ascii85(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut group = Vec::with_capacity(5);
    for b in input.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(format!("illegal ascii85 byte: {b}"));
        }
        group.push((b - 33) as u64);
        if group.len() == 5 {
            let mut val = 0u64;
            for &d in &group {
                val = val * 85 + d;
            }
            let word = u32::try_from(val).map_err(|_| "ascii85 overflow".to_string())?;
            out.extend_from_slice(&word.to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        if group.len() == 1 {
            return Err("corrupt ascii85 input".to_string());
        }
        let orig_len = group.len();
        while group.len() < 5 {
            group.push(84);
        }
        let mut val = 0u64;
        for &d in &group {
            val = val * 85 + d;
        }
        let word = u32::try_from(val).map_err(|_| "ascii85 overflow".to_string())?;
        let bytes = word.to_be_bytes();
        out.extend_from_slice(&bytes[..orig_len - 1]);
    }
    Ok(out)
}

/// `&` and `@` words of gosimple/slug v1.13.1 (languages_substitution.go) per language;
/// an unknown language falls back to English, as `slug.MakeLang` does.
fn slug_words(lang: &str) -> (&'static str, Option<&'static str>) {
    match lang.to_ascii_lowercase().as_str() {
        "cs" | "ces" => ("a", Some("zavinac")),
        "de" | "deu" => ("und", Some("an")),
        "es" | "spa" => ("y", Some("en")),
        "fi" | "fin" => ("ja", Some("at")),
        "fr" | "fra" => ("et", Some("arobase")),
        "gr" | "el" | "ell" => ("kai", None),
        "id" | "idn" | "ind" => ("dan", None),
        "it" | "ita" => ("e", Some("chiocciola")),
        "kz" | "kk" | "kaz" => ("jane", None),
        "nb" | "nob" => ("og", Some("at")),
        "nl" | "nld" => ("en", Some("at")),
        "pl" | "pol" => ("i", Some("na")),
        "ro" | "rou" => ("si", None),
        "sl" | "slv" => ("in", None),
        "sv" | "swe" => ("och", Some("snabel a")),
        "tr" | "tur" => ("ve", Some("et")),
        _ => ("and", Some("at")),
    }
}

/// gosimple/slug `MakeLang`: language substitutions, ASCII folding, lowercase, every run of
/// characters outside `[a-z0-9_-]` to one dash, trimmed of `-` and `_`.
/// ponytail: folds Latin diacritics only (no full unidecode table); other scripts drop out.
fn make_slug(s: &str, lang: &str) -> String {
    let (and, at) = slug_words(lang);
    let german = matches!(lang.to_ascii_lowercase().as_str(), "de" | "deu");
    let mut subst = String::with_capacity(s.len());
    for ch in s.trim().chars() {
        match ch {
            '"' | '\'' | '’' => {}
            '‒' | '–' | '—' | '―' => subst.push('-'),
            '&' => subst.push_str(and),
            '@' => subst.push_str(at.unwrap_or("@")),
            'ä' | 'Ä' if german => subst.push_str("ae"),
            'ö' | 'Ö' if german => subst.push_str("oe"),
            'ü' | 'Ü' if german => subst.push_str("ue"),
            c => subst.push(c),
        }
    }
    let mut out = String::new();
    for ch in subst.chars() {
        let mut ascii = [0u8; 4];
        let folded = if ch.is_ascii() {
            &*ch.encode_utf8(&mut ascii)
        } else {
            fold_diacritic(ch)
        };
        for fc in folded.chars() {
            let fc = fc.to_ascii_lowercase();
            if fc.is_ascii_alphanumeric() || fc == '_' || fc == '-' {
                if fc != '-' || !out.ends_with('-') {
                    out.push(fc);
                }
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    out.trim_matches(|c| c == '-' || c == '_').to_string()
}

fn fold_diacritic(c: char) -> &'static str {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'Á' | 'À' | 'Â' | 'Ä' => "a",
        'é' | 'è' | 'ê' | 'ë' | 'ě' | 'É' | 'È' | 'Ê' | 'Ë' | 'Ě' => "e",
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => "i",
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'Ó' | 'Ò' | 'Ô' | 'Ö' => "o",
        'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'Ú' | 'Ù' | 'Û' | 'Ü' | 'Ů' => "u",
        'ý' | 'ÿ' | 'Ý' => "y",
        'č' | 'ć' | 'ç' | 'Č' | 'Ć' | 'Ç' => "c",
        'ď' | 'Ď' => "d",
        'ľ' | 'ĺ' | 'ł' | 'Ľ' | 'Ĺ' | 'Ł' => "l",
        'ň' | 'ń' | 'ñ' | 'Ň' | 'Ń' | 'Ñ' => "n",
        'ř' | 'ŕ' | 'Ř' | 'Ŕ' => "r",
        'š' | 'ś' | 'Š' | 'Ś' => "s",
        'ť' | 'Ť' => "t",
        'ž' | 'ź' | 'ż' | 'Ž' | 'Ź' | 'Ż' => "z",
        'ß' => "ss",
        _ => "",
    }
}

#[cfg(test)]
mod go_regex_tests {
    #[test]
    fn every_thread_shares_one_compiled_pattern() {
        let pattern = r"go-regex-shared-\d+";
        let here = super::go_regex(pattern).expect("compiles");
        let there = std::thread::spawn(move || super::go_regex(pattern).expect("compiles"))
            .join()
            .expect("the thread ends");
        assert!(std::sync::Arc::ptr_eq(&here, &there));
        assert!(there.is_match("go-regex-shared-42"));
    }
}
