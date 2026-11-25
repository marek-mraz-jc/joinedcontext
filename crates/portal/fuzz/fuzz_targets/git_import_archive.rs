//! The archive a person uploads to `import?format=git`: every file it unpacks stays under the
//! root and within the upload limit, whatever the bytes.
#![no_main]

use joinedcontext_portal::api::import_git::unpack;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(files) = unpack(data) {
        assert!(files.len() <= 256);
        for path in files.keys() {
            assert!(
                !path.starts_with('/') && !path.split('/').any(|s| s == ".."),
                "{path}"
            );
        }
    }
});
