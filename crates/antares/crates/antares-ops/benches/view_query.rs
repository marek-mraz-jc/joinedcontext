// SPDX-License-Identifier: EUPL-1.2
//! The NGSI-LD view row of ADR-0027 section 6 (T-3667): the same query on
//! the root and through a view, p50 at most 1.05× and p99 at most 1.10× of
//! the root's. Exits non-zero when the view misses its row.
//!
//!     cargo bench -p antares-ops --bench view_query

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::io::Write as _;
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// The allocator the broker ships (`antares-broker`'s `main.rs`): the view
/// path allocates a few small things more than the root, and glibc malloc
/// times those differently.
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

async fn send(app: &Router, method: &str, uri: &str, body: Option<&Value>) -> StatusCode {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
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

/// One timed GET of `uri` against `app`.
async fn time(app: &Router, uri: &str) -> Result<Duration, String> {
    let start = Instant::now();
    let status = send(app, "GET", uri, None).await;
    let took = start.elapsed();
    if status == StatusCode::OK {
        Ok(took)
    } else {
        Err(format!("{uri}: {status}"))
    }
}

/// p50 and p99 of `runs`.
fn percentiles(mut runs: Vec<Duration>) -> (Duration, Duration) {
    runs.sort();
    let at = |p: usize| {
        runs.get(runs.len().saturating_mul(p) / 100)
            .copied()
            .unwrap_or_default()
    };
    (at(50), at(99))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = TestBroker::new().await?;
    let root = broker.router();
    // 100 Cars and a page of one: the view's cost must not grow with what
    // the store holds when the page does not
    for n in 0..100 {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
            "speed": {"type": "Property", "value": n}});
        let created = send(&root, "POST", "/ngsi-ld/v1/entities", Some(&car)).await;
        if created != StatusCode::CREATED {
            return Err(format!("seed: {created}").into());
        }
    }
    broker
        .store_view_doc(
            "cars",
            json!({"id": "cars", "tenant": "city", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                "operations": ["query_entities"]}),
        )
        .await?;
    let viewed = broker.serve_view("cars")?;
    let uri = "/ngsi-ld/v1/entities?type=Car&limit=1";
    // warm both paths, then time them in turns, one request each, so a
    // busy host slows both alike; the best of 9 rounds of each
    let (mut root_runs, mut view_runs) = (Vec::new(), Vec::new());
    for round in 0..10 {
        let (mut r, mut v) = (Vec::with_capacity(500), Vec::with_capacity(500));
        for _ in 0..500 {
            r.push(time(&root, uri).await?);
            v.push(time(&viewed, uri).await?);
        }
        if round > 0 {
            root_runs.push(percentiles(r));
            view_runs.push(percentiles(v));
        }
    }
    let best = |runs: &[(Duration, Duration)]| {
        let p50 = runs.iter().map(|r| r.0).min().unwrap_or_default();
        let p99 = runs.iter().map(|r| r.1).min().unwrap_or_default();
        (p50, p99)
    };
    let ((r50, r99), (v50, v99)) = (best(&root_runs), best(&view_runs));
    let mut out = std::io::stdout();
    writeln!(out, "root, 1 of 100 Cars: p50 {r50:?} p99 {r99:?}")?;
    writeln!(out, "view, 1 of 100 Cars: p50 {v50:?} p99 {v99:?}")?;
    let (q50, q99) = (
        v50.as_secs_f64() / r50.as_secs_f64(),
        v99.as_secs_f64() / r99.as_secs_f64(),
    );
    writeln!(
        out,
        "view / root: p50 {q50:.2}× (≤ 1.05×), p99 {q99:.2}× (≤ 1.10×)"
    )?;
    if q50 > 1.05 || q99 > 1.10 {
        return Err("the view misses its ADR-0027 section 6 row".into());
    }
    Ok(())
}
