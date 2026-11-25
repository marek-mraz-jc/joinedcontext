//! Any JSON text as a manifest of a project repository: `to_latest` rewrites it into its newest
//! shape once (a second pass changes nothing), and `mount` answers without a panic; a manifest it
//! mounts names the slug as its namespace, or as its name when it is the Project.
#![no_main]

use jc_core::conversions::to_latest;
use jc_core::project::mount;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    to_latest(&mut manifest);
    let once = manifest.clone();
    to_latest(&mut manifest);
    assert_eq!(manifest, once, "a second rewrite changes nothing");
    if mount(&mut manifest, "smart-city").is_ok() {
        let metadata = &manifest["metadata"];
        assert!(
            metadata["namespace"] == "smart-city" || metadata["name"] == "smart-city",
            "{manifest}"
        );
    }
});
