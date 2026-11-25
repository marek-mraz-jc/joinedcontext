//! An export's query string: the `revision` a forge URL carries and the `kinds` filter.
#![no_main]

use joinedcontext_portal::api::export::{is_commit, kind_filter, valid_revision};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if valid_revision(text) {
        assert!(!text.is_empty() && text.len() <= 128, "{text:?}");
        assert!(!text.starts_with('-') && !text.contains(".."), "{text:?}");
        assert!(
            text.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')),
            "{text:?}"
        );
    }
    if is_commit(text) {
        assert!(valid_revision(text), "a commit is a revision: {text:?}");
    }
    if let Some(kinds) = kind_filter(Some(text)) {
        assert!(!kinds.is_empty());
        for kind in kinds {
            assert!(
                kind.starts_with("unknown:")
                    || joinedcontext_portal::resource::by_kind(&kind).is_some(),
                "{kind:?}"
            );
        }
    }
});
