#![forbid(unsafe_code)]
//! Git pkt-line framing and side-band multiplexing on top of `gix-packetline` (S3, S4, T5).
//! Enforces `MAX_PKT_LINE_LEN` (`65520` bytes: 4-byte hex length + up to 65516 bytes payload).

use gix_packetline::{Channel, PacketLineRef, blocking_io::encode};
use shako_core::{MAX_PKT_LINE_LEN, ShakoError, ShakoResult};
use std::io::Write;

/// Maximum data payload inside one side-band-64k frame (`65520 - 4 - 1 = 65515` bytes).
pub const MAX_SIDEBAND_64K_DATA: usize = 65_515;
/// Maximum data payload inside one standard side-band frame (`1000 - 4 - 1 = 995` bytes).
pub const MAX_SIDEBAND_1K_DATA: usize = 995;

/// Decoded packet line owned by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PktLine {
    Data(Vec<u8>),
    Flush,
    Delimiter,
    ResponseEnd,
}

impl PktLine {
    /// Returns the payload as a UTF-8 string with any trailing `\n` or `\r\n` stripped.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Data(bytes) => {
                let s = std::str::from_utf8(bytes).ok()?;
                Some(s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s))
            }
            _ => None,
        }
    }
}

/// Parses one pkt-line at the front of `input`, returning `(PktLine, bytes_consumed)`.
/// Enforces `pkt-len <= 65520` (Threat T5).
pub fn read_pkt_line(input: &[u8]) -> ShakoResult<Option<(PktLine, usize)>> {
    if input.is_empty() {
        return Ok(None);
    }
    if input.len() < 4 {
        return Err(ShakoError::BadRequest(
            "truncated pkt-line header (< 4 bytes)".into(),
        ));
    }
    let prefix = std::str::from_utf8(&input[..4])
        .map_err(|_| ShakoError::BadRequest("non-hex pkt-line length prefix".into()))?;
    let len = usize::from_str_radix(prefix, 16)
        .map_err(|_| ShakoError::BadRequest(format!("invalid pkt-line hex length '{prefix}'")))?;

    match len {
        0 => Ok(Some((PktLine::Flush, 4))),
        1 => Ok(Some((PktLine::Delimiter, 4))),
        2 => Ok(Some((PktLine::ResponseEnd, 4))),
        3 => Err(ShakoError::BadRequest("invalid pkt-line length 0003".into())),
        4 => Ok(Some((PktLine::Data(Vec::new()), 4))),
        n if n > MAX_PKT_LINE_LEN => Err(ShakoError::BadRequest(format!(
            "pkt-line length {n} exceeds maximum {MAX_PKT_LINE_LEN}"
        ))),
        n => {
            if input.len() < n {
                return Err(ShakoError::BadRequest(format!(
                    "truncated pkt-line: header says {n} bytes, only {} available",
                    input.len()
                )));
            }
            // Validate via gix-packetline decode as well
            let slice = &input[..n];
            match gix_packetline::decode(slice) {
                Ok(PacketLineRef::Data(d)) => Ok(Some((PktLine::Data(d.to_vec()), n))),
                Ok(PacketLineRef::Flush) => Ok(Some((PktLine::Flush, n))),
                Ok(PacketLineRef::Delimiter) => Ok(Some((PktLine::Delimiter, n))),
                Ok(PacketLineRef::ResponseEnd) => Ok(Some((PktLine::ResponseEnd, n))),
                Err(_) => Ok(Some((PktLine::Data(input[4..n].to_vec()), n))),
            }
        }
    }
}

/// Reads all pkt-lines from `input` up to the end of the slice (for stateless HTTP requests
/// that contain no trailing raw packfile).
pub fn parse_all_pkt_lines(mut input: &[u8]) -> ShakoResult<Vec<PktLine>> {
    let mut out = Vec::new();
    while let Some((line, used)) = read_pkt_line(input)? {
        out.push(line);
        input = &input[used..];
    }
    Ok(out)
}

/// Writes a text pkt-line (ensuring trailing LF) using `gix-packetline`.
pub fn write_text_line(mut out: impl Write, text: &str) -> ShakoResult<()> {
    encode::text_to_write(text.as_bytes(), &mut out)
        .map_err(|e| ShakoError::Git(format!("pkt-line write failed: {e}")))?;
    Ok(())
}

/// Writes a raw binary/data pkt-line without altering trailing bytes.
pub fn write_data_line(mut out: impl Write, data: &[u8]) -> ShakoResult<()> {
    encode::data_to_write(data, &mut out)
        .map_err(|e| ShakoError::Git(format!("pkt-line data write failed: {e}")))?;
    Ok(())
}

/// Writes `0000` flush-pkt.
pub fn write_flush(mut out: impl Write) -> ShakoResult<()> {
    encode::flush_to_write(&mut out)
        .map_err(|e| ShakoError::Git(format!("pkt-line flush failed: {e}")))?;
    Ok(())
}

/// Writes `0001` delim-pkt (protocol v2).
pub fn write_delim(mut out: impl Write) -> ShakoResult<()> {
    encode::delim_to_write(&mut out)
        .map_err(|e| ShakoError::Git(format!("pkt-line delim failed: {e}")))?;
    Ok(())
}

/// Writes `ERR <message>\n` pkt-line.
pub fn write_err_line(mut out: impl Write, message: &str) -> ShakoResult<()> {
    encode::error_to_write(message.as_bytes(), &mut out)
        .map_err(|e| ShakoError::Git(format!("pkt-line error write failed: {e}")))?;
    Ok(())
}

/// Writes `data` chunked into sideband frames on `channel` (`Data = 1`, `Progress = 2`, `Error = 3`).
pub fn write_sideband_chunks(
    mut out: impl Write,
    channel: Channel,
    data: &[u8],
    max_chunk: usize,
) -> ShakoResult<()> {
    let chunk_limit = max_chunk.clamp(1, MAX_SIDEBAND_64K_DATA);
    for chunk in data.chunks(chunk_limit) {
        encode::band_to_write(channel, chunk, &mut out)
            .map_err(|e| ShakoError::Git(format!("sideband write failed: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pktline_round_trips_and_enforces_65520_cap() {
        let mut buf = Vec::new();
        write_text_line(&mut buf, "# service=git-upload-pack").expect("write");
        write_flush(&mut buf).expect("flush");
        write_delim(&mut buf).expect("delim");

        let lines = parse_all_pkt_lines(&buf).expect("parse");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].as_text(), Some("# service=git-upload-pack"));
        assert_eq!(lines[1], PktLine::Flush);
        assert_eq!(lines[2], PktLine::Delimiter);

        // 0xffff > 65520 -> refused
        let oversized = b"ffffabcd";
        assert!(read_pkt_line(oversized).is_err());
    }
}
