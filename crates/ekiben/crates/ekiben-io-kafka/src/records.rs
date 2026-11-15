//! Record batches (magic 2, Kafka 0.11+): the CRC-32C check, the five codecs, control batches and
//! aborted transactions under `read_committed`.
//!
//! A fetch answer may end in a partial batch (the broker cuts at `partition_max_bytes`); the tail
//! is dropped and fetched again from its offset. A batch inflates to at most
//! [`MAX_BATCH_BYTES`], so a small compressed batch cannot fill the runner's memory.

use std::collections::HashSet;
use std::io::Read;

use bytes::Bytes;

use crate::wire::{Dec, WireError, WireResult};

/// The most a batch's records may inflate to.
pub const MAX_BATCH_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub offset: i64,
    pub timestamp_ms: i64,
    pub key: Option<Bytes>,
    pub value: Option<Bytes>,
    pub headers: Vec<(String, Option<Bytes>)>,
}

/// An aborted transaction of a fetch answer: the producer and the first offset it wrote.
#[derive(Debug, Clone, Copy)]
pub struct Aborted {
    pub producer_id: i64,
    pub first_offset: i64,
}

/// The decoded records of one partition's `records` field.
#[derive(Debug, Default)]
pub struct Decoded {
    pub records: Vec<Record>,
    /// The offset after the last whole batch read (control and aborted batches included), or
    /// `None` when not one whole batch was there.
    pub next_offset: Option<i64>,
}

