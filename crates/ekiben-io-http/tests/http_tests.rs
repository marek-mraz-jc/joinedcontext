use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, ComponentConfig, OrgId, OrgQuotas, Part, ProcContext, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use tokio::net::TcpListener;
use tokio::sync::watch;

#[derive(Clone, Default)]
struct MockAppState {
    calls: Arc<AtomicUsize>,
}

async fn start_mock_server() -> (String, Arc<AtomicUsize>) {
    let state = MockAppState::default();
    let calls = state.calls.clone();

    let app = Router::new()
        .route(
            "/enrich",
            post(|State(st): State<MockAppState>, body: String| async move {
                let c = st.calls.fetch_add(1, Ordering::SeqCst);
                let mut headers = HeaderMap::new();
                headers.insert("X-Custom-Result", "enriched-ok".parse().unwrap());
                headers.insert("Ngsild-Results-Count", "1".parse().unwrap());
                (
                    StatusCode::OK,
                    headers,
                    format!(r#"{{"call":{c},"echo":{body}}}"#),
                )
            }),
        )
        .route(
            "/flaky",
            get(|State(st): State<MockAppState>| async move {
                let c = st.calls.fetch_add(1, Ordering::SeqCst);
                if c < 1 {
                    (StatusCode::TOO_MANY_REQUESTS, "rate limited".to_string())
                } else {
                    (StatusCode::OK, r#"{"recovered":true}"#.to_string())
                }
            }),
        )
        .route(
            "/content-type",
            get(|headers: HeaderMap| async move {
                let named = headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none")
                    .to_string();
                (StatusCode::OK, named)
            }),
        )
        .route(
            "/always-404",
            get(|| async { (StatusCode::NOT_FOUND, "not found".to_string()) }),
        )
        .route(
            "/oauth/token",
            post(|| async {
                Json(serde_json::json!({
                    "access_token": "mock-oauth-token-xyz",
                    "token_type": "Bearer",
                    "expires_in": 3600
                }))
            }),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), calls)
}

fn setup_contexts(allow_private_ips: bool) -> (Arc<Registry>, ProcContext, StreamContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_http::register_all(&reg);

    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-http");
    resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips,
            ..Default::default()
        },
    );
    let org = resources.get_or_create_org(&org_id);
    let (_tx, rx) = watch::channel(false);
    let metrics = StreamMetrics::new("http-test", "org-http");

    let pctx = ProcContext {
        stream_id: "http-test".to_string(),
        org: org.clone(),
        resources: resources.clone(),
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: metrics.clone(),
        label: "http_proc".to_string(),
        component_type: "http".to_string(),
        path: "pipeline.processors.0".to_string(),
        file_root: None,
        shutdown_rx: rx.clone(),
    };
    let sctx = StreamContext {
        stream_id: "http-test".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics,
        label: "http_io".to_string(),
        path: "output".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, pctx, sctx)
}

#[tokio::test]
async fn ssrf_and_scheme_security_guards() {
    let (reg, pctx, _) = setup_contexts(false); // allow_private_ips = false (default)

    // 1. Non-http/https scheme (`file://`, `ftp://`, `gopher://`) is refused at config creation
    let bad_scheme: serde_json::Value = serde_norway::from_str(
        r#"
http:
  url: "file:///etc/passwd"
"#,
    )
    .unwrap();
    let bad_conf = ComponentConfig::from_json_value(&bad_scheme, "noop", None).unwrap();
    assert!(
        reg.build_processor(&bad_conf, &pctx, "0").is_err(),
        "file:// scheme must be refused"
    );

    // 2. Private/loopback IP (127.0.0.1, 169.254.169.254, 10.0.0.1) is refused by SSRF guard when allow_private_ips = false
    for private_url in [
        "http://127.0.0.1:8080/secret",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/internal",
        "http://localhost:4195/streams",
        // a name the literal check does not match but that resolves to loopback on every host
        // (/etc/hosts): only the resolver can refuse it
        "http://localhost.:8080/secret",
        // IPv4-mapped IPv6 literal of loopback
        "http://[::ffff:127.0.0.1]:8080/secret",
    ] {
        let cfg_val = serde_json::json!({
            "http": {
                "url": private_url,
                "verb": "GET",
                "retries": 0
            }
        });
        let conf = ComponentConfig::from_json_value(&cfg_val, "noop", None).unwrap();
        let proc = reg.build_processor(&conf, &pctx, "0").unwrap();
        // Bento semantics: a failed request flags the part, the batch itself goes on.
        let out = proc
            .process_batch(&pctx, vec![Part::from_str("{}")])
            .await
            .expect("an http failure flags the part, not the batch");
        let msg = out[0][0]
            .error_get()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            msg.contains("SSRF") || msg.contains("access denied"),
            "{private_url}: part must be refused, got {msg:?}"
        );
    }

    // 3. The redirect policy uses this check per hop; mapped IPv6 literals count as private.
    assert!(ekiben_io_http::client::is_private_or_loopback_host(
        "[::ffff:169.254.169.254]"
    ));
    assert!(!ekiben_io_http::client::is_private_or_loopback_host(
        "[::ffff:8.8.8.8]"
    ));
}

