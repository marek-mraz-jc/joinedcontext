//! A secrets file path: a path that parses names a project and a secret whose own path it is.
#![no_main]

use joinedcontext_portal::secret_files::{parse_path, path_of, valid_name};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some((project, name)) = parse_path(text) {
        assert!(valid_name(name), "{name:?}");
        assert!(!project.is_empty() && !project.contains('/'), "{project:?}");
        assert_eq!(path_of(project, name), text, "the inverse of path_of");
    }
});
