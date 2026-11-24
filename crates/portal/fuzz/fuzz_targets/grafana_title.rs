//! A visualization's title as a person types it: a kept one is trimmed, short and printable.
#![no_main]

use joinedcontext_portal::api::grafana::title_of;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(title) = title_of(text) {
        assert_eq!(title, title.trim());
        assert!(
            !title.is_empty() && title.chars().count() <= 120,
            "{title:?}"
        );
        assert!(!title.chars().any(char::is_control), "{title:?}");
    }
});
