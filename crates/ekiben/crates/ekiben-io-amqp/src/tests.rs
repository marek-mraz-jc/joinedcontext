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
    AmqpInput::from_config(&conf(v))
        .err()
        .expect("accepted")
        .to_string()
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
fn malformed_lists_are_refused_by_index() {
    for (v, want) in [
        (
            json!({"urls": [7], "queue": "q"}),
            "urls[0] must be a string",
        ),
        (
            json!({"urls": [" , "], "queue": "q"}),
            "urls needs at least one URL",
        ),
        (
            json!({"urls": ["amqp://u:p@a.example.org/"], "queue": "q", "bindings_declare": ["amq.topic"]}),
            "bindings_declare[0] must be an object",
        ),
        (
            json!({"urls": ["amqp://u:p@a.example.org/"], "queue": "q", "nack_reject_patterns": [1]}),
            "nack_reject_patterns[0]",
        ),
    ] {
        let e = refusal(v.clone());
        assert!(e.contains(want), "{v}: {e}");
    }
}

#[test]
fn declare_bindings_and_defaults_are_read_as_written() {
    let url = "amqp://u:p@a.example.org:5672/";
    let i = AmqpInput::from_config(&conf(json!({
        "urls": [url],
        "queue": "q",
        "queue_declare": {"enabled": true, "durable": false, "auto_delete": true},
        "bindings_declare": [{"exchange": "amq.topic", "key": "sensors.#"}, {"exchange": "amq.direct"}],
        "consumer_tag": "ekiben-1",
        "auto_ack": true,
        "prefetch_count": 3,
        "tls": {"root_cas": "PEM"},
    })))
    .expect("builds");
    assert_eq!(i.s.declare, Some((false, true)));
    assert_eq!(
        i.s.bindings,
        [
            ("amq.topic".to_owned(), "sensors.#".to_owned()),
            ("amq.direct".to_owned(), String::new())
        ]
    );
    assert_eq!(i.s.consumer_tag, "ekiben-1");
    assert!(i.s.auto_ack);
    assert_eq!(i.s.prefetch_count, 3);
    assert!(!i.s.tls, "root_cas alone does not turn TLS on");
    assert_eq!(i.s.root_cas.as_deref(), Some("PEM"));

    let defaults = AmqpInput::from_config(&conf(json!({
        "urls": [url],
        "queue": "q",
        "queue_declare": {"enabled": true},
        "prefetch_count": null,
    })))
    .expect("builds");
    assert_eq!(defaults.s.declare, Some((true, false)));
    assert_eq!(defaults.s.prefetch_count, 10);
    assert!(!defaults.s.auto_ack);
    let off = AmqpInput::from_config(&conf(json!({
        "urls": [url], "queue": "q", "queue_declare": {"durable": true},
    })))
    .expect("builds");
    assert_eq!(
        off.s.declare, None,
        "queue_declare without enabled declares nothing"
    );
}

#[tokio::test]
async fn register_all_builds_and_lints_the_input() {
    let reg = Registry::new();
    register_all(&reg);
    let mut good = conf(json!({"urls": ["amqp://u:p@a.example.org/"], "queue": "q"}));
    good.label = "sensors".into();
    assert!(reg.lint_component("input", &good).is_empty());
    let bad = conf(json!({"urls": ["amqp://u:p@a.example.org/"], "queue": ""}));
    let errs = reg.lint_component("input", &bad);
    assert!(
        errs.len() == 1 && errs[0].contains("queue must name the queue"),
        "{errs:?}"
    );
    let input = reg
        .build_input(&good, &ctx(OrgQuotas::default()))
        .expect("builds");
    assert_eq!(input.input_type(), "amqp_0_9");
    assert_eq!(input.label(), "sensors");
    assert!(!input.is_connected());
    assert!(reg.build_input(&bad, &ctx(OrgQuotas::default())).is_err());
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
    let e = i
        .read_batch(&ctx(OrgQuotas::default()))
        .await
        .err()
        .expect("read from a private address");
    assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
    assert!(!e.to_string().contains("hunter2"), "{e}");
    // 10.11.12.13 does not answer: a connect attempt would wait out the 10 s timeout.
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!i.is_connected());
}

