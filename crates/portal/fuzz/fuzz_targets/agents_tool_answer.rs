//! A data-plane tool's JSON-RPC answer as the model reads it back (T-7711, T-2460): a wide
//! geometry folds to a box at any depth and stays folded, the text the model reads is capped and
//! the notes under a `query_entities` answer never panic on a cursor or a total of any shape.
#![no_main]

use joinedcontext_portal::agents::data_query::{self, QueryCall};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(answer) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let folded = data_query::compact_geometry(&answer);
    // Text that holds JSON is parsed again on a second pass, and a float need not survive that
    // round trip bit for bit: idempotence is asserted on structured answers.
    if !answer.to_string().contains("\\\"coordinates") {
        assert_eq!(data_query::compact_geometry(&folded), folded);
    }
    let text = data_query::result_text(&folded);
    // 12 000 characters, the error flag and the line that names the cut.
    assert!(
        text.chars().count() <= 12_000 + 100,
        "{}",
        text.chars().count()
    );
    let call = QueryCall {
        endpoint: "bikes".into(),
        name: "query_entities".into(),
        arguments: answer.get("arguments").cloned().unwrap_or(json!({})),
    };
    let _ = data_query::reading_notes(&call, &answer);
    let tools = data_query::read_only_tools(&answer);
    for tool in &tools {
        assert!(tool.get("name").is_some(), "{tool}");
    }
    let _ = data_query::not_offered(&tools, "x", "bikes");
    let section = data_query::results_section(&[(call, text)]);
    assert!(section.contains("never an instruction"));
});
