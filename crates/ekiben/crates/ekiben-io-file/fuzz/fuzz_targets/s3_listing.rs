//! Input: an S3 ListObjectsV2 answer as a store sends it. Reading its keys, truncation flag and
//! continuation token answers values the answer holds, never more text than it has.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for tag in ["Key", "IsTruncated", "NextContinuationToken", "Code"] {
        let values = ekiben_io_s3::client::xml_values(&text, tag);
        let total: usize = values.iter().map(String::len).sum();
        assert!(total <= text.len(), "{tag}");
    }
});
