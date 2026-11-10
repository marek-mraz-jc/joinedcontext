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

/// A listener with two mounts, `/broker` and `/portal`, recording each path it is asked for;
/// `/broker/hop` redirects into `/portal`.
async fn two_mount_listener() -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = Router::new().fallback(move |uri: axum::http::Uri| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(uri.path().to_string());
            if uri.path() == "/broker/hop" {
                return (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", "/portal/secret")],
                    "",
                );
            }
            (StatusCode::OK, [("content-type", "application/json")], "{}")
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, seen)
}

fn host_target_contexts(target: Option<ekiben_core::HostTarget>) -> (Arc<Registry>, ProcContext) {
    let (reg, mut pctx, _) = setup_contexts(false);
    let resources = match target {
        Some(t) => ResourceManager::new().with_host_target(t),
        None => ResourceManager::new(),
    };
    pctx.org = resources.get_or_create_org(&OrgId::new("org-host"));
    pctx.resources = resources;
    (reg, pctx)
}

async fn get_once(reg: &Registry, pctx: &ProcContext, url: &str) -> Option<String> {
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"http": {"url": url, "verb": "GET", "retries": 0}}),
        "noop",
        None,
    )
    .unwrap();
    let proc = reg.build_processor(&conf, pctx, "0").unwrap();
    let out = proc
        .process_batch(pctx, vec![Part::from_str("{}")])
        .await
        .unwrap();
    out[0][0].error_get().map(|e| e.to_string())
}

#[tokio::test]
async fn a_stream_reaches_the_hosts_broker_mount_and_no_other_loopback_target() {
    let (addr, seen) = two_mount_listener().await;
    let (other, other_seen) = two_mount_listener().await;
    let target = ekiben_core::HostTarget {
        addr,
        path_prefixes: vec!["/broker".into()],
        late: None,
    };
    let (reg, pctx) = host_target_contexts(Some(target));

    let upsert = format!("http://{addr}/broker/ngsi-ld/v1/entityOperations/upsert");
    assert_eq!(get_once(&reg, &pctx, &upsert).await, None);
    assert_eq!(
        get_once(&reg, &pctx, &format!("http://{addr}/broker")).await,
        None
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["/broker/ngsi-ld/v1/entityOperations/upsert", "/broker"]
    );
    seen.lock().unwrap().clear();

    let port = addr.port();
    for refused in [
        format!("http://{addr}/portal/api/v1/projects"),
        format!("http://{addr}/"),
        format!("http://{addr}/brokerx/y"),
        format!("http://{addr}/broker/../portal/x"),
        format!("http://{addr}/broker/%2e%2e/portal/x"),
        format!("http://{addr}/broker%2F..%2Fportal/x"),
        format!("http://{addr}/broker/%2E%2E%2Fportal"),
        format!("http://localhost:{port}/broker/x"),
        format!("http://[::ffff:127.0.0.1]:{port}/broker/x"),
        format!("http://[::1]:{port}/broker/x"),
        format!("https://{addr}/broker/x"),
        format!("http://user:pw@{addr}/broker/x"),
        format!("http://{other}/broker/x"),
        "http://10.0.0.1/broker/x".to_string(),
        "http://169.254.169.254/broker/x".to_string(),
    ] {
        let err = get_once(&reg, &pctx, &refused).await;
        assert!(err.is_some(), "{refused} must be refused");
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "{:?}",
        seen.lock().unwrap()
    );
    assert!(other_seen.lock().unwrap().is_empty());

    // A redirect from the broker mount is not followed into another mount.
    let err = get_once(&reg, &pctx, &format!("http://{addr}/broker/hop")).await;
    assert!(err.is_some_and(|e| e.contains("307")));
    assert_eq!(*seen.lock().unwrap(), vec!["/broker/hop"]);

    // Without a host target the broker mount is as closed as any loopback address.
    let (reg, pctx) = host_target_contexts(None);
    assert!(get_once(&reg, &pctx, &upsert).await.is_some());
    assert_eq!(*seen.lock().unwrap(), vec!["/broker/hop"]);
}

