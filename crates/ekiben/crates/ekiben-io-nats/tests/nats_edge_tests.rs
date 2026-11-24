//! Edge cases of the in-memory NATS cluster and of the core `nats` input, output and
//! `nats_request_reply` processor on it: config refusals, queue groups, request-reply acks,
//! deliver policies, KV history and the organization boundaries (T-7685).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    BatchError, ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Part, ProcContext, Registry,
    ResourceManager, StreamContext, StreamMetrics,
};
use ekiben_io_nats::broker_mem::InMemoryNatsCluster;
use serde_json::json;
use tokio::sync::watch;

struct Ctx {
    reg: Arc<Registry>,
    sctx: StreamContext,
    pctx: ProcContext,
    stop: watch::Sender<bool>,
}

fn ctx(org_id: &str, prefix: Option<&str>) -> Ctx {
    let reg = Arc::new(Registry::new());
    ekiben_io_nats::register_all(&reg);
    let resources = ResourceManager::new();
    let oid = OrgId::new(org_id);
    resources.set_org_quotas(
        &oid,
        OrgQuotas {
            nats_subject_prefix: prefix.map(String::from),
            ..Default::default()
        },
    );
    let org = resources.get_or_create_org(&oid);
    let (stop, rx) = watch::channel(false);
    let sctx = StreamContext {
        stream_id: "edge".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("edge", org_id),
        label: "edge".to_string(),
        path: "root".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    let pctx = sctx.to_proc_context("nats_request_reply", "rr", "pipeline.processors.0");
    Ctx {
        reg,
        sctx,
        pctx,
        stop,
    }
}

fn conf(v: serde_json::Value) -> ComponentConfig {
    ComponentConfig::from_json_value(&v, "drop", None).unwrap()
}

fn text(p: &Part) -> String {
    String::from_utf8_lossy(&p.as_bytes_ref()).into_owned()
}

async fn input(c: &Ctx, v: serde_json::Value) -> Box<dyn Input> {
    let mut i = c.reg.build_input(&conf(v), &c.sctx).unwrap();
    i.connect(&c.sctx).await.unwrap();
    i
}

async fn read(i: &mut Box<dyn Input>, c: &Ctx) -> (Part, ekiben_core::AckResponder) {
    let (mut b, ack) = tokio::time::timeout(Duration::from_secs(5), i.read_batch(&c.sctx))
        .await
        .expect("a message within 5 s")
        .unwrap();
    (b.remove(0), ack)
}

/// The refusal of building `kind` (`nats`, `nats_jetstream_in` and the other `_in` names are
/// inputs, `_out` names and `nats_jetstream`/`nats_kv` outputs, the rest processors).
fn build_err(c: &Ctx, kind: &str, v: serde_json::Value) -> String {
    let (role, name) = match kind {
        "nats" => ("in", "nats"),
        "nats_jetstream_in" => ("in", "nats_jetstream"),
        "nats_out" => ("out", "nats"),
        "nats_jetstream" | "nats_kv" => ("out", kind),
        "nats_kv_proc" => ("proc", "nats_kv"),
        other => ("proc", other),
    };
    let conf = conf(json!({ name: v }));
    let err = match role {
        "in" => c
            .reg
            .build_input(&conf, &c.sctx)
            .err()
            .map(|e| e.to_string()),
        "out" => c
            .reg
            .build_output(&conf, &c.sctx)
            .err()
            .map(|e| e.to_string()),
        _ => c
            .reg
            .build_processor(&conf, &c.pctx, "0")
            .err()
            .map(|e| e.to_string()),
    };
    err.unwrap_or_else(|| panic!("{kind} built"))
}

// ---------- core nats on the memory cluster ----------

#[tokio::test]
async fn core_output_reaches_matching_subscribers_and_one_member_per_queue_group() {
    let c = ctx("org-core", None);
    let url = "nats://memory/core_fanout";
    let mut plain = input(&c, json!({"nats": {"urls": [url], "subject": "city.>"}})).await;
    let mut q1 = input(
        &c,
        json!({"nats": {"urls": [url], "subject": "city.*", "queue": "workers"}}),
    )
    .await;
    let mut q2 = input(
        &c,
        json!({"nats": {"urls": [url], "subject": "city.*", "queue": "workers"}}),
    )
    .await;
    assert_eq!(plain.input_type(), "nats");
    assert!(plain.is_connected());
    // Connecting again keeps the subscription.
    plain.connect(&c.sctx).await.unwrap();
    let out = c
        .reg
        .build_output(
            &conf(json!({"nats": {"urls": url, "subject": "city.${! meta(\"zone\") }",
                "headers": {"X-Src": "edge", "ignored": 5}, "metadata": {"include_prefixes": ["trace"]}}})),
            &c.sctx,
        )
        .unwrap();
    assert_eq!(out.output_type(), "nats");
    out.connect(&c.sctx).await.unwrap();
    let mut a = Part::from_str("one");
    a.meta_set_str("zone", "north");
    a.meta_set_str("trace_id", "t-1");
    let mut b = Part::from_str("two");
    b.meta_set_str("zone", "south");
    out.write_batch(&c.sctx, vec![a, b]).await.unwrap();
    let (p1, ack) = read(&mut plain, &c).await;
    // A message without a reply subject is acked with nothing to answer.
    ack.ack();
    let (p2, _) = read(&mut plain, &c).await;
    assert_eq!(
        (text(&p1), text(&p2)),
        ("one".to_string(), "two".to_string())
    );
    assert_eq!(p1.meta_get_str("nats_subject"), "city.north");
    assert_eq!(p1.meta_get_str("X-Src"), "edge");
    assert_eq!(p1.meta_get_str("trace_id"), "t-1");
    assert_eq!(p1.meta_get_str("nats_reply_subject"), "");
    // The queue group splits the two messages between its members.
    let (g1, _) = read(&mut q1, &c).await;
    let (g2, _) = read(&mut q2, &c).await;
    let mut got = [text(&g1), text(&g2)];
    got.sort();
    assert_eq!(got, ["one", "two"]);
    plain.close().await.unwrap();
    assert!(!plain.is_connected());
    assert!(matches!(
        plain.read_batch(&c.sctx).await,
        Err(CoreError::NotConnected)
    ));
}

#[tokio::test]
async fn a_core_input_stops_reading_when_the_stream_stops() {
    let c = ctx("org-stop", None);
    let mut i = input(
        &c,
        json!({"nats": {"urls": ["nats://memory/core_stop"], "subject": "x"}}),
    )
    .await;
    c.stop.send(true).unwrap();
    assert!(matches!(
        i.read_batch(&c.sctx).await,
        Err(CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn request_reply_gets_the_response_the_input_stored_or_its_ack() {
    let c = ctx("org-rr", None);
    let url = "nats://memory/core_rr";
    let mut responder = input(&c, json!({"nats": {"urls": [url], "subject": "svc.echo"}})).await;
    let rr = c
        .reg
        .build_processor(
            &conf(
                json!({"nats_request_reply": {"urls": [url], "subject": "svc.echo", "timeout": "2s",
                "headers": {"X-Ask": "1", "skip": 1}}}),
            ),
            &c.pctx,
            "0",
        )
        .unwrap();
    assert_eq!(rr.processor_type(), "nats_request_reply");
    let pctx = c.pctx.clone();
    let rr2 = rr.clone();
    let asking =
        tokio::spawn(async move { rr2.process_batch(&pctx, vec![Part::from_str("ping")]).await });
    let (msg, ack) = read(&mut responder, &c).await;
    assert_eq!(text(&msg), "ping");
    assert_eq!(msg.meta_get_str("X-Ask"), "1");
    assert!(!msg.meta_get_str("nats_reply_subject").is_empty());
    // A pipeline response stored on the message is the reply.
    let mut response = Part::from_str("pong");
    response.meta_set_str("served_by", "edge");
    msg.sync_response_store().unwrap().add(&[response]);
    ack.ack();
    let out = asking.await.unwrap().unwrap();
    assert_eq!(text(&out[0][0]), "pong");
    assert_eq!(out[0][0].meta_get_str("served_by"), "edge");
    // An empty stored response, or none, makes the input answer `+ACK`.
    let pctx = c.pctx.clone();
    let rr_empty = rr.clone();
    let asking = tokio::spawn(async move {
        rr_empty
            .process_batch(&pctx, vec![Part::from_str("ping")])
            .await
    });
    let (msg, ack) = read(&mut responder, &c).await;
    msg.sync_response_store().unwrap().add(&[]);
    ack.ack();
    assert_eq!(text(&asking.await.unwrap().unwrap()[0][0]), "+ACK");
    let pctx = c.pctx.clone();
    let rr3 = rr.clone();
    let asking =
        tokio::spawn(async move { rr3.process_batch(&pctx, vec![Part::from_str("ping")]).await });
    let (_, ack) = read(&mut responder, &c).await;
    ack.ack();
    let out = asking.await.unwrap().unwrap();
    assert_eq!(text(&out[0][0]), "+ACK");
}

#[tokio::test]
async fn request_reply_times_out_when_the_responder_nacks_or_sends_no_ack() {
    let c = ctx("org-rr-quiet", None);
    let url = "nats://memory/core_rr_quiet";
    let mut quiet = input(
        &c,
        json!({"nats": {"urls": [url], "subject": "svc.q", "send_ack": false, "nak_delay": "1s"}}),
    )
    .await;
    let rr = c
        .reg
        .build_processor(
            &conf(json!({"nats_request_reply": {"urls": [url], "subject": "svc.q", "timeout": "100ms"}})),
            &c.pctx,
            "0",
        )
        .unwrap();
    let pctx = c.pctx.clone();
    let rr2 = rr.clone();
    let asking = tokio::spawn(async move {
        rr2.process_batch(&pctx, vec![Part::from_str("a"), Part::from_str("b")])
            .await
    });
    let (m1, ack1) = read(&mut quiet, &c).await;
    ack1.ack();
    let (_, ack2) = read(&mut quiet, &c).await;
    ack2.nack(BatchError::new(vec![m1], "refused"));
    let out = asking.await.unwrap().unwrap();
    for p in &out[0] {
        assert_eq!(
            p.error_get()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("timed out"),
            true,
            "{p:?}"
        );
    }
    // A subject that does not interpolate marks the message failed instead of asking.
    let broken = c
        .reg
        .build_processor(
            &conf(json!({"nats_request_reply": {"urls": [url], "subject": "${! throw(\"no subject\") }"}})),
            &c.pctx,
            "0",
        )
        .unwrap();
    let out = broken
        .process_batch(&c.pctx, vec![Part::from_str("x")])
        .await
        .unwrap();
    assert!(out[0][0].is_errored());
}

#[tokio::test]
async fn core_components_stay_inside_the_organization_subject_prefix() {
    let c = ctx("org-prefixed", Some("org_p"));
    let url = "nats://memory/core_prefix";
    assert!(
        build_err(&c, "nats", json!({"urls": [url], "subject": "other.x"}))
            .contains("outside organization prefix")
    );
    let out = c
        .reg
        .build_output(
            &conf(json!({"nats": {"urls": [url], "subject": "${! meta(\"to\") }"}})),
            &c.sctx,
        )
        .unwrap();
    let mut p = Part::from_str("x");
    p.meta_set_str("to", "other.x");
    let err = out.write_batch(&c.sctx, vec![p]).await.unwrap_err();
    assert!(
        err.to_string().contains("outside organization prefix"),
        "{err}"
    );
    let rr = c
        .reg
        .build_processor(
            &conf(json!({"nats_request_reply": {"urls": [url], "subject": "other.x"}})),
            &c.pctx,
            "0",
        )
        .unwrap();
    // The observed processor marks the message failed instead of asking.
    let out = rr
        .process_batch(&c.pctx, vec![Part::from_str("x")])
        .await
        .unwrap();
    let err = out[0][0]
        .error_get()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(err.contains("outside organization prefix"), "{err}");
}

#[tokio::test]
async fn two_organizations_on_one_memory_url_never_see_each_other() {
    let a = ctx("org-iso-a", None);
    let b = ctx("org-iso-b", None);
    let url = "nats://memory/shared_name";
    let mut b_in = input(&b, json!({"nats": {"urls": [url], "subject": ">"}})).await;
    let a_out = a
        .reg
        .build_output(
            &conf(json!({"nats": {"urls": [url], "subject": "secret.data"}})),
            &a.sctx,
        )
        .unwrap();
    a_out
        .write_batch(&a.sctx, vec![Part::from_str("org a only")])
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_millis(200), b_in.read_batch(&b.sctx)).await;
    assert!(got.is_err(), "org b read org a's message");
}

#[tokio::test]
async fn core_and_jetstream_configs_are_refused_with_the_field_named() {
    let c = ctx("org-conf", None);
    let url = "nats://memory/conf";
    for (kind, v, want) in [
        (
            "nats",
            json!({"urls": [url]}),
            "nats input requires a 'subject' field",
        ),
        (
            "nats",
            json!({"urls": [url], "subject": ""}),
            "nats input requires a 'subject' field",
        ),
        (
            "nats",
            json!({"urls": [url], "subject": "s", "prefetch_count": -1}),
            "prefetch count must be greater than or equal to zero",
        ),
        (
            "nats",
            json!({"urls": [url], "subject": "s", "nak_delay": "soon"}),
            "soon",
        ),
        ("nats", json!({"subject": "s"}), "at least one URL"),
        (
            "nats_out",
            json!({"urls": [url]}),
            "nats output requires a 'subject' field",
        ),
        (
            "nats_out",
            json!({"urls": [url], "subject": "${! "}),
            "does not parse",
        ),
        (
            "nats_jetstream",
            json!({"urls": [url]}),
            "nats_jetstream output requires a 'subject' field",
        ),
        (
            "nats_jetstream",
            json!({"urls": [url], "subject": "s", "max_in_flight": 0}),
            "max_in_flight must be a positive integer",
        ),
        (
            "nats_jetstream",
            json!({"urls": [url], "subject": "s", "max_in_flight": "many"}),
            "max_in_flight must be a positive integer",
        ),
        (
            "nats_jetstream_in",
            json!({"urls": [url], "subject": "s", "deliver": "sometimes"}),
            "deliver option sometimes was not recognised",
        ),
        (
            "nats_jetstream_in",
            json!({"urls": [url], "bind": true}),
            "stream or durable is required, when bind is true",
        ),
        (
            "nats_jetstream_in",
            json!({"urls": [url]}),
            "subject is empty",
        ),
        (
            "nats_jetstream_in",
            json!({"urls": [url], "subject": "s", "ack_wait": "later"}),
            "failed to parse ack wait duration",
        ),
        (
            "nats_kv",
            json!({"urls": [url], "key": "k"}),
            "nats_kv output requires a 'bucket' field",
        ),
        (
            "nats_kv",
            json!({"urls": [url], "bucket": "b"}),
            "nats_kv output requires a 'key' field",
        ),
        (
            "nats_request_reply",
            json!({"urls": [url]}),
            "nats_request_reply requires a 'subject' field",
        ),
        (
            "nats_kv_proc",
            json!({"urls": [url], "operation": "get", "key": "k"}),
            "nats_kv requires a 'bucket' field",
        ),
        (
            "nats_kv_proc",
            json!({"urls": [url], "bucket": "b", "key": "k"}),
            "nats_kv requires an 'operation' field",
        ),
        (
            "nats_kv_proc",
            json!({"urls": [url], "bucket": "b", "operation": "get", "key": ""}),
            "nats_kv 'key' must be set to a non-empty string",
        ),
        (
            "nats_kv_proc",
            json!({"urls": [url], "bucket": "b", "operation": "update", "key": "k"}),
            "'revision' must be set when operation is 'update'",
        ),
        (
            "nats_kv_proc",
            json!({"urls": [url], "bucket": "b", "operation": "get", "key": "k", "revision": "3"}),
            "'revision' cannot be set when operation is 'get'",
        ),
    ] {
        let err = build_err(&c, kind, v.clone());
        assert!(err.contains(want), "{kind} {v}: {err}");
    }
    let cache = c
        .reg
        .build_cache(&conf(json!({"nats_kv": {"urls": [url]}})), &c.sctx)
        .err()
        .unwrap();
    assert!(
        cache
            .to_string()
            .contains("nats_kv cache requires a 'bucket' field"),
        "{cache}"
    );
}

#[tokio::test]
async fn outputs_report_subject_and_header_interpolation_failures() {
    let c = ctx("org-interp", None);
    let url = "nats://memory/interp";
    for kind in ["nats", "nats_jetstream"] {
        let bad_subject = c
            .reg
            .build_output(
                &conf(json!({ kind: {"urls": [url], "subject": "${! throw(\"s\") }"}})),
                &c.sctx,
            )
            .unwrap();
        let err = bad_subject
            .write_batch(&c.sctx, vec![Part::from_str("x")])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("interpolation"), "{kind}: {err}");
        let bad_header = c
            .reg
            .build_output(&conf(json!({ kind: {"urls": [url], "subject": "s", "headers": {"H": "${! throw(\"h\") }"}}})), &c.sctx)
            .unwrap();
        let err = bad_header
            .write_batch(&c.sctx, vec![Part::from_str("x")])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("header"), "{kind}: {err}");
    }
    let kv = c
        .reg
        .build_output(
            &conf(json!({"nats_kv": {"urls": [url], "bucket": "b", "key": "${! throw(\"k\") }"}})),
            &c.sctx,
        )
        .unwrap();
    assert_eq!(kv.output_type(), "nats_kv");
    assert!(kv
        .write_batch(&c.sctx, vec![Part::from_str("x")])
        .await
        .is_err());
}

// ---------- jetstream on the memory cluster ----------

#[tokio::test]
async fn deliver_new_and_last_start_where_they_say() {
    let c = ctx("org-deliver", None);
    let url = "nats://memory/deliver";
    let out = c
        .reg
        .build_output(
            &conf(json!({"nats_jetstream": {"urls": [url], "subject": "ev.${! content() }"}})),
            &c.sctx,
        )
        .unwrap();
    assert_eq!(out.output_type(), "nats_jetstream");
    out.write_batch(&c.sctx, vec![Part::from_str("1"), Part::from_str("2")])
        .await
        .unwrap();
    let mut last = input(&c, json!({"nats_jetstream": {"urls": [url], "subject": "ev.*", "durable": "last", "deliver": "last"}})).await;
    let mut new = input(&c, json!({"nats_jetstream": {"urls": [url], "subject": "ev.*", "durable": "new", "deliver": "new"}})).await;
    assert_eq!(new.input_type(), "nats_jetstream");
    let (p, ack) = read(&mut last, &c).await;
    assert_eq!(text(&p), "2");
    assert_eq!(p.meta_get_str("nats_domain"), "ekiben");
    ack.ack();
    // `new` waits for the next message; it arrives while the read waits.
    let c_out = out.clone();
    let sctx = c.sctx.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c_out.write_batch(&sctx, vec![Part::from_str("3")]).await
    });
    let (p, _) = read(&mut new, &c).await;
    assert_eq!(text(&p), "3");
    writer.await.unwrap().unwrap();
    assert!(new.is_connected());
    new.close().await.unwrap();
    assert!(!new.is_connected());
    assert!(matches!(
        new.read_batch(&c.sctx).await,
        Err(CoreError::NotConnected)
    ));
    let (p, _) = read(&mut last, &c).await;
    assert_eq!(text(&p), "3");
    // Nothing left and the stream stopping: the waiting read ends.
    c.stop.send(true).unwrap();
    assert!(matches!(
        last.read_batch(&c.sctx).await,
        Err(CoreError::EndOfInput)
    ));
}

