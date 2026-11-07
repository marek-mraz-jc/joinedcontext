// SPDX-License-Identifier: EUPL-1.2
//! `/q/quality/{tenant}` and the quality sweep (T-4018, DM-90, DM-91): a
//! Tenant's `maxAge` rules are checked before they are stored, and the sweep
//! quarantines an Entity of the rule's type whose attribute was observed
//! longer ago than `maxAge`, lifts the mark when it is fresh again, and
//! touches no other type, no other Tenant and no `modifiedAt`.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const V: &str = "https://example.org/Vehicle";
const S: &str = "https://example.org/Stop";
const LOCATION: &str = "https://example.org/position";

/// One rule of three entries: a second attribute of the same type, and the
/// same attribute of a type no Entity here has.
const RULES: &str = r#"{"rules":[{"name":"stale-position","type":"https://example.org/Vehicle",
    "attribute":"https://example.org/position","maxAge":"PT10M"},
    {"name":"stale-position","type":"https://example.org/Vehicle",
    "attribute":"https://example.org/speed","maxAge":"PT10M"},
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

fn ago(minutes: i64) -> String {
    let now = chrono::Utc::now();
    now.checked_sub_signed(chrono::Duration::minutes(minutes))
        .unwrap_or(now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn positioned(id: &str, ty: &str, minutes_ago: i64) -> String {
    json!({"id": id, "type": ty,
        LOCATION: {"type": "Property", "value": 1, "observedAt": ago(minutes_ago)}})
    .to_string()
}

async fn entity(app: &Router, tenant: &str, id: &str) -> Value {
    let (status, body) = send(
        app,
        Method::GET,
        &format!("/ngsi-ld/v1/entities/{id}?options=sysAttrs"),
        Some(tenant),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tenant} {id}: {body}");
    body
}

async fn create(app: &Router, tenant: &str, body: String) {
    let (status, answer) = send(
        app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(tenant),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_document_is_checked_stored_replaced_and_deleted() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    let put = |body: &str| {
        send(
            &app,
            Method::PUT,
            "/q/quality/city",
            None,
            Some(body.into()),
        )
    };
    assert_eq!(put(RULES).await.0, StatusCode::CREATED);
    assert_eq!(put(RULES).await.0, StatusCode::NO_CONTENT);
    let (status, stored) = send(&app, Method::GET, "/q/quality/city", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["rules"][0]["name"], "stale-position");

    let (status, problem) = put(r#"{"rules":[{"name":"x","type":"Vehicle","attribute":"https://e.org/p","maxAge":"PT1M"}]}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let big = format!(r#"{{"rules":[],"pad":"{}"}}"#, "x".repeat(70 * 1024));
    assert_eq!(put(&big).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        send(
            &app,
            Method::PUT,
            "/q/quality/Not%20One",
            None,
            Some(RULES.into())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        send(&app, Method::DELETE, "/q/quality/city", None, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    for (method, uri, status) in [
        (Method::GET, "/q/quality/city", StatusCode::NOT_FOUND),
        (Method::DELETE, "/q/quality/city", StatusCode::NOT_FOUND),
        (Method::GET, "/q/quality/Not%20One", StatusCode::BAD_REQUEST),
        (
            Method::DELETE,
            "/q/quality/Not%20One",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            send(&app, method.clone(), uri, None, None).await.0,
            status,
            "{method} {uri}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_entity_is_quarantined_and_lifted_when_fresh_and_nothing_else_is_touched() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    assert_eq!(
        send(
            &app,
            Method::PUT,
            "/q/quality/city",
            None,
            Some(RULES.into())
        )
        .await
        .0,
        StatusCode::CREATED
    );
    create(&app, "city", positioned("urn:v:old", V, 30)).await;
    create(&app, "city", positioned("urn:v:new", V, 1)).await;
    create(&app, "city", positioned("urn:s:old", S, 30)).await;
    create(&app, "port", positioned("urn:v:old", V, 30)).await;
    let before = entity(&app, "city", "urn:v:old").await;

    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(
        changed
            .iter()
            .map(|c| (c.tenant.as_str(), c.rule.as_str(), c.outcome, c.count))
            .collect::<Vec<_>>(),
        [("city", "stale-position", "quarantine", 1)]
    );
    let marked = entity(&app, "city", "urn:v:old").await;
    assert_eq!(
        marked["quarantineRules"]["value"],
        json!(["stale-position"]),
        "{marked}"
    );
    assert_eq!(
        marked["quarantinedAt"]["value"]["@type"], "DateTime",
        "{marked}"
    );
    // The condition every permit but the owner's and steward's carries (DM-90) hides it.
    let (status, visible) = send(
        &app,
        Method::GET,
        "/ngsi-ld/v1/entities?type=https%3A%2F%2Fexample.org%2FVehicle&q=!quarantinedAt",
        Some("city"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{visible}");
    let ids: Vec<&str> = visible
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(ids, ["urn:v:new"]);
    assert_eq!(
        marked["modifiedAt"], before["modifiedAt"],
        "the mark is no client write"
    );
    for (tenant, id) in [
        ("city", "urn:v:new"),
        ("city", "urn:s:old"),
        ("port", "urn:v:old"),
    ] {
        let other = entity(&app, tenant, id).await;
        assert!(
            other.get("quarantinedAt").is_none(),
            "{tenant} {id}: {other}"
        );
    }

    // Marked once: a second sweep changes nothing.
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert!(changed.is_empty(), "{changed:?}");

    // A fresh observation: the next sweep lifts both Properties.
    let (status, body) = send(
        &app,
        Method::PATCH,
        "/ngsi-ld/v1/entities/urn:v:old/attrs",
        Some("city"),
        Some(json!({LOCATION: {"type": "Property", "value": 2, "observedAt": ago(0)}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(
        changed
            .iter()
            .map(|c| (c.outcome, c.count))
            .collect::<Vec<_>>(),
        [("lift", 1)]
    );
    let lifted = entity(&app, "city", "urn:v:old").await;
    assert!(lifted.get("quarantinedAt").is_none(), "{lifted}");
    assert!(lifted.get("quarantineRules").is_none(), "{lifted}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_without_a_document_and_a_deleted_document_mark_nothing() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    create(&app, "city", positioned("urn:v:old", V, 30)).await;
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert!(changed.is_empty(), "{changed:?}");

    send(
        &app,
        Method::PUT,
        "/q/quality/city",
        None,
        Some(RULES.into()),
    )
    .await;
    send(&app, Method::DELETE, "/q/quality/city", None, None).await;
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert!(changed.is_empty(), "{changed:?}");
    assert!(entity(&app, "city", "urn:v:old")
        .await
        .get("quarantinedAt")
        .is_none());
}

/// F-ANT4-011: a mark whose rule is gone, its whole document deleted
/// included, is lifted by the next sweep: no Entity stays hidden for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_mark_whose_rule_is_gone_is_lifted() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    send(
        &app,
        Method::PUT,
        "/q/quality/city",
        None,
        Some(RULES.into()),
    )
    .await;
    create(&app, "city", positioned("urn:v:old", V, 30)).await;
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert!(entity(&app, "city", "urn:v:old")
        .await
        .get("quarantinedAt")
        .is_some());

    send(&app, Method::DELETE, "/q/quality/city", None, None).await;
    let changed = b.sweep_quality(chrono::Utc::now()).await;
    assert_eq!(
        changed
            .iter()
            .map(|c| (c.tenant.as_str(), c.rule.as_str(), c.outcome, c.count))
            .collect::<Vec<_>>(),
        [("city", "stale-position", "lift", 1)]
    );
    let lifted = entity(&app, "city", "urn:v:old").await;
    assert!(lifted.get("quarantinedAt").is_none(), "{lifted}");
    assert!(lifted.get("quarantineRules").is_none(), "{lifted}");
}
