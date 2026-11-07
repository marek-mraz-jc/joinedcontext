// SPDX-License-Identifier: EUPL-1.2
//! The few lines of RFC 6455 the broker's WebSocket tests need (a
//! handshake, masked text frames out, unmasked frames in), so the broker
//! gains no dependency, development ones included: its tree is the vanilla
//! gate's. Included by `#[path]` from the tests that open a socket.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A client socket after a 101.
pub struct Socket(TcpStream);

impl Socket {
    /// Upgrade `path` on `port` with `headers`; [`Self::next`] gives up after
    /// `silence`.
    pub fn open(
        port: u16,
        path: &str,
        headers: &[(String, String)],
        silence: Duration,
    ) -> Result<Self, String> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(silence))
            .map_err(|e| e.to_string())?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{}\r\n",
            headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>()
        );
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        // the answer's head, a byte at a time so no frame is read with it
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut byte)
                .map_err(|e| format!("upgrade: {e}"))?;
            head.extend_from_slice(&byte);
        }
        if !head.starts_with(b"HTTP/1.1 101") {
            return Err(format!("upgrade: {}", String::from_utf8_lossy(&head)));
        }
        Ok(Self(s))
    }

    /// One masked text frame (RFC 6455 5.2: a client masks).
    pub fn send(&mut self, v: &Value) -> Result<(), String> {
        let payload = v.to_string().into_bytes();
        let key = [0x12u8, 0x34, 0x56, 0x78];
        let mut frame = vec![0x81u8];
        match u8::try_from(payload.len()) {
            Ok(n) if n < 126 => frame.push(0x80 | n),
            _ => {
                let n = u16::try_from(payload.len()).map_err(|e| e.to_string())?;
                frame.push(0x80 | 126);
                frame.extend_from_slice(&n.to_be_bytes());
            }
        }
        frame.extend_from_slice(&key);
        frame.extend(payload.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
        self.0.write_all(&frame).map_err(|e| e.to_string())
    }

    /// The next text message as JSON; `None` on silence or an end.
    /// Control frames (Ping) are skipped; the server sends unmasked frames.
    pub fn next(&mut self) -> Option<Value> {
        loop {
            let mut h = [0u8; 2];
            self.0.read_exact(&mut h).ok()?;
            let [first, second] = h;
            let len = match second & 0x7f {
                126 => {
                    let mut n = [0u8; 2];
                    self.0.read_exact(&mut n).ok()?;
                    u64::from(u16::from_be_bytes(n))
                }
                127 => {
                    let mut n = [0u8; 8];
                    self.0.read_exact(&mut n).ok()?;
                    u64::from_be_bytes(n)
                }
                n => u64::from(n),
            };
            let mut payload = vec![0u8; usize::try_from(len).ok()?];
            self.0.read_exact(&mut payload).ok()?;
            match first & 0x0f {
                0x1 => return serde_json::from_slice(&payload).ok(),
                0x8 => return None,
                _ => {}
            }
        }
    }
}
