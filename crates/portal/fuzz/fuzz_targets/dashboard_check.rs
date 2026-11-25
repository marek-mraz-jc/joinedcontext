//! An App's dashboard file as a person edits it: any JSON yields a list of refusals, never a
//! panic, each naming a path under `config`, and a file that is no object is refused whole.
#![no_main]

use joinedcontext_portal::api::app_dashboard::check;
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let before = json!({ "grid": { "source": "s", "type": "T" } });
    let stored = ["a".to_owned(), "b".to_owned(), "count_1".to_owned()];
    let found = check(&raw, &before, &stored);
    for refusal in &found {
        assert!(refusal.starts_with("config"), "{refusal}");
        assert!(refusal.contains(": "), "{refusal}");
    }
    if !raw.is_object() {
        assert_eq!(found, ["config: must be an object"]);
    }
});
