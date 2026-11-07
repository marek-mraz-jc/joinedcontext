// SPDX-License-Identifier: EUPL-1.2
//! `/q/schemas/{tenant}` and `ViewOps::schema` (T-3628): a Tenant's `LinkML`
//! schema stored beside the view table, bounded and checked before it is
//! stored, reloaded on every pod, and handed only to the views of its Tenant.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Ops, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Every test of this binary runs with the same bound, so setting it from
/// each test never races to a different value.
const MAX: usize = 1024;

/// `/x/schema/{view}` answers what `ViewOps::schema` hands the view: its
/// revision and bytes, or `null`.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "schema"
    }
    fn prefix(&self) -> &'static str {
        "/x/schema"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}",
            get(move |Path(id): Path<String>| async move {
                let schema = ops.view(&id).and_then(|v| v.schema()).map(|(rev, bytes)| {
                    json!({"revision": rev, "text": String::from_utf8_lossy(&bytes)})
                });
                axum::Json(json!(schema))
            }),
        )
    }
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn view(id: &str, tenant: &str) -> String {
    json!({
        "id": id, "tenant": tenant, "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities"]
    })
    .to_string()
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, MAX.to_string());
    let b = TestBroker::new().await?;
    let app = b.serve(Probe)?;
    for (id, tenant) in [("city-cars", "city"), ("port-cars", "port")] {
        let (s, body) = send(
            &app,
            Method::PUT,
            &format!("/q/views/{id}"),
            Some(&view(id, tenant)),
        )
        .await;
        if s != StatusCode::CREATED {
            return Err(format!("view {id}: {s} {body}"));
        }
    }
    Ok((b, app))
}

