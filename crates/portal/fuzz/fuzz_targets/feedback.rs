//! A feedback body's text, page and screenshot: the scrub keeps no address and takes any
//! string, a kept page is one plain path, a kept screenshot is a PNG.
#![no_main]

use joinedcontext_portal::api::feedback::{page_of, screenshot_of, scrub};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let scrubbed = scrub(text);
    assert_eq!(scrub(&scrubbed), scrubbed, "scrubbing twice changes nothing");
    if let Ok(page) = page_of(text) {
        assert!(page.starts_with('/'), "{page:?}");
        assert!(!page.contains(['?', '#', '@', ' ', '\n']), "{page:?}");
        assert_eq!(page_of(&page).ok().as_deref(), Some(page.as_str()), "a kept page stays");
    }
    if let Ok(bytes) = screenshot_of(text) {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
});
