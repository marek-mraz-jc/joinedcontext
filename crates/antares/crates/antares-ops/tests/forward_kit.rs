// SPDX-License-Identifier: EUPL-1.2
//! What an internal context source module builds on (ADR-0025 rules 3 and
//! 5): the transport `TestBroker::with_forward_transport` registers, the
//! export-grant documents, a route added with `Ops::add_admin`, a forward
//! served through a view's `ViewOps::forward_in_process`, and the depth
//! bound that cuts a chain of in-process forwards.

use antares_ops::test_kit::TestBroker;
use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport, Ops};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;

const SELF_URL: &str = "http://self.invalid";

/// Serves every forward in process, a read through view `v` while it is
/// loaded.
struct Through(Ops);

impl ForwardTransport for Through {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        let read = req.method() == axum::http::Method::GET;
        Some(match self.0.view("v").filter(|_| read) {
            Some(view) => Box::pin(async move { view.forward_in_process(req, SELF_URL).await }),
            None => Box::pin(self.0.forward_in_process(req, SELF_URL)),
        })
    }
}

/// The broker with [`Through`] registered, and the `Ops` it was made with.
async fn broker() -> (TestBroker, Ops) {
    let kept = Arc::new(Mutex::new(None));
    let keep = kept.clone();
    let b = TestBroker::new()
        .await
        .unwrap_or_else(|e| unreachable!("{e}"))
        .with_admin(Router::new().route("/kit", get(|| async { "kit" })))
        .with_forward_transport(move |ops| {
            ops.add_admin(Router::new().route("/probe", get(|| async { "probe" })));
            *keep.lock().unwrap_or_else(PoisonError::into_inner) = Some(ops.clone());
            Arc::new(Through(ops))
        })
        .unwrap_or_else(|e| unreachable!("{e}"));
    let ops = kept
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| unreachable!("the transport is made with the broker"));
    (b, ops)
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    tenant: &str,
    doc: Option<Value>,
) -> (StatusCode, Vec<String>, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("NGSILD-Tenant", tenant);
    let req = match doc {
        Some(d) => {
            let payload = d.to_string();
            req.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => req.body(Body::empty()),
    }
    .unwrap_or_else(|e| unreachable!("{e}"));
    let answer = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let status = answer.status();
    let warnings = answer
        .headers()
        .get_all("ngsild-warning")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let bytes = answer
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        warnings,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

#[tokio::test]
async fn grant_documents_round_trip_and_an_admin_route_answers_under_q() {
    let (b, ops) = broker().await;
    assert_eq!(ops.grant_doc("b~a").await, Ok(None));
    assert_eq!(ops.put_grant_doc("b~a", json!({"n": 1})).await, Ok(false));
    assert_eq!(ops.put_grant_doc("b~a", json!({"n": 2})).await, Ok(true));
    assert_eq!(ops.grant_doc("b~a").await, Ok(Some(json!({"n": 2}))));
    assert_eq!(ops.grant_docs().await.map(|d| d.len()), Ok(1));
    assert_eq!(ops.delete_grant_doc("b~a").await, Ok(true));
    assert_eq!(ops.delete_grant_doc("b~a").await, Ok(false));
    assert_eq!(ops.grant_docs().await.map(|d| d.len()), Ok(0));
    let (status, _, body) = send(&b.router(), "GET", "/q/probe", "a", None).await;
    assert_eq!((status, body), (StatusCode::OK, json!("probe")));
    let (status, _, body) = send(&b.router(), "GET", "/q/kit", "a", None).await;
    assert_eq!((status, body), (StatusCode::OK, json!("kit")));
    assert!(
        b.with_forward_transport(|ops| Arc::new(Through(ops)))
            .is_err(),
        "one transport per broker"
    );
}

/// t0 registers t1, t1 registers t2, and so on to t9, through view `v`; a
/// query in t0 reaches t8, eight in-process calls deep, and the forward to
/// t9 is not made (504, code 199). The same chain of redirect
/// registrations makes a write in w0 answer 508.
#[tokio::test]
async fn a_chain_of_forwards_past_the_depth_bound_is_cut() {
    let (b, ops) = broker().await;
    let view = json!({"id": "v", "tenant": "t0", "selector": ["Vehicle"],
                      "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}],
                      "operations": ["query_entities", "retrieve_entity"]});
    b.store_view_doc("v", view).await.expect("stored");
    assert!(ops.view("v").is_some(), "the chain runs through the view");
    let app = b.router();
    for n in 0..9u32 {
        let next = n.saturating_add(1);
        for (prefix, extra) in [
            ("t", json!({"operations": ["queryEntity"]})),
            (
                "w",
                json!({"mode": "redirect", "operations": ["createEntity"]}),
            ),
        ] {
            let mut reg = json!({
                "id": format!("urn:ngsi-ld:ContextSourceRegistration:{prefix}{n}"),
                "type": "ContextSourceRegistration",
                "information": [{"entities": [{"type": "Vehicle"}]}],
                "endpoint": SELF_URL, "tenant": format!("{prefix}{next}"),
            });
            if let (Some(r), Some(e)) = (reg.as_object_mut(), extra.as_object()) {
                r.extend(e.clone());
            }
            let tenant = format!("{prefix}{n}");
            let (status, _, body) = send(
                &app,
                "POST",
                "/ngsi-ld/v1/csourceRegistrations",
                &tenant,
                Some(reg),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{tenant}: {body}");
        }
    }
    for n in [8, 9] {
        let car = json!({"id": format!("urn:ngsi-ld:Vehicle:{n}"), "type": "Vehicle"});
        let tenant = format!("t{n}");
        let (status, _, body) =
            send(&app, "POST", "/ngsi-ld/v1/entities", &tenant, Some(car)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, warnings, body) =
        send(&app, "GET", "/ngsi-ld/v1/entities?type=Vehicle", "t0", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!([{"id": "urn:ngsi-ld:Vehicle:8", "type": "Vehicle"}])
    );
    assert!(
        warnings.iter().any(|w| w.starts_with("199 ")),
        "{warnings:?}"
    );
    let car = json!({"id": "urn:ngsi-ld:Vehicle:w", "type": "Vehicle"});
    let (status, _, body) = send(&app, "POST", "/ngsi-ld/v1/entities", "w0", Some(car)).await;
    assert_eq!(status, StatusCode::LOOP_DETECTED, "{body}");
}

/// F-ANT5-010/011: the `type=` a surface pushes. A view naming its types
/// offers the shown ones of the wanted names, none when it shows none; a
/// view of every type offers the Tenant's stored types of those names, and
/// the wanted names themselves when no Entity of them is stored yet.
#[tokio::test]
async fn a_surface_pushes_the_stored_types_of_a_view_of_every_type() {
    let (b, ops) = broker().await;
    let named = json!({"id": "named", "tenant": "t0", "selector": ["Vehicle"],
                       "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}],
                       "operations": ["query_entities"]});
    let every = json!({"id": "every", "tenant": "t0", "selector": ["*"],
                       "rules": [{"types": ["*"], "attributes": ["*"]}],
                       "operations": ["query_entities"]});
    b.store_view_doc("named", named).await.expect("stored");
    b.store_view_doc("every", every).await.expect("stored");
    let app = b.router();
    for (id, ty) in [
        ("urn:ngsi-ld:Event:1", "https://example.org/Event"),
        ("urn:ngsi-ld:Noise:1", "Noise"),
    ] {
        let (status, _, body) = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            "t0",
            Some(json!({"id": id, "type": ty})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let named = ops.view("named").expect("named view");
    assert_eq!(
        named.type_selector(&["Vehicle"]).await.as_deref(),
        Some("type=Vehicle")
    );
    assert_eq!(named.type_selector(&["Event"]).await, None);
    let every = ops.view("every").expect("every view");
    assert_eq!(
        every.type_selector(&["Event"]).await.as_deref(),
        Some("type=https%3A%2F%2Fexample.org%2FEvent")
    );
    assert_eq!(
        every.type_selector(&["Article"]).await.as_deref(),
        Some("type=Article")
    );
}
