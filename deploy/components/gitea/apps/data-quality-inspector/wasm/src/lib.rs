//! Helsinki data quality inspector (T-3336): schema validity, completeness and freshness
//! computed in the visitor's browser from entities and platform JSON schemas.

pub mod quality;
pub mod validate;

#[cfg(feature = "browser")]
use wasm_bindgen::prelude::wasm_bindgen;

pub use quality::{
    inspect_type, inspect_type_with, run, AttributeOutput, FreshnessOutput, Input, Output,
    TypeInput, TypeOutput, FINDINGS_SHOWN,
};
pub use validate::Finding;

/// WASM entry point: reads JSON input and answers JSON output or an error object.
#[cfg_attr(feature = "browser", wasm_bindgen)]
pub fn inspect(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(parsed) => serde_json::to_string(&run(&parsed))
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string()),
        Err(e) => {
            serde_json::json!({ "error": format!("the data could not be read: {e}") }).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inspect_with_valid_input() {
        let input = json!({
            "now": 1700000000.0,
            "types": [
                {
                    "type": "BikeHireDockingStation",
                    "schema": {
                        "properties": {
                            "name": { "type": "string" },
                            "totalSlotNumber": { "type": "integer", "minimum": 0 }
                        },
                        "required": ["name"]
                    },
                    "rows": [
                        { "id": "urn:ngsi-ld:BikeHireDockingStation:1", "name": "Asema", "totalSlotNumber": 20 }
                    ]
                }
            ]
        });
        let raw = inspect(&input.to_string());
        let val: serde_json::Value = serde_json::from_str(&raw).expect("valid json output");
        assert!(val.get("types").is_some());
        let types = val["types"].as_array().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0]["type"], "BikeHireDockingStation");
        assert_eq!(types[0]["entities"], 1);
        assert_eq!(types[0]["valid"], 1.0);
    }

    #[test]
    fn inspect_with_bad_json_returns_error_object() {
        let raw = inspect("this is not json");
        let val: serde_json::Value = serde_json::from_str(&raw).expect("valid json error");
        assert!(val.get("error").is_some());
        assert!(val["error"]
            .as_str()
            .unwrap()
            .starts_with("the data could not be read"));
    }

    #[test]
    fn odd_values_are_findings_never_a_panic() {
        // In the browser a panic aborts the worker (panic = "abort"), so every shape a feed can hand
        // over must come back as an answer.
        let schema = serde_json::json!({
            "properties": {
                "dateModified": { "type": ["string", "null"], "format": "date-time" },
                "code": { "type": "string", "pattern": "^[0-9]{1,10}$" },
                "day": { "type": "string", "format": "date" },
                "url": { "type": "string", "format": "uri" }
            },
            "additionalProperties": false
        });
        let odd = [
            serde_json::json!("2026-10-08T12:00:00ZZZZZZZZZZZZZ"),
            serde_json::json!("ääääääääääääääääääääää"),
            serde_json::json!("2026-1"),
            serde_json::json!(1e308),
            serde_json::json!(-0.0),
            serde_json::json!([[[[]]]]),
            serde_json::json!({ "a": { "b": null } }),
            serde_json::json!(""),
        ];
        let rows: Vec<serde_json::Value> = odd
            .iter()
            .enumerate()
            .map(|(i, v)| serde_json::json!({ "id": format!("e{i}"), "type": "T", "dateModified": v, "code": v, "day": v, "url": v }))
            .collect();
        let input = serde_json::json!({ "now": 1_791_460_900.0, "types": [{ "type": "T", "schema": schema, "rows": rows }] });
        let out: serde_json::Value =
            serde_json::from_str(&inspect(&input.to_string())).expect("an answer");
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(out["types"][0]["entities"], 8);
    }
}
