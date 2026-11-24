//! The text the chat parses: a question, the model's answer and the gateway's tool results
//! (T-7768). Each reader ends in a value, never a panic or a slice off a character boundary.
#![no_main]

use assistant::chat::{brief, cards, mcp, publishers, redact, routed, standard, trace, usage, viz};
use assistant::{extract, guide, heading};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let cap = data.first().map_or(1, |b| usize::from(*b) + 1);
    let _ = brief::brief(&text);
    let _ = cards::of(&text);
    let _ = mcp::cut(&text, cap);
    let _ = publishers::publishers_of(&text);
    let _ = redact::secrets(&text);
    let _ = redact::masked(&text, true);
    let _ = redact::redact(&text);
    let _ = standard::quoted(&text);
    let _ = trace::kept(&text);
    let _ = usage::cell(&text);
    let _ = viz::geometries(&text, cap);
    let _ = routed::type_names(&text);
    let _ = routed::table_of(&text, cap, &["name".to_owned()], Some("Park"));
    let _ = routed::total_of(&text);
    let _ = routed::normalized(&text);
    let _ = routed::without_invented_absence(&text, &["name".to_owned()]);
    let _ = extract::prose(&text);
    let _ = extract::chrome_only(&text);
    let _ = extract::language_of(&text);
    let at = extract::cut(&text, cap);
    assert!(text.is_char_boundary(at), "cut lands on a character boundary");
    let _ = extract::pieces(&text, cap);
    let _ = heading(&text);
    let _ = guide::rewrite(&text, "https://portal.example.org", "hronov");
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = assistant::catalogue::types_of(&value);
        let _ = assistant::chat::view::history(&text, &value);
        let _ = viz::geometry(&value);
    }
});
