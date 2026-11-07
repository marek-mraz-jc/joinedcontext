// SPDX-License-Identifier: EUPL-1.2
//! The parsers a client's bytes reach, for the fuzz targets alone (feature
//! `fuzzing`; MC20 and the tool arguments of MCP D-4).

use crate::cursor;
use crate::schema::{self, Names};
use crate::tools::{self, TOOLS};

/// Check `given` against every tool's arguments over a view showing `Car`
/// and `speed`, and build the request of each call that passes.
///
/// # Panics
/// When a built request leaves the NGSI-LD API root: the finding.
///
/// ```
/// let args = serde_json::json!({"type": ["Car"], "limit": 5});
/// antares_mcp::fuzzing::tool_arguments(&args.as_object().cloned().unwrap_or_default());
/// ```
pub fn tool_arguments(given: &serde_json::Map<String, serde_json::Value>) {
    let names = Names::new(vec!["Car".into()], vec!["speed".into()]);
    for tool in &TOOLS {
        if let Ok(args) = schema::check_all(tool.args, given, &names) {
            let built = tools::build(tool, &args, 20, 0);
            assert!(built.target.starts_with("/ngsi-ld/v1/"), "{}", built.target);
        }
    }
}

/// Decode `raw` as a cursor of every tool; a cursor this crate encodes
/// always decodes to its offset.
///
/// # Panics
/// When a cursor this crate made is too long or decodes to another
/// offset: the finding.
///
/// ```
/// antares_mcp::fuzzing::cursor("31", 7);
/// ```
pub fn cursor(raw: &str, offset: u64) {
    let args = schema::Args::new();
    for tool in &TOOLS {
        let digest = cursor::digest(tool.name, &args);
        let _ = cursor::decode(raw, tool.name, digest);
        let made = cursor::encode(tool.name, digest, offset);
        assert!(made.len() <= cursor::MAX_LEN);
        assert_eq!(cursor::decode(&made, tool.name, digest), Ok(offset));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fuzz_entries_hold_on_valid_and_crafted_input() {
        for args in [
            serde_json::json!({"type": ["Car"], "id": ["urn:ngsi-ld:Car:1"], "attrs": ["speed"]}),
            serde_json::json!({"id": "urn:ngsi-ld:Car:1", "timerel": "after", "timeAt": "2026-01-01T00:00:00Z"}),
            serde_json::json!({"type": "Car", "attrId": "speed"}),
            serde_json::json!({"tenant": "other"}),
        ] {
            super::tool_arguments(args.as_object().unwrap_or(&serde_json::Map::new()));
        }
        for (raw, offset) in [("", 0), ("31", 7), ("zz", u64::MAX)] {
            super::cursor(raw, offset);
        }
    }
}
