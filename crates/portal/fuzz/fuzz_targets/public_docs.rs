//! Input: `<chapter>/<page>` as a request path names a public documentation page (T-7729): a
//! page outside the public chapters or not a plain name is no page, and a rendered page is a
//! whole document whose links carry no script scheme.
#![no_main]

use std::collections::BTreeSet;

use joinedcontext_portal::public_docs::render;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (chapter, page) = text.split_once('/').unwrap_or((&text, ""));
    let mounted = BTreeSet::from(["odata".to_owned(), "mcp".to_owned(), "websocket".to_owned()]);
    if let Some(html) = render(chapter, page, &mounted) {
        assert!(page
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(html.contains("<title>"), "{chapter}/{page} has no title");
        assert!(!html.to_ascii_lowercase().contains("href=\"javascript:"));
    }
});
