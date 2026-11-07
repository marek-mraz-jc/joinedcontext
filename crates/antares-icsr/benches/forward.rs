// SPDX-License-Identifier: EUPL-1.2
//! The ADR-0027 section 6 budget row "internal CSR forward": a federated
//! Query Entities served in process against the same query forwarded over
//! HTTP to the same broker on a loopback listener; the in-process p99 is
//! held against half the HTTP one.
//!
//!     cargo bench -p antares-icsr --bench forward
//!
//! Tenant `b` holds `ENTITIES` Cars. Tenant `a` registers this broker by a
//! self URL (in process, under `b`'s read grant), tenant `c` by the
//! listener's address (HTTP). Each round asks both for the Cars; the times
//! are the whole outer request.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::io::Write as _;
use std::time::{Duration, Instant};
use tower::ServiceExt;

const ROUNDS: usize = 500;
const ENTITIES: usize = 20;
const SELF_URL: &str = "http://self.invalid";

type Error = Box<dyn std::error::Error>;

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    tenant: &str,
    body: Option<Value>,
) -> Result<(u16, Value), Error> {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("NGSILD-Tenant", tenant);
    let req = match body {
        Some(b) => {
            let text = b.to_string();
            req.header("Content-Type", "application/json")
                .header("Content-Length", text.len())
                .body(Body::from(text))?
        }
        None => req.body(Body::empty())?,
    };
    let answer = app.clone().oneshot(req).await?;
    let status = answer.status().as_u16();
    let bytes = answer.into_body().collect().await?.to_bytes();
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let i = sorted.len().saturating_mul(p).checked_div(100).unwrap_or(0);
    sorted
        .get(i.min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or_default()
}

async fn rounds(app: &axum::Router, tenant: &str) -> Result<Vec<Duration>, Error> {
    let mut times = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let (status, doc) = send(app, "GET", "/ngsi-ld/v1/entities?type=Car", tenant, None).await?;
        times.push(start.elapsed());
        let found = doc.as_array().map_or(0, Vec::len);
        if status != 200 || found != ENTITIES {
            return Err(format!("{tenant}: {status}, {found} Cars").into());
        }
    }
    times.sort();
    Ok(times)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let selfs = antares_icsr::SelfUrls::parse(SELF_URL)?;
    let broker = TestBroker::new()
        .await?
        .with_forward_transport(antares_icsr::transport(selfs))?;
    let app = broker.serve_mounted(Vec::new())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });

    for i in 0..ENTITIES {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                         "speed": {"type": "Property", "value": i}});
        send(&app, "POST", "/ngsi-ld/v1/entities", "b", Some(car)).await?;
    }
    send(
        &app,
        "PUT",
        "/q/grants/b/a",
        "b",
        Some(json!({"target": "b", "source": "a"})),
    )
    .await?;
    for (tenant, endpoint) in [("a", SELF_URL.to_owned()), ("c", format!("http://{addr}"))] {
        let reg = json!({"type": "ContextSourceRegistration", "endpoint": endpoint, "tenant": "b",
                         "information": [{"entities": [{"type": "Car"}]}]});
        let (status, doc) = send(
            &app,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            tenant,
            Some(reg),
        )
        .await?;
        if status != 201 {
            return Err(format!("register in {tenant}: {status} {doc}").into());
        }
    }

    let inner = rounds(&app, "a").await?;
    let http = rounds(&app, "c").await?;
    let (ip50, ip99) = (percentile(&inner, 50), percentile(&inner, 99));
    let (hp50, hp99) = (percentile(&http, 50), percentile(&http, 99));
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "internal CSR forward, {ROUNDS} rounds of {ENTITIES} Cars"
    )?;
    writeln!(out, "  in process  p50 {ip50:?}  p99 {ip99:?}")?;
    writeln!(out, "  over HTTP   p50 {hp50:?}  p99 {hp99:?}")?;
    writeln!(
        out,
        "  budget (in-process p99 <= 0.5 x HTTP p99): {}",
        if ip99.saturating_mul(2) <= hp99 {
            "met"
        } else {
            "MISSED"
        }
    )?;
    Ok(())
}