/// A listener recording the `X-Request-Id` of every call (empty when none came).
async fn id_listener() -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        let log = log.clone();
        async move {
            let id = headers
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            log.lock().unwrap().push(id.to_string());
            (StatusCode::OK, [("content-type", "application/json")], "{}")
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, seen)
}

async fn get_n(reg: &Registry, pctx: &ProcContext, conf: serde_json::Value, n: usize) {
    let conf = ComponentConfig::from_json_value(&conf, "noop", None).unwrap();
    let proc = reg.build_processor(&conf, pctx, "0").unwrap();
    for _ in 0..n {
        let out = proc
            .process_batch(pctx, vec![Part::from_str("{}")])
            .await
            .unwrap();
        assert!(out[0][0].error_get().is_none());
    }
}

/// A stream's calls to the host's own listener name the stream to it, numbered, so the gateway's
/// and the broker's lines for a write carry the stream's reference; a configured id is kept, and
/// a call to anywhere else gets no header of ours.
#[tokio::test]
async fn a_stream_names_its_calls_to_the_hosts_listener_and_no_other() {
    let (addr, seen) = id_listener().await;
    let target = ekiben_core::HostTarget {
        addr,
        path_prefixes: vec!["/broker".into()],
        late: None,
    };
    let (reg, pctx) = host_target_contexts(Some(target));
    let url = format!("http://{addr}/broker/ngsi-ld/v1/entityOperations/upsert");
    get_n(
        &reg,
        &pctx,
        serde_json::json!({"http": {"url": url, "verb": "GET"}}),
        2,
    )
    .await;
    let ids = seen.lock().unwrap().clone();
    assert_eq!(ids.len(), 2, "{ids:?}");
    let run = ids[0]
        .strip_suffix("-1")
        .expect("the first call is <run>-1");
    assert!(
        run.len() == 32 && run.bytes().all(|b| b.is_ascii_hexdigit()),
        "{ids:?}"
    );
    assert_eq!(ids[1], format!("{run}-2"));

    seen.lock().unwrap().clear();
    let headers = serde_json::json!({"X-Request-Id": "set-by-the-pipeline"});
    get_n(
        &reg,
        &pctx,
        serde_json::json!({"http": {"url": url, "verb": "GET", "headers": headers}}),
        1,
    )
    .await;
    assert_eq!(*seen.lock().unwrap(), vec!["set-by-the-pipeline"]);

    seen.lock().unwrap().clear();
    let (reg, pctx, _) = setup_contexts(true);
    get_n(
        &reg,
        &pctx,
        serde_json::json!({"http": {"url": url, "verb": "GET"}}),
        1,
    )
    .await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![""],
        "no id to a listener that is not the host's"
    );
}

