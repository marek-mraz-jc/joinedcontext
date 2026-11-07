// SPDX-License-Identifier: EUPL-1.2
//! The MQTT 3.1.1 (OASIS, 2014) packets the `SensorThings` MQTT extension reads and writes: a
//! client sends CONNECT, SUBSCRIBE, UNSUBSCRIBE, `PINGREQ` and DISCONNECT; the server answers
//! `CONNACK`, `SUBACK`, `UNSUBACK` and `PINGRESP` and publishes at `QoS` 0. A client PUBLISH is read only
//! to be refused (no `SensorThings` writes, T-5911).

/// The largest packet a client may send: a SUBSCRIBE of a few topics fits many times over.
pub(crate) const MAX_PACKET: usize = 64 * 1024;

/// `CONNACK` return codes (3.2.2.3).
pub(crate) mod rc {
    pub const ACCEPTED: u8 = 0;
    pub const BAD_PROTOCOL: u8 = 1;
    pub const NOT_AUTHORIZED: u8 = 5;
}

/// The `SUBACK` code of a refused topic filter (3.9.3).
pub(crate) const SUBACK_FAILURE: u8 = 0x80;

/// One client packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Packet {
    /// CONNECT: the protocol level, the keep alive and whether it carries a Will.
    Connect {
        level: u8,
        keep_alive: u16,
        will: bool,
    },
    /// SUBSCRIBE: its packet id and topic filters (the requested `QoS` is not kept: every grant
    /// is `QoS` 0).
    Subscribe {
        id: u16,
        topics: Vec<String>,
    },
    /// UNSUBSCRIBE: its packet id and topic filters.
    Unsubscribe {
        id: u16,
        topics: Vec<String>,
    },
    PingReq,
    Disconnect,
    /// PUBLISH: always refused.
    Publish,
}

/// Why the front of a buffer is not a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bad {
    /// More bytes are needed.
    Incomplete,
    /// Past [`MAX_PACKET`].
    TooBig,
    /// A malformed packet (4.8: the server closes the connection).
    Malformed(&'static str),
}

/// The bytes left of a packet, read from the front.
struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    fn u8(&mut self) -> Result<u8, Bad> {
        let (&b, rest) = self.0.split_first().ok_or(Bad::Malformed("short packet"))?;
        self.0 = rest;
        Ok(b)
    }

    fn u16(&mut self) -> Result<u16, Bad> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }

    fn bytes(&mut self) -> Result<&'a [u8], Bad> {
        let n = usize::from(self.u16()?);
        if self.0.len() < n {
            return Err(Bad::Malformed("short packet"));
        }
        let (b, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(b)
    }

    /// A UTF-8 string (1.5.3): no U+0000.
    fn string(&mut self) -> Result<String, Bad> {
        let text = std::str::from_utf8(self.bytes()?).map_err(|_| Bad::Malformed("not UTF-8"))?;
        if text.contains('\0') {
            return Err(Bad::Malformed("U+0000 in a string"));
        }
        Ok(text.to_owned())
    }
}

/// The remaining length at the front of `buf` (2.2.3) and the bytes it took.
fn remaining(buf: &[u8]) -> Result<(usize, usize), Bad> {
    let mut value = 0usize;
    for (i, &b) in buf.iter().enumerate().take(4) {
        let shift = u32::try_from(i.saturating_mul(7)).unwrap_or(u32::MAX);
        value |= usize::from(b & 0x7f).checked_shl(shift).unwrap_or(0);
        if b & 0x80 == 0 {
            return Ok((value, i.saturating_add(1)));
        }
    }
    if buf.len() >= 4 {
        Err(Bad::Malformed("remaining length"))
    } else {
        Err(Bad::Incomplete)
    }
}