#[test]
fn the_memory_cluster_pulls_acks_and_redelivers_by_consumer() {
    let cl = InMemoryNatsCluster::get("edge-test\nnats://memory/pull");
    cl.publish_jetstream("a.1".into(), BTreeMap::new(), Bytes::from_static(b"x"));
    cl.publish_jetstream("b.1".into(), BTreeMap::new(), Bytes::from_static(b"y"));
    cl.ensure_consumer("c", "b.*", "all", 1);
    // Ensuring it again keeps its position.
    cl.ensure_consumer("c", "b.*", "new", 1);
    // Unknown consumers and sequences change nothing.
    assert!(cl.try_pull_jetstream("ghost").is_none());
    cl.ack_jetstream("ghost", 1);
    cl.nak_jetstream("ghost", 1);
    let first = cl.try_pull_jetstream("c").unwrap();
    assert_eq!(first.msg.subject, "b.1");
    // max_ack_pending 1: nothing more until the ack.
    assert!(cl.try_pull_jetstream("c").is_none());
    cl.nak_jetstream("c", 99);
    cl.nak_jetstream("c", first.msg.stream_seq);
    let again = cl.try_pull_jetstream("c").unwrap();
    assert_eq!(
        (again.num_delivered, again.msg.subject.as_str()),
        (2, "b.1")
    );
    cl.ack_jetstream("c", again.msg.stream_seq);
    // The rest of the log holds no message of the filter.
    assert!(cl.try_pull_jetstream("c").is_none());
}

