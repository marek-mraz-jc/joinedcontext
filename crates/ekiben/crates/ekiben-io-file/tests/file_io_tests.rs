use ekiben_core::Cache;
use std::collections::HashMap;
use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, OrgId, Part, Registry, ResourceManager, StreamContext, StreamMetrics,
};
use tempfile::TempDir;
use tokio::sync::watch;

fn make_stream_ctx(file_root: Option<std::path::PathBuf>) -> (Arc<Registry>, StreamContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_file::register_all(&reg);

    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org-file"));
    let (_tx, rx) = watch::channel(false);

    let sctx = StreamContext {
        stream_id: "file-test".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("file-test", "org-file"),
        label: "root".to_string(),
        path: "root".to_string(),
        file_root,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, sctx)
}

#[tokio::test]
async fn file_input_output_and_path_traversal_guard() {
    let tmp = TempDir::new().unwrap();
    let (reg, sctx) = make_stream_ctx(Some(tmp.path().to_path_buf()));

    // 1. Path traversal `../outside.txt` must be refused!
    let trav_val = serde_json::json!({
        "file": {
            "path": "../outside.txt",
            "codec": "lines"
        }
    });
    let trav_conf = ComponentConfig::from_json_value(&trav_val, "stdout", None).unwrap();
    let trav_out = reg.build_output(&trav_conf, &sctx).unwrap();
    assert!(
        trav_out
            .write_batch(&sctx, vec![Part::from_str("hacked")])
            .await
            .is_err(),
        "path traversal '..' must be refused"
    );

    // 2. Write lines to file output and read back via file input
    let out_val = serde_json::json!({
        "file": {
            "path": "sub/data.jsonl",
            "codec": "lines"
        }
    });
    let out_conf = ComponentConfig::from_json_value(&out_val, "stdout", None).unwrap();
    let file_out = reg.build_output(&out_conf, &sctx).unwrap();
    file_out
        .write_batch(
            &sctx,
            vec![Part::from_str(r#"{"id":1}"#), Part::from_str(r#"{"id":2}"#)],
        )
        .await
        .unwrap();

    let in_val = serde_json::json!({
        "file": {
            "paths": ["sub/*.jsonl"]
        }
    });
    let in_conf = ComponentConfig::from_json_value(&in_val, "stdin", None).unwrap();
    let mut file_in = reg.build_input(&in_conf, &sctx).unwrap();
    file_in.connect(&sctx).await.unwrap();

    let (b1, ack1) = file_in.read_batch(&sctx).await.unwrap();
    ack1.ack();
    assert_eq!(b1[0].as_bytes_ref().as_ref(), br#"{"id":1}"#);
    assert!(!b1[0].meta_get_str("path").is_empty());

    let (b2, ack2) = file_in.read_batch(&sctx).await.unwrap();
    ack2.ack();
    assert_eq!(b2[0].as_bytes_ref().as_ref(), br#"{"id":2}"#);

    assert!(matches!(
        file_in.read_batch(&sctx).await,
        Err(ekiben_core::CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn generate_broker_switch_fallback_reject_errored_outputs() {
    let (reg, sctx) = make_stream_ctx(None);

    // Set up an org-scoped memory cache to verify output routing
    let dlq_cache = Arc::new(ekiben_processors::resources::MemoryCache::new(None, None));
    let ok_cache = Arc::new(ekiben_processors::resources::MemoryCache::new(None, None));
    sctx.resources
        .insert_cache(sctx.org_id(), "dlq", dlq_cache.clone());
    sctx.resources
        .insert_cache(sctx.org_id(), "ok", ok_cache.clone());

    // `fallback` + `reject_errored`: non-errored parts go to `ok` cache, errored parts fall back to `dlq` cache
    // with `fallback_error` metadata set!
    let fb_val: serde_json::Value = serde_norway::from_str(
        r#"
fallback:
  - reject_errored:
      cache:
        target: ok
        key: '${! json("id") }'
  - cache:
      target: dlq
      key: '${! json("id") }'
    processors:
      - mapping: |
          root = this
          root.dlq_reason = @fallback_error
"#,
    )
    .unwrap();
    let fb_conf = ComponentConfig::from_json_value(&fb_val, "drop", None).unwrap();
    let fb_out = reg.build_output(&fb_conf, &sctx).unwrap();

    let p_good = Part::from_str(r#"{"id":"good1"}"#);
    let mut p_bad = Part::from_str(r#"{"id":"bad1"}"#);
    p_bad.set_error_msg("validation failed");

    fb_out
        .write_batch(&sctx, vec![p_good, p_bad])
        .await
        .unwrap();

    // Verify `good1` went to `ok` and NOT `dlq`, while `bad1` went to `dlq` with `dlq_reason`!
    assert!(ok_cache.get("good1").await.is_ok());
    assert!(dlq_cache.get("good1").await.is_err());
    let dlq_bytes = dlq_cache.get("bad1").await.unwrap();
    let dlq_json: serde_json::Value = serde_json::from_slice(&dlq_bytes).unwrap();
    assert_eq!(dlq_json["id"], "bad1");
    assert_eq!(dlq_json["dlq_reason"], "validation failed");
}
