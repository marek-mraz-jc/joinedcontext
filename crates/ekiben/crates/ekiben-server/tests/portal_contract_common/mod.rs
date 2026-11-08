//! Shared by the Portal contract tests: the streams the Portal renders for the deployment seed
//! (`tests/portal-contract`, written by `scripts/portal-contract/regen.py` from
//! `tools/portal-dump`), an ekiben runner served as `jc` serves it, the Portal's calls to it
//! (`PORTAL-CALLS.md`), and a stub of everything a stream reaches: its sources (the recorded
//! answers of `tests/e2e-parity/fixtures`), the token sidecar, the gateway and the Portal's
//! internal listener.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use ekiben_core::{EnvAccess, OrgId, OrgQuotas, ResourceManager};
use ekiben_server::{
    build_full_registry, build_router, ServerAuthConfig, ServerState, StreamManager,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Where the Portal's internal listener stood when the streams were rendered.
pub const INTERNAL: &str = "http://portal-internal.test";
pub const ADMIN_TOKEN: &str = "portal-contract-admin-token-0123456789";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One stream the Portal hands its runner.
pub struct Case {
    pub project: String,
    pub name: String,
    pub id: String,
    pub stream: Value,
}

fn gunzip(path: PathBuf) -> String {
    let mut text = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&path).expect("fixture"))
        .read_to_string(&mut text)
        .expect("gzip");
    text
}

/// Every rendered stream, each `json_schema` schema put back in place.
pub fn cases() -> Vec<Case> {
    let dir = root().join("tests/portal-contract");
    let schemas: HashMap<String, String> =
        serde_json::from_str(&gunzip(dir.join("schemas.json.gz"))).expect("schemas");
    gunzip(dir.join("streams.jsonl.gz"))
        .lines()
        .map(|line| {
            let mut d: Value = serde_json::from_str(line).expect("stream line");
            let mut stream = d["stream"].take();
            inline(&mut stream, &schemas);
            Case {
                project: d["project"].as_str().unwrap_or_default().to_string(),
                name: d["name"].as_str().unwrap_or_default().to_string(),
                id: d["id"].as_str().unwrap_or_default().to_string(),
                stream,
            }
        })
        .collect()
}

fn inline(node: &mut Value, schemas: &HashMap<String, String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(s)) =
                map.get_mut("json_schema").and_then(|j| j.get_mut("schema"))
            {
                if let Some(key) = s.strip_prefix("sha256:") {
                    *s = schemas
                        .get(key)
                        .cloned()
                        .expect("schema in schemas.json.gz");
                }
            }
            map.values_mut().for_each(|v| inline(v, schemas));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| inline(v, schemas)),
        _ => {}
    }
}

/// Points every outside address of a stream at `stub` (`/up/<scheme>/<rest>`, as the parity
/// stub serves them) and the Portal's internal listener at `stub` itself. `${VAR}` addresses
/// stay: the runner's variables point them at the stub. MQTT brokers go to `mqtt_to`.
pub fn rewrite(node: &mut Value, stub: &str, mqtt_to: &str) {
    match node {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match (k.as_str(), &mut *v) {
                    ("url", Value::String(url)) if url.starts_with(INTERNAL) => {
                        *url = format!("{stub}{}", &url[INTERNAL.len()..]);
                    }
                    ("url", Value::String(url))
                        if url.starts_with("http://") || url.starts_with("https://") =>
                    {
                        let (scheme, rest) = url.split_once("://").unwrap_or_default();
                        *url = format!("{stub}/up/{scheme}/{rest}");
                    }
                    ("urls", Value::Array(urls)) => {
                        for u in urls.iter_mut() {
                            *u = Value::String(mqtt_to.to_string());
                        }
                    }
                    _ => rewrite(v, stub, mqtt_to),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rewrite(v, stub, mqtt_to)),
        _ => {}
    }
}

/// An ekiben runner as `jc` mounts it (allow-listed variables, a token on every call), with the
/// deployment's shared resources registered and the deployment runner's quotas.
pub async fn runner(allow_private_ips: bool) -> (String, tempfile::TempDir) {
    let file_root = tempfile::tempdir().unwrap();
    // The deployment mounts the GTFS-RT schema at /proto; ekiben reads absolute paths under file_root.
    std::fs::create_dir(file_root.path().join("proto")).unwrap();
    std::fs::copy(
        root().join("tests/portal-contract/proto/gtfs-realtime.proto"),
        file_root.path().join("proto/gtfs-realtime.proto"),
    )
    .unwrap();
    let resources = ResourceManager::new();
    resources.set_org_quotas(
        &OrgId::default(),
        OrgQuotas {
            max_concurrent_streams: 1024,
            max_in_flight_messages: 1_000_000,
            max_memory_bytes_per_run: 1 << 30,
            max_cpu_time_ms_per_run: 300_000,
            max_messages_per_sec: 10_000_000,
            max_http_requests_per_sec: 1_000_000,
            allow_private_ips,
            ..Default::default()
        },
    );
    let manager = StreamManager::new(
        build_full_registry(),
        resources,
        Some(file_root.path().to_path_buf()),
        EnvAccess::AllowListed,
    );
    let shared =
        std::fs::read_to_string(root().join("tests/portal-contract/resources.yaml")).unwrap();
    manager
        .register_resources(&OrgId::default(), &shared)
        .expect("the deployment's resources.yaml registers");
    let state = ServerState::new(
        manager,
        ServerAuthConfig {
            admin_bearer_token: Some(ADMIN_TOKEN.to_string()),
            require_auth: true,
            guard_probes: true,
            ..Default::default()
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), file_root)
}

