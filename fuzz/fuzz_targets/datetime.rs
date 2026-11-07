//! 4.6.3 DateTime values (`observedAt`, `timeAt`, a temporal instance):
//! never panics (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = antares_jsonld::expand::parse_datetime(text);
    }
});
