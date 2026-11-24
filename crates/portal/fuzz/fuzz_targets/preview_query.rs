//! An Endpoint preview's body: a page it accepts is a Query Entities query of 1 to 100 entities.
#![no_main]

use joinedcontext_portal::api::endpoint_preview::{query_of, PreviewRequest};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(request) = serde_json::from_slice::<PreviewRequest>(data) else {
        return;
    };
    if let Ok(query) = query_of(&request) {
        assert!(query.starts_with("type="), "{query:?}");
        assert!(
            query.contains("&limit=") && query.contains("&offset="),
            "{query:?}"
        );
        assert!(!query.contains([' ', '#', '\n']), "{query:?}");
        assert!(request.id.len() <= 100);
    }
});
