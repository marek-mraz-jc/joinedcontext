//! What an MCP call puts on the audit (`mcp::audited`, AG-147): never panics, and the tool and
//! the target it records are identifiers alone, never an input value.
#![no_main]
use libfuzzer_sys::fuzz_target;

fn is_word(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 63
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(message) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let (tool, target) = joinedcontext_portal::mcp::audited(&message);
    assert!(
        tool == "unknown"
            || (tool.len() <= 64
                && tool
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_/-".contains(&b))),
        "{tool:?}"
    );
    if let Some(target) = target {
        assert!(target.split('/').count() <= 4, "{target:?}");
        assert!(target.split('/').all(is_word), "{target:?}");
    }
});
