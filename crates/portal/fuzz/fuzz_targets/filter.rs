//! `filter` and `orderBy` of a list request, against the Pipeline kind's schema: any text parses
//! or is refused with a position inside it, and a parsed filter runs over a manifest.
#![no_main]

use joinedcontext_portal::resource::filter::{Filter, OrderBy};
use libfuzzer_sys::fuzz_target;
use serde_json::json;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let schema = json!({"type": "object", "properties": {
        "metadata": {"type": "object", "properties": {
            "name": {"type": "string"},
            "labels": {"type": "object", "additionalProperties": {"type": "string"}}}},
        "spec": {"type": "object", "properties": {
            "period": {"type": "integer"},
            "steps": {"type": "array", "items": {"type": "object", "properties": {
                "kind": {"type": "string"}}}}}}}});
    let doc = json!({"metadata": {"name": "a", "labels": {"x": "y"}},
        "spec": {"period": 60, "steps": [{"kind": "map"}]}});
    for schema in [None, Some(&schema)] {
        match Filter::parse(text, schema) {
            Ok(filter) => {
                let _ = filter.matches(&doc);
            }
            Err(e) => assert!(e.position <= text.len(), "{e:?} past the text"),
        }
        if let Ok(order) = OrderBy::parse(text, schema) {
            let key = order.key_of(&doc);
            let _ = order.cmp((&key, "a"), (&serde_json::Value::Null, "b"));
        }
    }
});
