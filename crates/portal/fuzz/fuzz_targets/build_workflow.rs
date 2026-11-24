//! The build workflow an App repository carries: its class comes from one `runs-on:` line naming
//! a build class, never from anything else in the file.
#![no_main]

use joinedcontext_portal::apps::build_pods::Class;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(class) = Class::of_workflow(text) {
        let wanted = format!("app-build-{}", class.name());
        assert!(text.lines().any(
            |line| line.trim().strip_prefix("runs-on:").map(str::trim) == Some(wanted.as_str())
        ));
    }
});
