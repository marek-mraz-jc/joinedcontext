//! The project door's hold on one JSON-RPC body (`mcp::held_to`, MCP-33): never panics, and a
//! body it lets through names no project but the door's, in `params`, its `arguments` or a
//! `jc://` address.
#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    // The door reads at most 1 MiB; a smaller cap keeps the run fast and loses no shape.
    if data.len() > 64 * 1024 {
        return;
    }
    // The first line is the door's project, the rest the body.
    let (project, body) = match data.iter().position(|b| *b == b'\n') {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (&b"doprava"[..], data),
    };
    let Ok(project) = std::str::from_utf8(project) else {
        return;
    };
    let Ok(held) = joinedcontext_portal::mcp::held_to(body, project) else {
        return;
    };
    let message: Value = serde_json::from_slice(&held).expect("a held body is JSON");
    let params = &message["params"];
    assert_eq!(params["project"], project, "{message}");
    let named = &params["arguments"]["project"];
    assert!(named.is_null() || named == project, "{message}");
    if let Some(rest) = params["uri"].as_str().and_then(|u| u.strip_prefix("jc://")) {
        let head = rest.split('/').next().unwrap_or_default();
        assert!(
            head == project || head == "-" || head == "schemas",
            "{message}"
        );
    }
});
