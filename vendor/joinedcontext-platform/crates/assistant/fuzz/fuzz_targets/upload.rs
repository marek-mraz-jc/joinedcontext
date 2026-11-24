//! An uploaded file's name and first bytes (T-7768): the name is kept or refused, the bytes are
//! sniffed to one of the allowed types or refused, never a panic.
#![no_main]

use assistant::upload;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let cut = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let name = String::from_utf8_lossy(&data[..cut]);
    let bytes = data.get(cut + 1..).unwrap_or_default();
    if let Some(kept) = upload::file_name(&name) {
        // Folders are kept, a way out never is: no empty, `.` or `..` part, no `\\`.
        assert!(
            kept.split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
            "a kept name has no way out: {kept:?}"
        );
        assert!(
            !kept.contains('\\'),
            "a kept name has no backslash: {kept:?}"
        );
        let _ = upload::sniff(kept, bytes);
    }
    let _ = upload::sniff(&name, bytes);
});
