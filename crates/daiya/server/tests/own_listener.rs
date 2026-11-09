#![forbid(unsafe_code)]
//! The host's own listener (`jc`'s listener inside `jc`) holds the gateway beside the host's
//! other mounts. daiya reaches only the gateway's Endpoints there
//! (`DaiyaState::own_reach`, `http://<own>/api/endpoint/<slug>/...`), judged on each request's
//! URL, for NGSI-LD and OData datasources only, whoever made the record: files or the API.
//! Every other path, kind, port or host spelling keeps the default outbound policy: no
//! loopback, no http.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use daiya_api::TokenBinding;
use daiya_model::{InstallationId, InstanceId, OrgId, PolicyAuthorizer, Subject};
use daiya_server::config::provision;
use daiya_server::{DaiyaState, OwnReach, router};
use daiya_store_memory::MemoryStore;
use http_body_util::BodyExt;
use jc_router::Mount;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(7);
/// The server administrator, as `jc` sets it up for the Portal: no bearer token on its reads,
/// so NGSI-LD and OData datasources read anonymously.
const ADMIN: &str = "grafana_admin";
const PASSWORD: &str = "a-long-enough-server-admin-password";

/// Request heads the stand-in listener saw.
type Heads = Arc<Mutex<Vec<String>>>;

/// A stand-in for the own listener on loopback that counts the connections it served.
async fn listener() -> (SocketAddr, Arc<AtomicUsize>) {
    let (addr, served, _) = logging_listener().await;
    (addr, served)
}

