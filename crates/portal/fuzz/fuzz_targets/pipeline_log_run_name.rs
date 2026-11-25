//! A run's name as the runner sends it: a kept one is short and plain, any other is the current
//! UTC hour.
#![no_main]

use joinedcontext_portal::pipeline_log::run_name;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for sent in [None, Some(text)] {
        let name = run_name(sent);
        assert!(!name.is_empty() && name.len() <= 40, "{name:?}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '.')),
            "{name:?}"
        );
        assert_eq!(run_name(Some(&name)), name, "a kept name stays");
    }
});
