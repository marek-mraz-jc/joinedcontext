//! The live sweep (by hand, outside CI: it reads the real sources): every run-once stream the
//! Portal renders for the seed runs in ekiben's runner against its real source, with a stub
//! gateway, token endpoint and Portal listener on loopback. It writes one line per pipeline to
//! `$CONTRACT_LIVE_OUT` (default `target/portal-contract-live.jsonl`): what the source answered,
//! what was written, what the validation stage refused and what the outcome reported.
//!
//! `cargo test -p ekiben-server --test portal_contract_live -- --ignored --nocapture`
//! (`CONTRACT_LIVE_ONLY=<regex-free substring>` narrows it to matching stream ids).

mod portal_contract_common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;
use portal_contract_common::{cases, deploy, rewrite, runner, ADMIN_TOKEN};
use serde_json::{json, Value};

/// What one pipeline's run did, as the stub saw it.
#[derive(Default, Clone)]
struct Run {
    sources: Vec<String>,
    written: usize,
    upserts: usize,
    rejected: Vec<String>,
    failed: Vec<Value>,
    outcomes: usize,
    tokens: usize,
}

#[derive(Clone)]
struct Stub {
    runs: Arc<Mutex<BTreeMap<String, Run>>>,
    http: reqwest::Client,
}

const MAX_SOURCE_BYTES: usize = 64 << 20;

