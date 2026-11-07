use bytes::Bytes;
use ekiben_bloblang::Value;
use ekiben_core::{
    deserialize_bytes, interpolate_env_str, redact_config_value, redact_url_str, serialize_bytes,
    BatchError, CombinedAcker, OrgId, OrgQuotas, Part, PartMetaAdapter, PartsView, ResourceManager,
    SortGroup, StreamConfig, TrackedTransaction,
};

#[test]
fn orphan_rule_parts_view_and_lazy_json_roundtrip() {
    // Section 4 Defect 1: PartsView<'a>(&'a [Part]) implements MessageBatch without touching [Part] or Vec<Part>
    let mut p0 = Part::from_str(r#"{"name":"alice","score":10}"#);
    p0.meta_set_str("topic", "users");
    let p1 = Part::from_str(r#"{"name":"bob","score":20}"#);
    let mut batch = vec![p0, p1];

    let env = ekiben_bloblang::Environment::new();
    let exec = env
        .compile_mapping(
            r#"
root.user = this.name.uppercase()
root.total = json("score").from_all().sum()
meta out_topic = @topic + "_processed"
"#,
        )
        .unwrap();

    let res = {
        let view = PartsView(&batch);
        let meta_out = {
            let mut m = batch[0].meta_all().clone();
            let r = exec.map_part(0, &view, &mut m, None).unwrap().unwrap();
            (r, m)
        };
        meta_out.1
    };
    assert_eq!(
        res.get("out_topic"),
        Some(&Value::String("users_processed".to_string()))
    );

    let mapped = {
        let view = PartsView(&batch);
        let mut copy = batch[0].shallow_copy();
        let mut adapter = PartMetaAdapter(&mut copy);
        let val = exec
            .map_part(0, &view, &mut adapter, None)
            .unwrap()
            .unwrap();
        copy.apply_mapped_value(val);
        copy
    };
    assert_eq!(mapped.meta_get_str("out_topic"), "users_processed");
    assert_eq!(
        mapped.as_structured_ref().unwrap().to_json_value(),
        serde_json::json!({"total": 30, "user": "ALICE"})
    );

    // Ensure lazy structured mutation does not re-parse JSON
    let s = batch[0].as_structured_mut().unwrap();
    s.set_path(&["extra".to_string()], Value::Bool(true))
        .unwrap();
    assert_eq!(
        batch[0].as_bytes(),
        Bytes::from_static(br#"{"extra":true,"name":"alice","score":10}"#)
    );
}

#[test]
fn binary_serialization_and_sort_group_tracked_transaction() {
    let raw_parts: &[&[u8]] = &[b"hello", b"world"];
    let blob = serialize_bytes(raw_parts);
    let unpacked = deserialize_bytes(&blob).unwrap();
    assert_eq!(unpacked, vec![b"hello".to_vec(), b"world".to_vec()]);

    // TrackedTransaction + SortGroup: if part 1 fails in a downstream batch but part 0 succeeds,
    // TrackedTransaction for part 0 acks Ok while part 1 nacks!
    let (ack0, mut rx0) = ekiben_core::AckResponder::channel();
    let (ack1, mut rx1) = ekiben_core::AckResponder::channel();
    let t0 = TrackedTransaction::new(vec![Part::from_str("msg0")], ack0);
    let t1 = TrackedTransaction::new(vec![Part::from_str("msg1")], ack1);

    let combined_batch = vec![t0.message()[0].clone(), t1.message()[0].clone()];
    let mut b_err = BatchError::new(combined_batch, "part 1 failed");
    b_err.failed(1, "part 1 failed");

    t0.resolve(Some(b_err.clone()));
    t1.resolve(Some(b_err));

    assert!(rx0.try_recv().unwrap().is_none(), "t0 must ack cleanly");
    assert!(rx1.try_recv().unwrap().is_some(), "t1 must nack");

    // CombinedAcker waits for all derived acks
    let (root_ack, mut root_rx) = ekiben_core::AckResponder::channel();
    let combiner = CombinedAcker::new(root_ack);
    let d1 = combiner.derive();
    let d2 = combiner.derive();
    d1.ack();
    assert!(root_rx.try_recv().is_err(), "still waiting on d2");
    d2.ack();
    assert!(root_rx.try_recv().unwrap().is_none());

    // SortGroup index tracking across copies
    let (grp, tagged) = SortGroup::new(vec![Part::from_str("a"), Part::from_str("b")]);
    assert_eq!(grp.get_index(&tagged[1]), Some(1));
}

#[test]
fn env_interpolation_and_secret_redaction() {
    // Grounded in website/docs/configuration/interpolation.md and PART B trap [T-5803]
    let res = interpolate_env_str(
        "url: ${HOST:http://localhost:8080}/api?escaped=${{foo}}&empty=${EMPTY:}&blobl=${! json(\"id\") }",
        |k| match k {
            "HOST" => Some("https://example.com".to_string()),
            _ => None,
        },
    )
    .unwrap();
    assert_eq!(
        res,
        "url: https://example.com/api?escaped=${foo}&empty=&blobl=${! json(\"id\") }"
    );

    // Undefined env var without ':' triggers lint error
    let err = interpolate_env_str("addr: ${UNDEFINED_VAR}", |_| None).unwrap_err();
    assert_eq!(err.missing, vec!["UNDEFINED_VAR".to_string()]);

    // URL and config secret redaction
    assert_eq!(
        redact_url_str("https://alice:supersecret@api.example.com/v1"),
        "https://alice:REDACTED@api.example.com/v1"
    );

    let cfg = serde_json::json!({
        "input": {
            "http_client": {
                "url": "https://user:pass123@example.com/data",
                "headers": {
                    "Authorization": "Bearer secret-jwt-token",
                    "Accept": "application/json"
                },
                "oauth2": {
                    "client_key": "my-client",
                    "client_secret": "top-secret-value"
                }
            }
        }
    });
    let redacted = redact_config_value(&cfg);
    let dumped = serde_json::to_string(&redacted).unwrap();
    assert!(!dumped.contains("pass123"));
    assert!(!dumped.contains("secret-jwt-token"));
    assert!(!dumped.contains("top-secret-value"));
    assert!(dumped.contains("REDACTED"));
}

#[test]
fn multi_org_isolation_and_quotas() {
    let rm = ResourceManager::new();
    let org_a = OrgId::new("org-a");
    let org_b = OrgId::new("org-b");

    rm.set_org_quotas(
        &org_b,
        OrgQuotas {
            max_concurrent_streams: 1,
            max_messages_per_sec: 2,
            allowed_http_hosts: Some(vec!["*.allowed.example".to_string()]),
            nats_subject_prefix: Some("org_b".to_string()),
            ..Default::default()
        },
    );

    let ctx_a = rm.get_or_create_org(&org_a);
    let ctx_b = rm.get_or_create_org(&org_b);

    // Org B hits its stream limit of 1 without affecting Org A
    let _guard_b1 = ctx_b.acquire_stream_slot().unwrap();
    assert!(ctx_b.acquire_stream_slot().is_err());
    let _guard_a1 = ctx_a.acquire_stream_slot().unwrap();
    let _guard_a2 = ctx_a.acquire_stream_slot().unwrap();

    // Org B hits its message rate limit of 2/sec without affecting Org A
    assert!(ctx_b.check_message_admission(2, 100).is_ok());
    assert!(ctx_b.check_message_admission(1, 50).is_err());
    assert!(ctx_a.check_message_admission(100, 1000).is_ok());

    // Org B outbound HTTP host allow-list and NATS subject prefix
    assert!(ctx_b.check_http_request("api.allowed.example").is_ok());
    assert!(ctx_b.check_http_request("evil.example.com").is_err());
    assert!(ctx_b.validate_nats_subject("org_b.events.created").is_ok());
    assert!(ctx_b.validate_nats_subject("org_a.events.created").is_err());

    // Stream config parsing
    let sc = StreamConfig::from_yaml_or_json(
        r#"
org: org-a
run: once
input:
  generate:
    count: 1
    mapping: 'root.ok = true'
pipeline:
  processors:
    - mapping: 'root = this'
output:
  drop: {}
"#,
    )
    .unwrap();
    assert_eq!(sc.org, OrgId::new("org-a"));
    assert_eq!(sc.run, ekiben_core::RunMode::Once);
}
