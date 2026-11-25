//! A manifest's spec as a bundle carries it: no scalar under a credential key survives.
#![no_main]

use joinedcontext_portal::api::export::strip_secret_values;
use joinedcontext_portal::api::mutate::{is_variable_reference, SECRET_KEYS};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fn clean(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().all(|(key, child)| {
            let literal = match child {
                Value::String(text) => !text.is_empty() && !is_variable_reference(text),
                Value::Number(_) | Value::Bool(_) => true,
                _ => false,
            };
            !(SECRET_KEYS.contains(&key.as_str()) && literal) && clean(child)
        }),
        Value::Array(items) => items.iter().all(clean),
        _ => true,
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(mut spec) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    strip_secret_values(&mut spec);
    assert!(clean(&spec), "{spec}");
});
