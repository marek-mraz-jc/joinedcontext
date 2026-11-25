//! An App's file key (`/apps/{name}/api/files/{key}`): every key the check lets through stays
//! under the App's own prefix: 1 to 512 bytes, relative, no control character or backslash, and
//! no empty, `.` or `..` segment.
#![no_main]

use joinedcontext_portal::apps::files::checked;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(key) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(kept) = checked(key) {
        assert_eq!(kept, key);
        assert!(!key.is_empty() && key.len() <= 512);
        assert!(!key.starts_with('/'));
        assert!(!key.chars().any(|c| c.is_control() || c == '\\'));
        assert!(key
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != ".."));
    }
});
