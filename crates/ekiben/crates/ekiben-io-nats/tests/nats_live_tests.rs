//! `nats_jetstream` and `nats_kv` against a real `nats-server -js` in docker. Each test prints
//! a SKIP line and passes when docker is not available.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    BatchError, ComponentConfig, Input, OrgId, OrgQuotas, Part, ProcContext, Registry,
    ResourceManager, StreamContext, StreamMetrics,
};
use tokio::sync::watch;

const IMAGE: &str = "nats:2.12-alpine";

/// A `nats-server` container removed on drop.
struct Server {
    id: String,
    url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.id]).output();
    }
}

fn start_server(test: &str, extra: &[&str], mounts: &[(&str, &str)]) -> Option<Server> {
    let ok = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("SKIP {test}: docker is not available");
        return None;
    }
    let mut args = vec!["run", "-d", "--rm", "-p", "127.0.0.1::4222"];
    let vols: Vec<String> = mounts.iter().map(|(h, c)| format!("{h}:{c}:ro")).collect();
    for v in &vols {
        args.extend(["-v", v.as_str()]);
    }
    args.push(IMAGE);
    args.push("-js");
    args.extend(extra);
    let out = Command::new("docker").args(&args).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "SKIP {test}: docker run {IMAGE} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let port = Command::new("docker")
        .args(["port", &id, "4222"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())?;
    let port = port.lines().next()?.rsplit(':').next()?.trim().to_string();
    let server = Server {
        id,
        url: format!("nats://127.0.0.1:{port}"),
    };
    for _ in 0..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            std::thread::sleep(Duration::from_millis(200));
            return Some(server);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("nats-server did not listen on {port}");
}