#[tokio::test]
async fn http_processor_branch_oauth2_extract_headers_and_retries() {
    let (base_url, calls) = start_mock_server().await;
    let (reg, pctx, sctx) = setup_contexts(true);

    // 1. Branch + HTTP processor with OAuth2 client_credentials and extract_headers
    let branch_val = serde_json::json!({
        "branch": {
            "request_map": "root = {\"id\": this.id}",
            "processors": [
                {
                    "http": {
                        "url": format!("{base_url}/enrich"),
                        "verb": "POST",
                        "extract_headers": {
                            "include_patterns": ["(?i)^ngsild-results-count$", "^x-custom-result$"]
                        },
                        "oauth2": {
                            "enabled": true,
                            "client_key": "test-client",
                            "client_secret": "test-secret",
                            "token_url": format!("{base_url}/oauth/token")
                        }
                    }
                }
            ],
            "result_map": "root.enriched = this.echo.id\nmeta count = meta(\"ngsild-results-count\")\nmeta custom = meta(\"x-custom-result\")"
        }
    });
    let conf = ComponentConfig::from_json_value(&branch_val, "noop", None).unwrap();
    let proc = reg.build_processor(&conf, &pctx, "0").unwrap();

    let res = execute_all(
        &pctx,
        &[proc],
        vec![vec![Part::from_str(r#"{"id":42,"keep":"yes"}"#)]],
    )
    .await
    .unwrap();
    let part = &res[0][0];
    assert!(!part.is_errored());
    assert_eq!(part.meta_get_str("count"), "1");
    assert_eq!(part.meta_get_str("custom"), "enriched-ok");
    let doc = part.as_structured_ref().unwrap().to_json_value();
    assert_eq!(doc["keep"], "yes");
    assert_eq!(doc["enriched"], 42);

    // 2. Retry on 429 (`backoff_on: [429]`)
    calls.store(0, Ordering::SeqCst);
    let flaky_val = serde_json::json!({
        "http": {
            "url": format!("{base_url}/flaky"),
            "verb": "GET",
            "retry_period": "10ms",
            "retries": 2
        }
    });
    let flaky_conf = ComponentConfig::from_json_value(&flaky_val, "noop", None).unwrap();
    let flaky_proc = reg.build_processor(&flaky_conf, &pctx, "1").unwrap();
    let flaky_res = execute_all(&pctx, &[flaky_proc], vec![vec![Part::from_str("{}")]])
        .await
        .unwrap();
    assert!(!flaky_res[0][0].is_errored());
    assert_eq!(
        flaky_res[0][0].as_structured_ref().unwrap().to_json_value()["recovered"],
        true
    );

    // 3. 404 in `drop_on` does not retry and sets `http_status_code = 404` on the errored part
    let err_val = serde_json::json!({
        "http": {
            "url": format!("{base_url}/always-404"),
            "verb": "GET",
            "drop_on": [404],
            "retries": 3
        }
    });
    let err_conf = ComponentConfig::from_json_value(&err_val, "noop", None).unwrap();
    let err_proc = reg.build_processor(&err_conf, &pctx, "2").unwrap();
    let err_res = execute_all(
        &pctx,
        &[err_proc],
        vec![vec![Part::from_str(r#"{"orig":1}"#)]],
    )
    .await
    .unwrap();
    assert!(err_res[0][0].is_errored());
    assert_eq!(err_res[0][0].meta_get_str("http_status_code"), "404");

    // 4. `http_client` output
    let out_val = serde_json::json!({
        "http_client": {
            "url": format!("{base_url}/enrich"),
            "verb": "POST"
        }
    });
    let out_conf = ComponentConfig::from_json_value(&out_val, "stdout", None).unwrap();
    let out = reg.build_output(&out_conf, &sctx).unwrap();
    out.write_batch(&sctx, vec![Part::from_str(r#"{"sent":true}"#)])
        .await
        .unwrap();
}

/// Bento sends `Content-Type: application/octet-stream` with every one-message request whose
/// headers name no type, an empty GET included (T-6624: 14 seed pipelines differed by it alone).
#[tokio::test]
async fn an_empty_get_names_the_octet_stream_type_as_bento_does() {
    let (base_url, _) = start_mock_server().await;
    let (reg, pctx, _) = setup_contexts(true);
    let named = |headers: serde_json::Value| {
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "http": {
                "url": format!("{base_url}/content-type"),
                "verb": "GET",
                "headers": headers
            }}),
            "noop",
            None,
        )
        .unwrap();
        reg.build_processor(&conf, &pctx, "0").unwrap()
    };
    for (headers, want) in [
        (serde_json::json!({}), "application/octet-stream"),
        (
            serde_json::json!({ "Accept": "application/json" }),
            "application/octet-stream",
        ),
        (
            serde_json::json!({ "Content-Type": "text/csv" }),
            "text/csv",
        ),
    ] {
        let res = execute_all(&pctx, &[named(headers)], vec![vec![Part::from_str("")]])
            .await
            .unwrap();
        assert_eq!(res[0][0].as_bytes_ref().as_ref(), want.as_bytes());
    }
}

/// A token endpoint that keeps each request's `Authorization` and body, and answers 401 to the
/// style it is told to refuse (`header` or `params`).
async fn token_server(refuse: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = seen.clone();
    let app = Router::new().route(
        "/token",
        post(move |headers: HeaderMap, body: String| {
            let kept = kept.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                kept.lock().unwrap().push(format!("{auth}|{body}"));
                let refused = match refuse {
                    "header" => !auth.is_empty(),
                    _ => auth.is_empty(),
                };
                if refused {
                    return (StatusCode::UNAUTHORIZED, "{}".to_string());
                }
                let token = r#"{"access_token":"t","token_type":"bearer","expires_in":3600}"#;
                (StatusCode::OK, token.to_string())
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// Bento's oauth2 (golang.org/x/oauth2, auto-detected style) sends the client in a Basic header
/// with only `grant_type` in the form, and the form style only after the header was refused
/// (T-6624: 56 seed pipelines' token requests differed by it).
#[tokio::test]
async fn oauth2_sends_the_client_as_bento_does() {
    let (reg, pctx, _) = setup_contexts(true);
    for (refuse, want) in [
        (
            "params",
            // base64("pipe%2Fline:s+e%26cret")
            vec!["Basic cGlwZSUyRmxpbmU6cytlJTI2Y3JldA==|grant_type=client_credentials&scope=a+b"],
        ),
        (
            "header",
            vec![
                "Basic cGlwZSUyRmxpbmU6cytlJTI2Y3JldA==|grant_type=client_credentials&scope=a+b",
                "|client_id=pipe%2Fline&client_secret=s+e%26cret&grant_type=client_credentials&scope=a+b",
            ],
        ),
    ] {
        let (base, seen) = token_server(refuse).await;
        let (api, _) = start_mock_server().await;
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "http": {
                "url": format!("{api}/content-type"),
                "verb": "GET",
                "oauth2": {
                    "enabled": true,
                    "client_key": "pipe/line",
                    "client_secret": "s e&cret",
                    "token_url": format!("{base}/token"),
                    "scopes": ["a", "b"]
                }
            }}),
            "noop",
            None,
        )
        .unwrap();
        let proc = reg.build_processor(&conf, &pctx, "0").unwrap();
        let res = execute_all(&pctx, &[proc], vec![vec![Part::from_str("")]])
            .await
            .unwrap();
        assert!(!res[0][0].is_errored(), "{refuse}");
        assert_eq!(*seen.lock().unwrap(), want, "{refuse}");
    }
}
