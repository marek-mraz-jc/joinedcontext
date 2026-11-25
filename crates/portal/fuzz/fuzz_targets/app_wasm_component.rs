//! A wasm App's bundle (AP-151): any tar.gz is refused with a reason or answers the bytes of
//! `.jc/component.wasm`, at most 64 MiB and starting with the component preamble.
#![no_main]

use joinedcontext_portal::apps::wasm_apps::{component_of, MAX_COMPONENT_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(component) = component_of(data) {
        assert!(component.len() as u64 <= MAX_COMPONENT_BYTES);
        assert_eq!(
            component.get(..8),
            Some(&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00][..])
        );
    }
});