/// jc's Portal binds its internal listener after ekiben is built: a stream's report sinks reach
/// it under its pipeline routes once the address is set, and nothing on it before or elsewhere.
#[tokio::test]
async fn a_stream_reaches_the_hosts_late_listener_once_bound_under_its_prefixes_only() {
    let (addr, _) = two_mount_listener().await;
    let (internal, seen) = two_mount_listener().await;
    let late = std::sync::Arc::new(std::sync::OnceLock::new());
    let target = ekiben_core::HostTarget {
        addr,
        path_prefixes: vec!["/broker".into()],
        late: Some((late.clone(), vec!["/internal/pipelines".into()])),
    };
    let (reg, pctx) = host_target_contexts(Some(target));
    let outcomes = format!("http://{internal}/internal/pipelines/p/n/outcomes");
    assert!(
        get_once(&reg, &pctx, &outcomes).await.is_some(),
        "unset: refused"
    );
    late.set(internal).unwrap();
    assert_eq!(get_once(&reg, &pctx, &outcomes).await, None);
    for refused in [
        format!("http://{internal}/internal/revocations"),
        format!("http://{internal}/internal/pipelinesx/p"),
        format!("http://{internal}/broker/x"),
        format!("http://{internal}/internal/pipelines/%2e%2e/revocations"),
    ] {
        assert!(
            get_once(&reg, &pctx, &refused).await.is_some(),
            "{refused} must be refused"
        );
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["/internal/pipelines/p/n/outcomes"]
    );
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
/// style it is told to refuse (`header` or `params`), or 429 to every request (`busy`).
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
                if refuse == "busy" {
                    return (StatusCode::TOO_MANY_REQUESTS, "{}".to_string());
                }
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

/// On jc-prod a project's pipelines asked the realm again at once after each 429 (1,262 token
/// requests in 5 min, two per refusal), which kept the edge's shared bucket empty for the
/// Portal's sign-ins. Components of one client share its token and its backoff: a 429 is asked
/// once, with no second, form-style request, and nobody asks again inside the wait.
#[tokio::test]
async fn components_of_one_client_share_its_token_and_its_backoff() {
    let (reg, pctx, _) = setup_contexts(true);
    let (api, _) = start_mock_server().await;
    let build = |base: &str, key: &str| {
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "http": {
                "url": format!("{api}/content-type"),
                "verb": "GET",
                "retries": 0,
                "oauth2": {
                    "enabled": true,
                    "client_key": key,
                    "client_secret": "s",
                    "token_url": format!("{base}/token")
                }
            }}),
            "noop",
            None,
        )
        .unwrap();
        reg.build_processor(&conf, &pctx, "0").unwrap()
    };

    let (busy, refused) = token_server("busy").await;
    let procs = [build(&busy, "busy-client"), build(&busy, "busy-client")];
    for _ in 0..3 {
        let res = execute_all(&pctx, &procs[..1], vec![vec![Part::from_str("")]])
            .await
            .unwrap();
        assert!(res[0][0].is_errored());
        let res = execute_all(&pctx, &procs[1..], vec![vec![Part::from_str("")]])
            .await
            .unwrap();
        assert!(res[0][0].is_errored());
    }
    assert_eq!(
        refused.lock().unwrap().len(),
        1,
        "one token request for six calls"
    );

    let (good, asked) = token_server("params").await;
    let procs = [build(&good, "good-client"), build(&good, "good-client")];
    for p in &procs {
        let res = execute_all(
            &pctx,
            std::slice::from_ref(p),
            vec![vec![Part::from_str("")]],
        )
        .await
        .unwrap();
        assert!(!res[0][0].is_errored());
    }
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "the second component reuses the token"
    );
}

/// A server that answers every request with the status line it is given, as some sources do
/// (a code without a phrase, a phrase of their own).
async fn status_line_server(line: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = "down";
                let answer = format!(
                    "HTTP/1.1 {line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(answer.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_unexpected_status_quotes_the_servers_own_phrase_as_go_does() {
    let (reg, pctx, _) = setup_contexts(true);
    // Go's Response.Status is the line as sent: Bento 1.22 printed "(598): 598 , Error: ...".
    for (line, want) in [
        ("598 ", "unexpected response code (598): 598 , Error: down"),
        (
            "503 Temporarily Down",
            "unexpected response code (503): 503 Temporarily Down, Error: down",
        ),
        (
            "404 Not Found",
            "unexpected response code (404): 404 Not Found, Error: down",
        ),
    ] {
        let url = status_line_server(line).await;
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "http": { "url": url, "verb": "GET", "retries": 0 } }),
            "noop",
            None,
        )
        .unwrap();
        let p = reg.build_processor(&conf, &pctx, "0").unwrap();
        let res = execute_all(&pctx, &[p], vec![vec![Part::from_str("")]])
            .await
            .unwrap();
        let err = res[0][0]
            .error_get()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains(want), "{line}: {err}");
    }
}
