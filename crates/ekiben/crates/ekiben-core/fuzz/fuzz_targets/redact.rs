//! Input: a URL or a JSON config shown to a person. Redaction never panics, a URL's parsed
//! password never survives it, and a config keeps its shape.
#![no_main]

use ekiben_core::{redact_config_value, redact_url_str};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let shown = redact_url_str(text);
    if let Ok(parsed) = url::Url::parse(text) {
        if let Some(pw) = parsed.password().filter(|p| p.len() > 8) {
            // the userinfo copy must go; copies of the same text in the path or query may stay.
            // Counted on the serialized input, where the password is written as password() reads.
            assert!(
                shown.matches(pw).count() < parsed.as_str().matches(pw).count(),
                "{text}"
            );
            let shown_url = url::Url::parse(&shown).expect("redacted URL parses");
            assert_eq!(
                shown_url.password(),
                Some(ekiben_core::redact::REDACTED_PLACEHOLDER),
                "{text}"
            );
        }
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        let out = redact_config_value(&value);
        assert_eq!(out.is_object(), value.is_object());
        assert_eq!(out.is_array(), value.is_array());
    }
});
