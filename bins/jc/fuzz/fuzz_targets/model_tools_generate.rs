//! Any request body or LinkML text: `/generate` never panics, answers 200 or 400 with an
//! `errors` list, and a 200 without errors carries every artifact a save commits (T-7641).
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // A body that parses as a JSON object is sent as one (imports included); any other text is
    // the LinkML source itself.
    let body = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(body)) => body,
        _ => {
            let mut body = Map::new();
            body.insert("source".to_owned(), Value::String(text.to_owned()));
            body
        }
    };
    let (status, out) = jc::model_tools_generate(&body);
    assert!(status == 200 || status == 400, "{status}");
    let errors = out["errors"]
        .as_array()
        .expect("every answer names its errors");
    assert!(errors.iter().all(Value::is_string), "{out}");
    if status == 400 {
        assert!(!errors.is_empty(), "a 400 says why");
        return;
    }
    assert!(out["generatorVersion"].is_string(), "{out}");
    if errors.is_empty() {
        for field in [
            "context",
            "brokerSchema",
            "jsonSchema",
            "docs",
            "example",
            "typescript",
        ] {
            assert!(out.get(field).is_some(), "{field} missing from {out}");
        }
    }
});
