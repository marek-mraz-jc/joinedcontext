//! Any `Link` header, any body and any model: the context guard and the unit check never panic,
//! the context guard refuses only with its one message, and a filled quantity carries the model's
//! code afterwards (T-7805).
#![no_main]

use http::header::LINK;
use http::{HeaderMap, HeaderValue};
use jc_core::kinds::MissingUnitCode;
use jc_core::ProblemDetails;
use jc_ngsi_input::context_guard::{check_body, check_link, REFUSAL};
use jc_ngsi_input::units::{enforce, Shape, UnitRules};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    // The Link header's bytes, NUL, the body, NUL, the model schema.
    let mut parts = data.splitn(3, |b| *b == 0);
    let (link, body) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let schema = parts.next().unwrap_or_default();
    if let Ok(value) = HeaderValue::from_bytes(link) {
        let mut headers = HeaderMap::new();
        headers.append(LINK, value);
        if let Err(words) = check_link(&headers) {
            assert_eq!(words, REFUSAL);
        }
    }
    if let Err(words) = check_body(body) {
        assert_eq!(words, REFUSAL);
    }
    let Ok(schema) = serde_json::from_slice::<Value>(schema) else {
        return;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    for missing in [MissingUnitCode::Fill, MissingUnitCode::Refuse] {
        let rules = UnitRules::from_schema(&schema, missing);
        let mut written = payload.clone();
        let shape = Shape { addressed: None, targeted: None };
        match enforce(&mut written, shape, &rules) {
            Ok(filled) => {
                if missing == MissingUnitCode::Refuse {
                    assert!(filled.is_empty(), "a refusing space fills nothing");
                }
                for note in &filled {
                    let (attribute, code) = note.split_once('=').expect("attribute=code");
                    assert!(rules.by_class.values().any(|units| units.get(attribute).map(String::as_str) == Some(code)));
                }
                if filled.is_empty() {
                    assert_eq!(written, payload, "nothing filled, nothing changed");
                }
            }
            Err(refusal) => {
                assert_eq!(ProblemDetails::from(refusal).status, 400);
            }
        }
    }
});
