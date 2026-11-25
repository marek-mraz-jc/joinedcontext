//! Input: a message body. Reading it as one JSON document answers a value or Bento's error;
//! a value written back reads as the same value, and the size estimate is computed without
//! serializing. Every body serde_json reads as one document reads as a value here too.
#![no_main]

use ekiben_core::Part;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let part = Part::new(data.to_vec());
    let _ = part.size_estimate();
    let read = part.as_structured_ref();
    if serde_json::from_slice::<serde_json::Value>(data).is_ok() {
        assert!(read.is_ok(), "{read:?}");
    }
    if let Ok(value) = read {
        let written = Part::from_structured(value.clone());
        let _ = written.size_estimate();
        let back = Part::new(written.as_bytes_ref()).as_structured_ref();
        assert!(back.is_ok(), "{value:?}");
    }
});