/// dial_one re-parses the URL parse() already checked; one that does not parse is a config
/// error that keeps the URL, password included, out of the message.
#[tokio::test]
async fn a_url_that_no_longer_parses_is_refused_without_naming_it() {
    let i = AmqpInput::from_config(&conf(json!({
        "urls": ["amqp://u:hunter2@a.example.org:5672/"],
        "queue": "q",
    })))
    .expect("builds");
    for bad in [
        "",
        "not a url",
        "http://u:hunter2@a.example.org/",
        "amqp://u:hunter2@[::1/",
    ] {
        let e = match i
            .dial_one(&SecretString::new(bad), &ctx(OrgQuotas::default()))
            .await
        {
            Ok(_) => panic!("dialled {bad:?}"),
            Err(e) => e,
        };
        assert!(matches!(e, CoreError::Config(_)), "{bad:?}: {e}");
        assert_eq!(
            e.to_string(),
            CoreError::config("amqp_0_9 input: a URL does not parse").to_string()
        );
        assert!(!e.to_string().contains("hunter2"), "{e}");
    }
}

#[test]
fn properties_and_headers_become_bentos_metadata() {
    let mut headers = BTreeMap::new();
    headers.insert(
        ShortString::from("station"),
        AMQPValue::LongString(LongString::from("s-1")),
    );
    headers.insert(ShortString::from("n"), AMQPValue::LongInt(7));
    for (k, v) in [
        ("short", AMQPValue::ShortString(ShortString::from("s"))),
        ("flag", AMQPValue::Boolean(true)),
        ("i8", AMQPValue::ShortShortInt(-8)),
        ("u8", AMQPValue::ShortShortUInt(8)),
        ("i16", AMQPValue::ShortInt(-16)),
        ("u16", AMQPValue::ShortUInt(16)),
        ("u32", AMQPValue::LongUInt(32)),
        ("i64", AMQPValue::LongLongInt(-64)),
        ("f32", AMQPValue::Float(1.5)),
        ("f64", AMQPValue::Double(2.25)),
        ("ts", AMQPValue::Timestamp(1_700_000_001)),
        ("void", AMQPValue::Void),
    ] {
        headers.insert(ShortString::from(k), v);
    }
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
    for (k, want) in [
        ("short", "s"),
        ("flag", "true"),
        ("i8", "-8"),
        ("u8", "8"),
        ("i16", "-16"),
        ("u16", "16"),
        ("u32", "32"),
        ("i64", "-64"),
        ("f32", "1.5"),
        ("f64", "2.25"),
        ("ts", "1700000001"),
    ] {
        assert_eq!(m[k], want, "{k}");
    }
    assert!(
        !m.contains_key("void"),
        "a header without a text form is left out"
    );
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

#[tokio::test]
async fn a_broker_that_never_answers_times_out_the_handshake() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // accepts and holds the socket without a byte of the AMQP handshake
    let held = tokio::spawn(async move { listener.accept().await.map(|(s, _)| s) });
    let i = AmqpInput::from_config(&conf(json!({
        "urls": [format!("amqp://u:hunter2@localhost:{port}/")],
        "queue": "q",
    })))
    .expect("builds");
    let quotas = OrgQuotas {
        allow_private_ips: true,
        ..OrgQuotas::default()
    };
    let e = i
        .dial(&ctx(quotas))
        .await
        .err()
        .expect("a silent listener completed the handshake")
        .to_string();
    assert!(e.contains("did not complete the handshake"), "{e}");
    assert!(!e.contains("hunter2"), "{e}");
    drop(held);
}

/// A certificate for `localhost` and its PEM, as a broker's CA would be given in `tls.root_cas`.
fn localhost_tls() -> (String, tokio_rustls::TlsAcceptor) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert");
    let pem = cert.cert.pem();
    let key = rustls_pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert.cert.der().clone()], key)
    .expect("server config");
    (pem, tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// A broker whose certificate `tls.root_cas` names is talked to over TLS: the AMQP protocol
/// header arrives inside the encrypted stream. A certificate from any other CA ends the
/// connection before a byte of AMQP is sent.
#[tokio::test]
async fn tls_trusts_root_cas_and_nothing_else() {
    use tokio::io::AsyncReadExt;
    for trusted in [true, false] {
        let (pem, acceptor) = localhost_tls();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.expect("accept");
            let mut s = acceptor.accept(s).await.ok()?;
            let mut header = [0u8; 4];
            s.read_exact(&mut header).await.ok()?;
            Some(header)
        });
        let root_cas = if trusted { pem } else { localhost_tls().0 };
        let i = AmqpInput::from_config(&conf(json!({
            "urls": [format!("amqps://u:p@localhost:{port}/")],
            "queue": "q",
            "tls": {"root_cas": root_cas},
        })))
        .expect("builds");
        let dialed = tokio::time::timeout(Duration::from_secs(3), i.dial(&loopback())).await;
        assert!(
            !matches!(dialed, Ok(Ok(_))),
            "no broker answered the handshake"
        );
        let header = server.await.expect("server");
        if trusted {
            assert_eq!(header, Some(*b"AMQP"));
        } else {
            assert_eq!(header, None, "the input talked to an untrusted broker");
            let Ok(Err(e)) = dialed else {
                panic!("an untrusted certificate must end the dial at once")
            };
            assert!(e.to_string().contains("connect"), "{e}");
        }
    }
}