/// The packet at the front of `buf` and its length in bytes.
pub(crate) fn read(buf: &[u8]) -> Result<(Packet, usize), Bad> {
    let (&first, rest) = buf.split_first().ok_or(Bad::Incomplete)?;
    let (len, took) = remaining(rest)?;
    if len > MAX_PACKET {
        return Err(Bad::TooBig);
    }
    let body = rest
        .get(took..took.saturating_add(len))
        .ok_or(Bad::Incomplete)?;
    let total = took.saturating_add(len).saturating_add(1);
    let mut cur = Cur(body);
    let packet = match first {
        0x10 => connect(&mut cur)?,
        0x82 => {
            let id = cur.u16()?;
            let mut topics = Vec::new();
            while !cur.0.is_empty() {
                topics.push(cur.string()?);
                if cur.u8()? > 2 {
                    return Err(Bad::Malformed("a QoS above 2"));
                }
            }
            if topics.is_empty() {
                return Err(Bad::Malformed("a SUBSCRIBE without a topic"));
            }
            Packet::Subscribe { id, topics }
        }
        0xa2 => {
            let id = cur.u16()?;
            let mut topics = Vec::new();
            while !cur.0.is_empty() {
                topics.push(cur.string()?);
            }
            if topics.is_empty() {
                return Err(Bad::Malformed("an UNSUBSCRIBE without a topic"));
            }
            Packet::Unsubscribe { id, topics }
        }
        0xc0 if len == 0 => Packet::PingReq,
        0xe0 if len == 0 => Packet::Disconnect,
        b if b >> 4 == 3 => Packet::Publish,
        _ => return Err(Bad::Malformed("not a packet a client sends a server")),
    };
    Ok((packet, total))
}

fn connect(cur: &mut Cur<'_>) -> Result<Packet, Bad> {
    let name = cur.string()?;
    let level = cur.u8()?;
    let flags = cur.u8()?;
    let keep_alive = cur.u16()?;
    if name != "MQTT" {
        return Err(Bad::Malformed("the protocol name is MQTT"));
    }
    if flags & 1 != 0 {
        return Err(Bad::Malformed("the reserved CONNECT flag is set"));
    }
    let will = flags & 0x04 != 0;
    // the payload is read to its end so a malformed one is refused; the client id and the
    // credentials are not kept: the identity is the upgrade request's (docs/src/sta.md)
    let _client = cur.string()?;
    if will {
        let _topic = cur.string()?;
        let _message = cur.bytes()?;
    }
    if flags & 0x80 != 0 {
        let _user = cur.string()?;
    }
    if flags & 0x40 != 0 {
        let _password = cur.bytes()?;
    }
    if !cur.0.is_empty() {
        return Err(Bad::Malformed("bytes after the CONNECT payload"));
    }
    Ok(Packet::Connect {
        level,
        keep_alive,
        will,
    })
}

/// A fixed header and `body`.
fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len().saturating_add(5));
    out.push(first);
    let mut n = body.len();
    loop {
        let byte = u8::try_from(n & 0x7f).unwrap_or(0);
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(body);
    out
}

pub(crate) fn connack(code: u8) -> Vec<u8> {
    packet(0x20, &[0, code])
}

pub(crate) fn suback(id: u16, codes: &[u8]) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    body.extend_from_slice(codes);
    packet(0x90, &body)
}

pub(crate) fn unsuback(id: u16) -> Vec<u8> {
    packet(0xb0, &id.to_be_bytes())
}

pub(crate) fn pingresp() -> Vec<u8> {
    packet(0xd0, &[])
}

