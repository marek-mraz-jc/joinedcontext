//! `redis` cache against a real server (the CI ekiben job's redis service). Runs where
//! REDIS_TEST_URL names one, as the sql parity cases run where PG_DSN names a database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    Cache, ComponentConfig, CoreError, OrgId, OrgQuotas, Registry, ResourceManager, StreamContext,
    StreamMetrics,
};
use ekiben_io_http::redis_cache::RedisCache;
use serde_json::json;
use tokio::sync::watch;

fn ctx(allow_private_ips: bool) -> StreamContext {
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(
        &OrgId::new("org-redis"),
        OrgQuotas {
            allow_private_ips,
            ..Default::default()
        },
    );
    let (_tx, rx) = watch::channel(false);
    StreamContext {
        stream_id: "redis-test".to_string(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("redis-test", "org-redis"),
        label: String::new(),
        path: "root.cache_resources".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    }
}

fn cache(url: &str, prefix: &str, ctx: &StreamContext) -> RedisCache {
    let conf = ComponentConfig {
        component_type: "redis".into(),
        config: json!({"url": url, "prefix": prefix}),
        ..Default::default()
    };
    RedisCache::from_config(&conf, ctx).expect("config")
}

#[tokio::test]
async fn keys_survive_a_new_cache_instance_and_expire_on_their_ttl() {
    let Ok(url) = std::env::var("REDIS_TEST_URL") else {
        eprintln!("REDIS_TEST_URL unset: the CI ekiben job runs this against its redis service");
        return;
    };
    let ctx = ctx(true);
    let prefix = format!("{}:", uuid::Uuid::new_v4().simple());
    let first = cache(&url, &prefix, &ctx);
    first
        .add("k", Bytes::from_static(b"v1"), None)
        .await
        .expect("add");
    assert!(matches!(
        first.add("k", Bytes::from_static(b"v2"), None).await,
        Err(CoreError::KeyAlreadyExists(_))
    ));
    // a restarted runner builds a new cache: what the first one added is still there (PL-51)
    let second = cache(&url, &prefix, &ctx);
    assert!(matches!(
        second.add("k", Bytes::from_static(b"v3"), None).await,
        Err(CoreError::KeyAlreadyExists(_))
    ));
    assert_eq!(second.get("k").await.unwrap(), Bytes::from_static(b"v1"));
    second
        .set("k", Bytes::from_static(b"v4"), None)
        .await
        .unwrap();
    assert_eq!(first.get("k").await.unwrap(), Bytes::from_static(b"v4"));
    second.delete("k").await.unwrap();
    assert!(matches!(
        first.get("k").await,
        Err(CoreError::KeyNotFound(_))
    ));

    second
        .set(
            "t",
            Bytes::from_static(b"x"),
            Some(Duration::from_millis(200)),
        )
        .await
        .unwrap();
    assert!(second.get("t").await.is_ok());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(matches!(
        second.get("t").await,
        Err(CoreError::KeyNotFound(_))
    ));
}

#[tokio::test]
async fn a_loopback_server_is_refused_without_private_ips() {
    let c = cache("redis://127.0.0.1:6379", "", &ctx(false));
    let e = c.get("k").await.expect_err("refused");
    assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
}
