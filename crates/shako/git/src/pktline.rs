#![forbid(unsafe_code)]
//! Git pkt-line framing and side-band multiplexing on top of `gix-packetline` (S3, S4, T5).
//! Enforces `MAX_PKT_LINE_LEN` (`65520` bytes: 4-byte hex length + up to 65516 bytes payload).

use gix_packetline::{Channel, blocking_io::encode};
use shako_core::{MAX_PKT_LINE_LEN, ShakoError, ShakoResult};
use std::io::{Read, Write};

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
                Some(
                    s.strip_suffix("\r\n")
                        .or_else(|| s.strip_suffix('\n'))
                        .unwrap_or(s),
                )
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
        3 => Err(ShakoError::BadRequest(
            "invalid pkt-line length 0003".into(),
        )),
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
            // A length of 5 or more past the checks above is a data line of the bytes after the
            // header: the special packets are the lengths 0 to 2 matched above.
            Ok(Some((PktLine::Data(input[4..n].to_vec()), n)))
        }
    }
}

/// Reads one pkt-line from `input` and nothing past it, with the bytes it took; `None` at the
/// end of the input.
pub fn read_pkt_line_from(mut input: impl Read) -> ShakoResult<Option<(PktLine, usize)>> {
    let unreadable = |e| ShakoError::BadRequest(format!("pkt-line read failed: {e}"));
    let mut line = Vec::with_capacity(4);
    (&mut input)
        .take(4)
        .read_to_end(&mut line)
        .map_err(unreadable)?;
    if let Some(len) = std::str::from_utf8(&line)
        .ok()
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
        && (5..=MAX_PKT_LINE_LEN).contains(&len)
    {
        (&mut input)
            .take(len as u64 - 4)
            .read_to_end(&mut line)
            .map_err(unreadable)?;
    }
    read_pkt_line(&line)
}

/// Lines of fewer than 40 payload bytes one message may carry: a `have` or a ref is longer, so
/// only a few capability, flush and status lines are short. A list of short lines costs some
/// times its bytes; this keeps a parsed message within twice and a bit of its size.
pub const MAX_SHORT_PKT_LINES: usize = 4096;

