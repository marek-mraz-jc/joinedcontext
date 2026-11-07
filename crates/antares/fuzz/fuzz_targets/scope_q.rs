//! 4.19 scopeQ evaluation: never panics on arbitrary query or document.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let doc = serde_json::json!({"scope": ["/a/b", "/c"]});
        let _ = antares_api::scope_matches(s, &doc);
        // the 4.19 grammar check (T-3912) answers Ok or a typed 400/403
        if let Err(e) = antares_ql::scope::check_scope_q(s) {
            assert!(matches!(e.status(), 400 | 403));
        }
        // and the query side against a scope taken from the input itself
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            let _ = antares_api::scope_matches("/a/#", &v);
        }
    }
});
