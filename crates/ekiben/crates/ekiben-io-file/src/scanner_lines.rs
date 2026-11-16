//! `lines` scanner (`internal/impl/pure/scanner_lines.go`): one message per line, read with a
//! bounded buffer as Go's `bufio.Scanner`. The default split drops a trailing `\r`;
//! `custom_delimiter` splits on its exact bytes; `omit_empty` skips empty lines.

use std::io::BufRead;

use bytes::Bytes;
use ekiben_core::CoreError;
use serde_json::Value as JsonValue;

/// `bufio.MaxScanTokenSize`, Bento's default `max_buffer_size`.
const MAX_SCAN_TOKEN_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct LinesScanner {
    delimiter: Option<Vec<u8>>,
    max_buffer_size: usize,
    omit_empty: bool,
}

impl Default for LinesScanner {
    fn default() -> Self {
        Self {
            delimiter: None,
            max_buffer_size: MAX_SCAN_TOKEN_SIZE,
            omit_empty: false,
        }
    }
}

impl LinesScanner {
    /// Whether this splits on `\n` (no `custom_delimiter`).
    pub fn splits_on_newline(&self) -> bool {
        self.delimiter.is_none()
    }

    pub fn from_config(obj: &serde_json::Map<String, JsonValue>) -> Result<Self, CoreError> {
        let delimiter = match obj.get("custom_delimiter") {
            None => None,
            Some(v) => match v.as_str() {
                Some("") => None,
                Some(s) => Some(s.as_bytes().to_vec()),
                None => {
                    return Err(CoreError::config(
                        "field custom_delimiter must be a string, for example \"|\"",
                    ))
                }
            },
        };
        let max_buffer_size = match obj.get("max_buffer_size") {
            None => MAX_SCAN_TOKEN_SIZE,
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    CoreError::config(
                        "field max_buffer_size must be a positive integer, for example 65536",
                    )
                })?,
        };
        let omit_empty = obj
            .get("omit_empty")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        Ok(Self {
            delimiter,
            max_buffer_size,
            omit_empty,
        })
    }

    /// Calls `emit` with each line of `r`; holds at most `max_buffer_size` bytes of one line.
    pub fn scan(
        &self,
        r: &mut dyn BufRead,
        emit: &mut dyn FnMut(Bytes) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let delim: &[u8] = self.delimiter.as_deref().unwrap_or(b"\n");
        let last = delim[delim.len() - 1];
        let mut line = Vec::new();
        loop {
            let (found, used) = {
                let buf = r.fill_buf().map_err(read_err)?;
                if buf.is_empty() {
                    break;
                }
                match memchr(last, buf) {
                    Some(i) => (true, i + 1),
                    None => (false, buf.len()),
                }
            };
            if line.len() + used > self.max_buffer_size {
                return Err(CoreError::other("bufio.Scanner: token too long"));
            }
            line.extend_from_slice(&r.fill_buf().map_err(read_err)?[..used]);
            r.consume(used);
            if found && line.ends_with(delim) {
                line.truncate(line.len() - delim.len());
                if self.delimiter.is_none() && line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.push(&mut line, emit)?;
            }
        }
        if !line.is_empty() {
            if self.delimiter.is_none() && line.last() == Some(&b'\r') {
                line.pop();
            }
            self.push(&mut line, emit)?;
        }
        Ok(())
    }

    fn push(
        &self,
        line: &mut Vec<u8>,
        emit: &mut dyn FnMut(Bytes) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let token = std::mem::take(line);
        if self.omit_empty && token.is_empty() {
            return Ok(());
        }
        emit(Bytes::from(token))
    }
}

fn memchr(b: u8, s: &[u8]) -> Option<usize> {
    s.iter().position(|c| *c == b)
}

fn read_err(e: std::io::Error) -> CoreError {
    CoreError::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(conf: serde_json::Value, input: &[u8]) -> Result<Vec<String>, CoreError> {
        let s = LinesScanner::from_config(conf.as_object().expect("object"))?;
        let mut out = Vec::new();
        // A 2-byte buffer makes every delimiter cross a refill.
        let mut r = std::io::BufReader::with_capacity(2, input);
        s.scan(&mut r, &mut |b| {
            out.push(String::from_utf8_lossy(&b).into_owned());
            Ok(())
        })?;
        Ok(out)
    }

    // Outputs recorded from Bento 1.22 (`file` input, `lines` scanner) on the same bytes.
    #[test]
    fn default_lines_drop_cr_and_keep_empty_lines() {
        let got = lines(serde_json::json!({}), b"a\r\n\nb\nc").expect("lines");
        assert_eq!(got, ["a", "", "b", "c"]);
        let got = lines(serde_json::json!({"omit_empty": true}), b"a\r\n\nb\nc").expect("lines");
        assert_eq!(got, ["a", "b", "c"]);
    }

    #[test]
    fn a_custom_delimiter_splits_on_its_bytes() {
        let got =
            lines(serde_json::json!({"custom_delimiter": "b"}), b"a\r\n\nb\nc").expect("lines");
        assert_eq!(got, ["a\r\n\n", "\nc"]);
        let got =
            lines(serde_json::json!({"custom_delimiter": "<>"}), b"x<y<>z<><>").expect("lines");
        assert_eq!(got, ["x<y", "z", ""]);
    }

    #[test]
    fn a_line_past_max_buffer_size_is_too_long() {
        let err = lines(
            serde_json::json!({"custom_delimiter": "b", "max_buffer_size": 3}),
            b"a\r\n\nb\nc",
        )
        .expect_err("too long");
        assert_eq!(err.to_string().trim(), "bufio.Scanner: token too long");
        assert!(lines(serde_json::json!({"max_buffer_size": 3}), b"ab\ncd").is_ok());
        let err = LinesScanner::from_config(
            serde_json::json!({"max_buffer_size": 0})
                .as_object()
                .expect("object"),
        )
        .expect_err("refused");
        assert!(err.to_string().contains("max_buffer_size"), "{err}");
    }
}
