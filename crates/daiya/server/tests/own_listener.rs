#![forbid(unsafe_code)]
//! The host's own listener (`DaiyaState::own_addresses`, `jc`'s listener inside `jc`) is reached
//! only by the datasources provisioning files wrote, named by its exact address. A datasource
//! created through the API at the same address, a provisioned one changed since, and one naming
//! another port keep the default outbound policy: no loopback, no http.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use daiya_api::TokenBinding;
use daiya_model::{InstallationId, InstanceId, OrgId, PolicyAuthorizer, Subject};
use daiya_server::config::provision;
use daiya_server::{DaiyaState, router};
use daiya_store_memory::MemoryStore;
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(7);

/// A Prometheus stand-in on loopback that counts the connections it served.
async fn listener() -> (SocketAddr, Arc<AtomicUsize>) {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let served = Arc::new(AtomicUsize::new(0));
    let count = served.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = socket.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let mut buf = vec![0u8; 16384];
            let _ = sock.read(&mut buf).await;
            let body =
                json!({ "status": "success", "data": { "resultType": "vector", "result": [] } })
                    .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (addr, served)
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A provisioning root whose one datasource file holds `uid -> url` for each entry.
fn provisioning(datasources: &[(&str, String)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!("daiya-own-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("datasources")).unwrap();
    let mut yaml = String::from("apiVersion: 1\ndatasources:\n");
    for (uid, url) in datasources {
        yaml.push_str(&format!(
            "  - name: {uid}\n    uid: {uid}\n    type: prometheus\n    url: {url}\n    orgId: 1\n"
        ));
    }
    std::fs::write(dir.join("datasources/ds.yaml"), yaml).unwrap();
    Scratch(dir.canonicalize().unwrap())
}

async fn app(own: SocketAddr, root: &Scratch) -> (DaiyaState, Router) {
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Arc::new(MemoryStore::new()),
        Arc::new(PolicyAuthorizer::allow_org_members(ORG, "ops")),
    )
    .with_tokens(vec![TokenBinding::new(
        "ops-token",
        Subject::authenticated("ops", vec![ORG], vec![]),
    )]);
    state.own_addresses = vec![own];
    provision(&state, &root.0, &BTreeMap::new()).await.unwrap();
    let app = router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    (state, app)
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

/// Asks the health of `uid` and says whether the listener served a connection for it.
async fn reaches(app: &Router, uid: &str, served: &AtomicUsize) -> (bool, Value) {
    let before = served.load(Ordering::SeqCst);
    let (_, health) = call(
        app,
        "GET",
        &format!("/api/datasources/uid/{uid}/health"),
        None,
    )
    .await;
    (served.load(Ordering::SeqCst) > before, health)
}

#[tokio::test]
async fn a_provisioned_datasource_reaches_the_own_listener_by_its_exact_address() {
    let (own, served) = listener().await;
    let root = provisioning(&[("own", format!("http://{own}"))]);
    let (_, app) = app(own, &root).await;
    let (reached, health) = reaches(&app, "own", &served).await;
    assert!(
        reached,
        "the provisioned datasource is let through: {health}"
    );
    assert_eq!(health["status"], "ok", "{health}");
}

#[tokio::test]
async fn an_api_datasource_at_the_own_address_is_refused() {
    let (own, served) = listener().await;
    let root = provisioning(&[]);
    let (_, app) = app(own, &root).await;
    let (status, body) = call(
        &app,
        "POST",
        "/api/datasources",
        Some(json!({ "uid": "api", "name": "api", "type": "prometheus", "url": format!("http://{own}") })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (reached, health) = reaches(&app, "api", &served).await;
    assert!(!reached, "an API datasource never reaches the own listener");
    assert_ne!(health["status"], "ok", "{health}");
}

#[tokio::test]
async fn a_provisioned_datasource_changed_since_is_refused() {
    let (own, served) = listener().await;
    let root = provisioning(&[("own", format!("http://{own}"))]);
    let (state, app) = app(own, &root).await;
    let mut record = state
        .store
        .get_datasource(ORG, "own")
        .await
        .unwrap()
        .unwrap();
    record.json_data = json!({ "httpHeaderName1": "X-Changed" });
    record.version += 1;
    state.store.upsert_datasource(record).await.unwrap();
    let (reached, health) = reaches(&app, "own", &served).await;
    assert!(!reached, "a changed record loses the own address: {health}");
}

#[tokio::test]
async fn a_provisioned_datasource_on_another_port_or_by_name_is_refused() {
    let (own, _) = listener().await;
    let (other, served) = listener().await;
    let root = provisioning(&[
        ("port", format!("http://{other}")),
        ("name", format!("http://localhost:{}", own.port())),
    ]);
    let (_, app) = app(own, &root).await;
    let (reached, health) = reaches(&app, "port", &served).await;
    assert!(
        !reached,
        "another port of the same IP stays refused: {health}"
    );
    let (_, health) = call(&app, "GET", "/api/datasources/uid/name/health", None).await;
    assert_ne!(
        health["status"], "ok",
        "only the IP literal matches: {health}"
    );
}