/// The Portal's calls for one stream (`deploy_stream`, `reconciler/streams.rs:1345-1393`): the
/// project's write budget, then PUT, then POST when the runner does not hold it. The admin token
/// is the one `jc` asks for (the Portal sends none today: PORTAL-CALLS.md §1).
pub async fn deploy(
    client: &reqwest::Client,
    runner: &str,
    case: &Case,
    stream: &Value,
) -> Result<(), String> {
    let egress = format!(
        "{runner}/resources/rate_limit/{}__pipeline_egress",
        case.project.replace('-', "_")
    );
    let r = client
        .post(egress)
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({"local": {"count": 500, "interval": "1s"}}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!(
            "egress: {} {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ));
    }
    let url = format!("{runner}/streams/{}", case.id);
    let mut r = client
        .put(&url)
        .bearer_auth(ADMIN_TOKEN)
        .json(stream)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status() == StatusCode::NOT_FOUND {
        r = client
            .post(&url)
            .bearer_auth(ADMIN_TOKEN)
            .json(stream)
            .send()
            .await
            .map_err(|e| e.to_string())?;
    }
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "runner answered {}: {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ))
    }
}

/// What the stub saw.
#[derive(Default)]
pub struct Seen {
    /// (client id from Basic, grant_type) per token request.
    pub tokens: Vec<(String, String)>,
    /// (bearer, slug, query, body) per gateway upsert.
    pub upserts: Vec<(String, String, String, Value)>,
    /// (bearer, path, body) per post to the internal listener.
    pub internal: Vec<(String, String, Value)>,
    /// Source requests without a recorded answer.
    pub missing: Vec<String>,
}

pub type Shared = Arc<Mutex<Seen>>;

/// The stub on loopback: `/token` (the runner's token sidecar), `/gw/...` (the gateway),
/// `/internal/...` (the Portal), `/up/...` (sources, recorded answers).
pub async fn stub() -> (String, Shared) {
    let seen: Shared = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .fallback(answer)
        .with_state(seen.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
}

/// `scripts/e2e/stub.py` `fixture_key`.
fn fixture_key(method: &str, url: &str, headers: &HeaderMap, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(format!("{method}\n{url}\n").as_bytes());
    for name in ["range", "accept"] {
        let mut value = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if name == "accept" && value == "*/*" {
            value.clear();
        }
        h.update(format!("{name}:{value}\n").as_bytes());
    }
    h.update(body);
    hex::encode(h.finalize())
}

async fn answer(
    State(seen): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let json_body = || serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    if path == "/token" {
        use base64::Engine as _;
        let client = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .and_then(|pair| pair.split_once(':').map(|(id, _)| percent_decode(id)))
            .unwrap_or_default();
        let form = String::from_utf8_lossy(&body).into_owned();
        let grant = form
            .split('&')
            .find_map(|kv| kv.strip_prefix("grant_type="))
            .unwrap_or_default()
            .to_string();
        seen.lock()
            .unwrap()
            .tokens
            .push((client.clone(), grant.clone()));
        // The sidecar names a pipeline `{project}/{pipeline}`; it wants no secret.
        if grant != "client_credentials" || client.matches('/').count() != 1 {
            return (StatusCode::UNAUTHORIZED, "{\"error\":\"invalid_client\"}").into_response();
        }
        let token = format!("tok:{client}");
        return axum::Json(
            json!({"access_token": token, "token_type": "bearer", "expires_in": 3600}),
        )
        .into_response();
    }
    if let Some(rest) = path.strip_prefix("/gw/") {
        let token = bearer(&headers);
        if !token.starts_with("tok:") {
            return (StatusCode::UNAUTHORIZED, "{\"title\":\"no token\"}").into_response();
        }
        if rest.ends_with("/ngsi-ld/v1/entityOperations/upsert") {
            let slug = rest.split('/').nth(2).unwrap_or_default().to_string();
            let query = uri.query().unwrap_or_default().to_string();
            seen.lock()
                .unwrap()
                .upserts
                .push((token, slug, query, json_body()));
            return StatusCode::NO_CONTENT.into_response();
        }
        return (
            [
                ("NGSILD-Results-Count", "0"),
                ("Content-Type", "application/json"),
            ],
            "[]",
        )
            .into_response();
    }
    if path.starts_with("/internal/") {
        seen.lock()
            .unwrap()
            .internal
            .push((bearer(&headers), path, json_body()));
        return StatusCode::OK.into_response();
    }
    if let Some(rest) = path.strip_prefix("/up/") {
        let (scheme, rest) = rest.split_once('/').unwrap_or_default();
        let url = match uri.query() {
            Some(q) => format!("{scheme}://{rest}?{q}"),
            None => format!("{scheme}://{rest}"),
        };
        let key = fixture_key(method.as_str(), &url, &headers, &body);
        let file = root()
            .join("tests/e2e-parity/fixtures")
            .join(&key[..2])
            .join(format!("{key}.json"));
        let Ok(text) = std::fs::read_to_string(&file) else {
            seen.lock().unwrap().missing.push(format!("{method} {url}"));
            return (StatusCode::from_u16(599).unwrap(), "no recorded answer").into_response();
        };
        let f: Value = serde_json::from_str(&text).unwrap();
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::STANDARD
            .decode(f["body"].as_str().unwrap_or_default())
            .unwrap_or_default();
        let status = StatusCode::from_u16(f["status"].as_u64().unwrap_or(200) as u16)
            .unwrap_or(StatusCode::OK);
        let ctype = f["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string();
        return (status, [("Content-Type", ctype)], payload).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
