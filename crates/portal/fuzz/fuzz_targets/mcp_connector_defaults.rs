//! A one-project connector's default project (`mcp::connector::defaults_project`, T-6064):
//! never panics, puts in only the stored scope's project, and never replaces one the call named.
#![no_main]
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(mut message) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let before = message.clone();
    let connector =
        joinedcontext_portal::connectors::of_conversation("r1", "f:1:ana", "doprava", "pipeline");
    let changed = joinedcontext_portal::mcp::connector::defaults_project(&connector, &mut message);
    if changed {
        assert_eq!(message["method"], "tools/call");
        assert_eq!(message["params"]["arguments"]["project"], "doprava", "{message}");
        assert!(before["params"]["arguments"].get("project").is_none(), "{before}");
    } else if before["method"] != "tools/call" || !before["params"].is_object() {
        assert_eq!(message, before);
    } else {
        assert_eq!(
            message["params"]["arguments"]["project"],
            before["params"]["arguments"]["project"],
            "{before}"
        );
    }
});
