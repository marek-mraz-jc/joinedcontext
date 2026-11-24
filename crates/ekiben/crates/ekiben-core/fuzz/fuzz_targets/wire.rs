//! Input: bytes read as Bento's binary batch format (a part count, then length-prefixed parts).
//! A decoded batch serializes back to bytes that decode to the same parts, and no part count
//! the input did not pay for is allocated.
#![no_main]

use ekiben_core::{deserialize_bytes, serialize_bytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(parts) = deserialize_bytes(data) {
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        let again = deserialize_bytes(&serialize_bytes(&refs)).expect("own output decodes");
        assert_eq!(again, parts);
    }
});
