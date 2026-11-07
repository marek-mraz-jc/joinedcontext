// SPDX-License-Identifier: EUPL-1.2
//! Rule M4 (surfaces-threat-model 9.6, G6): a surface that serves through
//! views is closed, every route 404 and `closed` in `/q/health`, until a view
//! has loaded; it opens with the first view, closes with the last, and a
//! view that failed validation opens nothing. There is no allow-all view.
//!
//! Here and not in `antares-broker/tests`: no surface on the broker's shelf
//! serves through views until the view root (T-3498) joins it, so the test
//! mounts its own through the same `mount` the shelf will use.

use antares_ops::test_kit::TestBroker;
use antares_ops::views::View;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Two routes, one through an operation and one that answers by itself.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/probe"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new()
            .route("/static", get(|| async { "served" }))
            .route(
                "/types",
                get(move |caller: Caller| async move {
                    let req = OpsRequest::new(Method::GET, "/ngsi-ld/v1/types");
                    match ops
                        .run(OperationId::RetrieveEntityTypes, &caller, req)
                        .await
                    {
                        Ok(r) => r,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                }),
            )
    }
}

const ROUTES: [&str; 3] = ["/x/probe/static", "/x/probe/types", "/x/probe/nowhere"];

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
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

fn view(id: &str) -> Value {
    json!({
        "id": id, "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["retrieve_entity_types", "query_entities"]
    })
}

async fn health(app: &Router) -> Value {
    let (s, h) = send(app, Method::GET, "/q/health", None).await;
    assert_eq!(s, StatusCode::OK);
    h
}

