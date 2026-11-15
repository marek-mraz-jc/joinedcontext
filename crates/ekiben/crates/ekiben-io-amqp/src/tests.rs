use std::collections::HashMap;
use std::sync::Arc;

use ekiben_core::{OrgId, OrgQuotas, ResourceManager, StreamMetrics};
use lapin::types::{LongString, ShortString};
use serde_json::json;

use super::*;

fn conf(v: JsonValue) -> ComponentConfig {
    ComponentConfig {
        label: String::new(),
        component_type: "amqp_0_9".into(),
        config: v,
        processors: Vec::new(),
    }
}

fn refusal(v: JsonValue) -> String {
    match AmqpInput::from_config(&conf(v)) {
        Ok(_) => panic!("accepted"),
        Err(e) => e.to_string(),
    }
}

pub(crate) fn ctx(quotas: OrgQuotas) -> StreamContext {
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(&OrgId::new("org-amqp"), quotas);
    StreamContext {
        stream_id: "amqp-test".into(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("amqp-test", "org-amqp"),
        label: String::new(),
        path: "root.input".into(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

#[test]
fn config_refusals_name_the_field_and_never_the_password() {
    let base = |extra: JsonValue| {
        let mut v = json!({"urls": ["amqp://u:hunter2@rabbit.example.org:5672/"], "queue": "q"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    for (extra, want) in [
        (
            json!({"tls": {"skip_cert_verify": true}}),
            "skip_cert_verify",
        ),
        (
            json!({"tls": {"enable_renegotiation": true}}),
            "enable_renegotiation",
        ),
        (
            json!({"tls": {"root_cas_file": "/etc/ca.pem"}}),
            "root_cas_file",
        ),
        (
            json!({"tls": {"client_certs": [{"cert": "x", "key": "y"}]}}),
            "client_certs",
        ),
        (json!({"tls": {"verify": false}}), "tls.verify"),
        (json!({"queue": ""}), "queue"),
        (json!({"prefetch_count": 70000}), "prefetch_count"),
        (json!({"prefetch_size": 1024}), "prefetch_size"),
        (
            json!({"nack_reject_patterns": ["("]}),
            "nack_reject_patterns[0]",
        ),
        (
            json!({"bindings_declare": [{"key": "a"}]}),
            "bindings_declare[0].exchange",
        ),
        (json!({"frobnicate": 1}), "frobnicate"),
    ] {
        let e = refusal(base(extra.clone()));
        assert!(e.contains(want), "{extra}: {e}");
        assert!(!e.contains("hunter2"), "{e}");
    }
    let e = refusal(json!({"urls": ["http://u:hunter2@x"], "queue": "q"}));
    assert!(e.contains("urls[0]") && !e.contains("hunter2"), "{e}");
    let e = refusal(json!({"queue": "q"}));
    assert!(e.contains("urls"), "{e}");
}

#[test]
fn comma_separated_urls_expand_and_stay_out_of_debug() {
    let i = AmqpInput::from_config(&conf(json!({
        "urls": ["amqp://u:hunter2@a.example.org:5672/, amqp://u:hunter2@b.example.org:5672/"],
        "queue": "q",
        "tls": {"enabled": true, "root_cas": "-----BEGIN CERTIFICATE-----"},
    })))
    .expect("builds");
    assert_eq!(i.s.urls.len(), 2);
    assert!(i.s.tls);
    assert!(!format!("{:?}", i.s.urls).contains("hunter2"));
}

#[tokio::test]
async fn a_private_broker_is_refused_before_any_connection() {
    let mut i = AmqpInput::from_config(&conf(json!({
        "urls": ["amqp://u:hunter2@10.11.12.13:5672/"],
        "queue": "q",
    })))
    .expect("builds");
    let started = std::time::Instant::now();
    let e = match i.read_batch(&ctx(OrgQuotas::default())).await {
        Ok(_) => panic!("read from a private address"),
        Err(e) => e,
    };
    assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
    assert!(!e.to_string().contains("hunter2"), "{e}");
    // 10.11.12.13 does not answer: a connect attempt would wait out the 10 s timeout.
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!i.is_connected());
}

#[test]
fn properties_and_headers_become_bentos_metadata() {
    let mut headers = BTreeMap::new();
    headers.insert(
        ShortString::from("station"),
        AMQPValue::LongString(LongString::from("s-1")),
    );
    headers.insert(ShortString::from("n"), AMQPValue::LongInt(7));
    let p = lapin::BasicProperties::default()
        .with_content_type("application/json".into())
        .with_delivery_mode(2)
        .with_message_id("m-1".into())
        .with_timestamp(1_700_000_000)
        .with_headers(FieldTable::from(headers));
    let m = metadata(&p);
    assert_eq!(m["amqp_content_type"], "application/json");
    assert_eq!(m["amqp_delivery_mode"], "2");
    assert_eq!(m["amqp_message_id"], "m-1");
    assert_eq!(m["amqp_timestamp"], "1700000000");
    assert_eq!(m["station"], "s-1");
    assert_eq!(m["n"], "7");
    assert!(!m.contains_key("amqp_priority"));
}

#[tokio::test]
async fn tls_is_spoken_on_the_wire_and_never_falls_back_to_plain() {
    // A listener that records the first byte the input sends: a TLS ClientHello starts with
    // record type 22, an AMQP client with "AMQP".
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let first = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut s, _) = listener.accept().await.expect("accept");
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.expect("read");
        b[0]
    });
    let mut i = AmqpInput::from_config(&conf(json!({
        "urls": [format!("amqp://u:p@localhost:{port}/")],
        "queue": "q",
        "tls": {"enabled": true},
    })))
    .expect("builds");
    let quotas = OrgQuotas {
        allow_private_ips: true,
        ..OrgQuotas::default()
    };
    let c = ctx(quotas);
    let _ = tokio::time::timeout(Duration::from_secs(2), i.read_batch(&c)).await;
    assert_eq!(first.await.expect("listener"), 22);
}
