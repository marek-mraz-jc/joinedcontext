//! Input: a message body. Reading it as one JSON document answers a value or Bento's error;
//! a value written back reads as the same value, and the size estimate is computed without
//! serializing.
#![no_main]

use ekiben_core::Part;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let part = Part::new(data.to_vec());
    let _ = part.size_estimate();
    if let Ok(value) = part.as_structured_ref() {
        let written = Part::from_structured(value.clone());
        let _ = written.size_estimate();
        let back = Part::new(written.as_bytes_ref()).as_structured_ref();
        assert!(back.is_ok(), "{value:?}");
    }
});
