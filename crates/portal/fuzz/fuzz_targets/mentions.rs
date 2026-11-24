//! A comment's text: every mention it yields is a lower-case identifier the text writes after an
//! `@`, named once, never an audience (`@@x`).
#![no_main]

use joinedcontext_portal::api::entity_comments::mentions_in;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let found = mentions_in(text);
    let lower = text.to_ascii_lowercase();
    for (i, mention) in found.iter().enumerate() {
        assert!(!mention.is_empty() && mention.len() <= 256, "{mention:?}");
        assert!(!mention.starts_with('@'), "{mention:?}");
        assert_eq!(mention, &mention.to_ascii_lowercase());
        assert!(
            lower.contains(&format!("@{mention}")),
            "{mention:?} in {text:?}"
        );
        assert!(!found[..i].contains(mention), "{mention:?} twice");
    }
});
