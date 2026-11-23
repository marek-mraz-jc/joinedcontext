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
        Value::Timestamp(t) => write_str(out, &format_rfc3339_nano(t)),
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
        let len = self.uint(n)?;
        if len > (self.buf.len() - self.pos) as u64 {
            return Err("msgpack: unexpected EOF".to_string());
        }
        Ok(len as usize)
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

    fn hex_of(v: &Value) -> String {
        hex::encode(encode(v))
    }

    #[test]
    fn integers_take_the_smallest_code_at_each_boundary() {
        for (u, want) in [
            (0x7f_u64, "7f"),
            (0x80, "cc80"),
            (0xff, "ccff"),
            (0x100, "cd0100"),
            (0xffff, "cdffff"),
            (0x1_0000, "ce00010000"),
            (0xffff_ffff, "ceffffffff"),
            (0x1_0000_0000, "cf0000000100000000"),
        ] {
            assert_eq!(hex_of(&Value::Uint(u)), want, "uint {u:#x}");
            assert_eq!(decode(&encode(&Value::Uint(u))), Ok(Value::Int(u as i64)));
        }
        assert_eq!(hex_of(&Value::Uint(u64::MAX)), "cfffffffffffffffff");
        assert_eq!(
            decode(&encode(&Value::Uint(u64::MAX))),
            Ok(Value::Uint(u64::MAX))
        );
        for (i, want) in [
            (5_i64, "05"),
            (-1, "ff"),
            (-32, "e0"),
            (-33, "d0df"),
            (-128, "d080"),
            (-129, "d1ff7f"),
            (-32768, "d18000"),
            (-32769, "d2ffff7fff"),
            (i64::from(i32::MIN), "d280000000"),
            (i64::from(i32::MIN) - 1, "d3ffffffff7fffffff"),
            (i64::MIN, "d38000000000000000"),
        ] {
            assert_eq!(hex_of(&Value::Int(i)), want, "int {i}");
            assert_eq!(decode(&encode(&Value::Int(i))), Ok(Value::Int(i)));
        }
    }

    #[test]
    fn strings_bytes_arrays_and_maps_switch_header_width_at_their_limits() {
        let s = |n: usize| Value::String("x".repeat(n));
        for (n, head) in [
            (31, "bf"),
            (32, "d920"),
            (255, "d9ff"),
            (256, "da0100"),
            (0xffff, "daffff"),
            (0x1_0000, "db00010000"),
        ] {
            assert!(hex_of(&s(n)).starts_with(head), "str {n}");
            assert_eq!(decode(&encode(&s(n))), Ok(s(n)));
        }
        let b = |n: usize| Value::Bytes(vec![7; n]);
        for (n, head) in [
            (0, "c400"),
            (255, "c4ff"),
            (256, "c50100"),
            (0x1_0000, "c600010000"),
        ] {
            assert!(hex_of(&b(n)).starts_with(head), "bin {n}");
            assert_eq!(decode(&encode(&b(n))), Ok(b(n)));
        }
        let a = |n: usize| Value::Array(vec![Value::Null; n].into());
        for (n, head) in [(15, "9f"), (16, "dc0010"), (0x1_0000, "dd00010000")] {
            assert!(hex_of(&a(n)).starts_with(head), "array {n}");
            assert_eq!(decode(&encode(&a(n))), Ok(a(n)));
        }
        let m = |n: usize| {
            Value::Object(
                (0..n)
                    .map(|i| (format!("{i:05}"), Value::Bool(i % 2 == 0)))
                    .collect(),
            )
        };
        for (n, head) in [(15, "8f"), (16, "de0010"), (0x1_0000, "df00010000")] {
            assert!(hex_of(&m(n)).starts_with(head), "map {n}");
            assert_eq!(decode(&encode(&m(n))), Ok(m(n)));
        }
    }

    #[test]
    fn scalars_number_text_and_timestamps_encode_as_bento_does(
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(hex_of(&Value::Null), "c0");
        assert_eq!(hex_of(&Value::Nothing), "c0");
        assert_eq!(hex_of(&Value::Delete), "c0");
        assert_eq!(hex_of(&Value::Bool(false)), "c2");
        assert_eq!(hex_of(&Value::Bool(true)), "c3");
        assert_eq!(hex_of(&Value::Number("1E2".into())), "cb4059000000000000");
        assert_eq!(hex_of(&Value::Number("7".into())), "07");
        let ts = "2024-01-02T03:04:05Z"
            .parse::<jiff::Timestamp>()?
            .to_zoned(jiff::tz::TimeZone::UTC);
        assert_eq!(
            decode(&encode(&Value::Timestamp(ts))),
            Ok(Value::String("2024-01-02T03:04:05Z".into()))
        );
        Ok(())
    }

    #[test]
    fn decode_reads_every_code_family() {
        assert_eq!(decode(&[0xc2]), Ok(Value::Bool(false)));
        assert_eq!(decode(&[0xc3]), Ok(Value::Bool(true)));
        assert_eq!(decode(&[0xca, 0x3f, 0xc0, 0, 0]), Ok(Value::Float(1.5)));
        assert_eq!(decode(&[0xd0, 0x80]), Ok(Value::Int(-128)));
        assert_eq!(decode(&[0xd1, 0xff, 0x7f]), Ok(Value::Int(-129)));
        assert_eq!(decode(&[0xf0]), Ok(Value::Int(-16)));
        assert_eq!(decode(&[0xc4, 1, 9]), Ok(Value::Bytes(vec![9])));
        // A map key that is no string is read as its text, as Bento's decoder coerces it.
        assert_eq!(
            decode(&[0x82, 0x01, 0xc3, 0xc0, 0x02]),
            Ok(Value::Object(
                [
                    ("1".to_string(), Value::Bool(true)),
                    ("null".to_string(), Value::Int(2)),
                ]
                .into_iter()
                .collect()
            ))
        );
    }

    #[test]
    fn truncated_input_and_unsupported_codes_are_refused_with_the_reason() {
        let eof = Err("msgpack: unexpected EOF".to_string());
        assert_eq!(decode(&[]), eof);
        assert_eq!(decode(&[0xcd, 0x01]), eof);
        assert_eq!(decode(&[0xc4, 2, 9]), eof);
        assert_eq!(decode(&[0xd9, 1]), eof);
        assert_eq!(decode(&[0xdc, 0, 1]), eof);
        assert_eq!(decode(&[0xde, 0, 1]), eof);
        for code in [0xc1_u8, 0xc7, 0xd4, 0xd8] {
            assert_eq!(
                decode(&[code, 0, 0, 0]),
                Err(format!("msgpack: unsupported code {code:#x}"))
            );
        }
    }

    #[test]
    fn nesting_is_allowed_to_the_limit_and_refused_past_it() {
        let nested = |depth: usize| {
            let mut buf = vec![0x91; depth];
            buf.push(0xc0);
            buf
        };
        assert!(decode(&nested(MAX_DEPTH)).is_ok());
        assert_eq!(
            decode(&nested(MAX_DEPTH + 1)),
            Err("msgpack: exceeded max depth".to_string())
        );
        let mut map = vec![0x81, 0xa1, b'k'];
        map.extend(nested(MAX_DEPTH));
        assert_eq!(decode(&map), Err("msgpack: exceeded max depth".to_string()));
    }

    #[test]
    fn forged_lengths_and_depth_are_refused() {
        assert!(decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0x91; 200]).is_err());
    }
}
