//! The body of a connector request (`POST /api/v1/connectors`): any bytes are refused or parse,
//! and a request the default ceiling lets through names registered segments, a DNS-1123 name and
//! 1 to 50 scope items, and asks nothing above the ceiling.
#![no_main]

use joinedcontext_portal::connectors::{is_segment, validate, Ceiling, NewConnector};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(new) = serde_json::from_slice::<NewConnector>(data) else {
        return;
    };
    let ceiling = Ceiling::of_policies(Vec::new());
    let now = "2026-11-25T00:00:00Z".parse().expect("a fixed time");
    if let Ok(above) = validate(&new, &ceiling, now) {
        assert!(
            above.is_empty(),
            "{above:?} asked above the default ceiling"
        );
        assert!(new.segments.iter().all(|segment| is_segment(segment)));
        assert!(
            !new.name.is_empty() && new.name.len() <= 63,
            "{:?}",
            new.name
        );
        assert!((1..=50).contains(&new.scope.len()), "{}", new.scope.len());
    }
});
