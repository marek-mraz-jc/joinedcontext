//! Input: `<path>\n<robots.txt body>`, and the whole input as a link. A robots.txt with no rule
//! lets every path be read; a link that comes back is http(s) on an askable host, without a
//! fragment.
#![no_main]

use joinedcontext_portal::link_check::{askable_host, link, robots_allow};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (path, body) = text.split_once('\n').unwrap_or((&text, ""));
    let allowed = robots_allow(body, path);
    if !body.to_ascii_lowercase().contains("allow") {
        assert!(allowed, "no rule yet {path:?} refused");
    }
    if let Some(url) = link(&text) {
        assert!(matches!(url.scheme(), "http" | "https"));
        assert!(url.fragment().is_none());
        assert!(url.host_str().is_some_and(askable_host));
    }
    if text.parse::<std::net::IpAddr>().is_ok() {
        assert!(!askable_host(&text));
    }
});
