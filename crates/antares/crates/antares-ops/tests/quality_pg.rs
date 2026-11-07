// SPDX-License-Identifier: EUPL-1.2
//! The quality sweep on the Postgres arm (T-4018, DM-90, DM-91): the same
//! mark and lift as `tests/quality.rs`, through the store's query and row-lock
//! paths of the database. Skips loudly without `ANTARES_TEST_DATABASE_URL` (see
//! antares-sql/tests/pg.rs for the recipe).

use antares_ops::test_kit::TestBroker;
use antares_sql::store::any::{AnyStore, PgBackend};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const LOCATION: &str = "https://example.org/position";
const RULES: &str = r#"{"rules":[{"name":"stale-position","type":"https://example.org/Vehicle",
    "attribute":"https://example.org/position","maxAge":"PT10M"},
    {"name":"stale-position","type":"https://example.org/Bus",
    "attribute":"https://example.org/position","maxAge":"PT10M"}]}"#;

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: Option<&str>,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
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
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn positioned(id: &str, ty: &str, minutes_ago: i64) -> String {
    let now = chrono::Utc::now();
    let at = now
        .checked_sub_signed(chrono::Duration::minutes(minutes_ago))
        .unwrap_or(now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    json!({"id": id, "type": ty, LOCATION: {"type": "Property", "value": 1, "observedAt": at}})
        .to_string()
}

/// The Vehicles of `tenant` the condition every other permit carries (DM-90)
/// lets through.
async fn unquarantined(app: &Router, tenant: &str) -> Vec<String> {
    let (_, visible) = send(
        app,
        Method::GET,
        "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2FVehicle&q=!quarantinedAt",
        Some(tenant),
        None,
    )
    .await;
    let ids: Vec<String> = visible
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect();
    ids
}

/// A fresh observation of `urn:v:old`'s position in `tenant`.
async fn observe_now(app: &Router, tenant: &str) {
    let fresh = json!({LOCATION: {"type": "Property", "value": 2,
        "observedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}});
    let (s, _) = send(
        app,
        Method::PATCH,
        "/ngsi-ld/v1/entities/urn:v:old/attrs",
        Some(tenant),
        Some(fresh.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

/// Four Entities: a stale and a fresh Vehicle and a stale Stop in `city`, a
/// stale Vehicle in `port`.
async fn seed(app: &Router, city: &str, port: &str) {
    for (tenant, body) in [
        (
            city,
            positioned("urn:v:old", "https://example.org/Vehicle", 30),
        ),
        (
            city,
            positioned("urn:v:new", "https://example.org/Vehicle", 1),
        ),
        (
            city,
            positioned("urn:s:old", "https://example.org/Stop", 30),
        ),
        (
            port,
            positioned("urn:v:old", "https://example.org/Vehicle", 30),
        ),
    ] {
        let (s, answer) = send(
            app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(tenant),
            Some(body),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{answer}");
    }
}

#[expect(clippy::print_stderr, reason = "a skipped database test says so")]
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_entity_is_quarantined_and_lifted_on_postgres() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let state = antares_api::AppState::with_store(
        "antares-test".into(),
        Arc::new(AnyStore::Pg(PgBackend::new(pool))),
        "postgres",
    );
    let b = TestBroker::over(state).await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    // Tenants of this run alone: the database is shared with other tests.
    let run = uuid_like();
    let (city, port) = (format!("q{run}a"), format!("q{run}b"));
    let put = send(
        &app,
        Method::PUT,
        &format!("/q/quality/{city}"),
        None,
        Some(RULES.into()),
    )
    .await;
    assert_eq!(put.0, StatusCode::CREATED, "{}", put.1);
    seed(&app, &city, &port).await;
    let get = |tenant: String, id: &'static str| {
        let app = app.clone();
        async move {
            send(
                &app,
                Method::GET,
                &format!("/ngsi-ld/v1/entities/{id}?options=sysAttrs"),
                Some(&tenant),
                None,
            )
            .await
            .1
        }
    };
    let before = get(city.clone(), "urn:v:old").await;

    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(
        changed
            .iter()
            .map(|c| (c.tenant.as_str(), c.outcome, c.count))
            .collect::<Vec<_>>(),
        [(city.as_str(), "quarantine", 1)]
    );
    let marked = get(city.clone(), "urn:v:old").await;
    assert_eq!(
        marked["quarantineRules"]["value"],
        json!(["stale-position"]),
        "{marked}"
    );
    assert_eq!(
        marked["modifiedAt"], before["modifiedAt"],
        "the mark is no client write"
    );
    for (tenant, id) in [
        (city.clone(), "urn:v:new"),
        (city.clone(), "urn:s:old"),
        (port.clone(), "urn:v:old"),
    ] {
        assert!(get(tenant, id).await.get("quarantinedAt").is_none(), "{id}");
    }
    assert_eq!(unquarantined(&app, &city).await, ["urn:v:new"]);
    assert!(
        b.sweep_quality(chrono::Utc::now()).await.is_empty(),
        "marked once"
    );

    observe_now(&app, &city).await;
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(
        changed
            .iter()
            .map(|c| (c.outcome, c.count))
            .collect::<Vec<_>>(),
        [("lift", 1)]
    );
    let lifted = get(city.clone(), "urn:v:old").await;
    assert!(
        lifted.get("quarantinedAt").is_none() && lifted.get("quarantineRules").is_none(),
        "{lifted}"
    );
    send(
        &app,
        Method::DELETE,
        &format!("/q/quality/{city}"),
        None,
        None,
    )
    .await;
}

/// A short run id from the clock, enough to keep two runs' Tenants apart.
fn uuid_like() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().checked_rem(0xffff_ffff).unwrap_or_default())
            .unwrap_or_default()
    )
}
