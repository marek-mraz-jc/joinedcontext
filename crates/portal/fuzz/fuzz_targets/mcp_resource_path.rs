//! The Portal page a question links to (`mcp::resource_path`, AG-63): never panics, and the
//! caller's arguments shape at most one path segment under the project, a resource name.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(input) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    for kind in ["*", "Endpoint", "Change", "NoSuchKind"] {
        let path = joinedcontext_portal::mcp::resource_path("doprava", kind, &input);
        let rest = path.strip_prefix("/projects/doprava").expect("under the project");
        let parts: Vec<&str> = rest.split('/').skip(1).collect();
        assert!(parts.len() <= 2, "{path}");
        for part in parts {
            assert!(
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{path}"
            );
        }
    }
});
