//! A record the runner posts to the rejected route: masking any JSON keeps its shape, and the
//! text helpers take any string.
#![no_main]

use joinedcontext_portal::pipeline_outcomes::{credential_shaped, json_in_words, mask, mask_text};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(record) = serde_json::from_slice::<serde_json::Value>(data) {
        let masked = mask(&record);
        assert_eq!(masked.is_object(), record.is_object());
        assert_eq!(mask(&masked), masked, "masking twice changes nothing");
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = json_in_words(text);
        let _ = credential_shaped(text);
        let _ = mask_text(text);
    }
});