/// Reads all pkt-lines from `input` up to the end of the slice (for stateless HTTP requests
/// that contain no trailing raw packfile).
pub fn parse_all_pkt_lines(mut input: &[u8]) -> ShakoResult<Vec<PktLine>> {
    let mut out = Vec::new();
    let mut short = 0;
    while let Some((line, used)) = read_pkt_line(input)? {
        if !matches!(&line, PktLine::Data(d) if d.len() >= 40) {
            short += 1;
            if short > MAX_SHORT_PKT_LINES {
                return Err(ShakoError::BadRequest(format!(
                    "more than {MAX_SHORT_PKT_LINES} short pkt-lines"
                )));
            }
        }
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

    #[test]
    fn each_special_and_malformed_length_prefix_is_read_or_refused() {
        assert_eq!(read_pkt_line(b"").expect("empty"), None);
        assert_eq!(
            read_pkt_line(b"0002").expect("end"),
            Some((PktLine::ResponseEnd, 4))
        );
        assert_eq!(
            read_pkt_line(b"0004").expect("empty data"),
            Some((PktLine::Data(Vec::new()), 4))
        );
        for (input, says) in [
            (&b"00"[..], "truncated pkt-line header"),
            (&b"\xff\xfe00"[..], "non-hex"),
            (&b"zz10"[..], "invalid pkt-line hex length 'zz10'"),
            (&b"0003"[..], "0003"),
            (&b"fff1"[..], "exceeds maximum 65520"),
            (&b"0009abc"[..], "header says 9 bytes, only 7 available"),
        ] {
            let err = read_pkt_line(input).expect_err("refused");
            assert!(
                matches!(&err, ShakoError::BadRequest(m) if m.contains(says)),
                "{err}"
            );
        }
        // The longest line the protocol allows still reads.
        let mut longest = format!("{MAX_PKT_LINE_LEN:04x}").into_bytes();
        longest.resize(MAX_PKT_LINE_LEN, b'a');
        let (line, used) = read_pkt_line(&longest).expect("read").expect("a line");
        assert_eq!(used, MAX_PKT_LINE_LEN);
        assert!(matches!(line, PktLine::Data(d) if d.len() == MAX_PKT_LINE_LEN - 4));
    }

    #[test]
    fn a_line_reads_as_text_without_its_line_ending_and_only_when_utf8() {
        assert_eq!(PktLine::Data(b"ok\r\n".to_vec()).as_text(), Some("ok"));
        assert_eq!(PktLine::Data(b"ok\n".to_vec()).as_text(), Some("ok"));
        assert_eq!(PktLine::Data(b"ok".to_vec()).as_text(), Some("ok"));
        assert_eq!(PktLine::Data(vec![0xff, 0xfe]).as_text(), None);
        assert_eq!(PktLine::Flush.as_text(), None);
        assert_eq!(PktLine::Delimiter.as_text(), None);
    }

    #[test]
    fn a_reader_yields_one_line_at_a_time_and_names_a_read_failure() {
        let mut body = Vec::new();
        write_text_line(&mut body, "want x").expect("write");
        write_flush(&mut body).expect("flush");
        let mut reader = std::io::Cursor::new(body);
        let (first, used) = read_pkt_line_from(&mut reader)
            .expect("read")
            .expect("line");
        assert_eq!(first.as_text(), Some("want x"));
        assert_eq!(used, 11);
        assert_eq!(
            read_pkt_line_from(&mut reader).expect("read"),
            Some((PktLine::Flush, 4))
        );
        assert_eq!(read_pkt_line_from(&mut reader).expect("read"), None);
        // A length the reader cannot fill is the truncated-line refusal.
        let err = read_pkt_line_from(&b"0010ab"[..]).expect_err("truncated");
        assert!(err.to_string().contains("only 6 available"), "{err}");

        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reset by peer"))
            }
        }
        let err = read_pkt_line_from(Broken).expect_err("unreadable");
        assert!(
            matches!(&err, ShakoError::BadRequest(m) if m.contains("reset by peer")),
            "{err}"
        );
    }

    #[test]
    fn a_body_of_short_lines_is_cut_off_after_the_limit() {
        let mut body = Vec::new();
        for _ in 0..MAX_SHORT_PKT_LINES {
            write_flush(&mut body).expect("flush");
        }
        assert_eq!(
            parse_all_pkt_lines(&body).expect("at the limit").len(),
            MAX_SHORT_PKT_LINES
        );
        // Long data lines do not count against it.
        let long = "a".repeat(40);
        let mut with_long = body.clone();
        write_text_line(&mut with_long, &long).expect("write");
        assert!(parse_all_pkt_lines(&with_long).is_ok());
        write_flush(&mut body).expect("flush");
        let err = parse_all_pkt_lines(&body).expect_err("over the limit");
        assert!(err.to_string().contains("short pkt-lines"), "{err}");
    }

    #[test]
    fn sideband_chunks_split_at_the_limit_and_a_zero_limit_means_one_byte() {
        let mut out = Vec::new();
        write_sideband_chunks(&mut out, Channel::Data, b"abc", 2).expect("write");
        let lines = parse_all_pkt_lines(&out).expect("parse");
        assert_eq!(
            lines,
            [
                PktLine::Data(b"\x01ab".to_vec()),
                PktLine::Data(b"\x01c".to_vec())
            ]
        );
        let mut one = Vec::new();
        write_sideband_chunks(&mut one, Channel::Progress, b"xy", 0).expect("write");
        assert_eq!(parse_all_pkt_lines(&one).expect("parse").len(), 2);
        let mut none = Vec::new();
        write_sideband_chunks(&mut none, Channel::Data, b"", 10).expect("write");
        assert!(none.is_empty());
    }

    #[test]
    fn every_writer_failure_is_a_git_error() {
        use crate::testutil::Refusing;
        for err in [
            write_text_line(Refusing, "x").expect_err("text"),
            write_flush(Refusing).expect_err("flush"),
            write_delim(Refusing).expect_err("delim"),
            write_sideband_chunks(Refusing, Channel::Data, b"x", 10).expect_err("band"),
        ] {
            assert!(
                matches!(&err, ShakoError::Git(m) if m.contains("disk full")),
                "{err}"
            );
        }
    }
}
