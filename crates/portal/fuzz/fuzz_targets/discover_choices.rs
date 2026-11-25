//! What a data source discovery reads from a public service (T-7817): a CKAN, ArcGIS, OData,
//! OGC API or SensorThings JSON answer, or a WFS capabilities text cut anywhere by the runner's
//! byte cap. Any bytes yield choices or a reason, never a panic.
#![no_main]

use jc_core::kinds::HttpPresetKind;
use joinedcontext_portal::ops::datasource_discover::{choices, MAX_CHOICES};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let answer = serde_json::from_slice::<Value>(data).unwrap_or(Value::String(text.into_owned()));
    for kind in [
        HttpPresetKind::Ckan,
        HttpPresetKind::ArcgisFeature,
        HttpPresetKind::Odata,
        HttpPresetKind::Wfs,
        HttpPresetKind::OgcFeatures,
        HttpPresetKind::SensorThings,
    ] {
        for whole in [true, false] {
            if let Ok((found, _cut)) = choices(kind, &answer, whole) {
                assert!(found.len() <= MAX_CHOICES, "the choices stay capped");
                assert!(
                    found.iter().all(|c| c.value.chars().count() <= 300),
                    "each name is cut"
                );
            }
        }
    }
});
