//! MessagePack as vmihailenco/msgpack v5 writes and reads it for Bento's `format_msgpack`
//! and `parse_msgpack`: compact integers (unsigned codes for non-negative values), float64,
//! str8/16/32, bin for bytes. Bento's map key order is Go's random map order; keys are
//! written sorted here. ponytail: timestamps are written as RFC 3339 strings and the
//! timestamp extension is refused on read; add ext -1 when a pipeline needs it.

use std::collections::BTreeMap;

use crate::time_format::format_rfc3339_nano;
use crate::value::Value;

/// Nesting limit, as serde_json applies to JSON.
const MAX_DEPTH: usize = 128;

pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, v);
    out
}

fn len_header(out: &mut Vec<u8>, len: usize, fix: Option<(u8, usize)>, codes: [u8; 3]) {
    match fix {
        Some((base, max)) if len < max => out.push(base | len as u8),
        _ if len <= 0xff && codes[0] != 0 => out.extend([codes[0], len as u8]),
        _ if len <= 0xffff => {
            out.push(codes[1]);
            out.extend((len as u16).to_be_bytes());
        }
        _ => {
            out.push(codes[2]);
            out.extend((len as u32).to_be_bytes());
        }
    }
}

fn write_uint(out: &mut Vec<u8>, u: u64) {
    if u <= 0x7f {
        out.push(u as u8);
    } else if u <= 0xff {
        out.extend([0xcc, u as u8]);
    } else if u <= 0xffff {
        out.push(0xcd);
        out.extend((u as u16).to_be_bytes());
    } else if u <= 0xffff_ffff {
        out.push(0xce);
        out.extend((u as u32).to_be_bytes());
    } else {
        out.push(0xcf);
        out.extend(u.to_be_bytes());
    }
}

fn write_int(out: &mut Vec<u8>, i: i64) {
    if let Ok(u) = u64::try_from(i) {
        write_uint(out, u);
    } else if i >= -32 {
        out.push(i as u8);
    } else if i >= i64::from(i8::MIN) {
        out.extend([0xd0, i as u8]);
    } else if i >= i64::from(i16::MIN) {
        out.push(0xd1);
        out.extend((i as i16).to_be_bytes());
    } else if i >= i64::from(i32::MIN) {
        out.push(0xd2);
        out.extend((i as i32).to_be_bytes());
    } else {
        out.push(0xd3);
        out.extend(i.to_be_bytes());
    }
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    len_header(out, s.len(), Some((0xa0, 32)), [0xd9, 0xda, 0xdb]);
    out.extend_from_slice(s.as_bytes());
}

fn write(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Number(_) => write(out, &v.sanitized()),
        Value::Null | Value::Nothing | Value::Delete => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Int(i) => write_int(out, *i),
        Value::Uint(u) => write_uint(out, *u),
        Value::Float(f) => {
            out.push(0xcb);
            out.extend(f.to_bits().to_be_bytes());
        }
        Value::String(s) => write_str(out, s),
        Value::Timestamp(t) => write_str(out, &format_rfc3339_nano(*t)),
        Value::Bytes(b) => {
            len_header(out, b.len(), None, [0xc4, 0xc5, 0xc6]);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            len_header(out, items.len(), Some((0x90, 16)), [0, 0xdc, 0xdd]);
            items.iter().for_each(|i| write(out, i));
        }
        Value::Object(map) => {
            len_header(out, map.len(), Some((0x80, 16)), [0, 0xde, 0xdf]);
            for (k, item) in map {
                write_str(out, k);
                write(out, item);
            }
        }
    }
}

pub fn decode(input: &[u8]) -> Result<Value, String> {
    let mut r = Reader { buf: input, pos: 0 };
    let v = r.value(0)?;
    Ok(v)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len());
        let end = end.ok_or_else(|| "msgpack: unexpected EOF".to_string())?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn uint(&mut self, n: usize) -> Result<u64, String> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }

    fn int(&mut self, n: usize) -> Result<i64, String> {
        let u = self.uint(n)?;
        let shift = 64 - 8 * n as u32;
        Ok(((u << shift) as i64) >> shift)
    }

    /// A length header must fit in what is left of the input (one byte per element at
    /// least), so a forged length cannot reserve memory.
    fn len(&mut self, n: usize) -> Result<usize, String> {
        let len = usize::try_from(self.uint(n)?).map_err(|_| "msgpack: length overflow")?;
        if len > self.buf.len() - self.pos {
            return Err("msgpack: unexpected EOF".to_string());
        }
        Ok(len)
    }

    fn string(&mut self, len: usize) -> Result<Value, String> {
        let raw = self.take(len)?;
        Ok(Value::String(String::from_utf8_lossy(raw).into_owned()))
    }

    fn array(&mut self, len: usize, depth: usize) -> Result<Value, String> {
        (0..len)
            .map(|_| self.value(depth + 1))
            .collect::<Result<_, _>>()
            .map(Value::Array)
    }

    fn map(&mut self, len: usize, depth: usize) -> Result<Value, String> {
        let mut out = BTreeMap::new();
        for _ in 0..len {
            let key = match self.value(depth + 1)? {
                Value::String(s) => s,
                other => other.to_string_coerced(),
            };
            out.insert(key, self.value(depth + 1)?);
        }
        Ok(Value::Object(out.into()))
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("msgpack: exceeded max depth".to_string());
        }
        let code = self.take(1)?[0];
        Ok(match code {
            0x00..=0x7f => Value::Int(i64::from(code)),
            0x80..=0x8f => self.map(usize::from(code & 0x0f), depth)?,
            0x90..=0x9f => self.array(usize::from(code & 0x0f), depth)?,
            0xa0..=0xbf => self.string(usize::from(code & 0x1f))?,
            0xc0 => Value::Null,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4..=0xc6 => {
                let n = self.len(1 << (code - 0xc4))?;
                Value::Bytes(self.take(n)?.to_vec())
            }
            0xca => Value::Float(f64::from(f32::from_bits(self.uint(4)? as u32))),
            0xcb => Value::Float(f64::from_bits(self.uint(8)?)),
            0xcc..=0xcf => {
                let u = self.uint(1 << (code - 0xcc))?;
                i64::try_from(u).map_or(Value::Uint(u), Value::Int)
            }
            0xd0..=0xd3 => Value::Int(self.int(1 << (code - 0xd0))?),
            0xd9..=0xdb => {
                let n = self.len(1 << (code - 0xd9))?;
                self.string(n)?
            }
            0xdc | 0xdd => {
                let n = self.len(2 << (code - 0xdc))?;
                self.array(n, depth)?
            }
            0xde | 0xdf => {
                let n = self.len(2 << (code - 0xde))?;
                self.map(n, depth)?
            }
            0xe0..=0xff => Value::Int(i64::from(code as i8)),
            other => return Err(format!("msgpack: unsupported code {other:#x}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_bento_bytes() {
        // .bento/bento: {"a":1,"b":-1,"f":300,"g":4294967296} -> a1 61 01 a1 62 ff ...
        let v = Value::from_json_value(
            serde_json::json!({"a":1,"b":-1,"c":1.5,"f":300,"g":4294967296u64}),
        );
        assert_eq!(
            hex::encode(encode(&v)),
            "85a16101a162ffa163cb3ff8000000000000a166cd012ca167cf0000000100000000"
        );
        assert_eq!(decode(&encode(&v)), Ok(v));
    }

    #[test]
    fn forged_lengths_and_depth_are_refused() {
        assert!(decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0x91; 200]).is_err());
    }
}
