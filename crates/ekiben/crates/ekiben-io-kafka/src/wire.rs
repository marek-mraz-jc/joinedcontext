//! Kafka's wire primitives (non-flexible versions only): big-endian integers, i16-length strings,
//! i32-length bytes, i32-count arrays, and the zigzag varints of record batches.
//!
//! The decoder never trusts a length or a count: each element read checks the bytes left, so a
//! hostile count fails at the first missing byte instead of reserving memory for it.

use bytes::{Buf, Bytes};

#[derive(Debug, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed Kafka response: {}", self.0)
    }
}

pub type WireResult<T> = Result<T, WireError>;

#[derive(Default)]
pub struct Enc(pub Vec<u8>);

impl Enc {
    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.0.push(v as u8);
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.0.push(u8::from(v));
        self
    }
    /// A string or a name: an i16 length. Kafka names are short; a longer text is cut by the
    /// caller's validation, so the cast never truncates silently in practice.
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.i16(v.len().min(i16::MAX as usize) as i16);
        self.0
            .extend_from_slice(&v.as_bytes()[..v.len().min(i16::MAX as usize)]);
        self
    }
    pub fn null_str(&mut self) -> &mut Self {
        self.i16(-1)
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.i32(v.len() as i32);
        self.0.extend_from_slice(v);
        self
    }
    /// An array: the count, then `each` for every element.
    pub fn array<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.i32(items.len() as i32);
        for it in items {
            each(self, it);
        }
        self
    }
    #[cfg(test)]
    pub fn varint(&mut self, v: i64) -> &mut Self {
        let mut z = ((v << 1) ^ (v >> 63)) as u64;
        loop {
            let b = (z & 0x7f) as u8;
            z >>= 7;
            if z == 0 {
                self.0.push(b);
                return self;
            }
            self.0.push(b | 0x80);
        }
    }
}

pub struct Dec(pub Bytes);

impl Dec {
    fn need(&self, n: usize) -> WireResult<()> {
        if self.0.remaining() < n {
            Err(WireError("truncated"))
        } else {
            Ok(())
        }
    }
    pub fn i8(&mut self) -> WireResult<i8> {
        self.need(1)?;
        Ok(self.0.get_i8())
    }
    pub fn i16(&mut self) -> WireResult<i16> {
        self.need(2)?;
        Ok(self.0.get_i16())
    }
    pub fn i32(&mut self) -> WireResult<i32> {
        self.need(4)?;
        Ok(self.0.get_i32())
    }
    pub fn i64(&mut self) -> WireResult<i64> {
        self.need(8)?;
        Ok(self.0.get_i64())
    }
    pub fn u32(&mut self) -> WireResult<u32> {
        self.need(4)?;
        Ok(self.0.get_u32())
    }
    pub fn bool(&mut self) -> WireResult<bool> {
        Ok(self.i8()? != 0)
    }
    pub fn take(&mut self, n: usize) -> WireResult<Bytes> {
        self.need(n)?;
        Ok(self.0.split_to(n))
    }
    pub fn str(&mut self) -> WireResult<String> {
        Ok(self.null_str()?.unwrap_or_default())
    }
    pub fn null_str(&mut self) -> WireResult<Option<String>> {
        let n = self.i16()?;
        if n < 0 {
            return Ok(None);
        }
        let b = self.take(n as usize)?;
        String::from_utf8(b.to_vec())
            .map(Some)
            .map_err(|_| WireError("a string is not UTF-8"))
    }
    pub fn null_bytes(&mut self) -> WireResult<Option<Bytes>> {
        let n = self.i32()?;
        if n < 0 {
            return Ok(None);
        }
        self.take(n as usize).map(Some)
    }
    pub fn bytes(&mut self) -> WireResult<Bytes> {
        Ok(self.null_bytes()?.unwrap_or_default())
    }
    /// An array; a null array reads as empty.
    pub fn array<T>(
        &mut self,
        mut each: impl FnMut(&mut Self) -> WireResult<T>,
    ) -> WireResult<Vec<T>> {
        let n = self.i32()?;
        let mut out = Vec::new();
        for _ in 0..n.max(0) {
            out.push(each(self)?);
        }
        Ok(out)
    }
    pub fn varint(&mut self) -> WireResult<i64> {
        let mut z: u64 = 0;
        for shift in (0..70).step_by(7) {
            let b = self.i8()? as u8;
            z |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(((z >> 1) as i64) ^ -((z & 1) as i64));
            }
        }
        Err(WireError("a varint is longer than 10 bytes"))
    }
    /// Varint-length bytes; -1 is null.
    pub fn var_bytes(&mut self) -> WireResult<Option<Bytes>> {
        let n = self.varint()?;
        if n < 0 {
            return Ok(None);
        }
        let n = usize::try_from(n).map_err(|_| WireError("a length is out of range"))?;
        self.take(n).map(Some)
    }
    pub fn remaining(&self) -> usize {
        self.0.remaining()
    }
}

/// A request frame: size, header v1 (api key, version, correlation id, client id), body.
pub fn frame(
    api_key: i16,
    version: i16,
    correlation: i32,
    client_id: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut e = Enc::default();
    e.i32(0)
        .i16(api_key)
        .i16(version)
        .i32(correlation)
        .str(client_id);
    e.0.extend_from_slice(body);
    let size = (e.0.len() - 4) as i32;
    e.0[..4].copy_from_slice(&size.to_be_bytes());
    e.0
}

/// Kafka's error code names the input reports (KafkaProtocol's ErrorCodes).
pub fn error_name(code: i16) -> &'static str {
    match code {
        1 => "OFFSET_OUT_OF_RANGE",
        3 => "UNKNOWN_TOPIC_OR_PARTITION",
        5 => "LEADER_NOT_AVAILABLE",
        6 => "NOT_LEADER_OR_FOLLOWER",
        7 => "REQUEST_TIMED_OUT",
        14 => "COORDINATOR_LOAD_IN_PROGRESS",
        15 => "COORDINATOR_NOT_AVAILABLE",
        16 => "NOT_COORDINATOR",
        22 => "ILLEGAL_GENERATION",
        23 => "INCONSISTENT_GROUP_PROTOCOL",
        24 => "INVALID_GROUP_ID",
        25 => "UNKNOWN_MEMBER_ID",
        26 => "INVALID_SESSION_TIMEOUT",
        27 => "REBALANCE_IN_PROGRESS",
        29 => "TOPIC_AUTHORIZATION_FAILED",
        30 => "GROUP_AUTHORIZATION_FAILED",
        31 => "CLUSTER_AUTHORIZATION_FAILED",
        33 => "UNSUPPORTED_SASL_MECHANISM",
        34 => "ILLEGAL_SASL_STATE",
        35 => "UNSUPPORTED_VERSION",
        58 => "SASL_AUTHENTICATION_FAILED",
        74 => "FENCED_LEADER_EPOCH",
        75 => "UNKNOWN_LEADER_EPOCH",
        _ => "an error",
    }
}
