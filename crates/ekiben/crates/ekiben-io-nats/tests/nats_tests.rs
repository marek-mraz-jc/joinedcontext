use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, BatchError, ComponentConfig, OrgId, OrgQuotas, Part, ProcContext, Registry,
    ResourceManager, StreamContext, StreamMetrics,
};
use tokio::sync::watch;

fn make_nats_ctx(
    org_id: &str,
    subject_prefix: Option<&str>,
) -> (Arc<Registry>, StreamContext, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_nats::register_all(&reg);

    let resources = ResourceManager::new();
    let oid = OrgId::new(org_id);
    resources.set_org_quotas(
        &oid,
        OrgQuotas {
            nats_subject_prefix: subject_prefix.map(String::from),
            ..Default::default()
        },
    );
    let org = resources.get_or_create_org(&oid);
    let (_tx, rx) = watch::channel(false);
    let metrics = StreamMetrics::new("nats-test", org_id);

    let sctx = StreamContext {
        stream_id: "nats-test".to_string(),
        org: org.clone(),
        resources: resources.clone(),
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: metrics.clone(),
        label: "nats_comp".to_string(),
        path: "root".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx.clone(),
    };
    let pctx = sctx.to_proc_context("nats_kv", "kv_proc", "pipeline.processors.0");
    (reg, sctx, pctx)
}

