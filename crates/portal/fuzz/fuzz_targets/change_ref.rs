//! A Change id from a request path and a branch name from a forge webhook: either parses into
//! what it names or is refused, and an id that parses is the same id formatted back.
#![no_main]

use joinedcontext_portal::api::changes::{parse_branch_name, parse_change_id, parse_change_ref};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok((organization, number)) = parse_change_ref(text) {
        let prefix = if organization { "chg-org-" } else { "chg-" };
        assert_eq!(format!("{prefix}{number:08x}"), text);
        assert_eq!(parse_change_id(text).is_ok(), !organization);
    }
    if let Some(info) = parse_branch_name(text) {
        assert!(!info.resource_name.is_empty(), "{text}");
    }
});
