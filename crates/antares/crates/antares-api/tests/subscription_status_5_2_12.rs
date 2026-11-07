// SPDX-License-Identifier: EUPL-1.2
//! TS 104 175 5.2.12, as the stored Subscription reads after any operation.
//!
//! Table 5.2.12-2: a Subscription's `status` has the allowed
//! values "active", "paused" and "expired". A failed delivery is the
//! Notification's status (5.8.6: "Update notification.status to "failed"";
//! Table 5.2.14.2-1 "ok", "failed"), so the Subscription stays "active"
//! (Coraine subscription_stats_status).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let resp = antares_api::router(st.clone())
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("req"),
        )
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_delivery_leaves_the_subscription_active() {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("test-sub-status".into());
    antares_api::wire(&mut st).await;
    // a closed port: every delivery fails
    let sub = r#"{"id":"urn:S1","type":"Subscription","entities":[{"type":"Vehicle"}],
        "notification":{"endpoint":{"uri":"http://127.0.0.1:9/n"}}}"#;
    let (s, b) = send(&st, "POST", "/ngsi-ld/v1/subscriptions", Some(sub)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let ent = r#"{"id":"urn:V1","type":"Vehicle","speed":{"type":"Property","value":1}}"#;
    let (s, b) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(ent)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");

    let mut got = Value::Null;
    for _ in 0..100 {
        (_, got) = send(&st, "GET", "/ngsi-ld/v1/subscriptions/urn:S1", None).await;
        if got["notification"]["status"] == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(got["notification"]["status"], "failed", "{got}");
    assert_eq!(got["status"], "active", "{got}");
}

/// 5.2.12: "At least one of (a) entities or (b) watchedAttributes shall be
/// present, unless the member localOnly is set to true". An update that
/// deletes both leaves no Subscription the clause allows: refused, and the
/// stored one keeps its selector (Coraine subscription_patch_validation_errors).
#[tokio::test(flavor = "multi_thread")]
async fn an_update_cannot_remove_the_last_selector() {
    let mut st = AppState::new("test-sub-selector".into());
    antares_api::wire(&mut st).await;
    let sub = r#"{"id":"urn:S2","type":"Subscription","entities":[{"type":"Vehicle"}],
        "watchedAttributes":["speed"],"notification":{"endpoint":{"uri":"http://127.0.0.1:9/n"}}}"#;
    let (s, b) = send(&st, "POST", "/ngsi-ld/v1/subscriptions", Some(sub)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let uri = "/ngsi-ld/v1/subscriptions/urn:S2";
    let both = r#"{"entities":"urn:ngsi-ld:null","watchedAttributes":"urn:ngsi-ld:null"}"#;
    let (s, b) = send(&st, "PATCH", uri, Some(both)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(
        b["type"],
        "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
    );
    // one of the two may go
    let one = r#"{"watchedAttributes":"urn:ngsi-ld:null"}"#;
    let (s, b) = send(&st, "PATCH", uri, Some(one)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (_, got) = send(&st, "GET", uri, None).await;
    assert_eq!(got["entities"][0]["type"], "Vehicle", "{got}");
    assert!(got.get("watchedAttributes").is_none(), "{got}");
}

/// F-ANT3-016: Table 5.2.12-1's exclusive pairs hold for the updated
/// Subscription, not only within the fragment: a PATCH adding
/// `timeInterval` to a Subscription with `watchedAttributes` (or
/// `throttling`) is refused and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_patch_may_not_pair_time_interval_with_a_stored_member() {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("test-sub-pairs".into());
    antares_api::wire(&mut st).await;
    let sub = r#"{"id":"urn:S2","type":"Subscription","entities":[{"type":"Vehicle"}],
        "watchedAttributes":["speed"],"throttling":5,
        "notification":{"endpoint":{"uri":"http://127.0.0.1:9/n"}}}"#;
    let (s, b) = send(&st, "POST", "/ngsi-ld/v1/subscriptions", Some(sub)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/subscriptions/urn:S2",
        Some(r#"{"timeInterval":10}"#),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let (_, got) = send(&st, "GET", "/ngsi-ld/v1/subscriptions/urn:S2", None).await;
    assert_eq!(got.get("timeInterval"), None, "{got}");
}