const SCHEMA: &str = r#"{"id": "https://w3id.org/city", "name": "city", "classes": {"Car": {}}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_schema_is_put_read_replaced_and_deleted() {
    let (_b, app) = broker().await.expect("broker");
    let (s, _) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, body) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(
        (s, body.as_str()),
        (StatusCode::OK, SCHEMA),
        "the bytes as written"
    );

    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some("{}")).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "a second write replaces");
    let (_, body) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(body, "{}");

    let (_, h) = send(&app, Method::GET, "/q/health", None).await;
    let h = json_of(&h);
    assert_eq!(h["schemas"]["city"]["bytes"], 2, "{h}");
    assert!(
        h["schemas"]["city"].get("text").is_none(),
        "health never serves the document"
    );

    let (s, _) = send(&app, Method::DELETE, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, body) = send(&app, Method::DELETE, "/q/schemas/city", None).await;
    assert_eq!(
        (s, json_of(&body)["title"].clone()),
        (StatusCode::NOT_FOUND, json!("ResourceNotFound"))
    );
    let (_, h) = send(&app, Method::GET, "/q/health", None).await;
    assert_eq!(json_of(&h)["schemas"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_or_unparseable_schema_is_refused_and_not_stored() {
    let (_b, app) = broker().await.expect("broker");
    let at_max = format!("\"{}\"", "a".repeat(MAX - 2));
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(&at_max)).await;
    assert_eq!(s, StatusCode::CREATED, "exactly the bound is taken");
    let (s, _) = send(&app, Method::DELETE, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let over = format!("\"{}\"", "a".repeat(MAX - 1));
    let (s, body) = send(&app, Method::PUT, "/q/schemas/city", Some(&over)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(
        json_of(&body)["title"],
        "BadRequestData",
        "one byte over: a problem document"
    );
    let far = "a".repeat(MAX * 4);
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(&far)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "far over: refused unread");

    for bad in ["{", "classes: {}", "{\"a\": 1} trailing"] {
        let (s, body) = send(&app, Method::PUT, "/q/schemas/city", Some(bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert!(
            json_of(&body)["detail"]
                .as_str()
                .is_some_and(|d| d.starts_with("not JSON")),
            "{body}"
        );
    }
    let invalid_utf8 = Request::builder()
        .method(Method::PUT)
        .uri("/q/schemas/city")
        .header("Content-Length", 3)
        .body(Body::from(vec![b'"', 0xff, b'"']))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(invalid_utf8).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let (s, _) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing refused was stored");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_path_names_a_tenant_a_client_can() {
    let (_b, app) = broker().await.expect("broker");
    for (method, body) in [
        (Method::PUT, Some(SCHEMA)),
        (Method::GET, None),
        (Method::DELETE, None),
    ] {
        for tenant in ["antares-views", "Bad%20Tenant", "distsub-index"] {
            let (s, out) = send(&app, method.clone(), &format!("/q/schemas/{tenant}"), body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{method} {tenant}: {out}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_reads_the_schema_of_its_own_tenant_only() {
    let (_b, app) = broker().await.expect("broker");
    let (_, none) = send(&app, Method::GET, "/x/schema/city-cars", None).await;
    assert_eq!(json_of(&none), Value::Null, "no schema stored");
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, city) = send(&app, Method::GET, "/x/schema/city-cars", None).await;
    assert_eq!(json_of(&city)["text"], SCHEMA);
    let (_, port) = send(&app, Method::GET, "/x/schema/port-cars", None).await;
    assert_eq!(
        json_of(&port),
        Value::Null,
        "another Tenant's schema never reaches a view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_revision_changes_with_every_write_that_changes_the_bytes() {
    let (_b, app) = broker().await.expect("broker");
    let mut seen = Vec::new();
    for doc in [SCHEMA, "{}", "[]", SCHEMA] {
        let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(doc)).await;
        assert!(s.is_success());
        let (_, got) = send(&app, Method::GET, "/x/schema/city-cars", None).await;
        seen.push(json_of(&got)["revision"].clone());
    }
    assert_ne!(seen[0], seen[1]);
    assert_ne!(seen[1], seen[2]);
    assert_eq!(
        seen[0], seen[3],
        "the same bytes keep their revision: a parse cached under it stays right"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn another_pod_serves_the_schema_after_its_reload() {
    let (b, app) = broker().await.expect("broker");
    let sibling = b.sibling().await.expect("sibling");
    let other = sibling.serve(Probe).expect("router");
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, before) = send(&other, Method::GET, "/q/health", None).await;
    assert_eq!(json_of(&before)["schemas"], json!({}), "not reloaded yet");
    sibling.reload_now().await.expect("reload");
    let (_, after) = send(&other, Method::GET, "/x/schema/city-cars", None).await;
    assert_eq!(json_of(&after)["text"], SCHEMA);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_routes_never_read_or_delete_a_schema() {
    let (b, app) = broker().await.expect("broker");
    let (s, _) = send(&app, Method::PUT, "/q/schemas/city", Some(SCHEMA)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = send(&app, Method::GET, "/q/views/schema:city", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, Method::DELETE, "/q/views/schema:city", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(
        &app,
        Method::PUT,
        "/q/views/schema:city",
        Some(&view("schema:city", "city")),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, list) = send(&app, Method::GET, "/q/views", None).await;
    assert!(json_of(&list).get("schema:city").is_none(), "{list}");
    let (s, _) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::OK, "still stored");

    b.store_view_doc("schema:port", json!({"id": "schema:port"}))
        .await
        .expect("stored");
    let (_, h) = send(&app, Method::GET, "/q/health", None).await;
    let h = json_of(&h);
    assert!(
        h["schemas"].get("port").is_none(),
        "a schema document without text is skipped: {h}"
    );
    assert!(
        h["views"].get("schema:port").is_none(),
        "and is no view: {h}"
    );
}

/// A slot named after a member of an Attribute (`value`, `object`,
/// `unitCode`, `json`) could never be an Attribute: the PUT refuses it,
/// naming the slot, wherever the schema declares it (T-3956). `location` is
/// an Attribute, and `id`, `type` and `observedAt` are the Entity's own as
/// the platform's core model declares them: they load.
#[tokio::test(flavor = "multi_thread")]
async fn a_slot_named_after_an_ngsi_ld_member_is_refused() {
    let (_b, app) = broker().await.expect("broker");
    let schema = |slots: Value, classes: Value| {
        json!({"id": "https://w3id.org/city", "name": "city", "slots": slots, "classes": classes})
            .to_string()
    };
    for member in ["value", "object", "unitCode", "json"] {
        for doc in [
            schema(
                json!({member: {}, "pm": {}}),
                json!({"R": {"slots": ["pm"]}}),
            ),
            schema(json!({"pm": {}}), json!({"R": {"slots": ["pm", member]}})),
            schema(
                json!({"pm": {}}),
                json!({"R": {"slot_usage": {member: {}}}}),
            ),
        ] {
            let (s, body) = send(&app, Method::PUT, "/q/schemas/city", Some(&doc)).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{doc}: {body}");
            let detail = json_of(&body)["detail"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert!(
                detail.contains(&format!("\"{member}\"")) && detail.contains("another name"),
                "{doc}: {detail}"
            );
        }
    }
    let (s, _) = send(&app, Method::GET, "/q/schemas/city", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing refused was stored");
    let located = schema(
        json!({"location": {}, "pm": {}}),
        json!({"R": {"slots": ["location", "pm"]}}),
    );
    let (s, body) = send(&app, Method::PUT, "/q/schemas/city", Some(&located)).await;
    assert_eq!(s, StatusCode::CREATED, "location is an Attribute: {body}");
}
