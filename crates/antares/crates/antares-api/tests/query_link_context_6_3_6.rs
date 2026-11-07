// SPDX-License-Identifier: EUPL-1.2
//! CIM 009 V1.9.1 6.3.6 (p.271-272), TS 104 176 6.2.3 (p.23-24): "If the
//! target response's MIME type is "application/json" such response shall
//! include a Link to the associated JSON-LD @context". A POST query whose
//! @context is inline has no URL of its own, so the broker hosts it as an
//! ImplicitlyCreated @context (TS 104 175 13.1) and the Link names that URL
//! (Coraine context_volatile_hosting_local steps 02-05).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const INLINE: &str =
    r#"{"T": "https://example.org/types/T", "spd": "https://example.org/veh/speed"}"#;

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, Option<String>, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp");
    let status = resp.status();
    let link = resp
        .headers()
        .get("Link")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        link,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn post(uri: &str, accept: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/ld+json")
        .header("Accept", accept)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("req")
}

fn query(ctx: &str) -> String {
    format!(r#"{{"type": "Query", "entities": [{{"type": "T"}}], "@context": {ctx}}}"#)
}

/// The URL between `<` and `>` of a Link header.
fn link_url(link: &str) -> &str {
    link.split_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(url, _)| url)
        .expect("Link names a URL")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_json_answer_to_an_inline_context_query_links_the_hosted_context() {
    let mut st = AppState::new("test-query-link-context".into());
    antares_api::wire(&mut st).await;
    let entity = format!(
        r#"{{"id": "urn:E1", "type": "T", "spd": {{"type": "Property", "value": 5}}, "@context": {INLINE}}}"#
    );
    let (status, _, err) = send(
        &st,
        post("/ngsi-ld/v1/entities", "application/json", entity),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{err}");

    let q = "/ngsi-ld/v1/entityOperations/query";
    let (status, link, body) = send(&st, post(q, "application/json", query(INLINE))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body[0]["spd"]["value"],
        json!(5),
        "compacted with the inline terms"
    );
    let link = link.expect("a json answer carries a Link (6.3.6)");
    let url = link_url(&link).to_owned();
    assert!(
        url.contains("/ngsi-ld/v1/jsonldContexts/urn:ngsi-ld:Context:v"),
        "Link names a hosted @context, not the core one: {link}"
    );

    // the same inline @context again: the same URL, no second row
    let (_, again, _) = send(&st, post(q, "application/json", query(INLINE))).await;
    assert_eq!(link_url(&again.expect("Link")), url);

    // the hosted URL serves the request's @context, more than once
    let path = &url[url.find("/ngsi-ld/").expect("path")..];
    for _ in 0..2 {
        let (status, _, served) = send(
            &st,
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("req"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            served["@context"]["spd"],
            json!("https://example.org/veh/speed")
        );
    }

    // under ld+json the @context travels in the body and nothing is hosted:
    // a different inline @context leaves no row behind
    let other = r#"{"T": "https://example.org/types/T", "s": "https://example.org/veh/speed"}"#;
    let (status, _, body) = send(&st, post(q, "application/ld+json", query(other))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body[0]["@context"]["s"],
        json!("https://example.org/veh/speed")
    );
    let (_, _, listed) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/jsonldContexts?kind=ImplicitlyCreated")
            .body(Body::empty())
            .expect("req"),
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
}

/// The hosted row belongs to the Tenant whose request created it (5.5.10):
/// another Tenant sending the same @context gets its own URL, and cannot
/// read the first one's.
#[tokio::test(flavor = "multi_thread")]
async fn the_hosted_context_is_per_tenant() {
    let mut st = AppState::new("test-query-link-context-tenant".into());
    antares_api::wire(&mut st).await;
    let q = "/ngsi-ld/v1/entityOperations/query";
    let (_, a, _) = send(&st, post(q, "application/json", query(INLINE))).await;
    // a create makes the Tenant exist (5.5.10)
    let entity = format!(r#"{{"id": "urn:E2", "type": "T", "@context": {INLINE}}}"#);
    let mut req = post("/ngsi-ld/v1/entities", "application/json", entity);
    req.headers_mut()
        .insert("NGSILD-Tenant", "other".parse().expect("header"));
    let (status, _, err) = send(&st, req).await;
    assert_eq!(status, StatusCode::CREATED, "{err}");
    let mut req = post(q, "application/json", query(INLINE));
    req.headers_mut()
        .insert("NGSILD-Tenant", "other".parse().expect("header"));
    let (status, b, body) = send(&st, req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (a, b) = (a.expect("Link"), b.expect("Link"));
    assert_ne!(link_url(&a), link_url(&b));
    let path = {
        let u = link_url(&a);
        u[u.find("/ngsi-ld/").expect("path")..].to_owned()
    };
    let (status, _, _) = send(
        &st,
        Request::builder()
            .uri(path)
            .header("NGSILD-Tenant", "other")
            .body(Body::empty())
            .expect("req"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
