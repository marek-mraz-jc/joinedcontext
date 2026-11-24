//! A model's answer, as the assistant's driver reads it (T-7711): the `change_resource`,
//! `query_endpoint`, `search_catalog` and `draft_kpi_pipeline` calls in its fences. Whatever the
//! text holds, a read call names an endpoint and a tool and carries an object, a search is
//! trimmed words, a KPI trigger runs at least once a minute and a planned pipeline keeps its
//! name, its project and an indicator space of its own.
#![no_main]

use joinedcontext_portal::agents::{change, data_query, kpi_pipeline};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(answer) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(Ok(call)) = change::tool_call(answer) {
        let asked = answer.to_lowercase();
        if let Some(why) = change::wider_than_asked(&call, &asked) {
            assert_eq!(call.kind.trim(), "Role");
            assert!(why.contains("no writes"), "{why}");
        }
    }
    for call in data_query::tool_calls(answer).into_iter().flatten() {
        assert!(!call.endpoint.is_empty() && call.endpoint.trim() == call.endpoint);
        assert!(!call.name.is_empty() && call.name.trim() == call.name);
        assert!(call.arguments.is_object(), "{call:?}");
        assert_eq!(data_query::rpc(&call)["params"]["name"], json!(call.name));
    }
    for words in data_query::search_calls(answer) {
        assert!(!words.is_empty() && words.trim() == words, "{words:?}");
    }
    // The answer as its own earlier turn: a call it spells is written by that data.
    let _ = data_query::written_by_data(answer, &[], "");
    let Some(Ok(call)) = kpi_pipeline::tool_call(answer) else {
        return;
    };
    if let Ok(kpi_pipeline::Trigger::Every { seconds, .. }) = kpi_pipeline::trigger_of(&call) {
        assert!(seconds >= 60, "{seconds}");
    }
    if let Ok(space) = kpi_pipeline::target_space("helsinki", call.target_space.as_deref()) {
        assert!(space.ends_with("-kpi"), "{space}");
    }
    let endpoints = [json!({
        "metadata": { "name": "helsinki-all", "namespace": "helsinki" },
        "spec": { "contextSpaceRef": "helsinki", "slug": "s" },
    })];
    let world = kpi_pipeline::World {
        project: "helsinki",
        org_domain: "hel.fi",
        endpoints: &endpoints,
        spaces: &[],
        policies: &[],
        run_endpoint: Some("helsinki-all"),
        new_slug: "newslug",
    };
    if let Ok(plan) = kpi_pipeline::plan(&call, &world) {
        assert_eq!(plan.pipeline["metadata"]["name"], Value::from(call.name.trim()));
        assert_eq!(plan.pipeline["metadata"]["namespace"], "helsinki");
        assert_ne!(plan.target_space, plan.source_space);
        assert!(plan.target_space.ends_with("-kpi"));
    }
});