async fn assert_closed(app: &Router) {
    for route in ROUTES {
        let (s, body) = send(app, Method::GET, route, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{route}: {body}");
        assert_eq!(
            body.get("title"),
            Some(&json!("ResourceNotFound")),
            "{route}: {body}"
        );
    }
    let h = health(app).await;
    assert_eq!(
        h.pointer("/surfaces/probe/state"),
        Some(&json!("closed")),
        "{h}"
    );
    assert_eq!(
        h.pointer("/surfaces/probe/reason"),
        Some(&json!("no view is loaded")),
        "{h}"
    );
}

async fn assert_open(app: &Router) {
    let (s, body) = send(app, Method::GET, "/x/probe/static", None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = send(app, Method::GET, "/x/probe/types", None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let h = health(app).await;
    assert_eq!(
        h.pointer("/surfaces/probe/state"),
        Some(&json!("open")),
        "{h}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selected_surface_without_a_view_is_closed() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    assert_closed(&app).await;
    assert_eq!(health(&app).await["views"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_surface_opens_when_its_first_view_loads() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    let (s, body) = send(&app, Method::PUT, "/q/views/cars", Some(&view("cars"))).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    assert_open(&app).await;
    let h = health(&app).await;
    assert_eq!(
        h["views"]["cars"],
        json!({"state": "loaded", "tenant": "city", "operations": 2, "notes": []}),
        "{h}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn closes_again_when_its_last_view_is_removed() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    for id in ["one", "two"] {
        let (s, _) = send(
            &app,
            Method::PUT,
            &format!("/q/views/{id}"),
            Some(&view(id)),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (s, _) = send(&app, Method::DELETE, "/q/views/one", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_open(&app).await;
    let (s, _) = send(&app, Method::DELETE, "/q/views/two", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_closed(&app).await;
    assert_eq!(health(&app).await["views"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_that_failed_validation_does_not_open_a_surface() {
    let broker = TestBroker::new().await.expect("broker");
    let app = broker.serve(Probe).expect("mounts");
    let mut refused = view("refused");
    refused["operations"] = json!(["drop_tables"]);

    // Through /q/views it is refused and nothing is stored.
    let (s, body) = send(&app, Method::PUT, "/q/views/refused", Some(&refused)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("drop_tables")),
        "{body}"
    );
    let (s, _) = send(&app, Method::GET, "/q/views/refused", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Left in the store some other way, it is named and not served.
    broker
        .store_view_doc("refused", refused)
        .await
        .expect("stored");
    assert_closed(&app).await;
    let h = health(&app).await;
    assert_eq!(h["views"]["refused"]["state"], "invalid", "{h}");
    assert!(
        h["views"]["refused"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("drop_tables")),
        "{h}"
    );
}

/// No view serves everything: `View::default()` does not exist (the
/// `compile_fail` example of `antares_ops::views`), and every way of
/// naming nothing is refused at load.
#[test]
fn there_is_no_allow_all_view() {
    let base = view("all");
    for (member, nothing) in [
        ("selector", json!([])),
        ("rules", json!([])),
        ("operations", json!([])),
    ] {
        let mut v = base.clone();
        v[member] = nothing;
        let parsed: View = serde_json::from_value(v).expect("a view document");
        assert!(parsed.load().is_err(), "an empty {member} loads");
    }
    // A rule that shares no type with the selector is dropped; with none
    // left, nothing would narrow.
    let mut v = base.clone();
    v["rules"] = json!([{"types": ["User"], "attributes": ["name"]}]);
    let parsed: View = serde_json::from_value(v).expect("a view document");
    assert!(
        parsed.load().is_err(),
        "a view with no rule of its selector's types loads"
    );
    for absent in ["selector", "rules", "operations", "tenant", "id"] {
        let mut v = base.clone();
        v.as_object_mut().expect("an object").remove(absent);
        assert!(
            serde_json::from_value::<View>(v).is_err(),
            "{absent} has a default"
        );
    }
}

/// T-3569: `/q/views` is written through the admin surface, so a broker with
/// an admin secret refuses a view read or write without it, and takes one
/// with it.
#[tokio::test(flavor = "multi_thread")]
async fn views_sit_behind_the_admin_secret() {
    const SECRET: &str = "views-are-written-with-the-admin-secret";
    let auth = antares_api::admin_auth::AdminAuth::new(SECRET, false).expect("secret");
    let app = TestBroker::new()
        .await
        .expect("broker")
        .with_admin_auth(auth)
        .serve(Probe)
        .expect("mounts");
    let call = |method: Method, uri: &'static str, body: Option<Value>, bearer: Option<&str>| {
        let bytes = body.as_ref().map(Value::to_string).unwrap_or_default();
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Length", bytes.len());
        if body.is_some() {
            req = req.header("Content-Type", "application/json");
        }
        if let Some(token) = bearer {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let app = app.clone();
        async move {
            let Ok(resp) = app
                .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
                .await;
            resp.status()
        }
    };
    for bearer in [None, Some("not-the-admin-secret")] {
        assert_eq!(
            call(Method::PUT, "/q/views/cars", Some(view("cars")), bearer).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(Method::GET, "/q/views", None, bearer).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_closed(&app).await;
    assert_eq!(
        call(
            Method::PUT,
            "/q/views/cars",
            Some(view("cars")),
            Some(SECRET)
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(Method::GET, "/q/views", None, Some(SECRET)).await,
        StatusCode::OK
    );
    assert_open(&app).await;
}

/// `Probe` on the NGSI-LD root itself: every operation runs unnarrowed
/// through the policy seam, as the root does, so no view gates it.
struct RootProbe;

impl Surface for RootProbe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/probe"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Probe.router(ops)
    }
    fn needs_view(&self) -> bool {
        false
    }
}

/// A surface that answers `needs_view() == false` (the WebSocket channel
/// of a Tenant, ADR-0026 rule 2) is open with no view loaded, and stays
/// open while views come and go; an unknown route under it is still 404.
#[tokio::test(flavor = "multi_thread")]
async fn a_surface_of_the_root_is_open_without_a_view() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(RootProbe)
        .expect("mounts");
    assert_open(&app).await;
    let (s, _) = send(&app, Method::GET, "/x/probe/nowhere", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, Method::PUT, "/q/views/cars", Some(&view("cars"))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_open(&app).await;
    let (s, _) = send(&app, Method::DELETE, "/q/views/cars", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_open(&app).await;
}
