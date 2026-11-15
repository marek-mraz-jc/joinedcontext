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

fn generate(
    reg: &Registry,
    sctx: &StreamContext,
    conf: serde_json::Value,
) -> Box<dyn ekiben_core::Input> {
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({ "generate": conf }),
        "generate",
        None,
    )
    .unwrap();
    reg.build_input(&conf, sctx).unwrap()
}

// Ported from Bento input_generate_test.go: a duration's first batch is free, a cron
// schedule waits for its first activation, and a bad interval names both readings.
#[tokio::test(start_paused = true)]
async fn generate_paces_by_duration_and_by_cron() {
    let (reg, sctx) = make_stream_ctx(None);
    let mut every = generate(
        &reg,
        &sctx,
        serde_json::json!({"mapping": "root = 1", "interval": "50ms", "count": 2}),
    );
    let t0 = tokio::time::Instant::now();
    every.read_batch(&sctx).await.unwrap();
    assert_eq!(t0.elapsed(), std::time::Duration::ZERO);
    every.read_batch(&sctx).await.unwrap();
    assert_eq!(t0.elapsed(), std::time::Duration::from_millis(50));

    // `@every 1s` is a cron schedule: it fires on the next whole second after one second.
    let mut cron = generate(
        &reg,
        &sctx,
        serde_json::json!({"mapping": "root = 1", "interval": "@every 1s"}),
    );
    let t0 = tokio::time::Instant::now();
    cron.read_batch(&sctx).await.unwrap();
    let waited = t0.elapsed();
    assert!(
        waited > std::time::Duration::ZERO && waited <= std::time::Duration::from_secs(1),
        "{waited:?}"
    );

    // No interval is Bento's `1s` default; an empty one does not wait.
    let mut default = generate(&reg, &sctx, serde_json::json!({"mapping": "root = 1"}));
    default.read_batch(&sctx).await.unwrap();
    let t0 = tokio::time::Instant::now();
    default.read_batch(&sctx).await.unwrap();
    assert_eq!(t0.elapsed(), std::time::Duration::from_secs(1));
    let mut fast = generate(
        &reg,
        &sctx,
        serde_json::json!({"mapping": "root = 1", "interval": ""}),
    );
    fast.read_batch(&sctx).await.unwrap();
    let t0 = tokio::time::Instant::now();
    fast.read_batch(&sctx).await.unwrap();
    assert_eq!(t0.elapsed(), std::time::Duration::ZERO);

    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"generate": {"mapping": "root = 1", "interval": "61 * * * *"}}),
        "generate",
        None,
    )
    .unwrap();
    let err = reg
        .build_input(&conf, &sctx)
        .err()
        .expect("refused")
        .to_string();
    assert!(
        err.contains("failed to parse interval as duration string"),
        "{err}"
    );
    assert!(
        err.contains("end of range (61) above maximum (59): 61"),
        "{err}"
    );
}

#[tokio::test]
async fn generate_replays_a_nacked_batch_before_it_ends() {
    use ekiben_core::BatchError;
    let (reg, sctx) = make_stream_ctx(None);
    let mut input = generate(
        &reg,
        &sctx,
        serde_json::json!({"mapping": "root = counter()", "interval": "", "count": 1}),
    );
    let (batch, ack) = input.read_batch(&sctx).await.unwrap();
    assert_eq!(batch[0].as_bytes_ref().as_ref(), b"1");
    // The answer comes later: the exhausted input waits for it instead of ending.
    let pending = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ack.nack(BatchError::new(batch, "output refused".to_string()));
    });
    let (again, ack) = input.read_batch(&sctx).await.unwrap();
    pending.await.unwrap();
    assert_eq!(again[0].as_bytes_ref().as_ref(), b"1");
    // A responder dropped unanswered does not hold the input open.
    drop(ack);
    assert!(matches!(
        input.read_batch(&sctx).await,
        Err(ekiben_core::CoreError::EndOfInput)
    ));

    let mut no_replay = generate(
        &reg,
        &sctx,
        serde_json::json!({"mapping": "root = 1", "interval": "", "count": 1, "auto_replay_nacks": false}),
    );
    let (batch, ack) = no_replay.read_batch(&sctx).await.unwrap();
    ack.nack(BatchError::new(batch, "output refused".to_string()));
    assert!(matches!(
        no_replay.read_batch(&sctx).await,
        Err(ekiben_core::CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn a_composite_output_closes_its_children_and_a_batched_child_flushes() {
    let (reg, sctx) = make_stream_ctx(None);
    let ok_cache = Arc::new(ekiben_processors::resources::MemoryCache::new(None, None));
    sctx.resources
        .insert_cache(sctx.org_id(), "ok", ok_cache.clone());
    // The shape of the Portal's outcome post: a batched output in a fallback in a broker.
    let val: serde_json::Value = serde_norway::from_str(
        r#"
broker:
  pattern: fan_out_sequential_fail_fast
  outputs:
    - switch:
        cases:
          - check: errored()
            output: {reject: '${! error() }'}
          - output: {drop: {}}
    - fallback:
        - cache:
            target: ok
            key: '${! json("id") }'
            batching: {count: 100, period: 1h}
        - drop: {}
"#,
    )
    .unwrap();
    let conf = ComponentConfig::from_json_value(&val, "drop", None).unwrap();
    let out = reg.build_output(&conf, &sctx).unwrap();
    out.write_batch(&sctx, vec![Part::from_str(r#"{"id":"r1"}"#)])
        .await
        .unwrap();
    assert!(ok_cache.get("r1").await.is_err(), "waits for its batch");
    out.close().await.unwrap();
    assert!(
        ok_cache.get("r1").await.is_ok(),
        "closing the broker flushed it"
    );
}
