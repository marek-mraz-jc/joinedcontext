//! Input: JSON `[declared, live]` of a seed entity, an export spec, and a store prefix argument.
//! An entity always equals itself; a prefix only ever names two base32 ids.
#![no_main]

use jcctl::commands::artifacts::project_prefix;
use jcctl::commands::export::literal_credentials;
use jcctl::csr::registration_id;
use jcctl::entities::{action, Action};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(prefix) = project_prefix(&text) {
        assert!(!prefix.contains(".."), "{prefix}");
        assert_eq!(prefix.len(), "org//project//".len() + 52, "{prefix}");
    }
    let _ = registration_id(&text);
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let _ = literal_credentials(&value);
    assert_eq!(action(&value, None), Action::Create);
    if value.is_object() {
        assert_eq!(action(&value, Some(&value)), Action::Unchanged);
    }
    if let Value::Array(pair) = &value {
        if let [declared, live] = pair.as_slice() {
            let _ = action(declared, Some(live));
        }
    }
});
