// SPDX-License-Identifier: EUPL-1.2
//! A unit a person reads beside each `unitCode` of a tool result.
//!
//! NGSI-LD writes a quantity's unit as a UN/CEFACT Recommendation 20
//! common code (`GQ`), which a model reading the answer takes for "no
//! unit". A tool result gains `"unit": "µg/m³"` beside every code the
//! table knows; the stored entity and the NGSI-LD answers are unchanged.

use serde_json::Value;

/// Rec 20 common code to the symbol, or the name of a unit with none.
static LABELS: &[(&str, &str)] = include!("rec20_labels.rs");

/// The label of `code`, when Rec 20 has the code.
fn label(code: &str) -> Option<&'static str> {
    let at = LABELS.partition_point(|(c, _)| *c < code);
    LABELS
        .get(at)
        .filter(|(c, _)| *c == code)
        .map(|(_, label)| *label)
}

/// The members that hold a client's own data (4.5.2 `value`, 4.5.24 `json`,
/// 4.5.21 `valueList`, the temporalValues pairs): an object in them is no
/// attribute instance, and a `unitCode` in it is data the broker stored.
const DATA_MEMBERS: [&str; 4] = ["value", "json", "valueList", "values"];

/// Adds `unit` beside every known `unitCode` below `value` that has no
/// `unit` of its own, outside the [`DATA_MEMBERS`].
pub(crate) fn name(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let unit = map
                .get("unitCode")
                .and_then(Value::as_str)
                .and_then(label)
                .filter(|_| !map.contains_key("unit"));
            if let Some(unit) = unit {
                map.insert("unit".to_owned(), Value::String(unit.to_owned()));
            }
            map.iter_mut()
                .filter(|(k, _)| !DATA_MEMBERS.contains(&k.as_str()))
                .for_each(|(_, v)| name(v));
        }
        Value::Array(items) => items.iter_mut().for_each(name),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_table_is_sorted_and_each_code_once() {
        assert!(LABELS.is_sorted_by(|a, b| a.0 < b.0));
        assert_eq!(label("GQ"), Some("µg/m³"));
        assert_eq!(label("H87"), Some("piece"));
        // past the last code, and between two codes
        assert_eq!(label("ZZZ9"), None);
        assert_eq!(label("GQA"), None);
    }

    /// F-R2ANT2-018: a `unitCode` inside a client's own value is data,
    /// left as stored.
    #[test]
    fn a_unit_code_inside_a_value_stays_data() {
        let stored = json!({ "id": "urn:ngsi-ld:T:1",
            "doc": { "type": "JsonProperty", "json": { "unitCode": "GQ" } },
            "spec": { "type": "Property", "value": { "unitCode": "GQ" } },
            "list": { "type": "ListProperty", "valueList": [{ "unitCode": "GQ" }] },
            "pm10": { "type": "Property", "values": [[{ "unitCode": "GQ" }, "2026-01-01T00:00:00Z"]] } });
        let mut got = stored.clone();
        name(&mut got);
        assert_eq!(got, stored);
    }

    #[test]
    fn a_known_code_gains_its_label_and_nothing_else_changes() {
        let mut got = json!({ "entities": [{
            "pm10": { "type": "Property", "value": 8.93, "unitCode": "GQ" },
            "co": { "type": "Property", "value": 0.3, "unitCode": "GP" },
            "odd": { "type": "Property", "value": 1, "unitCode": "ZZZ9" },
            "own": { "unitCode": "GQ", "unit": "ug/m3" },
            "notText": { "unitCode": 7 },
            "name": { "type": "Property", "value": "unitCode" },
        }] });
        name(&mut got);
        let at = |path: &str| got.pointer(&format!("/entities/0/{path}")).cloned();
        assert_eq!(at("pm10/unit"), Some(json!("µg/m³")));
        assert_eq!(at("pm10/unitCode"), Some(json!("GQ")));
        assert_eq!(at("co/unit"), Some(json!("mg/m³")));
        assert_eq!(at("own/unit"), Some(json!("ug/m3")));
        for plain in ["odd", "notText", "name"] {
            assert_eq!(at(&format!("{plain}/unit")), None, "{plain}");
        }
    }
}
