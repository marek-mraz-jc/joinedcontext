//! `quotas.yaml` of `--config-dir` and the body of `PUT /orgs/{org}/quotas`: every quota that
//! parses holds limits of at least one.
#![no_main]

use ekiben_server::config_dir::{parse_quotas_file, quotas_from_map};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(orgs) = parse_quotas_file(text) {
            for q in orgs.values() {
                assert!(q.validate().is_ok());
            }
        }
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        if let Ok(q) = quotas_from_map(&value) {
            assert!(q.validate().is_ok());
        }
    }
});
