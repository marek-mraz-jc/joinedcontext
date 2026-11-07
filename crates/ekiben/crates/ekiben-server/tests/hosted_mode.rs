//! ekiben hosted in a shared process (`jc`): a posted stream config reads only `EKIBEN_VAR_*` from
//! the process environment (never another module's secret), and the API refuses every request
//! without a token, even when no token is configured.

use std::sync::Arc;

use ekiben_core::{EnvAccess, ResourceManager, StreamConfig};
use ekiben_server::{
    build_full_registry, build_router, run_stream_once, ServerAuthConfig, ServerState,
    StreamManager,
};
use tokio::net::TcpListener;

async fn serve(state: Arc<ServerState>) -> String {
    let app = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn hosted_state(
    file_root: Option<std::path::PathBuf>,
    admin_token: Option<&str>,
) -> Arc<ServerState> {
    let manager = StreamManager::new(
        build_full_registry(),
        ResourceManager::new(),
        file_root,
        EnvAccess::AllowListed,
    );
    ServerState::new(
        manager,
        ServerAuthConfig {
            admin_bearer_token: admin_token.map(str::to_owned),
            org_tokens: Vec::new(),
            require_auth: true,
        },
    )
}

#[test]
fn allow_listed_access_reads_only_ekiben_var_names() {
    std::env::set_var("JC_HOSTED_UNIT_SECRET", "wardkeep-kek-would-be-here");
    std::env::set_var("EKIBEN_VAR_HOSTED_UNIT", "visible");
    assert_eq!(EnvAccess::AllowListed.lookup("JC_HOSTED_UNIT_SECRET"), None);
    assert_eq!(
        EnvAccess::AllowListed
            .lookup("EKIBEN_VAR_HOSTED_UNIT")
            .as_deref(),
        Some("visible")
    );
    assert_eq!(EnvAccess::None.lookup("EKIBEN_VAR_HOSTED_UNIT"), None);
    assert_eq!(
        EnvAccess::Process
            .lookup("JC_HOSTED_UNIT_SECRET")
            .as_deref(),
        Some("wardkeep-kek-would-be-here")
    );
    assert_eq!(EnvAccess::from(true), EnvAccess::Process);
    assert_eq!(EnvAccess::from(false), EnvAccess::None);
}

#[test]
fn bloblang_env_of_an_allow_listed_stream_cannot_read_other_variables() {
    std::env::set_var("JC_HOSTED_BLOBL_SECRET", "portal-oidc-secret");
    std::env::set_var("EKIBEN_VAR_HOSTED_BLOBL", "fine");
    let env = ekiben_core::config::build_blobl_env_for_stream(
        &std::collections::HashMap::new(),
        EnvAccess::AllowListed,
    );
    assert_eq!(env.lookup_env("JC_HOSTED_BLOBL_SECRET"), None);
    assert_eq!(
        env.lookup_env("EKIBEN_VAR_HOSTED_BLOBL").as_deref(),
        Some("fine")
    );
    let none = ekiben_core::config::build_blobl_env_for_stream(
        &std::collections::HashMap::new(),
        EnvAccess::None,
    );
    assert_eq!(none.lookup_env("EKIBEN_VAR_HOSTED_BLOBL"), None);
}

#[tokio::test]
async fn a_posted_config_cannot_substitute_another_modules_secret() {
    std::env::set_var("JC_HOSTED_API_SECRET", "s3cr3t-of-another-module");
    std::env::set_var("EKIBEN_VAR_HOSTED_API", "allowed-value");
    let base = serve(hosted_state(None, Some("tok"))).await;
    let client = reqwest::Client::new();
    let config = |var: &str| {
        format!(
            "input:\n  generate:\n    interval: 1h\n    mapping: 'root = \"${{{var}}}\"'\noutput:\n  drop: {{}}\n"
        )
    };

    // Strict: the foreign variable is undefined, so the config is refused.
    let resp = client
        .post(format!("{base}/streams/leak"))
        .bearer_auth("tok")
        .body(config("JC_HOSTED_API_SECRET"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = resp.text().await.unwrap();
    assert!(body.contains("JC_HOSTED_API_SECRET"), "{body}");
    assert!(!body.contains("s3cr3t-of-another-module"), "{body}");

    // Chilled: accepted, but the secret is not substituted into the stored config.
    let resp = client
        .post(format!("{base}/streams/leak?chilled=true"))
        .bearer_auth("tok")
        .body(config("JC_HOSTED_API_SECRET"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let read = client
        .get(format!("{base}/streams/leak"))
        .bearer_auth("tok")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!read.contains("s3cr3t-of-another-module"), "{read}");

    // An allow-listed variable is substituted.
    let resp = client
        .post(format!("{base}/streams/ok"))
        .bearer_auth("tok")
        .body(config("EKIBEN_VAR_HOSTED_API"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let read = client
        .get(format!("{base}/streams/ok"))
        .bearer_auth("tok")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(read.contains("allowed-value"), "{read}");

    // The resources endpoint substitutes the same way.
    let resp = client
        .post(format!("{base}/resources/cache/c1"))
        .bearer_auth("tok")
        .body("memory:\n  default_ttl: ${JC_HOSTED_API_SECRET}\n")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_running_stream_cannot_read_another_modules_secret_through_env() {
    std::env::set_var("JC_HOSTED_RUN_SECRET", "kek-bytes-of-wardkeep");
    std::env::set_var("EKIBEN_VAR_HOSTED_RUN", "ok-value");
    let root = tempfile::tempdir().unwrap();
    let yaml = r#"
input:
  generate:
    count: 1
    interval: ""
    mapping: 'root = {"leak": env("JC_HOSTED_RUN_SECRET"), "ok": env("EKIBEN_VAR_HOSTED_RUN")}'
output:
  file:
    path: out.jsonl
    codec: lines
"#;
    let (conf, _) = StreamConfig::parse_str_with_env(yaml, false, |_| None).expect("config");
    let summary = run_stream_once(
        "hosted-run",
        conf,
        build_full_registry(),
        ResourceManager::new(),
        Some(root.path().to_path_buf()),
        EnvAccess::AllowListed,
    )
    .await
    .expect("run");
    assert!(summary.success, "{summary:?}");
    let out = std::fs::read_to_string(root.path().join("out.jsonl")).expect("output file");
    assert!(!out.contains("kek-bytes-of-wardkeep"), "{out}");
    assert!(out.contains("ok-value"), "{out}");
}

#[tokio::test]
async fn hosted_api_refuses_requests_without_a_token_even_with_none_configured() {
    let base = serve(hosted_state(None, None)).await;
    let client = reqwest::Client::new();
    for path in [
        "/streams",
        "/metrics",
        "/stats",
        "/ready",
        "/version",
        "/endpoints",
        "/bento/streams",
    ] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "{path}");
    }
    let resp = client
        .post(format!("{base}/streams/x"))
        .body("input:\n  generate:\n    mapping: 'root = 1'\noutput:\n  drop: {}\n")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    // Liveness stays open.
    for path in ["/ping", "/bento/ping"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn hosted_api_with_a_token_serves_only_the_token_holder() {
    let base = serve(hosted_state(None, Some("admin-token"))).await;
    let client = reqwest::Client::new();
    let resp = client.get(format!("{base}/metrics")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let resp = client
        .get(format!("{base}/metrics"))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    for path in ["/metrics", "/streams", "/ready"] {
        let resp = client
            .get(format!("{base}{path}"))
            .bearer_auth("admin-token")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "{path}");
    }
}
