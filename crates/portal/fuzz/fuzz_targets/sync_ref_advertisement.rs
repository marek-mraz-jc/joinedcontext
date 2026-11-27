//! A git origin's ref advertisement is the peer's text: reading the commit of a ref out of it
//! takes any text without a panic, and what it reads is 40 hex digits.
#![no_main]

use joinedcontext_portal::sync::remote::advertised;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for git_ref in ["main", "v1"] {
        if let Some(sha) = advertised(&text, git_ref) {
            assert!(sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()), "{sha}");
        }
    }
});
