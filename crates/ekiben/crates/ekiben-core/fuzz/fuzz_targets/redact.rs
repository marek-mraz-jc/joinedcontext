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
            // the same text may sit in the path or query; only the userinfo is a secret
            let shown_pw = url::Url::parse(&shown)
                .ok()
                .and_then(|u| u.password().map(str::to_owned));
            assert_ne!(shown_pw.as_deref(), Some(pw), "{text}");
        }
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        let out = redact_config_value(&value);
        assert_eq!(out.is_object(), value.is_object());
        assert_eq!(out.is_array(), value.is_array());
    }
});