/// [`listener`], also keeping each request head.
async fn logging_listener() -> (SocketAddr, Arc<AtomicUsize>, Heads) {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let served = Arc::new(AtomicUsize::new(0));
    let count = served.clone();
    let heads: Heads = Arc::default();
    let log = heads.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = socket.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            log.lock()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
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
    (addr, served, heads)
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A provisioning root whose one datasource file holds `uid -> url` for each entry.
fn provisioning(datasources: &[(&str, String)]) -> Scratch {
    let typed: Vec<(&str, &str, String)> = datasources
        .iter()
        .map(|(uid, url)| (*uid, "prometheus", url.clone()))
        .collect();
    provisioning_typed(&typed)
}

/// A provisioning root whose one datasource file holds `uid -> (type, url)` for each entry.
fn provisioning_typed(datasources: &[(&str, &str, String)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!("daiya-own-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("datasources")).unwrap();
    let mut yaml = String::from("apiVersion: 1\ndatasources:\n");
    for (uid, kind, url) in datasources {
        yaml.push_str(&format!(
            "  - name: {uid}\n    uid: {uid}\n    type: {kind}\n    url: '{url}'\n    orgId: 1\n"
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
    )])
    .with_server_admin(ADMIN, PASSWORD)
    .unwrap()
    .with_person_tokens();
    state.own_reach = vec![OwnReach::endpoints(&format!("http://{own}/api/endpoint/")).unwrap()];
    provision(&state, &root.0, &BTreeMap::new()).await.unwrap();
    let app = router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    (state, app)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header(
        "authorization",
        format!("Basic {}", B64.encode(format!("{ADMIN}:{PASSWORD}"))),
    );
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

const SLUG: &str = "kallio-air-quality-public-abcdefgh";

/// Makes a datasource through the API, as the Portal or a person does.
async fn made(app: &Router, uid: &str, kind: &str, url: &str, json_data: Value) {
    let (status, body) = call(
        app,
        "POST",
        "/api/datasources",
        Some(json!({ "uid": uid, "name": uid, "type": kind, "url": url, "jsonData": json_data })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn an_endpoint_datasource_reaches_the_own_gateway_whoever_made_it() {
    let (own, served) = listener().await;
    let url = format!("http://{own}/api/endpoint/{SLUG}");
    let root = provisioning_typed(&[("file-ngsild", "ngsild", url.clone())]);
    let (_, app) = app(own, &root).await;
    made(
        &app,
        "api-ngsild",
        "ngsild",
        &url,
        json!({ "pageSize": 1000 }),
    )
    .await;
    made(
        &app,
        "api-odata",
        "odata",
        &format!("{url}/odata"),
        json!({}),
    )
    .await;
    for uid in ["file-ngsild", "api-ngsild", "api-odata"] {
        let (reached, health) = reaches(&app, uid, &served).await;
        assert!(reached, "{uid} reads the gateway's Endpoint: {health}");
    }
    let (reached, health) = reaches(&app, "file-ngsild", &served).await;
    assert!(reached && health["status"] == "ok", "{health}");
}

#[tokio::test]
async fn a_datasource_of_another_kind_never_reaches_the_own_gateway() {
    let (own, served) = listener().await;
    let url = format!("http://{own}/api/endpoint/{SLUG}");
    let root = provisioning_typed(&[("file-prom", "prometheus", url.clone())]);
    let (_, app) = app(own, &root).await;
    made(&app, "api-prom", "prometheus", &url, json!({})).await;
    for uid in ["file-prom", "api-prom"] {
        let (reached, health) = reaches(&app, uid, &served).await;
        assert!(!reached, "{uid} stays refused: {health}");
        assert_ne!(health["status"], "ok", "{health}");
    }
}

/// A datasource a person makes by hand gets the gateway's Endpoints and nothing else on the own
/// listener: the other mounts stay refused for every kind.
#[tokio::test]
async fn a_person_made_datasource_reaches_no_other_mount_of_the_own_listener() {
    let (own, served) = listener().await;
    let root = provisioning(&[]);
    let (_, app) = app(own, &root).await;
    let mut leaks = Vec::new();
    for (i, path) in [
        "/id/realms/jc/protocol",
        "/portal/api/v1/me",
        "/git/api/v1/user",
        "/api/v1/d/x",
        "/",
    ]
    .iter()
    .enumerate()
    {
        for kind in ["prometheus", "ngsild", "odata"] {
            let uid = format!("{kind}-{i}");
            let url = format!("http://{own}{path}");
            made(&app, &uid, kind, &url, json!({})).await;
            if reaches(&app, &uid, &served).await.0 {
                leaks.push(format!("{uid} at {url}"));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "reached the own listener:\n{}",
        leaks.join("\n")
    );
}

#[tokio::test]
async fn an_endpoint_datasource_on_another_port_or_by_name_is_refused() {
    let (own, _) = listener().await;
    let (other, served) = listener().await;
    let root = provisioning_typed(&[
        (
            "port",
            "ngsild",
            format!("http://{other}/api/endpoint/{SLUG}"),
        ),
        (
            "name",
            "ngsild",
            format!("http://localhost:{}/api/endpoint/{SLUG}", own.port()),
        ),
        (
            "short",
            "ngsild",
            format!("http://127.1:{}/api/endpoint/{SLUG}", own.port()),
        ),
    ]);
    let (_, app) = app(own, &root).await;
    let (reached, health) = reaches(&app, "port", &served).await;
    assert!(
        !reached,
        "another port of the same IP stays refused: {health}"
    );
    for uid in ["name", "short"] {
        let (_, health) = call(
            &app,
            "GET",
            &format!("/api/datasources/uid/{uid}/health"),
            None,
        )
        .await;
        assert_ne!(
            health["status"], "ok",
            "only the IP literal as written matches: {health}"
        );
    }
}

/// jc-daiya-public's token rule holds through the own gateway: the Portal's read of a public
/// Endpoint carries no token; a person's token, exchanged for the one Endpoint, goes to that
/// Endpoint only; an OData `audience` naming something other than that Endpoint sends none.
#[tokio::test]
async fn the_own_gateway_gets_a_token_only_for_its_endpoint() {
    let (own, _, heads) = logging_listener().await;
    let url = format!("http://{own}/api/endpoint/{SLUG}");
    let root = provisioning(&[]);
    let (_, app) = app(own, &root).await;
    made(&app, "public", "ngsild", &url, json!({})).await;
    made(
        &app,
        "odata-other",
        "odata",
        &format!("{url}/odata"),
        json!({ "audience": "portal" }),
    )
    .await;

    let (_, health) = call(&app, "GET", "/api/datasources/uid/public/health", None).await;
    assert_eq!(health["status"], "ok", "{health}");
    let head = heads.lock().last().cloned().unwrap_or_default();
    assert!(
        !head.to_ascii_lowercase().contains("authorization:"),
        "{head}"
    );

    let token = person_jwt(&format!("endpoint:{SLUG}"));
    let before = heads.lock().len();
    let (_, health) = call_as_person(&app, "/api/datasources/uid/public/health", &token).await;
    assert_eq!(health["status"], "ok", "{health}");
    let head = heads.lock().get(before).cloned().unwrap_or_default();
    assert!(head.contains(&format!("Bearer {token}")), "{head}");

    let before = heads.lock().len();
    let (_, health) = call_as_person(&app, "/api/datasources/uid/odata-other/health", &token).await;
    let sent: Vec<String> = heads.lock()[before..].to_vec();
    assert!(
        sent.iter().all(|h| !h.contains(&token)),
        "the person's token never leaves for another audience: {health} {sent:?}"
    );
}

/// An unsigned JWT for one person with `scope`; daiya reads it, the gateway would verify it.
fn person_jwt(scope: &str) -> String {
    let part = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.signature",
        part(json!({ "alg": "RS256", "typ": "JWT" })),
        part(json!({ "sub": "pia", "scope": format!("openid {scope}") }))
    )
}

/// A read as the Portal makes it for a signed-in person: its server-administrator credential
/// and the person's exchanged token beside it.
async fn call_as_person(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header(
            "authorization",
            format!("Basic {}", B64.encode(format!("{ADMIN}:{PASSWORD}"))),
        )
        .header(daiya_api::PERSON_TOKEN_HEADER, token)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The own listener holds the host's other mounts beside the gateway (`/id`, `/portal`, `/git`,
/// the assistant at `/api/v1/d`): a provisioned datasource of any kind naming one of them, or
/// walking out of `/api/endpoint/<slug>` by dot segments or encoded separators, is refused.
/// (A bare `/api/endpoint` base is no case: the NGSI-LD driver appends `/ngsi-ld/v1/...`, an
/// Endpoint-shaped path the gateway answers as an unknown Endpoint.)
#[tokio::test]
async fn a_provisioned_datasource_reaches_no_other_mount_of_the_own_listener() {
    let (own, served) = listener().await;
    let paths = [
        "/id/realms/jc",
        "/portal/api/v1/projects",
        "/git/api/v1/repos",
        "/api/v1/d/x",
        "/api",
        "/api/endpoint/x/../../../id",
        "/api/endpoint/x/%2e%2e/%2e%2e/id",
        "/api/endpoint/x%2f..%2f..%2fid",
        "/api/endpoint/%2e%2e/v1/d",
        "/api/endpoint/x%5c..%5cid",
        "/api/endpointx/y",
    ];
    let mut datasources = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        for kind in ["prometheus", "ngsild", "odata"] {
            datasources.push((format!("{kind}-{i}"), kind, format!("http://{own}{path}")));
        }
    }
    let named: Vec<(&str, &str, String)> = datasources
        .iter()
        .map(|(uid, kind, url)| (uid.as_str(), *kind, url.clone()))
        .collect();
    let root = provisioning_typed(&named);
    let (_, app) = app(own, &root).await;
    let mut leaks = Vec::new();
    for (uid, _, url) in &datasources {
        let (reached, _) = reaches(&app, uid, &served).await;
        if reached {
            leaks.push(format!("{uid} at {url}"));
        }
    }
    assert!(
        leaks.is_empty(),
        "reached the own listener:\n{}",
        leaks.join("\n")
    );
}
