//! A repository path a caller types (`jc_repository_tree`, `jc_repository_file`): any text is
//! its folders or a refusal, and the folders never climb out of the repository.
#![no_main]

use joinedcontext_portal::ops::repositories::path_segments;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let path = String::from_utf8_lossy(data);
    if let Ok(segments) = path_segments(&path) {
        assert!(segments.len() <= 32, "at most 32 folders");
        for segment in segments {
            assert!(
                !segment.is_empty() && segment != "." && segment != "..",
                "no empty, `.` or `..` folder: {path:?}"
            );
            assert!(
                !segment.contains('\\') && !segment.chars().any(char::is_control),
                "no backslash or control character: {path:?}"
            );
        }
    }
});