/// The live broker of `AMQP_TEST_URL` (as tests/rabbitmq.rs), or `None` to skip.
/// The test's name is its thread's, as libtest names it.
fn live_url() -> Option<String> {
    let url = std::env::var("AMQP_TEST_URL").ok();
    let skip = if url.is_none() { "SKIP " } else { "" };
    eprintln!("{skip}{:?}: AMQP_TEST_URL", std::thread::current().name());
    url
}

fn loopback() -> StreamContext {
    ctx(OrgQuotas {
        allow_private_ips: true,
        ..OrgQuotas::default()
    })
}

/// What the broker refuses while the input sets up its consumer comes back as an error that
/// names the step: a missing queue, an exchange that does not exist, a declare that
/// contradicts the queue's settings.
#[tokio::test]
async fn the_broker_refusing_a_setup_step_names_the_step() {
    let Some(url) = live_url() else { return };
    let queue = format!("ekiben-refuse-{}", std::process::id());
    let dial = |extra: JsonValue| {
        let mut v = json!({"urls": [url.clone()], "queue": queue.clone()});
        v.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        AmqpInput::from_config(&conf(v)).expect("builds")
    };
    let error = |i: AmqpInput| async move {
        i.dial(&loopback())
            .await
            .err()
            .expect("the broker accepted")
            .to_string()
    };
    let e = error(dial(json!({}))).await;
    assert!(e.contains("consume"), "{e}");
    let declare =
        json!({"queue_declare": {"enabled": true, "durable": false, "auto_delete": false}});
    let live = dial(declare.clone())
        .dial(&loopback())
        .await
        .expect("declares");
    let e = error(dial(
        json!({"queue_declare": {"enabled": true, "durable": true}}),
    ))
    .await;
    assert!(e.contains("queue declare"), "{e}");
    let mut bind = declare;
    bind["bindings_declare"] = json!([{"exchange": "ekiben-no-such-exchange", "key": "k"}]);
    let e = error(dial(bind)).await;
    assert!(e.contains("queue bind"), "{e}");
    let ch = live.conn.create_channel().await.expect("channel");
    let _ = ch
        .queue_delete(queue.as_str().into(), Default::default())
        .await;
    let _ = live.conn.close(200, "test done".into()).await;
}

/// With auto_ack the broker forgets a message once delivered, and a declared binding routes
/// an exchange's messages into the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_ack_and_a_declared_binding_deliver_from_the_exchange() {
    use lapin::options::BasicPublishOptions;
    let Some(url) = live_url() else { return };
    let queue = format!("ekiben-bind-{}", std::process::id());
    // amq.topic is shared by every test process on the broker: a key of our own keeps their
    // messages out of this queue
    let key = format!("ekiben.{}.station", std::process::id());
    let mut input = AmqpInput::from_config(&conf(json!({
        "urls": [url.clone()],
        "queue": queue,
        "queue_declare": {"enabled": true, "durable": false, "auto_delete": false},
        "bindings_declare": [{"exchange": "amq.topic", "key": format!("ekiben.{}.#", std::process::id())}],
        "auto_ack": true,
        "consumer_tag": "ekiben-auto",
    })))
    .expect("builds");
    let c = loopback();
    // the first read connects, declares and binds; nothing is queued yet
    assert!(matches!(
        input.read_batch(&c).await,
        Err(CoreError::Timeout)
    ));
    assert!(input.is_connected());
    let conn = Connection::connect(&url, ConnectionProperties::default())
        .await
        .expect("publisher connects");
    let ch = conn.create_channel().await.expect("channel");
    ch.basic_publish(
        "amq.topic".into(),
        key.as_str().into(),
        BasicPublishOptions::default(),
        b"reading",
        lapin::BasicProperties::default(),
    )
    .await
    .expect("publish")
    .await
    .expect("confirm");
    let (batch, ack) = read_one(&mut input, &c).await;
    assert_eq!(&*batch[0].as_bytes_ref(), b"reading");
    assert_eq!(batch[0].meta_get_str("amqp_exchange"), "amq.topic");
    assert_eq!(batch[0].meta_get_str("amqp_routing_key"), key.as_str());
    assert_eq!(batch[0].meta_get_str("amqp_consumer_tag"), "ekiben-auto");
    // acknowledging is a no-op: the broker already forgot the message
    ack.nack(ekiben_core::BatchError::new(batch, "ignored".to_string()));
    assert!(matches!(
        input.read_batch(&c).await,
        Err(CoreError::Timeout)
    ));
    input.close().await.expect("close");
    assert!(!input.is_connected());
    let left = ch
        .queue_delete(queue.as_str().into(), Default::default())
        .await
        .expect("delete");
    assert_eq!(left, 0, "auto_ack: nothing went back to the queue on close");
    let _ = conn.close(200, "test done".into()).await;
}

