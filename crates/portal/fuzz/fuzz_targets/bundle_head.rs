//! A git bundle's header as the forge hands it back: a head is 40 hex digits the header names.
#![no_main]

use joinedcontext_portal::api::export_git::bundle_head;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(head) = bundle_head(data) {
        assert_eq!(head.len(), 40);
        assert!(head.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(data.starts_with(b"# v2 git bundle"));
        assert!(data
            .windows(head.len() + 5)
            .any(|w| w == format!("{head} HEAD").as_bytes()));
    }
});
