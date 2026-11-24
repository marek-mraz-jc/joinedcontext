//! Any query string: parsing, the GW31 shape check, the grant dimensions and the forwarded
//! query never panic, and encode/decode is a round trip (T-7767).
#![no_main]

use context_gateway::query;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let params = query::parse(raw);
    if let Some(reason) = query::malformed(&params) {
        // The reason names the parameter, never echoes the caller's value (AG-21).
        assert!(
            reason.starts_with("the type") || reason.starts_with("q "),
            "{reason}"
        );
    }
    let _ = query::requested(&params);
    let _ = query::referenced_attributes(&params);
    let _ = query::unselected(&params);
    let forwarded = query::view_read(&params, &["Thing".to_owned()]);
    let _ = query::parse(&forwarded);
    assert_eq!(query::decode(&query::encode(raw)), raw);
    assert_eq!(query::decode_path(&query::encode(raw)), raw);
    let _ = query::decode_path(raw);
});
