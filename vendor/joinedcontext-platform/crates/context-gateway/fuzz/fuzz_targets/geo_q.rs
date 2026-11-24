//! Any caller `geoQ` against any grants, and any GeoJSON: the intersection never panics, and
//! never forwards an area wider than a grant when one is granted (T-7767).
#![no_main]

use context_gateway::pdp::geo;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    // First line the caller's geoQ ("-" for none), each further line a grant.
    let mut lines = raw.split('\n');
    let caller = lines.next().filter(|line| *line != "-");
    let grants: Vec<&str> = lines.take(8).collect();
    let decided = geo::intersect(caller, &grants);
    if !grants.is_empty() {
        assert_eq!(decided.areas.len(), grants.len());
        if let Some(forwarded) = &decided.geo_q {
            assert!(
                grants.contains(&forwarded.as_str()) || Some(forwarded.as_str()) == caller,
                "forwarded {forwarded}, granted {grants:?}"
            );
        }
    }
    for line in raw.split('\n') {
        let _ = geo::granted_polygon(line);
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = geo::polygon_from(&value);
        let _ = geo::entity_point(&value);
    }
});
