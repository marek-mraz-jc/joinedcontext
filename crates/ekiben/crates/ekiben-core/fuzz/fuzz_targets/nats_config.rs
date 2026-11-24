//! Input: a nats component config as JSON, then a subject and a bucket name from the same bytes.
//! The config parses or is refused; a subject accepted under an organization prefix stays inside
//! it, and so does a bucket name; subject matching never panics.
#![no_main]

use ekiben_core::{OrgContext, OrgId, OrgQuotas};
use ekiben_io_nats::connection::{subject_matches, NatsConnectionDetails};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let org = OrgContext::new(
        OrgId::new("fuzz"),
        OrgQuotas {
            nats_subject_prefix: Some("org_f".into()),
            ..OrgQuotas::default()
        },
    );
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Ok(c) = NatsConnectionDetails::from_json(&v, org.clone(), None) {
            assert!(!c.urls.is_empty());
            let _ = c.tls.client_config();
        }
    }
    let (a, b) = text.split_once('\n').unwrap_or((text, ""));
    let _ = subject_matches(a, b);
    if let Ok(subject) = org.validate_nats_subject(a) {
        assert!(
            subject == "org_f" || subject.starts_with("org_f."),
            "{subject}"
        );
    }
    if org.validate_nats_bucket(b).is_ok() {
        assert!(b.starts_with("org_f"), "{b}");
    }
});