/// A `QoS` 0 PUBLISH of `payload` on `topic`, a topic filter this extension read (at most 1024
/// bytes, so its length fits a string's).
pub(crate) fn publish(topic: &str, payload: &[u8]) -> Vec<u8> {
    let n = u16::try_from(topic.len()).unwrap_or(u16::MAX);
    let mut body = Vec::with_capacity(topic.len().saturating_add(payload.len()).saturating_add(2));
    body.extend_from_slice(&n.to_be_bytes());
    body.extend_from_slice(topic.as_bytes());
    body.extend_from_slice(payload);
    packet(0x30, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(s: &str) -> Vec<u8> {
        let mut out = u16::try_from(s.len()).unwrap_or(0).to_be_bytes().to_vec();
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn connect_bytes(flags: u8, extra: &[&str]) -> Vec<u8> {
        let mut body = string("MQTT");
        body.extend_from_slice(&[4, flags, 0, 30]);
        body.extend(string("client"));
        for e in extra {
            body.extend(string(e));
        }
        packet(0x10, &body)
    }

    #[test]
    fn a_connect_reads_with_its_credentials_dropped() {
        let bytes = connect_bytes(0xc2, &["user", "secret"]);
        let read = read(&bytes);
        let want = Packet::Connect {
            level: 4,
            keep_alive: 30,
            will: false,
        };
        assert_eq!(read, Ok((want, bytes.len())));
    }

    #[test]
    fn a_will_is_read_and_reported() {
        let bytes = connect_bytes(0x04, &["v1.1/Things", "x"]);
        let will = matches!(read(&bytes), Ok((Packet::Connect { will: true, .. }, _)));
        assert!(will);
    }

    #[test]
    fn a_short_buffer_is_incomplete_and_a_wrong_one_malformed() {
        let bytes = connect_bytes(0x02, &[]);
        for cut in 0..bytes.len() {
            assert_eq!(
                read(bytes.get(..cut).unwrap_or_default()).err(),
                Some(Bad::Incomplete),
                "{cut}"
            );
        }
        let reserved = connect_bytes(0x03, &[]);
        assert!(matches!(read(&reserved), Err(Bad::Malformed(_))));
        assert!(matches!(read(&[0xc1, 0]), Err(Bad::Malformed(_))));
        assert!(matches!(
            read(&[0x82, 0xff, 0xff, 0xff, 0xff]),
            Err(Bad::Malformed(_))
        ));
        assert_eq!(read(&[0x82, 0xff, 0xff, 0x7f]).err(), Some(Bad::TooBig));
    }

    #[test]
    fn subscribe_unsubscribe_ping_and_publish_read() {
        let mut body = vec![0, 7];
        body.extend(string("v1.1/Things"));
        body.push(1);
        body.extend(string("v1.1/Datastreams"));
        body.push(0);
        let sub = packet(0x82, &body);
        let topics = vec!["v1.1/Things".to_owned(), "v1.1/Datastreams".to_owned()];
        assert_eq!(
            read(&sub),
            Ok((Packet::Subscribe { id: 7, topics }, sub.len()))
        );
        let mut body = vec![0, 8];
        body.extend(string("v1.1/Things"));
        let unsub = packet(0xa2, &body);
        let topics = vec!["v1.1/Things".to_owned()];
        assert_eq!(
            read(&unsub),
            Ok((Packet::Unsubscribe { id: 8, topics }, unsub.len()))
        );
        assert_eq!(read(&[0xc0, 0]), Ok((Packet::PingReq, 2)));
        assert_eq!(read(&[0xe0, 0]), Ok((Packet::Disconnect, 2)));
        let publish = publish("v1.1/Things", b"{}");
        assert_eq!(read(&publish), Ok((Packet::Publish, publish.len())));
        assert!(matches!(read(&[0x82, 2, 0, 1]), Err(Bad::Malformed(_))));
    }

    #[test]
    fn every_malformed_packet_is_named() {
        let mut nul = string("MQTT");
        nul.extend_from_slice(&[4, 2, 0, 0]);
        nul.extend(string("a\0b"));
        let mut qos = vec![0, 1];
        qos.extend(string("t"));
        qos.push(3);
        let mut trailing = string("MQTT");
        trailing.extend_from_slice(&[4, 2, 0, 0]);
        trailing.extend(string("c"));
        trailing.push(9);
        let mut name = string("MQXX");
        name.extend_from_slice(&[4, 2, 0, 0]);
        name.extend(string("c"));
        for (bytes, why) in [
            (packet(0x10, &[0, 9, b'M']), "short packet"),
            (packet(0x10, &nul), "U+0000 in a string"),
            (packet(0x82, &qos), "a QoS above 2"),
            (packet(0xa2, &[0, 1]), "an UNSUBSCRIBE without a topic"),
            (packet(0xc0, &[0]), "not a packet a client sends a server"),
            (packet(0xe0, &[0]), "not a packet a client sends a server"),
            (packet(0x10, &name), "the protocol name is MQTT"),
            (packet(0x10, &trailing), "bytes after the CONNECT payload"),
        ] {
            assert_eq!(read(&bytes).err(), Some(Bad::Malformed(why)), "{why}");
        }
    }

    #[test]
    fn a_long_payload_takes_a_multi_byte_length() {
        let payload = vec![b'x'; 200];
        let out = publish("t", &payload);
        assert_eq!(out.get(..3), Some(&[0x30, 0xcb, 0x01][..]));
        assert_eq!(out.len(), 3 + 2 + 1 + 200);
        assert_eq!(connack(rc::NOT_AUTHORIZED), vec![0x20, 2, 0, 5]);
        assert_eq!(
            suback(1, &[0, SUBACK_FAILURE]),
            vec![0x90, 4, 0, 1, 0, 0x80]
        );
        assert_eq!(unsuback(1), vec![0xb0, 2, 0, 1]);
        assert_eq!(pingresp(), vec![0xd0, 0]);
    }
}
