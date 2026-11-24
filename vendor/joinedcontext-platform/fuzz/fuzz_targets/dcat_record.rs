//! Any JSON as a DCAT record or feed: every profile's rendering, the Turtle writer and the small
//! helpers never panic, and Turtle never ends inside an open literal (T-7794).
#![no_main]

use jc_dcat::profile::{render, render_catalog, Profile};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = jc_dcat::ws_address(text);
    let literal = jc_dcat::literal(text);
    assert!(literal.starts_with('"') && literal.ends_with('"'), "{literal}");
    let _ = jc_dcat::escape(text);
    let Ok(record) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let _ = jc_dcat::major_of(&record);
    let _ = jc_dcat::turtle(&record);
    let _ = jc_dcat::feed_turtle(&record);
    for id in ["dcat-ap", "dcat-ap-sk", "dcat-ap-cz"] {
        let profile = Profile::parse(id).expect("every profile is built in");
        let rendered = render(record.clone(), profile, None);
        let _ = jc_dcat::turtle(&rendered);
        let catalog = render_catalog(record.clone(), profile);
        let _ = jc_dcat::feed_turtle(&catalog);
    }
});
