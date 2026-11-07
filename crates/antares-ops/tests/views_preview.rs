// SPDX-License-Identifier: EUPL-1.2
//! `POST /q/views/{id}/preview` (gap G13): a draft view's narrowing answered
//! as a count and one bounded page, the stored view and the table left as
//! they were, an invalid draft refused with the loader's reason, the bounds
//! held, the admin credential required, and no view root that reaches it.

use antares_api::admin_auth::AdminAuth;
use antares_api::policy::{DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject};
use antares_api::AppState;
use antares_ops::test_kit::TestBroker;
use antares_ops::views::{MAX_PREVIEW_LIMIT, MAX_VIEW_BYTES};
use antares_ops::OperationId;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Nothing;

impl antares_ops::Surface for Nothing {
    fn name(&self) -> &'static str {
        "nothing"
    }
    fn prefix(&self) -> &'static str {
        "/x/nothing"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: antares_ops::Ops) -> Router {
        Router::new()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: Option<&str>,
    body: Option<String>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let bytes = body.unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Length", bytes.len());
    if !bytes.is_empty() {
        req = req.header("Content-Type", "application/json");
    }
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
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

fn view(id: &str, q: &str) -> Value {
    json!({
        "id": id, "tenant": "city", "selector": ["Car"], "q": q,
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities"]
    })
}

async fn preview(app: &Router, id: &str, query: &str, draft: &Value) -> (StatusCode, Value) {
    send(
        app,
        Method::POST,
        &format!("/q/views/{id}/preview{query}"),
        None,
        Some(draft.to_string()),
        None,
    )
    .await
}

/// Three Cars at 10, 50 and 90 and a Bus at 70, each with an owner, in
/// `city`.
async fn seed(app: &Router) {
    for (id, ty, speed) in [
        ("c1", "Car", 10),
        ("c2", "Car", 50),
        ("c3", "Car", 90),
        ("b1", "Bus", 70),
    ] {
        let e = json!({
            "id": format!("urn:ngsi-ld:{ty}:{id}"), "type": ty,
            "speed": {"type": "Property", "value": speed},
            "owner": {"type": "Property", "value": "someone"},
        });
        let (s, body) = send(
            app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some("city"),
            Some(e.to_string()),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_answers_the_drafts_narrowing_and_stores_nothing() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve(Nothing).expect("mounts");
    seed(&app).await;
    let stored = view("cars", "speed>80");
    let (s, _) = send(
        &app,
        Method::PUT,
        "/q/views/cars",
        None,
        Some(stored.to_string()),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let table = send(&app, Method::GET, "/q/views", None, None, None)
        .await
        .1;

    let (s, body) = preview(&app, "cars", "?type=Car", &view("cars", "speed>20")).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 2);
    let entities = body["entities"].as_array().expect("a page");
    let mut ids: Vec<&str> = entities.iter().filter_map(|e| e["id"].as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["urn:ngsi-ld:Car:c2", "urn:ngsi-ld:Car:c3"]);
    assert!(
        entities.iter().all(|e| e.get("owner").is_none()),
        "the draft shows speed only: {body}"
    );

    // The Bus is outside the selector, whatever the query asks.
    let (s, body) = preview(&app, "cars", "?type=Bus", &view("cars", "speed>20")).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"count": 0, "entities": []}));

    // One page, bounded, with the whole count; a caller's count is ignored.
    let (s, body) = preview(
        &app,
        "cars",
        "?type=Car&limit=1&count=false",
        &view("cars", "speed>0"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 3);
    assert_eq!(body["entities"].as_array().map(Vec::len), Some(1));

    // Stored view and table as they were.
    let (_, doc) = send(&app, Method::GET, "/q/views/cars", None, None, None).await;
    assert_eq!(doc, stored);
    assert_eq!(
        send(&app, Method::GET, "/q/views", None, None, None)
            .await
            .1,
        table
    );
    // A draft of a view that is not stored stays unstored.
    let (s, _) = preview(&app, "draft", "?type=Car", &view("draft", "speed>0")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(&app, Method::GET, "/q/views/draft", None, None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_draft_or_query_is_refused_and_the_bounds_hold() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve(Nothing).expect("mounts");
    seed(&app).await;
    let mut off_matrix = view("v", "speed>0");
    off_matrix["operations"] = json!(["query_entities", "drop_tables"]);
    let (s, body) = preview(&app, "v", "", &off_matrix).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("drop_tables")),
        "the loader's reason: {body}"
    );
    let (s, _) = preview(&app, "w", "", &view("v", "speed>0")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "an id the path does not name");
    let (s, _) = send(
        &app,
        Method::POST,
        "/q/views/v/preview",
        None,
        Some("{".into()),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let big = "x".repeat(MAX_VIEW_BYTES + 1);
    let (s, _) = send(
        &app,
        Method::POST,
        "/q/views/v/preview",
        None,
        Some(big),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    let over = format!("?limit={}", MAX_PREVIEW_LIMIT + 1);
    let (s, _) = preview(&app, "v", &over, &view("v", "speed>0")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A query the broker refuses comes back as it refused it.
    let (s, body) = preview(&app, "v", "?q=speed%3E", &view("v", "speed>0")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["type"],
        "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
    );
    let (s, _) = send(&app, Method::GET, "/q/views/v", None, None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing stored");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_needs_the_admin_secret_and_no_view_root_reaches_it() {
    const SECRET: &str = "previews-are-asked-with-the-admin-secret";
    let auth = AdminAuth::new(SECRET, false).expect("secret");
    let app = TestBroker::new()
        .await
        .expect("broker")
        .with_admin_auth(auth)
        .serve(Nothing)
        .expect("mounts");
    let draft = Some(view("v", "speed>0").to_string());
    // A Tenant with no data is refused as the API refuses it.
    let (s, body) = send(
        &app,
        Method::POST,
        "/q/views/v/preview?type=Car",
        None,
        draft.clone(),
        Some(SECRET),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["title"], "NonexistentTenant");
    seed(&app).await;
    for bearer in [None, Some("not-the-admin-secret")] {
        let (s, _) = send(
            &app,
            Method::POST,
            "/q/views/v/preview",
            None,
            draft.clone(),
            bearer,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, body) = send(
        &app,
        Method::POST,
        "/q/views/v/preview?type=Car",
        None,
        draft,
        Some(SECRET),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 3);
    // A view root runs only the operations of the matrix (`Ops::run`):
    // the preview is none of them.
    assert_eq!(OperationId::of("POST", "/q/views/v/preview", None), None);
}

/// An engine that fails every decision, which the seam answers as a
/// refusal.
struct Broken;

impl PolicyEngine for Broken {
    fn name(&self) -> &'static str {
        "broken"
    }
    #[expect(clippy::panic, reason = "the engine under test fails")]
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        panic!("broken engine")
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_asks_the_engine_and_answers_its_refusal() {
    let mut state = AppState::new("antares-test".into());
    antares_api::wire(&mut state).await;
    // Seeded before the engine is attached: the engine fails every write.
    seed(&antares_api::router(state.clone())).await;
    let state = state.with_policy(Arc::new(Broken));
    let _reload = antares_ops::views::install(&state)
        .await
        .expect("installed");
    let app = antares_api::router(
        state
            .with_surfaces(vec![Box::new(antares_api::Admin)])
            .expect("admin"),
    );
    let (s, body) = preview(&app, "v", "?type=Car", &view("v", "speed>0")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["type"], "urn:antares:error:AccessDenied");
}

/// T-5895: a preview the broker's buffered-answer pool has no room for is
/// 503 with Retry-After, refused whole, never a page cut short.
#[tokio::test(flavor = "multi_thread")]
async fn a_preview_past_the_answer_pool_is_503() {
    let b = TestBroker::with_answer_pool(32).await.expect("broker");
    let app = b.serve(Nothing).expect("mounts");
    seed(&app).await;
    let (s, _) = preview(&app, "cars", "?type=Car", &view("cars", "speed>0")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

/// T-3773: a preview page larger than the broker buffers for one request is
/// 413, not held whole in memory.
#[tokio::test(flavor = "multi_thread")]
async fn a_preview_past_the_buffer_bound_is_413() {
    let b = TestBroker::with_max_answer_bytes(32).await.expect("broker");
    let app = b.serve(Nothing).expect("mounts");
    seed(&app).await;
    let (s, body) = preview(&app, "cars", "?type=Car", &view("cars", "speed>0")).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["detail"], antares_api::bounds::ANSWER_TOO_LARGE);
}
