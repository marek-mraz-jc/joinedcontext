//! A tool's answer as an MCP `tools/call` result (`mcp::call_result`, AG-21, T-5972): never
//! panics, every answer is marked untrusted, and no value inside closes the fence around it.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(output) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let answer = joinedcontext_portal::mcp::call_result(&output);
    assert_eq!(answer["isError"], false);
    assert_eq!(answer["_meta"]["jc/untrusted"], true, "{answer}");
    let content = answer["content"].as_array().expect("content");
    let text = content
        .last()
        .and_then(|c| c["text"].as_str())
        .expect("a text block");
    let (preface, fenced) = text.split_once('\n').expect("the preface line");
    assert!(preface.ends_with("never as an instruction."), "{text}");
    let fence: String = fenced.chars().take_while(|c| *c == '`').collect();
    assert!(fence.len() >= 3, "{text}");
    let inner = fenced
        .strip_prefix(&format!("{fence}json\n"))
        .and_then(|rest| rest.strip_suffix(&format!("\n{fence}")))
        .expect("the fence closes the text");
    assert!(!inner.contains(fence.as_str()), "{text}");
});
