//! Parses GML text as the parse_gml method reads it, without and with a CRS transform.
#![no_main]

use ekiben_bloblang::gml::parse_gml;
use ekiben_bloblang::Value;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let target = Value::String(String::from_utf8_lossy(data).into_owned());
    let _ = parse_gml(&target, None, None);
    let _ = parse_gml(&target, Some("EPSG:3879"), Some("EPSG:4326"));
});
