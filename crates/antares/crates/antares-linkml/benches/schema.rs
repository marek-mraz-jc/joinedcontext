// SPDX-License-Identifier: EUPL-1.2
//! What a LinkML document costs (ADR-0027 section 6: ≤ 5 ms p99 for a
//! 100-class schema, computed once per schema revision): the first request
//! after the Tenant's schema changes, which reads, narrows and renders the schema, and
//! a request answered from the cache.
//!
//!     cargo bench -p antares-linkml --bench schema

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use std::io::Write as _;
use std::time::{Duration, Instant};
use tower::ServiceExt;

async fn send(app: &Router, method: &str, uri: &str, body: Option<&Value>) -> StatusCode {
    let mut req = Request::builder().method(method).uri(uri);
    let bytes = body.map(Value::to_string).unwrap_or_default();
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let _ = resp.into_body().collect().await;
    status
}

/// A schema of `classes` classes of ten slots each, version `revision`.
fn schema(classes: usize, revision: usize) -> Value {
    let mut cs = Map::new();
    let mut slots = Map::new();
    for c in 0..classes {
        let own: Vec<String> = (0..10).map(|s| format!("c{c}s{s}")).collect();
        for s in &own {
            slots.insert(
                s.clone(),
                json!({"range": "float", "description": "a reading"}),
            );
        }
        cs.insert(format!("C{c}"), json!({"slots": own}));
    }
    json!({"id": "https://e.org/bench", "name": "bench", "version": format!("{revision}.0.0"),
           "default_prefix": "b",
           "prefixes": {"b": {"prefix_prefix": "b", "prefix_reference": "https://e.org/bench/"}},
           "classes": cs, "slots": slots})
}

fn view() -> Value {
    let types: Vec<String> = (0..100)
        .map(|c| format!("https://e.org/bench/C{c}"))
        .collect();
    let attrs: Vec<String> = (0..100)
        .flat_map(|c| (0..2).map(move |s| format!("https://e.org/bench/c{c}s{s}")))
        .collect();
    json!({"id": "bench", "tenant": "city", "selector": types,
           "rules": [{"types": types, "attributes": attrs}],
           "operations": ["retrieve_entity_type_details"]})
}

fn report(name: &str, mut runs: Vec<Duration>) -> std::io::Result<()> {
    runs.sort();
    let at = |q: usize| {
        runs.get(runs.len().saturating_mul(q) / 100)
            .or(runs.last())
            .map_or(0, Duration::as_micros)
    };
    writeln!(
        std::io::stdout(),
        "{name}: p50 {} µs, p99 {} µs ({} runs)",
        at(50),
        at(99),
        runs.len()
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var(
        antares_ops::views::SCHEMA_MAX_ENV,
        (1024 * 1024).to_string(),
    );
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_linkml::Linkml)?;
    let doc = "/x/views/bench/linkml/schema/v{major}/model.linkml.yaml";
    send(&app, "PUT", "/q/views/bench", Some(&view())).await;
    let mut cold = Vec::new();
    for revision in 1..=50 {
        send(&app, "PUT", "/q/schemas/city", Some(&schema(100, revision))).await;
        let uri = doc.replace("{major}", &revision.to_string());
        let start = Instant::now();
        let status = send(&app, "GET", &uri, None).await;
        cold.push(start.elapsed());
        if status != StatusCode::OK {
            return Err(format!("{uri}: {status}").into());
        }
    }
    let uri = doc.replace("{major}", "50");
    let mut warm = Vec::new();
    for _ in 0..500 {
        let start = Instant::now();
        send(&app, "GET", &uri, None).await;
        warm.push(start.elapsed());
    }
    report("100 classes, first request after a change", cold)?;
    report("100 classes, cached", warm)?;
    Ok(())
}