/// queue_declare.auto_delete reaches the broker: the queue goes when its consumer does.
#[tokio::test]
async fn an_auto_delete_queue_goes_with_its_consumer() {
    let Some(url) = live_url() else { return };
    let queue = format!("ekiben-auto-delete-{}", std::process::id());
    let mut input = AmqpInput::from_config(&conf(json!({
        "urls": [url.clone()],
        "queue": queue,
        "queue_declare": {"enabled": true, "durable": false, "auto_delete": true},
    })))
    .expect("builds");
    let c = loopback();
    assert!(matches!(
        input.read_batch(&c).await,
        Err(CoreError::Timeout)
    ));
    input.close().await.expect("close");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let conn = Connection::connect(&url, ConnectionProperties::default())
        .await
        .expect("connects");
    let ch = conn.create_channel().await.expect("channel");
    let passive = ch
        .queue_declare(
            queue.as_str().into(),
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await;
    assert!(
        passive.is_err(),
        "the auto_delete queue outlived its consumer"
    );
    let _ = conn.close(200, "test done".into()).await;
}

async fn read_one(input: &mut AmqpInput, c: &StreamContext) -> (Batch, AckResponder) {
    let mut got = None;
    for _ in 0..10 {
        match input.read_batch(c).await {
            Ok(b) => {
                got = Some(b);
                break;
            }
            Err(e) => assert!(matches!(e, CoreError::Timeout), "read: {e}"),
        }
    }
    got.expect("no message in 10 s")
}

/// An acknowledgement that cannot reach the broker leaves the message with it: one given
/// outside a runtime is never sent, one after the connection closed is dropped by the client,
/// and the broker delivers the message again, marked redelivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsent_or_failed_ack_leaves_the_message_redelivered() {
    use lapin::options::BasicPublishOptions;
    let Some(url) = live_url() else { return };
    let queue = format!("ekiben-ack-{}", std::process::id());
    let config = json!({
        "urls": [url.clone()],
        "queue": queue,
        "queue_declare": {"enabled": true, "durable": false, "auto_delete": false},
    });
    let c = loopback();
    let mut input = AmqpInput::from_config(&conf(config.clone())).expect("builds");
    assert!(matches!(
        input.read_batch(&c).await,
        Err(CoreError::Timeout)
    ));
    let conn = Connection::connect(&url, ConnectionProperties::default())
        .await
        .expect("publisher connects");
    let ch = conn.create_channel().await.expect("channel");
    ch.basic_publish(
        "".into(),
        queue.as_str().into(),
        BasicPublishOptions::default(),
        b"kept",
        lapin::BasicProperties::default(),
    )
    .await
    .expect("publish")
    .await
    .expect("confirm");

    let (b, ack) = read_one(&mut input, &c).await;
    assert_eq!(b[0].meta_get_str("amqp_redelivered"), "false");
    std::thread::spawn(move || ack.ack())
        .join()
        .expect("ack thread");
    input.close().await.expect("close");

    let mut input = AmqpInput::from_config(&conf(config.clone())).expect("builds");
    let (b, ack) = read_one(&mut input, &c).await;
    assert_eq!(&*b[0].as_bytes_ref(), b"kept");
    assert_eq!(b[0].meta_get_str("amqp_redelivered"), "true");
    input.close().await.expect("close");
    ack.ack();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut input = AmqpInput::from_config(&conf(config)).expect("builds");
    let (b, ack) = read_one(&mut input, &c).await;
    assert_eq!(
        &*b[0].as_bytes_ref(),
        b"kept",
        "the failed ack left the message queued"
    );
    ack.ack();
    tokio::time::sleep(Duration::from_millis(300)).await;
    input.close().await.expect("close");
    let q = ch
        .queue_delete(queue.as_str().into(), Default::default())
        .await
        .expect("delete");
    assert_eq!(q, 0, "the last ack removed the message");
    let _ = conn.close(200, "test done".into()).await;
}

