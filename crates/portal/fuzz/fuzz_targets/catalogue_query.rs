//! The public catalogue's query string, as any visitor sends it: the search text stays at 200
//! characters and the page at 1 or more.
#![no_main]

use joinedcontext_portal::api::catalogue::Filters;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let query = String::from_utf8_lossy(data);
    let filters = Filters::parse(Some(&query));
    assert!(filters.q.chars().count() <= 200);
    assert!(filters.page >= 1);
});
