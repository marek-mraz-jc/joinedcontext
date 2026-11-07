// SPDX-License-Identifier: EUPL-1.2
//! What an OData read costs on top of the query it runs (ADR-0027 section
//! 6, row "OData read"; surfaces-test-plan section 10): 100 Cars read on
//! the NGSI-LD root, through the view, and as an OData EntitySet with and
//! without a `$filter`, then `$metadata` from the model cache. It prints
//! the budget row, p50 ≤ vanilla + 0.15 ms + 1 µs per Entity and p99 ≤
//! 1.25 × vanilla; it reports and never fails (T-3523's job decides that).
//!
//!     cargo bench -p antares-odata --bench render

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::json;
use std::io::Write as _;
use std::time::{Duration, Instant};
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ENTITIES: u32 = 100;
const RUNS: usize = 300;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "label"]}},
  "slots": {"speed": {"range": "float"}, "label": {"range": "string"}}
}"#;

async fn send(app: &Router, method: &str, uri: &str, body: Option<String>) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let _ = resp.into_body().collect().await;
    status
}

/// p50 and p99 of `RUNS` reads of `uri`.
async fn sample(name: &str, app: &Router, uri: &str) -> std::io::Result<(Duration, Duration)> {
    let mut took = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        let status = send(app, "GET", uri, None).await;
        if status != StatusCode::OK {
            return Err(std::io::Error::other(format!("{name}: {status}")));
        }
        took.push(start.elapsed());
    }
    took.sort_unstable();
    let at = |p: usize| {
        took.get(RUNS.saturating_mul(p) / 100)
            .copied()
            .unwrap_or_default()
    };
    let (p50, p99) = (at(50), at(99));
    writeln!(
        std::io::stdout(),
        "{name}: p50 {} µs, p99 {} µs ({RUNS} runs)",
        p50.as_micros(),
        p99.as_micros()
    )?;
    Ok((p50, p99))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_odata::OData)?;
    let view = json!({"id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:label"]}],
        "operations": ["query_entities", "retrieve_entity"]});
    send(&app, "PUT", "/q/views/cars", Some(view.to_string())).await;
    send(&app, "PUT", "/q/schemas/city", Some(SCHEMA.to_owned())).await;
    for n in 0..ENTITIES {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"),
            format!("{CITY}speed"): {"type": "Property", "value": n},
            format!("{CITY}label"): {"type": "Property", "value": format!("car {n}")}});
        send(&app, "POST", "/ngsi-ld/v1/entities", Some(car.to_string())).await;
    }
    let root =
        "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2Fcity%2FCar&limit=100&count=true";
    let (vanilla_p50, vanilla_p99) = sample("NGSI-LD root: 100 Cars", &app, root).await?;
    sample(
        "NGSI-LD root: 1 Car",
        &app,
        "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2Fcity%2FCar&limit=1",
    )
    .await?;
    sample("OData: 1 Car", &app, "/x/views/cars/odata/v4/Car?$top=1").await?;
    let viewed = broker.serve_view("cars")?;
    sample("view: 100 Cars", &viewed, root).await?;
    sample(
        "view: 1 Car",
        &viewed,
        "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2Fcity%2FCar&limit=1",
    )
    .await?;
    let odata = "/x/views/cars/odata/v4/Car?$top=100&$count=true";
    let (p50, p99) = sample("OData: 100 Cars", &app, odata).await?;
    sample(
        "OData: $filter speed gt 49 and contains(label,'car')",
        &app,
        "/x/views/cars/odata/v4/Car?$top=100&$filter=speed%20gt%2049%20and%20contains(label,'car')",
    )
    .await?;
    sample(
        "OData: $metadata (cached model)",
        &app,
        "/x/views/cars/odata/v4/$metadata",
    )
    .await?;
    let p50_budget = vanilla_p50
        .saturating_add(Duration::from_micros(150))
        .saturating_add(Duration::from_micros(u64::from(ENTITIES)));
    let p99_budget = vanilla_p99
        .saturating_mul(5)
        .checked_div(4)
        .unwrap_or_default();
    let verdict = |got: Duration, budget: Duration| if got <= budget { "within" } else { "over" };
    writeln!(
        std::io::stdout(),
        "budget OData read ({ENTITIES} Entities): p50 {} µs of {} µs {}, p99 {} µs of {} µs {}",
        p50.as_micros(),
        p50_budget.as_micros(),
        verdict(p50, p50_budget),
        p99.as_micros(),
        p99_budget.as_micros(),
        verdict(p99, p99_budget),
    )?;
    Ok(())
}