fn make_ctx(org_id: &str, prefix: Option<&str>) -> (Arc<Registry>, StreamContext, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_nats::register_all(&reg);
    let resources = ResourceManager::new();
    let oid = OrgId::new(org_id);
    resources.set_org_quotas(
        &oid,
        OrgQuotas {
            nats_subject_prefix: prefix.map(String::from),
            // The test server listens on loopback, which the default outbound rule refuses.
            allow_private_ips: true,
            ..Default::default()
        },
    );
    let org = resources.get_or_create_org(&oid);
    let (_tx, rx) = watch::channel(false);
    std::mem::forget(_tx);
    let sctx = StreamContext {
        stream_id: "nats-live".to_string(),
        org,
        resources: resources.clone(),
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("nats-live", org_id),
        label: "nats_live".to_string(),
        path: "root".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    let pctx = sctx.to_proc_context("nats_kv", "kv", "pipeline.processors.0");
    (reg, sctx, pctx)
}

fn conf(value: serde_json::Value) -> ComponentConfig {
    ComponentConfig::from_json_value(&value, "drop", None).unwrap()
}

async fn js(url: &str) -> jetstream::Context {
    jetstream::new(async_nats::connect(url).await.unwrap())
}

async fn add_stream(url: &str, name: &str, subjects: &[&str]) {
    js(url)
        .await
        .create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn input(reg: &Registry, sctx: &StreamContext, v: serde_json::Value) -> Box<dyn Input> {
    let mut i = reg.build_input(&conf(v), sctx).unwrap();
    i.connect(sctx).await.unwrap();
    i
}

async fn read(i: &mut Box<dyn Input>, sctx: &StreamContext) -> (Part, ekiben_core::AckResponder) {
    let (mut b, ack) = tokio::time::timeout(Duration::from_secs(10), i.read_batch(sctx))
        .await
        .expect("a message within 10 s")
        .unwrap();
    (b.remove(0), ack)
}

#[tokio::test]
async fn jetstream_output_then_durable_input_with_metadata_nak_redelivery_and_consumer_config() {
    let Some(srv) = start_server("jetstream_durable", &[], &[]) else {
        return;
    };
    let (reg, sctx, _) = make_ctx("org-a", Some("org_a"));
    add_stream(&srv.url, "ORDERS", &["org_a.orders.>"]).await;

    let out = reg
        .build_output(
            &conf(serde_json::json!({"nats_jetstream": {
                "urls": [srv.url],
                "subject": "org_a.orders.${! json(\"type\") }",
                "headers": {"X-Source": "ekiben"},
                "metadata": {"include_prefixes": ["trace_"]}
            }})),
            &sctx,
        )
        .unwrap();
    out.connect(&sctx).await.unwrap();
    let mut m1 = Part::from_str(r#"{"id":1,"type":"created"}"#);
    m1.meta_set_str("trace_id", "tr-1");
    out.write_batch(
        &sctx,
        vec![m1, Part::from_str(r#"{"id":2,"type":"updated"}"#)],
    )
    .await
    .unwrap();

    let mut i = input(
        &reg,
        &sctx,
        serde_json::json!({"nats_jetstream": {
            "urls": [srv.url], "subject": "org_a.orders.*", "durable": "orders",
            "ack_wait": "5s", "max_ack_pending": 7, "deliver": "all"
        }}),
    )
    .await;
    let (p1, ack1) = read(&mut i, &sctx).await;
    assert_eq!(p1.meta_get_str("nats_subject"), "org_a.orders.created");
    assert_eq!(p1.meta_get_str("nats_sequence_stream"), "1");
    assert_eq!(p1.meta_get_str("nats_num_delivered"), "1");
    assert_eq!(p1.meta_get_str("X-Source"), "ekiben");
    assert_eq!(p1.meta_get_str("trace_id"), "tr-1");
    assert!(!p1.meta_get_str("nats_timestamp_unix_nano").is_empty());
    ack1.nack(BatchError::new(vec![p1], "downstream failed"));

    let mut seen = HashMap::new();
    for _ in 0..2 {
        let (p, ack) = read(&mut i, &sctx).await;
        seen.insert(
            p.meta_get_str("nats_sequence_stream").to_string(),
            p.meta_get_str("nats_num_delivered").to_string(),
        );
        ack.ack();
    }
    assert_eq!(
        seen.get("1").map(String::as_str),
        Some("2"),
        "Nak redelivers"
    );
    assert_eq!(seen.get("2").map(String::as_str), Some("1"));

    let info = js(&srv.url)
        .await
        .get_stream("ORDERS")
        .await
        .unwrap()
        .consumer_info("orders")
        .await
        .unwrap();
    assert_eq!(info.config.ack_wait, Duration::from_secs(5));
    assert_eq!(info.config.max_ack_pending, 7);
    assert_eq!(info.config.filter_subject, "org_a.orders.*");
    assert!(
        info.config.deliver_subject.is_none(),
        "durable without queue is a pull consumer"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let info = js(&srv.url)
        .await
        .get_stream("ORDERS")
        .await
        .unwrap()
        .consumer_info("orders")
        .await
        .unwrap();
    assert_eq!(info.num_ack_pending, 0, "every message acked");
}

/// Two instances on one durable (pull) and two on one queue (push with deliver group) each
/// process every message exactly once between them.
#[tokio::test]
async fn jetstream_instances_share_a_durable_and_a_queue_group() {
    let Some(srv) = start_server("jetstream_share", &[], &[]) else {
        return;
    };
    let (reg, sctx, _) = make_ctx("org-a", Some("org_a"));
    add_stream(&srv.url, "WORK", &["org_a.work.>"]).await;
    let ctx = js(&srv.url).await;
    for (round, (label, cfg)) in [
        ("durable", serde_json::json!({"durable": "shared"})),
        ("queue", serde_json::json!({"queue": "workers"})),
    ]
    .into_iter()
    .enumerate()
    {
        // deliver new: each round reads only what it publishes after both instances joined.
        let mut v =
            serde_json::json!({"urls": [srv.url], "subject": "org_a.work.>", "deliver": "new"});
        v.as_object_mut()
            .unwrap()
            .extend(cfg.as_object().unwrap().clone());
        let mut a = input(
            &reg,
            &sctx,
            serde_json::json!({"nats_jetstream": v.clone()}),
        )
        .await;
        let mut b = input(&reg, &sctx, serde_json::json!({"nats_jetstream": v})).await;
        for n in 0..2000 {
            ctx.publish(
                format!("org_a.work.{}", n % 3),
                format!("{round}-{n}").into(),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        }
        let mut got: Vec<String> = Vec::new();
        let (mut from_a, mut from_b) = (0, 0);
        while got.len() < 2000 {
            tokio::select! {
                r = a.read_batch(&sctx) => { let (p, ack) = r.unwrap(); got.push(String::from_utf8_lossy(&p[0].as_bytes_ref()).into()); ack.ack(); from_a += 1; }
                r = b.read_batch(&sctx) => { let (p, ack) = r.unwrap(); got.push(String::from_utf8_lossy(&p[0].as_bytes_ref()).into()); ack.ack(); from_b += 1; }
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("{label}: stalled at {}", got.len()),
            }
        }
        let unique: HashSet<_> = got.iter().collect();
        assert_eq!(unique.len(), 2000, "{label}: every message once");
        assert!(
            from_a > 0 && from_b > 0,
            "{label}: both instances worked ({from_a}/{from_b})"
        );
        // Nothing more arrives: no duplicates once all are acked.
        let extra = tokio::time::timeout(Duration::from_millis(800), a.read_batch(&sctx)).await;
        assert!(extra.is_err(), "{label}: no redelivery after acks");
    }
}

#[tokio::test]
async fn jetstream_deliver_policies_match_bento() {
    let Some(srv) = start_server("jetstream_deliver", &[], &[]) else {
        return;
    };
    let (reg, sctx, _) = make_ctx("default", None);
    add_stream(&srv.url, "D", &["d.>"]).await;
    let ctx = js(&srv.url).await;
    for (s, v) in [("d.x", "1"), ("d.y", "2"), ("d.x", "3")] {
        ctx.publish(s, Bytes::from(v)).await.unwrap().await.unwrap();
    }
    for (deliver, want) in [("all", "1"), ("last", "3"), ("last_per_subject", "2")] {
        let mut i = input(
            &reg,
            &sctx,
            serde_json::json!({"nats_jetstream": {"urls": [srv.url], "subject": "d.>", "deliver": deliver}}),
        )
        .await;
        let (p, ack) = read(&mut i, &sctx).await;
        ack.ack();
        assert_eq!(&p.as_bytes_ref()[..], want.as_bytes(), "deliver {deliver}");
    }
    let mut i = input(
        &reg,
        &sctx,
        serde_json::json!({"nats_jetstream": {"urls": [srv.url], "subject": "d.>", "deliver": "new"}}),
    )
    .await;
    ctx.publish("d.z", "4".into()).await.unwrap().await.unwrap();
    let (p, _) = read(&mut i, &sctx).await;
    assert_eq!(
        &p.as_bytes_ref()[..],
        b"4",
        "deliver new skips stored messages"
    );
}

/// Binding to an existing consumer: its filter subject decides, and one organization can
/// never bind to a consumer or stream that reads another organization's subjects.
#[tokio::test]
async fn jetstream_bind_reads_an_existing_consumer_and_refuses_another_orgs() {
    let Some(srv) = start_server("jetstream_bind", &[], &[]) else {
        return;
    };
    add_stream(&srv.url, "B", &["org_b.>"]).await;
    let stream = js(&srv.url).await.get_stream("B").await.unwrap();
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("bpull".into()),
            filter_subject: "org_b.>".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    js(&srv.url)
        .await
        .publish("org_b.secret", "s".into())
        .await
        .unwrap()
        .await
        .unwrap();

    let (reg_b, sctx_b, _) = make_ctx("org-b", Some("org_b"));
    let mut i = input(
        &reg_b,
        &sctx_b,
        serde_json::json!({"nats_jetstream": {"urls": [srv.url], "bind": true, "stream": "B", "durable": "bpull"}}),
    )
    .await;
    let (p, ack) = read(&mut i, &sctx_b).await;
    ack.ack();
    assert_eq!(p.meta_get_str("nats_subject"), "org_b.secret");

    let (reg_a, sctx_a, _) = make_ctx("org-a", Some("org_a"));
    for v in [
        serde_json::json!({"bind": true, "stream": "B", "durable": "bpull"}),
        serde_json::json!({"bind": true, "stream": "B"}),
    ] {
        let mut v = v;
        v["urls"] = serde_json::json!([srv.url]);
        let mut i = reg_a
            .build_input(&conf(serde_json::json!({"nats_jetstream": v})), &sctx_a)
            .unwrap();
        let err = i.connect(&sctx_a).await.expect_err("org A refused");
        assert!(
            err.to_string().contains("outside organization prefix"),
            "{err}"
        );
    }
    assert!(reg_a
        .build_input(
            &conf(serde_json::json!({"nats_jetstream": {"urls": [srv.url], "subject": "org_b.>"}})),
            &sctx_a
        )
        .is_err());
    let missing = reg_b
        .build_input(
            &conf(serde_json::json!({"nats_jetstream": {"urls": [srv.url], "bind": true, "stream": "B", "durable": "nope"}})),
            &sctx_b,
        )
        .unwrap()
        .connect(&sctx_b)
        .await;
    assert!(missing.is_err(), "bind to a missing consumer fails");
}

#[tokio::test]
async fn nats_kv_cache_processor_and_output_on_a_real_bucket() {
    let Some(srv) = start_server("nats_kv", &[], &[]) else {
        return;
    };
    js(&srv.url)
        .await
        .create_key_value(jetstream::kv::Config {
            bucket: "org_a_kv".into(),
            history: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    let (reg, sctx, pctx) = make_ctx("org-a", Some("org_a"));
    let kv = serde_json::json!({"nats_kv": {"urls": [srv.url], "bucket": "org_a_kv"}});
    let cache = reg.build_cache(&conf(kv.clone()), &sctx).unwrap();
    cache
        .set("k1", Bytes::from_static(b"v1"), None)
        .await
        .unwrap();
    assert_eq!(cache.get("k1").await.unwrap(), Bytes::from_static(b"v1"));
    assert!(cache
        .add("k1", Bytes::from_static(b"x"), None)
        .await
        .is_err());
    cache.delete("k1").await.unwrap();
    assert!(
        !cache.exists("k1").await.unwrap(),
        "deleted key reads as missing"
    );

    let out = reg
        .build_output(
            &conf(serde_json::json!({"nats_kv": {"urls": [srv.url], "bucket": "org_a_kv", "key": "${! json(\"id\") }"}})),
            &sctx,
        )
        .unwrap();
    out.write_batch(
        &sctx,
        vec![
            Part::from_str(r#"{"id":"a"}"#),
            Part::from_str(r#"{"id":"a","n":2}"#),
        ],
    )
    .await
    .unwrap();

    let proc = |op: &str| {
        reg.build_processor(
            &conf(serde_json::json!({"nats_kv": {"urls": [srv.url], "bucket": "org_a_kv", "operation": op, "key": "a"}})),
            &pctx,
            "0",
        )
        .unwrap()
    };
    let got = proc("get")
        .process_batch(&pctx, vec![Part::from_str("{}")])
        .await
        .unwrap();
    let p = &got[0][0];
    assert_eq!(&p.as_bytes_ref()[..], br#"{"id":"a","n":2}"#);
    assert_eq!(p.meta_get_str("nats_kv_operation"), "KeyValuePutOp");
    assert_eq!(p.meta_get_str("nats_kv_bucket"), "org_a_kv");
    let hist = proc("history")
        .process_batch(&pctx, vec![Part::from_str("{}")])
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&hist[0][0].as_bytes_ref()).to_string();
    assert_eq!(text.matches("\"revision\"").count(), 2, "{text}");

    let (reg_b, sctx_b, _) = make_ctx("org-b", Some("org_b"));
    assert!(
        reg_b.build_cache(&conf(kv), &sctx_b).is_err(),
        "org B refused org A's bucket"
    );
}

fn openssl(args: &[&str], dir: &std::path::Path) {
    let st = Command::new("openssl")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
}

/// TLS: the server certificate is verified against `root_cas_file`; a wrong CA fails and
/// `skip_cert_verify: true` is refused at build.
#[tokio::test]
async fn jetstream_tls_verifies_the_server_certificate() {
    if Command::new("openssl").arg("version").output().is_err() {
        eprintln!("SKIP jetstream_tls: openssl is not available");
        return;
    }
    let dir = std::env::temp_dir().join(format!("ekiben-nats-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.as_path();
    for ca in ["ca", "other"] {
        openssl(
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                &format!("/CN={ca}"),
                "-keyout",
                &format!("{ca}.key"),
                "-out",
                &format!("{ca}.pem"),
            ],
            d,
        );
    }
    std::fs::write(
        d.join("ext.cnf"),
        "subjectAltName=IP:127.0.0.1,DNS:localhost\n",
    )
    .unwrap();
    openssl(
        &[
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ],
        d,
    );
    openssl(
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-days",
            "1",
            "-extfile",
            "ext.cnf",
            "-out",
            "server.pem",
        ],
        d,
    );
    for f in ["server.key", "server.pem"] {
        let mut perm = std::fs::metadata(d.join(f)).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o644);
        std::fs::set_permissions(d.join(f), perm).unwrap();
    }
    let dir_s = dir.to_string_lossy().to_string();
    let Some(srv) = start_server(
        "jetstream_tls",
        &[
            "--tls",
            "--tlscert",
            "/certs/server.pem",
            "--tlskey",
            "/certs/server.key",
        ],
        &[(&dir_s, "/certs")],
    ) else {
        return;
    };
    let url = srv.url.replace("nats://", "tls://");
    let (reg, sctx, _) = make_ctx("default", None);
    let build = |tls: serde_json::Value| {
        reg.build_output(
            &conf(serde_json::json!({"nats_jetstream": {"urls": [url], "subject": "t.x", "tls": tls}})),
            &sctx,
        )
    };
    let good =
        build(serde_json::json!({"enabled": true, "root_cas_file": d.join("ca.pem")})).unwrap();
    let res = good.connect(&sctx).await;
    assert!(res.is_ok(), "verified TLS connects: {res:?}");
    let ca_pem = std::fs::read_to_string(d.join("ca.pem")).unwrap();
    let inline = build(serde_json::json!({"enabled": true, "root_cas": ca_pem})).unwrap();
    assert!(
        inline.connect(&sctx).await.is_ok(),
        "inline root_cas connects"
    );
    let wrong =
        build(serde_json::json!({"enabled": true, "root_cas_file": d.join("other.pem")})).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), wrong.connect(&sctx))
            .await
            .map_or(true, |r| r.is_err()),
        "a server certificate from another CA is refused"
    );
    assert!(build(serde_json::json!({"enabled": true, "skip_cert_verify": true})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
