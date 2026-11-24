//! `streams` mode is process-wide, so its refusals run in a test binary of their own.

use std::collections::HashMap;
use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, OrgId, Registry, ResourceManager, StreamContext, StreamMetrics,
};
use serde_json::json;

fn context() -> (Arc<Registry>, StreamContext) {
    let reg = Arc::new(Registry::new());
    ekiben_io_http::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org-streams"));
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let ctx = StreamContext {
        stream_id: "s".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("s", "org-streams"),
        label: String::new(),
        path: "root.input".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, ctx)
}

async fn refusal(conf: serde_json::Value) -> String {
    let (reg, ctx) = context();
    let component = ComponentConfig {
        label: String::new(),
        component_type: "http_server".to_string(),
        config: conf,
        processors: Vec::new(),
    };
    let mut input = reg.build_input(&component, &ctx).expect("builds");
    input.connect(&ctx).await.expect_err("refused").to_string()
}

#[tokio::test]
async fn streams_mode_refuses_own_listeners_missing_auth_and_a_missing_ingest_listener() {
    use ekiben_io_http::http_server::{is_streams_mode, set_streams_mode};
    let err = refusal(json!({ "path": "/a" })).await;
    assert!(
        err.contains("set address (for example 0.0.0.0:4195)"),
        "{err}"
    );
    set_streams_mode();
    assert!(is_streams_mode());
    for (conf, want) in [
        (
            json!({ "address": "127.0.0.1:0", "auth": { "public": true } }),
            "http_server.address is not allowed in streams mode",
        ),
        (
            json!({ "path": "/b" }),
            "http_server needs auth in streams mode",
        ),
        (
            json!({ "path": "/c", "auth": { "public": true } }),
            "this server has no ingest listener; start it with --ingest-addr",
        ),
    ] {
        let err = refusal(conf.clone()).await;
        assert!(err.contains(want), "{conf}: {err}");
    }
}
