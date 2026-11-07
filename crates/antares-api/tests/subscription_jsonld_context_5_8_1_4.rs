// SPDX-License-Identifier: EUPL-1.2
//! TS 104 175 5.8.1.4: "The @context to be used for sending Notifications
//! related to this Subscription shall be the one specified in the
//! jsonldContext field. If not present, the jsonldContext field shall be
//! initialized with the @context applicable for the Subscription (if
//! @context is also not present in the Subscription, see clause 5.5.5)."
//!
//! A client reading its Subscription back sees which @context its
//! Notifications use (Coraine subscription_jsonldcontext_autofill).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const V18: &str = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld";

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Create Subscription `id` with `extra` members and an optional Link, then
/// return its jsonldContext as read back.
async fn initialized(st: &AppState, id: &str, extra: Value, link: Option<&str>) -> Value {
    let mut sub = json!({
        "id": id,
        "type": "Subscription",
        "entities": [{"type": "Vehicle"}],
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"}},
    });
    let ld = extra.get("@context").is_some();
    sub.as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    let body = sub.to_string();
    let mut req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/subscriptions")
        .header(
            "Content-Type",
            if ld {
                "application/ld+json"
            } else {
                "application/json"
            },
        )
        .header("Content-Length", body.len());
    if let Some(url) = link {
        req = req.header(
            "Link",
            format!(
                "<{url}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\""
            ),
        );
    }
    let (status, err) = send(st, req.body(Body::from(body)).expect("req")).await;
    assert_eq!(status, StatusCode::CREATED, "{id}: {err}");
    let (status, got) = send(
        st,
        Request::builder()
            .uri(format!("/ngsi-ld/v1/subscriptions/{id}"))
            .body(Body::empty())
            .expect("req"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    got.get("jsonldContext").cloned().unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_jsonld_context_is_initialized_with_the_subscriptions_context() {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("test-sub-jsonld-context".into());
    antares_api::wire(&mut st).await;

    // no @context anywhere: the core context (5.5.5)
    assert_eq!(
        initialized(&st, "urn:S1", json!({}), None).await,
        antares_jsonld::CORE_CONTEXT
    );
    // a Link: that URL
    assert_eq!(initialized(&st, "urn:S2", json!({}), Some(V18)).await, V18);
    // a one-element array holds one URL: that URL, nothing minted
    assert_eq!(
        initialized(&st, "urn:S3", json!({"@context": [V18]}), None).await,
        V18
    );
    // a client's own jsonldContext stays as it is
    assert_eq!(
        initialized(&st, "urn:S4", json!({"jsonldContext": V18}), None).await,
        V18
    );
    // an inline @context: hosted by the broker at a URL that answers with it
    let minted = initialized(
        &st,
        "urn:S5",
        json!({"@context": {"Vehicle": "https://example.org/Vehicle"}}),
        None,
    )
    .await;
    let url = minted.as_str().expect("a URL");
    let path = url
        .find("/ngsi-ld/v1/jsonldContexts/")
        .map(|i| &url[i..])
        .expect("hosted under jsonldContexts");
    let (status, body) = send(
        &st,
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("req"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["@context"]["Vehicle"], "https://example.org/Vehicle");
}

/// 5.8.1.4 is about entity Subscriptions; a Context Source Registration
/// Subscription keeps no jsonldContext it was not given (ETSI 044, 045).
#[tokio::test(flavor = "multi_thread")]
async fn a_csource_subscription_gets_no_jsonld_context() {
    let mut st = AppState::new("test-csub-jsonld-context".into());
    antares_api::wire(&mut st).await;
    let body = json!({
        "id": "urn:CS1",
        "type": "Subscription",
        "entities": [{"type": "Vehicle"}],
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"}},
    })
    .to_string();
    let (status, err) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceSubscriptions")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{err}");
    let (status, got) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/csourceSubscriptions/urn:CS1")
            .body(Body::empty())
            .expect("req"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(got.get("jsonldContext").is_none(), "{got}");
}
