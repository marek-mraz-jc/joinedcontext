//! Parses WKT; a parsed geometry must format back to WKT and transform between WGS84 and
//! Web Mercator without a panic.
#![no_main]

use ekiben_bloblang::geo::{format_wkt, geo_transform, parse_wkt};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(geometry) = parse_wkt(text) else {
        return;
    };
    let _ = format_wkt(&geometry);
    let _ = geo_transform(&geometry, "EPSG:4326", "EPSG:3857");
});
