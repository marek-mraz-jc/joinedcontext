//! The paths and texts of an App the model writes (T-7711, SDK-11, SDK-46): a writable path never
//! climbs out of its folder or reaches the entry or the types, a scope is a writable `.tsx` file
//! and a line from 1, an outline keeps only lines of its text and the template filters drop
//! their markers.
#![no_main]

use joinedcontext_portal::agents::code;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if code::writable(text) {
        assert!(!text.split('/').any(|p| p.is_empty() || p == "." || p == ".."), "{text:?}");
        assert!(text != "src/main.tsx" && text != code::TYPES, "{text:?}");
    }
    if let Some((file, line)) = code::scope(text) {
        assert!(line >= 1 && file.ends_with(".tsx") && code::writable(file), "{text:?}");
    }
    let lines: Vec<&str> = text.lines().collect();
    for line in code::outline(text).lines() {
        assert!(lines.contains(&line), "{line:?} not in {text:?}");
    }
    let kept = code::service_examples(text, |name| name.len() % 2 == 0);
    assert!(kept.len() <= text.len());
    assert!(!kept.lines().any(|l| l.trim() == "// jc:end"), "{kept:?}");
    assert!(code::for_apps(text).len() <= text.len() + lines.len() + 1);
    let _ = code::services_section(text, |_| true);
});
