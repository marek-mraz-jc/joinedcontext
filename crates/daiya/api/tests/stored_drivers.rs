#![forbid(unsafe_code)]
//! A datasource created through the Grafana compat API, with no driver registered by hand, is
//! queried, health-checked and asked for editor resources by the running router: the driver
//! comes from the stored record (R3).

use axum::Router;
use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{InstallationId, InstanceId, OrgId, PolicyAuthorizer, Subject};
use daiya_query::{MapSecretResolver, OutboundPolicy};
use daiya_store_memory::MemoryStore;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(5);

async fn mock(routes: Vec<(&'static str, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
            log.lock().push(head);
            let (status, body) = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map_or((404, "{}".to_owned()), |(_, body)| (200, body.to_string()));
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), seen)
}

fn app(policy: OutboundPolicy) -> (DaiyaState, Router) {
    let store = Arc::new(MemoryStore::new());
    let authz = Arc::new(PolicyAuthorizer::allow_org_members(ORG, "ops"));
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        store,
        authz,
    )
    .with_tokens(vec![TokenBinding::new(
        "ops-token",
        Subject::authenticated("ops", vec![ORG], vec![]),
    )]);
    let mut secrets = MapSecretResolver::new();
    secrets.insert(ORG, "prom-cred", "prom-bearer-xyz");
    state.secret_resolver = Arc::new(secrets);
    state.outbound_policy = policy;
    let router = api_router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    (state, router)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer ops-token");
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())),
    )
}

fn loopback() -> OutboundPolicy {
    OutboundPolicy {
        allow_http: true,
        allowed_networks: vec!["127.0.0.0/8".parse().unwrap()],
        ..OutboundPolicy::default()
    }
}

#[tokio::test]
async fn a_datasource_created_through_the_api_is_queried_by_the_router() {
    let (url, seen) = mock(vec![
        (
            "/api/v1/query_range",
            json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                { "metric": { "job": "api" }, "values": [[1700000000, "3"], [1700000060, "4"]] }
            ]}}),
        ),
        (
            "/api/v1/query",
            json!({ "status": "success", "data": { "resultType": "vector", "result": [] }}),
        ),
        (
            "/api/v1/labels",
            json!({ "status": "success", "data": ["job", "instance"] }),
        ),
    ])
    .await;
    let (state, app) = app(loopback());
    let (status, body) = call(
        &app,
        "POST",
        "/api/datasources",
        Some(json!({
            "uid": "prom",
            "name": "Prometheus",
            "type": "prometheus",
            "url": url,
            "jsonData": { "timeInterval": "15s" },
            "secretRefs": { "token": "prom-cred" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        state.get_datasource_driver(ORG, "prom").is_none(),
        "no hand-registered driver"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/api/ds/query",
        Some(json!({
            "from": "1700000000000",
            "to": "1700000600000",
            "queries": [{ "refId": "A", "datasource": { "type": "prometheus", "uid": "prom" }, "expr": "up" }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"]["A"]["status"], 200);
    assert_eq!(
        body["results"]["A"]["frames"][0]["fields"][1]["values"],
        json!([3.0, 4.0])
    );
    let head = seen.lock()[0].to_ascii_lowercase();
    assert!(
        head.contains("authorization: bearer prom-bearer-xyz"),
        "{head}"
    );
    assert!(
        !body.to_string().contains("prom-bearer-xyz"),
        "the credential never comes back"
    );

    // Health and editor resources go through the same built driver.
    let (status, health) = call(&app, "GET", "/api/datasources/uid/prom/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "ok", "{health}");
    let (status, labels) = call(
        &app,
        "GET",
        "/api/datasources/uid/prom/resources/labels",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{labels}");
    let (status, editor) = call(
        &app,
        "GET",
        "/odata/v4/Datasources('prom')/EditorSchema",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{editor}");
    assert_eq!(editor["kind"], "prometheus");

    // Changing the record rebuilds the driver: the new URL is used at once.
    let (url2, seen2) = mock(vec![(
        "/api/v1/query_range",
        json!({ "status": "success", "data": { "resultType": "matrix", "result": [] }}),
    )])
    .await;
    let (status, body) = call(
        &app,
        "PUT",
        "/api/datasources/uid/prom",
        Some(json!({ "name": "Prometheus", "type": "prometheus", "url": url2 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = call(
        &app,
        "POST",
        "/api/ds/query",
        Some(json!({
            "from": "1700000000000",
            "to": "1700000900000",
            "queries": [{ "refId": "B", "datasource": { "uid": "prom" }, "expr": "up" }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen2.lock().len(), 1);
}

#[tokio::test]
async fn a_stored_datasource_whose_host_is_refused_reports_unhealthy_and_queries_nothing() {
    let (url, seen) = mock(vec![("/", json!({}))]).await;
    // Default operator policy: https only, no private network open.
    let (_state, app) = app(OutboundPolicy::default());
    let (status, _) = call(
        &app,
        "POST",
        "/api/datasources",
        Some(json!({ "uid": "local", "name": "Local", "type": "prometheus", "url": url })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, health) = call(&app, "GET", "/api/datasources/uid/local/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "error", "{health}");
    assert!(
        health["message"].as_str().unwrap().contains("scheme"),
        "{health}"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/api/ds/query",
        Some(json!({ "queries": [{ "refId": "A", "datasource": { "uid": "local" }, "expr": "up" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "cr.ssrf_blocked");
    assert!(seen.lock().is_empty(), "nothing left the host");

    // A kind without a driver is unhealthy with its reason, and its editor is a plain form.
    let (status, _) = call(
        &app,
        "POST",
        "/api/datasources",
        Some(json!({ "uid": "influx", "name": "Influx", "type": "influxdb", "url": "https://influx.example" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, health) = call(&app, "GET", "/api/datasources/uid/influx/health", None).await;
    assert_eq!(health["status"], "error");
    assert!(
        health["message"].as_str().unwrap().contains("influxdb"),
        "{health}"
    );
    let (status, editor) = call(
        &app,
        "GET",
        "/odata/v4/Datasources('influx')/EditorSchema",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(editor["kind"], "influxdb");
    assert_eq!(editor["fieldHints"], json!([]));
    let (status, _) = call(
        &app,
        "GET",
        "/api/datasources/uid/influx/resources/labels",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let (status, _) = call(&app, "GET", "/api/datasources/uid/nope/health", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "GET",
        "/odata/v4/Datasources('nope')/EditorSchema",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