/// CRC-32C (Castagnoli), as record batches carry it.
pub fn crc32c(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0x82F6_3B78 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

fn bounded(mut r: impl Read) -> WireResult<Vec<u8>> {
    let mut out = Vec::new();
    (&mut r)
        .take(MAX_BATCH_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| WireError("a compressed batch does not inflate"))?;
    if out.len() as u64 > MAX_BATCH_BYTES {
        return Err(WireError("a batch inflates past 64 MiB"));
    }
    Ok(out)
}

/// Snappy as Kafka's Java clients write it (xerial framing: a magic, then length-prefixed raw
/// blocks), or a bare raw block as librdkafka and franz-go write it.
fn unsnappy(data: &[u8]) -> WireResult<Vec<u8>> {
    const XERIAL: &[u8] = b"\x82SNAPPY\x00";
    let raw = |b: &[u8], out: &mut Vec<u8>| -> WireResult<()> {
        let n = snap::raw::decompress_len(b).map_err(|_| WireError("a snappy block is corrupt"))?;
        if (out.len() + n) as u64 > MAX_BATCH_BYTES {
            return Err(WireError("a batch inflates past 64 MiB"));
        }
        out.extend(
            snap::raw::Decoder::new()
                .decompress_vec(b)
                .map_err(|_| WireError("a snappy block is corrupt"))?,
        );
        Ok(())
    };
    let mut out = Vec::new();
    match data.strip_prefix(XERIAL) {
        None => raw(data, &mut out)?,
        Some(rest) => {
            // version and compatible version, then blocks
            let mut d = Dec(Bytes::copy_from_slice(rest));
            d.i32()?;
            d.i32()?;
            while d.remaining() > 0 {
                let n = d.i32()?;
                let block = d.take(
                    usize::try_from(n)
                        .map_err(|_| WireError("a snappy block length is negative"))?,
                )?;
                raw(&block, &mut out)?;
            }
        }
    }
    Ok(out)
}

fn inflate(codec: i16, data: Bytes) -> WireResult<Bytes> {
    Ok(match codec {
        0 => data,
        1 => bounded(flate2::read::GzDecoder::new(&data[..]))?.into(),
        2 => unsnappy(&data)?.into(),
        3 => bounded(lz4_flex::frame::FrameDecoder::new(&data[..]))?.into(),
        4 => bounded(
            zstd::stream::read::Decoder::new(&data[..])
                .map_err(|_| WireError("a zstd batch does not inflate"))?,
        )?
        .into(),
        _ => return Err(WireError("a batch names an unknown compression codec")),
    })
}

fn record(d: &mut Dec, base_offset: i64, base_ts: i64) -> WireResult<Record> {
    let len = d.varint()?;
    let mut r =
        Dec(d.take(usize::try_from(len).map_err(|_| WireError("a record length is negative"))?)?);
    r.i8()?; // attributes, unused
    let ts_delta = r.varint()?;
    let off_delta = r.varint()?;
    let key = r.var_bytes()?;
    let value = r.var_bytes()?;
    let n = r.varint()?;
    let mut headers = Vec::new();
    for _ in 0..n.max(0) {
        let k = r.var_bytes()?.unwrap_or_default();
        let k =
            String::from_utf8(k.to_vec()).map_err(|_| WireError("a header key is not UTF-8"))?;
        headers.push((k, r.var_bytes()?));
    }
    Ok(Record {
        offset: base_offset.saturating_add(off_delta),
        timestamp_ms: base_ts.saturating_add(ts_delta),
        key,
        value,
        headers,
    })
}

/// Decodes a partition's record batches, keeping records at or past `from` (a batch may start
/// before the offset asked for). With `aborted` (read_committed) the data of aborted
/// transactions is dropped.
pub fn decode(data: Bytes, from: i64, aborted: Option<&[Aborted]>) -> WireResult<Decoded> {
    let mut out = Decoded::default();
    let mut d = Dec(data);
    let mut pending: Vec<Aborted> = aborted.unwrap_or_default().to_vec();
    pending.sort_by_key(|a| a.first_offset);
    let mut pending = pending.into_iter().peekable();
    let mut aborting: HashSet<i64> = HashSet::new();
    // Each batch: base offset (8) and length (4), then `length` bytes.
    while d.remaining() >= 12 {
        let head = d.0.slice(..12);
        let mut h = Dec(head);
        let base_offset = h.i64()?;
        let len = h.i32()?;
        let len = usize::try_from(len).map_err(|_| WireError("a batch length is negative"))?;
        if d.remaining() < 12 + len {
            break; // the partial tail of a capped fetch
        }
        d.take(12)?;
        let mut b = Dec(d.take(len)?);
        b.i32()?; // partition leader epoch
        let magic = b.i8()?;
        if magic != 2 {
            return Err(WireError(
                "a message set older than Kafka 0.11 (magic 0 or 1); upgrade the topic's message format",
            ));
        }
        let crc = b.u32()?;
        if crc32c(&b.0) != crc {
            return Err(WireError("a record batch fails its CRC-32C check"));
        }
        let attributes = b.i16()?;
        let last_delta = b.i32()?;
        let base_ts = b.i64()?;
        b.i64()?; // max timestamp
        let producer_id = b.i64()?;
        b.i16()?; // producer epoch
        b.i32()?; // base sequence
        let count = b.i32()?;
        let next = base_offset.saturating_add(i64::from(last_delta)) + 1;
        out.next_offset = Some(next);
        let transactional = attributes & 0x10 != 0;
        let control = attributes & 0x20 != 0;
        if aborted.is_some() {
            while let Some(a) = pending.next_if(|a| a.first_offset < next) {
                aborting.insert(a.producer_id);
            }
        }
        if control {
            // a commit or abort marker ends the producer's transaction
            aborting.remove(&producer_id);
            continue;
        }
        if transactional && aborting.contains(&producer_id) {
            continue;
        }
        let mut r = Dec(inflate(attributes & 0x07, b.0)?);
        for _ in 0..count.max(0) {
            let rec = record(&mut r, base_offset, base_ts)?;
            if rec.offset >= from {
                out.records.push(rec);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod encode {
    //! The batch writer the tests and the test producer use.
    use super::*;
    use crate::wire::Enc;

    pub fn batch(
        base_offset: i64,
        codec: i16,
        producer: Option<(i64, bool)>,
        recs: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut body = Enc::default();
        for (i, (k, v)) in recs.iter().enumerate() {
            let mut r = Enc::default();
            r.i8(0).varint(i as i64).varint(i as i64);
            r.varint(k.len() as i64);
            r.0.extend_from_slice(k.as_bytes());
            r.varint(v.len() as i64);
            r.0.extend_from_slice(v.as_bytes());
            r.varint(1).varint(1);
            r.0.push(b'h');
            r.varint(1);
            r.0.push(b'x');
            body.varint(r.0.len() as i64);
            body.0.extend_from_slice(&r.0);
        }
        let payload = match codec {
            0 => body.0,
            1 => {
                use std::io::Write;
                let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                z.write_all(&body.0).expect("gzip");
                z.finish().expect("gzip")
            }
            2 => snap::raw::Encoder::new()
                .compress_vec(&body.0)
                .expect("snappy"),
            3 => {
                use std::io::Write;
                let mut z = lz4_flex::frame::FrameEncoder::new(Vec::new());
                z.write_all(&body.0).expect("lz4");
                z.finish().expect("lz4")
            }
            4 => zstd::encode_all(&body.0[..], 1).expect("zstd"),
            _ => unreachable!("codec"),
        };
        let (pid, control) = producer.unwrap_or((-1, false));
        let mut attrs = codec;
        if producer.is_some() {
            attrs |= 0x10;
        }
        if control {
            attrs |= 0x20;
        }
        let mut after_crc = Enc::default();
        after_crc
            .i16(attrs)
            .i32(recs.len().saturating_sub(1) as i32)
            .i64(1_700_000_000_000)
            .i64(1_700_000_000_000)
            .i64(pid)
            .i16(0)
            .i32(0)
            .i32(recs.len() as i32);
        after_crc.0.extend_from_slice(&payload);
        let mut e = Enc::default();
        e.i64(base_offset)
            .i32((4 + 1 + 4 + after_crc.0.len()) as i32)
            .i32(0)
            .i8(2)
            .i32(crc32c(&after_crc.0) as i32);
        e.0.extend_from_slice(&after_crc.0);
        e.0
    }
}

#[cfg(test)]
mod tests {
    use super::encode::batch;
    use super::*;

    fn values(d: &Decoded) -> Vec<String> {
        d.records
            .iter()
            .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).into_owned())
            .collect()
    }

    #[test]
    fn crc32c_matches_the_rfc_3720_vector() {
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn every_codec_round_trips() {
        for codec in 0..=4 {
            let b = batch(10, codec, None, &[("k1", "one"), ("k2", "two")]);
            let d = decode(b.into(), 0, None).expect("decodes");
            assert_eq!(values(&d), ["one", "two"], "codec {codec}");
            assert_eq!(d.records[1].offset, 11);
            assert_eq!(d.records[0].key.as_deref(), Some(&b"k1"[..]));
            assert_eq!(
                d.records[0].headers,
                vec![("h".into(), Some(Bytes::from_static(b"x")))]
            );
            assert_eq!(d.next_offset, Some(12));
        }
    }

    #[test]
    fn xerial_snappy_is_read() {
        let raw = snap::raw::Encoder::new()
            .compress_vec(b"hello")
            .expect("snappy");
        let mut x = b"\x82SNAPPY\x00".to_vec();
        x.extend_from_slice(&1i32.to_be_bytes());
        x.extend_from_slice(&1i32.to_be_bytes());
        x.extend_from_slice(&(raw.len() as i32).to_be_bytes());
        x.extend_from_slice(&raw);
        assert_eq!(unsnappy(&x).expect("xerial"), b"hello");
    }

    #[test]
    fn a_partial_tail_and_earlier_offsets_are_dropped() {
        let mut b = batch(5, 0, None, &[("a", "five"), ("b", "six")]);
        let second = batch(7, 0, None, &[("c", "seven")]);
        b.extend_from_slice(&second[..second.len() - 3]);
        let d = decode(b.into(), 6, None).expect("decodes");
        assert_eq!(values(&d), ["six"]);
        assert_eq!(d.next_offset, Some(7));
    }

    #[test]
    fn a_corrupt_batch_is_refused() {
        let mut b = batch(0, 0, None, &[("a", "x")]);
        let last = b.len() - 1;
        b[last] ^= 1;
        assert_eq!(
            decode(b.into(), 0, None).unwrap_err(),
            WireError("a record batch fails its CRC-32C check")
        );
    }

    #[test]
    fn a_compression_bomb_stops_at_the_cap() {
        let big = vec![0u8; (MAX_BATCH_BYTES + 1024) as usize];
        let z = zstd::encode_all(&big[..], 3).expect("zstd");
        assert!(inflate(4, z.into()).is_err());
    }

    #[test]
    fn read_committed_drops_aborted_transactions() {
        let mut b = batch(0, 0, Some((7, false)), &[("a", "aborted")]);
        b.extend(batch(1, 0, Some((7, true)), &[("", "")]));
        b.extend(batch(2, 0, Some((8, false)), &[("a", "committed")]));
        b.extend(batch(3, 0, None, &[("a", "plain")]));
        let aborted = [Aborted {
            producer_id: 7,
            first_offset: 0,
        }];
        let d = decode(b.clone().into(), 0, Some(&aborted)).expect("decodes");
        assert_eq!(values(&d), ["committed", "plain"]);
        assert_eq!(d.next_offset, Some(4));
        // read_uncommitted keeps the aborted data and still skips the marker
        let d = decode(b.into(), 0, None).expect("decodes");
        assert_eq!(values(&d), ["aborted", "committed", "plain"]);
    }
}
