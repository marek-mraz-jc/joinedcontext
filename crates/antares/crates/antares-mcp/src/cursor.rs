// SPDX-License-Identifier: EUPL-1.2
//! The paging cursor (MCP section 8, MC20): opaque to the client, bound to
//! the tool and its arguments, at most [`MAX_LEN`] characters. It carries
//! no integrity tag: a forged one can only ask another offset of the same
//! gated query, which the caller may ask anyway.

use std::hash::{Hash, Hasher};

/// The longest cursor accepted.
pub(crate) const MAX_LEN: usize = 2048;

/// Why a cursor is refused.
pub(crate) const REFUSED: &str = "argument cursor: not a cursor of this tool and these arguments";

/// The digest of a tool's checked arguments a cursor is bound to.
pub(crate) fn digest(tool: &str, args: &crate::schema::Args) -> u64 {
    // ponytail: the standard library's hasher with its fixed keys; the
    // cursor is not a secret and the same binary on every pod computes the
    // same digest.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tool.hash(&mut h);
    format!("{args:?}").hash(&mut h);
    h.finish()
}

/// The cursor of the row at `offset` of `tool` under `digest`.
pub(crate) fn encode(tool: &str, digest: u64, offset: u64) -> String {
    hex(&format!("1:{tool}:{digest:016x}:{offset}"))
}

fn hex(plain: &str) -> String {
    use std::fmt::Write as _;
    plain.bytes().fold(
        String::with_capacity(plain.len().saturating_mul(2)),
        |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        },
    )
}

/// The offset `cursor` names for `tool` under `digest`.
pub(crate) fn decode(cursor: &str, tool: &str, digest: u64) -> Result<u64, &'static str> {
    if cursor.len() > MAX_LEN || !cursor.len().is_multiple_of(2) {
        return Err(REFUSED);
    }
    let bytes: Vec<u8> = (0..cursor.len())
        .step_by(2)
        .map(|i| {
            cursor
                .get(i..i.saturating_add(2))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect::<Option<_>>()
        .ok_or(REFUSED)?;
    let plain = String::from_utf8(bytes).map_err(|_| REFUSED)?;
    let mut parts = plain.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("1"), Some(t), Some(d), Some(offset))
            if t == tool && u64::from_str_radix(d, 16) == Ok(digest) =>
        {
            offset.parse().map_err(|_| REFUSED)
        }
        _ => Err(REFUSED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Args, Checked};

    #[test]
    fn a_cursor_names_its_offset_for_its_tool_and_arguments_only() {
        let mut args = Args::new();
        args.insert("type", Checked::One("Car".into()));
        let d = digest("query_entities", &args);
        let c = encode("query_entities", d, 40);
        assert_eq!(decode(&c, "query_entities", d), Ok(40));
        assert_eq!(decode(&c, "query_temporal", d), Err(REFUSED));
        args.insert("q", Checked::One("speed>1".into()));
        assert_ne!(digest("query_entities", &args), d);
        assert_eq!(
            decode(&c, "query_entities", digest("query_entities", &args)),
            Err(REFUSED)
        );
    }

    #[test]
    fn a_crafted_cursor_is_refused() {
        let d = digest("t", &Args::new());
        for bad in [
            String::new(),
            "z".into(),
            "zz".into(),
            "ff".repeat(4),
            "0".repeat(MAX_LEN + 2),
            encode("t", d, 1).replace("31", "32"),
            hex("1:t:0000000000000000:-1"),
        ] {
            assert_eq!(decode(&bad, "t", d), Err(REFUSED), "{bad}");
        }
    }
}