async fn answer(
    State(stub): State<Stub>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let json_body = || serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    if path == "/token" {
        use base64::Engine as _;
        let client = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .map(|b| String::from_utf8_lossy(&b).replace("%2F", "/"))
            .and_then(|pair| pair.split_once(':').map(|(id, _)| id.to_string()))
            .unwrap_or_default();
        let id = client.replacen('/', ".", 1);
        stub.runs.lock().unwrap().entry(id).or_default().tokens += 1;
        return axum::Json(json!({"access_token": format!("tok:{client}"), "token_type": "bearer", "expires_in": 3600}))
            .into_response();
    }
    if let Some(rest) = path.strip_prefix("/gw/") {
        let id = bearer
            .strip_prefix("tok:")
            .unwrap_or_default()
            .replacen('/', ".", 1);
        if rest.ends_with("/ngsi-ld/v1/entityOperations/upsert") {
            let n = json_body().as_array().map_or(0, Vec::len);
            let mut runs = stub.runs.lock().unwrap();
            let run = runs.entry(id).or_default();
            run.upserts += 1;
            run.written += n;
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
    // `/c/{id}/internal/...` and `/c/{id}/up/<scheme>/<rest>`: `rewrite` was given `/c/{id}`.
    let Some(rest) = path.strip_prefix("/c/") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (id, rest) = rest.split_once('/').unwrap_or_default();
    if rest.starts_with("internal/") {
        let b = json_body();
        let mut runs = stub.runs.lock().unwrap();
        let run = runs.entry(id.to_string()).or_default();
        if rest.ends_with("/rejected") {
            run.rejected.push(format!(
                "{}: {}",
                b["record"]["id"].as_str().unwrap_or_default(),
                b["error"].as_str().unwrap_or_default()
            ));
        } else if rest.ends_with("/outcomes") {
            run.outcomes += 1;
            run.failed
                .extend(b["failed"].as_array().cloned().unwrap_or_default());
        }
        return StatusCode::OK.into_response();
    }
    let Some(up) = rest.strip_prefix("up/") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (scheme, rest) = up.split_once('/').unwrap_or_default();
    let url = match uri.query() {
        Some(q) => format!("{scheme}://{rest}?{q}"),
        None => format!("{scheme}://{rest}"),
    };
    let mut req = stub.http.request(method.clone(), &url).body(body.to_vec());
    for (k, v) in &headers {
        if !matches!(
            k.as_str(),
            "host" | "content-length" | "connection" | "accept-encoding"
        ) {
            req = req.header(k, v);
        }
    }
    let (status, ctype, payload, note) = match req.send().await {
        Ok(mut r) => {
            let status = r.status();
            let ctype = r.headers().get("content-type").cloned();
            let mut payload = Vec::new();
            let mut note = String::new();
            loop {
                match r.chunk().await {
                    Ok(Some(c)) if payload.len() + c.len() <= MAX_SOURCE_BYTES => {
                        payload.extend_from_slice(&c)
                    }
                    Ok(Some(_)) => {
                        note = " (cut at 64 MiB)".into();
                        break;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        note = format!(" (body: {e})");
                        break;
                    }
                }
            }
            (status, ctype, payload, note)
        }
        Err(e) => (StatusCode::BAD_GATEWAY, None, Vec::new(), format!(" ({e})")),
    };
    stub.runs
        .lock()
        .unwrap()
        .entry(id.to_string())
        .or_default()
        .sources
        .push(format!(
            "{method} {url} -> {} {}B{note}",
            status.as_u16(),
            payload.len()
        ));
    let mut resp = (status, payload).into_response();
    if let Some(c) = ctype {
        resp.headers_mut().insert("content-type", c);
    }
    resp
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "reads the real sources; run by hand"]
async fn every_run_once_seed_stream_runs_against_its_real_source() {
    let stub = Stub {
        runs: Arc::default(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .user_agent("joinedcontext-contract-sweep")
            .build()
            .unwrap(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(answer)
        .with_state(stub.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    for (k, v) in [
        ("JC_GATEWAY_URL", format!("{stub_url}/gw")),
        ("JC_PIPELINE_TOKEN_URL", format!("{stub_url}/token")),
        ("JC_TOKEN_URL", format!("{stub_url}/token")),
        (
            "JC_GATEWAY_HOST",
            stub_url.trim_start_matches("http://").to_string(),
        ),
        ("JC_ORG_DOMAIN", "example.org".to_string()),
        ("JC_MQTT_NAMESPACE", "contract".to_string()),
    ] {
        std::env::set_var(format!("EKIBEN_VAR_{k}"), v);
    }
    let (runner, _root) = runner(true).await;
    let only = std::env::var("CONTRACT_LIVE_ONLY").unwrap_or_default();
    let all: Vec<_> = cases()
        .into_iter()
        .filter(|c| c.stream["input"]["generate"]["count"] == json!(1) && c.id.contains(&only))
        .collect();
    eprintln!("{} run-once streams", all.len());
    let client = reqwest::Client::new();
    let lines: Vec<Value> = futures::stream::iter(all)
        .map(|case| {
            let (client, runner, stub, stub_url) = (client.clone(), runner.clone(), stub.clone(), stub_url.clone());
            async move {
                let mut stream = case.stream.clone();
                rewrite(&mut stream, &format!("{stub_url}/c/{}", case.id), "tcp://127.0.0.1:9");
                let started = Instant::now();
                let mut state = match deploy(&client, &runner, &case, &stream).await {
                    Ok(()) => "running".to_string(),
                    Err(why) => format!("refused: {why}"),
                };
                let url = format!("{runner}/streams/{}", case.id);
                while state == "running" {
                    let one: Value = match client.get(&url).bearer_auth(ADMIN_TOKEN).send().await {
                        Ok(r) => r.json().await.unwrap_or(Value::Null),
                        Err(_) => Value::Null,
                    };
                    if one["active"] == json!(false) {
                        state = "finished".into();
                    } else if started.elapsed() > Duration::from_secs(300) {
                        state = "still running after 300 s".into();
                    } else {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
                let _ = client.delete(&url).bearer_auth(ADMIN_TOKEN).send().await;
                let run = stub.runs.lock().unwrap().get(&case.id).cloned().unwrap_or_default();
                let line = json!({
                    "id": case.id, "state": state, "seconds": started.elapsed().as_secs_f64().round(),
                    "sources": run.sources, "written": run.written, "upserts": run.upserts,
                    "rejected": run.rejected.len(), "rejectedFirst": run.rejected.first(),
                    "failed": run.failed.len(), "failedFirst": run.failed.first(),
                    "outcomes": run.outcomes, "tokens": run.tokens,
                });
                eprintln!("{line}");
                line
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let out = std::env::var("CONTRACT_LIVE_OUT")
        .unwrap_or_else(|_| "target/portal-contract-live.jsonl".into());
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&out, text).unwrap();
    eprintln!("wrote {out}");
}
