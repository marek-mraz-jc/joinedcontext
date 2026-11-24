//! A forge write's body, a read's query, an MCP body and every door's path, any bytes: a pinned
//! write carries the run's branch and trailer, a read takes only the run's ref, a read-only body
//! parses, and a path the rule lets through stays under its base (T-7795).
#![no_main]

use agent_proxy::routes;
use agent_proxy::runs::RunContext;
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

static RUN: LazyLock<RunContext> = LazyLock::new(|| {
    serde_json::from_value(serde_json::json!({
        "id": "run-1", "project": "p", "appName": "a", "endpointSlug": "e",
        "allowsWrite": true, "branch": "agent/run-1", "pathPrefix": "apps/a", "status": "building",
        "ticketHash": "", "maxTokens": 1, "allowedHosts": [], "requestsPerMinute": 1,
        "maxResponseBytes": 1, "createdBy": "u", "modelName": "m"
    }))
    .expect("a run")
});

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(2, |b| *b == b'\n');
    let line = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let body = parts.next().unwrap_or_default();
    routes::forge::fuzzing::body(body, &line, &RUN);
    routes::data::fuzzing::reads_only(body);
    routes::fuzzing::path(&line);
});