/// The ready messages of `queue` once they reach `want`, read with a passive declare; the
/// broker requeues an unacknowledged message only after it saw the consumer's connection go,
/// so a fixed sleep races it on a loaded host.
async fn wait_ready(ch: &lapin::Channel, queue: &str, want: u32) -> u32 {
    let mut seen = u32::MAX;
    for _ in 0..300 {
        seen = ch
            .queue_declare(
                queue.into(),
                QueueDeclareOptions {
                    passive: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .expect("passive declare")
            .message_count();
        if seen == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    seen
}

/// An ack given as the stream stops goes out while the connection closes: the client refuses
/// it (the channel is closing), the input logs it, and the broker keeps the message for the
/// next consumer, marked redelivered. The close is polled once before the ack, so the
/// connection is Closing and the close is queued ahead of the ack: the ack can never reach the
/// broker first (it did under the coverage build when it was spawned before the close).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ack_racing_the_close_fails_and_leaves_the_message_queued() {
    use lapin::options::BasicPublishOptions;
    let Some(url) = live_url() else { return };
    let queue = format!("ekiben-ack-race-{}", std::process::id());
    let config = json!({
        "urls": [url.clone()],
        "queue": queue,
        "queue_declare": {"enabled": true, "durable": false, "auto_delete": false},
    });
    let c = loopback();
    let mut input = AmqpInput::from_config(&conf(config.clone())).expect("builds");
    assert!(matches!(
        input.read_batch(&c).await,
        Err(CoreError::Timeout)
    ));
    let conn = Connection::connect(&url, ConnectionProperties::default())
        .await
        .expect("publisher connects");
    let ch = conn.create_channel().await.expect("channel");
    ch.basic_publish(
        "".into(),
        queue.as_str().into(),
        BasicPublishOptions::default(),
        b"in flight",
        lapin::BasicProperties::default(),
    )
    .await
    .expect("publish")
    .await
    .expect("confirm");
    let (b, ack) = read_one(&mut input, &c).await;
    assert_eq!(b[0].meta_get_str("amqp_redelivered"), "false");
    let mut closing = std::pin::pin!(input.close());
    assert!(
        futures::poll!(&mut closing).is_pending(),
        "the close waits for the broker's close-ok"
    );
    ack.ack();
    closing.await.expect("close");
    assert_eq!(
        wait_ready(&ch, &queue, 1).await,
        1,
        "the refused ack left the message queued"
    );

    let mut input = AmqpInput::from_config(&conf(config)).expect("builds");
    let (b, ack) = read_one(&mut input, &c).await;
    assert_eq!(&*b[0].as_bytes_ref(), b"in flight");
    assert_eq!(
        b[0].meta_get_str("amqp_redelivered"),
        "true",
        "the refused ack left the message with the broker"
    );
    ack.ack();
    tokio::time::sleep(Duration::from_millis(300)).await;
    input.close().await.expect("close");
    let left = ch
        .queue_delete(queue.as_str().into(), Default::default())
        .await
        .expect("delete");
    assert_eq!(left, 0, "the second ack removed the message");
    let _ = conn.close(200, "test done".into()).await;
}

/// A broker that refuses the connection is retried with a doubling wait, so a stream does not
/// hammer a broker that is down.
#[tokio::test]
async fn a_refused_connection_is_retried_with_a_growing_wait() {
    // a bound socket that never listens refuses every connection
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket
        .bind("127.0.0.1:0".parse().expect("addr"))
        .expect("bind");
    let port = socket.local_addr().expect("addr").port();
    let mut i = AmqpInput::from_config(&conf(json!({
        "urls": [format!("amqp://u:p@127.0.0.1:{port}/")],
        "queue": "q",
    })))
    .expect("builds");
    let read = tokio::time::timeout(Duration::from_secs(2), i.read_batch(&loopback())).await;
    assert!(read.is_err(), "a refused broker is retried, not reported");
    assert!(
        i.backoff >= Duration::from_millis(400) && i.backoff <= Duration::from_secs(30),
        "{:?}",
        i.backoff
    );
    drop(socket);
}