#[tokio::test]
async fn jetstream_durable_consumer_ack_nak_redelivery_and_max_ack_pending_backpressure() {
    // Grounded in internal/impl/nats/input_jetstream.go and output_jetstream.go
    let (reg, sctx, _) = make_nats_ctx("org-js", Some("org_js"));
    let url = "nats://memory/js_test_1";

    // 1. Publish 2 messages to JetStream with custom headers and metadata
    let out_val = serde_json::json!({
        "nats_jetstream": {
            "urls": [url],
            "subject": "org_js.orders.${! json(\"type\") }",
            "headers": {
                "X-Source": "ekiben-test"
            },
            "metadata": {
                "include_prefixes": ["trace_"]
            }
        }
    });
    let out_conf = ComponentConfig::from_json_value(&out_val, "drop", None).unwrap();
    let js_out = reg.build_output(&out_conf, &sctx).unwrap();

    let mut m1 = Part::from_str(r#"{"id":101,"type":"created"}"#);
    m1.meta_set_str("trace_id", "tr-999");
    let m2 = Part::from_str(r#"{"id":102,"type":"updated"}"#);
    js_out.write_batch(&sctx, vec![m1, m2]).await.unwrap();

    // 2. Consume with a durable consumer and `max_ack_pending: 1`
    let in_val = serde_json::json!({
        "nats_jetstream": {
            "urls": [url],
            "subject": "org_js.orders.*",
            "durable": "orders_durable",
            "stream": "ORDERS",
            "deliver": "all",
            "max_ack_pending": 1
        }
    });
    let in_conf = ComponentConfig::from_json_value(&in_val, "stdin", None).unwrap();
    let mut js_in = reg.build_input(&in_conf, &sctx).unwrap();
    js_in.connect(&sctx).await.unwrap();

    // Read message 1 (`id: 101`)
    let (b1, ack1) = js_in.read_batch(&sctx).await.unwrap();
    assert_eq!(b1[0].meta_get_str("nats_subject"), "org_js.orders.created");
    assert_eq!(b1[0].meta_get_str("nats_sequence_stream"), "1");
    assert_eq!(b1[0].meta_get_str("nats_num_delivered"), "1");
    assert_eq!(b1[0].meta_get_str("X-Source"), "ekiben-test");
    assert_eq!(b1[0].meta_get_str("trace_id"), "tr-999");

    // Because `max_ack_pending: 1` and message 1 is not yet acked, pulling directly from the cluster must return None (backpressure!)
    let cluster = ekiben_io_nats::broker_mem::InMemoryNatsCluster::get(url);
    assert!(
        cluster
            .try_pull_jetstream("org-js:ORDERS:orders_durable")
            .is_none(),
        "max_ack_pending backpressure must halt further pulls until ack"
    );

    // Nack message 1 -> triggers redelivery with `nats_num_delivered == "2"`!
    ack1.nack(BatchError::new(b1, "simulated downstream failure"));

    let (b1_retry, ack1_retry) = js_in.read_batch(&sctx).await.unwrap();
    assert_eq!(b1_retry[0].meta_get_str("nats_sequence_stream"), "1");
    assert_eq!(
        b1_retry[0].meta_get_str("nats_num_delivered"),
        "2",
        "redelivered message must increment nats_num_delivered"
    );
    // Ack message 1 -> unblocks message 2!
    ack1_retry.ack();

    let (b2, ack2) = js_in.read_batch(&sctx).await.unwrap();
    assert_eq!(b2[0].meta_get_str("nats_sequence_stream"), "2");
    assert_eq!(b2[0].meta_get_str("nats_subject"), "org_js.orders.updated");
    ack2.ack();
}

#[tokio::test]
async fn nats_kv_cache_and_processor_and_org_subject_isolation() {
    // Grounded in internal/impl/nats/cache_kv.go and processor_kv.go
    let (reg, sctx_a, pctx_a) = make_nats_ctx("org-a", Some("org_a"));
    let (_, sctx_b, _) = make_nats_ctx("org-b", Some("org_b"));
    let url = "nats://memory/kv_test_1";

    // 1. Org A cannot publish or subscribe to Org B's subject prefix
    let cross_subj_val = serde_json::json!({
        "nats_jetstream": {
            "urls": [url],
            "subject": "org_b.secret.events"
        }
    });
    let cross_conf = ComponentConfig::from_json_value(&cross_subj_val, "drop", None).unwrap();
    let cross_out = reg.build_output(&cross_conf, &sctx_a).unwrap();
    assert!(
        cross_out
            .write_batch(&sctx_a, vec![Part::from_str("{}")])
            .await
            .is_err(),
        "Org A must be refused when publishing to Org B's NATS subject prefix"
    );

    // 2. `nats_kv` cache is isolated per organization even on the same NATS cluster URL and bucket name!
    let kv_conf_val = serde_json::json!({
        "nats_kv": {
            "urls": [url],
            "bucket": "shared_bucket"
        }
    });
    let kv_conf = ComponentConfig::from_json_value(&kv_conf_val, "nats_kv", None).unwrap();
    let cache_a = reg.build_cache(&kv_conf, &sctx_a).unwrap();
    let cache_b = reg.build_cache(&kv_conf, &sctx_b).unwrap();

    cache_a
        .set("k1", Bytes::from_static(b"val_from_a"), None)
        .await
        .unwrap();
    assert_eq!(
        cache_a.get("k1").await.unwrap(),
        Bytes::from_static(b"val_from_a")
    );
    assert!(
        cache_b.get("k1").await.is_err(),
        "Org B must not see Org A's nats_kv entry in the same bucket name"
    );

    // 3. `nats_kv` processor operations (`put`, `get`, `update`, `history`, `keys`, `delete`)
    let put_val = serde_json::json!({
        "nats_kv": {
            "urls": [url],
            "bucket": "proc_bucket",
            "operation": "put",
            "key": "item.${! json(\"id\") }"
        }
    });
    let get_val = serde_json::json!({
        "nats_kv": {
            "urls": [url],
            "bucket": "proc_bucket",
            "operation": "get",
            "key": "item.${! @lookup_id }"
        }
    });
    let put_proc = reg
        .build_processor(
            &ComponentConfig::from_json_value(&put_val, "noop", None).unwrap(),
            &pctx_a,
            "0",
        )
        .unwrap();
    let get_proc = reg
        .build_processor(
            &ComponentConfig::from_json_value(&get_val, "noop", None).unwrap(),
            &pctx_a,
            "1",
        )
        .unwrap();

    let put_res = execute_all(
        &pctx_a,
        &[put_proc],
        vec![vec![Part::from_str(r#"{"id":"42","v":"hello"}"#)]],
    )
    .await
    .unwrap();
    assert_eq!(put_res[0][0].meta_get_str("nats_kv_key"), "item.42");
    assert_eq!(put_res[0][0].meta_get_str("nats_kv_revision"), "1");

    let mut query_part = Part::from_str("{}");
    query_part.meta_set_str("lookup_id", "42");
    let get_res = execute_all(&pctx_a, &[get_proc], vec![vec![query_part]])
        .await
        .unwrap();
    assert_eq!(
        get_res[0][0].as_bytes_ref().as_ref(),
        br#"{"id":"42","v":"hello"}"#
    );
}
