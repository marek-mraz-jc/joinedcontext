//! 5.2.33 `idPattern` regular expressions: never panics, and a compiled
//! pattern matches in bounded time on an id of the size the API admits
//! (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    let Ok(pattern) = std::str::from_utf8(data) else { return };
    if let Ok(re) = antares_ql::regex::compile(pattern) {
        let _ = re.is_match("urn:ngsi-ld:Vehicle:A4567");
        let _ = re.is_match(&"a".repeat(256));
    }
});
