//! The zip a person uploads to `import?format=files`: every file it accepts lands once, as text,
//! under the project's own folder, whatever the bytes.
#![no_main]

use joinedcontext_portal::api::import_files::repository_files;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(files) = repository_files(data, "zilina") {
        let mut seen = std::collections::HashSet::new();
        for (path, _) in &files {
            assert!(path.starts_with("projects/zilina/"), "{path}");
            assert!(!path.split('/').any(|s| s == ".." || s.is_empty()), "{path}");
            assert!(seen.insert(path.clone()), "{path} twice");
        }
    }
});
