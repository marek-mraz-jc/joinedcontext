//! A view's geoQ as a ModelProjection's filter writes it (T-7808): any text yields the broker's
//! geo object, which always holds `georel`, `geometry` and `coordinates`, or a refusal naming
//! the geoQ; never a panic.
#![no_main]

use joinedcontext_portal::reconciler::views;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    match views::geo(raw) {
        Ok(object) => {
            for key in ["georel", "geometry", "coordinates"] {
                assert!(object.get(key).is_some(), "a geo object holds {key}");
            }
        }
        Err(why) => assert!(why.starts_with("geoQ "), "{why}"),
    }
});