#[tokio::test]
async fn a_waiter_wakes_on_jetstream_activity() {
    let cl = InMemoryNatsCluster::get("edge-test\nnats://memory/wake");
    let waiter = cl.clone();
    let wait = tokio::spawn(async move { waiter.wait_for_js_activity().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cl.publish_jetstream("w".into(), BTreeMap::new(), Bytes::new());
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn the_memory_kv_keeps_history_and_refuses_stale_writes() {
    let cl = InMemoryNatsCluster::get("edge-test\nnats://memory/kv");
    let not_found = |r: Result<_, CoreError>| matches!(r, Err(CoreError::KeyNotFound(_)));
    assert!(not_found(cl.kv_get("nobucket", "k")));
    assert!(not_found(cl.kv_get_revision("nobucket", "k", 1)));
    let r1 = cl.kv_create("b", "k", Bytes::from_static(b"v1")).unwrap();
    assert!(matches!(
        cl.kv_create("b", "k", Bytes::new()),
        Err(CoreError::KeyAlreadyExists(_))
    ));
    assert!(not_found(cl.kv_get("b", "other")));
    assert!(not_found(cl.kv_get_revision("b", "other", r1)));
    let err = cl.kv_update("b", "k", Bytes::new(), r1 + 5).unwrap_err();
    assert!(err.to_string().contains("revision mismatch"), "{err}");
    let r2 = cl
        .kv_update("b", "k", Bytes::from_static(b"v2"), r1)
        .unwrap();
    assert_eq!(&cl.kv_get_revision("b", "k", r1).unwrap().value[..], b"v1");
    let missing_rev = cl.kv_get_revision("b", "k", 999).unwrap_err();
    assert!(missing_rev.to_string().contains("k@999"), "{missing_rev}");
    cl.kv_put("b", "k2", Bytes::from_static(b"x"));
    let mut keys = cl.kv_keys("b", ">");
    keys.sort();
    assert_eq!(keys, ["k", "k2"]);
    let r3 = cl.kv_delete("b", "k");
    assert!(r3 > r2);
    assert!(not_found(cl.kv_get("b", "k")));
    assert_eq!(cl.kv_keys("b", ">"), ["k2"]);
    // A deleted key may be created again.
    cl.kv_create("b", "k", Bytes::from_static(b"v3")).unwrap();
    let ops: Vec<&str> = cl
        .kv_history("b", "k")
        .iter()
        .map(|e| e.operation)
        .collect();
    assert_eq!(
        ops,
        [
            "KeyValuePutOp",
            "KeyValuePutOp",
            "KeyValueDeleteOp",
            "KeyValuePutOp"
        ]
    );
    cl.kv_purge("b", "k");
    let ops: Vec<&str> = cl
        .kv_history("b", "k")
        .iter()
        .map(|e| e.operation)
        .collect();
    assert_eq!(ops, ["KeyValuePurgeOp"]);
    assert!(cl.kv_history("nobucket", ">").is_empty());
    assert!(cl.kv_keys("nobucket", ">").is_empty());
    assert!(cl.kv_keys("b", "z.*").is_empty());
}

#[tokio::test]
async fn the_kv_processor_reads_revisions_updates_and_reports_missing_keys() {
    let c = ctx("org-kvp", None);
    let url = "nats://memory/kvp";
    let proc = |op: &str, extra: serde_json::Value| {
        let mut v =
            json!({"urls": [url], "bucket": "bk", "operation": op, "key": "${! meta(\"k\") }"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        c.reg
            .build_processor(&conf(json!({ "nats_kv": v })), &c.pctx, "0")
            .unwrap()
    };
    let msg = |k: &str, body: &str, rev: Option<&str>| {
        let mut p = Part::from_str(body);
        p.meta_set_str("k", k);
        if let Some(r) = rev {
            p.meta_set_str("rev", r);
        }
        p
    };
    let rev_field = json!({"revision": "${! meta(\"rev\") }"});
    let create = proc("create", json!({}));
    assert_eq!(create.processor_type(), "nats_kv");
    let out = create
        .process_batch(&c.pctx, vec![msg("a", "v1", None)])
        .await
        .unwrap();
    let r1 = out[0][0].meta_get_str("nats_kv_revision");
    // Creating it again fails with nats.go's key-exists text.
    let out = create
        .process_batch(&c.pctx, vec![msg("a", "v1", None)])
        .await
        .unwrap();
    assert!(out[0][0].is_errored());
    let update = proc("update", rev_field.clone());
    let out = update
        .process_batch(
            &c.pctx,
            vec![
                msg("a", "v2", Some(&r1)),
                msg("a", "v3", Some("not a number")),
            ],
        )
        .await
        .unwrap();
    assert!(!out[0][0].is_errored());
    // A revision that does not parse is revision 0: a stale write.
    assert!(out[0][1].is_errored());
    let get_rev = proc("get_revision", rev_field);
    let out = get_rev
        .process_batch(
            &c.pctx,
            vec![msg("a", "", Some(&r1)), msg("a", "", Some("x"))],
        )
        .await
        .unwrap();
    assert_eq!(text(&out[0][0]), "v1");
    let err = out[0][1]
        .error_get()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(err.contains("key not found"), "{err}");
    let purge = proc("purge", json!({}));
    purge
        .process_batch(&c.pctx, vec![msg("a", "", None)])
        .await
        .unwrap();
    let get = proc("get", json!({}));
    let out = get
        .process_batch(&c.pctx, vec![msg("a", "", None)])
        .await
        .unwrap();
    let err = out[0][0]
        .error_get()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert_eq!(err, "nats: key not found");
}
