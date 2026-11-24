//! The `mqtt` input against a small MQTT 3.1.1 broker written here (packets by hand), as the
//! Portal renders it for an MqttBroker source (crates/portal `render.rs` `datasource_input`).
#![cfg(feature = "websocket")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ekiben_bloblang::{Environment, Value as BValue};
use ekiben_core::{
    ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Output, Part, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

async fn read_packet(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut first = [0u8; 1];
    s.read_exact(&mut first).await.unwrap();
    let (mut len, mut mult) = (0usize, 1usize);
    loop {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.unwrap();
        len += (b[0] & 0x7f) as usize * mult;
        mult *= 128;
        if b[0] & 0x80 == 0 {
            break;
        }
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.unwrap();
    (first[0], body)
}

fn publish(topic: &str, qos: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let mut body = (topic.len() as u16).to_be_bytes().to_vec();
    body.extend_from_slice(topic.as_bytes());
    if qos > 0 {
        body.extend_from_slice(&id.to_be_bytes());
    }
    body.extend_from_slice(payload);
    let mut out = vec![0x30 | (qos << 1), body.len() as u8];
    out.extend(body);
    out
}

/// Accepts a client, answers CONNECT and SUBSCRIBE, and hands over the stream with what the
/// client sent.
async fn accept(listener: &TcpListener) -> (TcpStream, Vec<u8>, Vec<u8>) {
    let (mut s, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("the input connects")
        .unwrap();
    let (first, connect) = read_packet(&mut s).await;
    assert_eq!(first, 0x10);
    s.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();
    let (first, subscribe) = read_packet(&mut s).await;
    assert_eq!(first, 0x82);
    s.write_all(&[0x90, 0x03, subscribe[0], subscribe[1], 0x01])
        .await
        .unwrap();
    (s, connect, subscribe)
}

fn context(allow_private_ips: bool) -> (Arc<Registry>, StreamContext, watch::Sender<bool>) {
    let reg = Arc::new(Registry::new());
    ekiben_io_http::register_all(&reg);
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-mqtt");
    let org = resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips,
            ..Default::default()
        },
    );
    let (tx, rx) = watch::channel(false);
    let ctx = StreamContext {
        stream_id: "mqtt-test".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("mqtt-test", "org-mqtt"),
        label: String::new(),
        path: "root.input".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, ctx, tx)
}

fn build(conf: Value, ctx: &StreamContext, reg: &Registry) -> Box<dyn Input> {
    reg.build_input(
        &ComponentConfig {
            label: String::new(),
            component_type: "mqtt".to_string(),
            config: conf,
            processors: Vec::new(),
        },
        ctx,
    )
    .expect("the input builds")
}

#[tokio::test]
async fn reads_a_qos1_message_and_acknowledges_it_once_acked() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(true);
    // The Portal's render for an in-cluster MqttBroker source.
    let mut input = build(
        json!({"clean_session": true, "client_id": "jc-helsinki-mqtt-sensors", "qos": 1,
               "topics": ["sensors/+/air"], "urls": [format!("tcp://{addr}")],
               "user": "pipeline-runner-helsinki"}),
        &ctx,
        &reg,
    );
    let (connected, server) = tokio::join!(input.connect(&ctx), accept(&listener));
    connected.expect("connects");
    let (mut s, connect, subscribe) = server;
    assert_eq!(&connect[..7], b"\x00\x04MQTT\x04");
    assert_eq!(
        connect[7],
        0x80 | 0x02,
        "user name and clean session, no password"
    );
    assert!(String::from_utf8_lossy(&connect).contains("jc-helsinki-mqtt-sensors"));
    assert!(String::from_utf8_lossy(&connect).contains("pipeline-runner-helsinki"));
    assert_eq!(&subscribe[2..], b"\x00\x0dsensors/+/air\x01");

    s.write_all(&publish("sensors/a1/air", 1, 42, br#"{"pm10":3}"#))
        .await
        .unwrap();
    let (batch, ack) = tokio::time::timeout(Duration::from_secs(10), input.read_batch(&ctx))
        .await
        .expect("a message arrives")
        .expect("read");
    assert_eq!(batch[0].as_bytes_ref().as_ref(), br#"{"pm10":3}"#);
    assert_eq!(
        batch[0].meta_get("mqtt_topic"),
        Some(&BValue::String("sensors/a1/air".into()))
    );
    assert_eq!(batch[0].meta_get("mqtt_qos"), Some(&BValue::Int(1)));
    assert_eq!(batch[0].meta_get("mqtt_message_id"), Some(&BValue::Int(42)));
    ack.ack();
    let (first, body) = tokio::time::timeout(Duration::from_secs(5), read_packet(&mut s))
        .await
        .expect("PUBACK");
    assert_eq!((first, body), (0x40, vec![0, 42]));
    input.close().await.unwrap();
}

#[tokio::test]
async fn a_lost_connection_is_dialled_again() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(true);
    let mut input = build(
        json!({"qos": 0, "topics": ["t"], "urls": [format!("mqtt://{addr}")], "client_id": "c"}),
        &ctx,
        &reg,
    );
    let (connected, (first, _, _)) = tokio::join!(input.connect(&ctx), accept(&listener));
    connected.unwrap();
    drop(first);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let (mut s, _, _) = accept(&listener).await;
        s.write_all(&publish("t", 0, 0, b"again")).await.unwrap();
        let _ = tx.send(());
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let (batch, _) = tokio::time::timeout(Duration::from_secs(15), input.read_batch(&ctx))
        .await
        .expect("a message after the reconnect")
        .expect("read");
    assert_eq!(batch[0].as_bytes_ref().as_ref(), b"again");
    rx.recv().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn a_broker_down_at_start_is_dialled_until_it_answers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (reg, ctx, _stop) = context(true);
    let mut input = build(
        json!({"qos": 0, "topics": ["t"], "urls": [format!("tcp://{addr}")], "client_id": "c"}),
        &ctx,
        &reg,
    );
    input
        .connect(&ctx)
        .await
        .expect("a broker that does not answer yet leaves the stream running");
    let listener = TcpListener::bind(addr).await.unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _, _) = accept(&listener).await;
        s.write_all(&publish("t", 0, 0, b"late")).await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let (batch, _) = tokio::time::timeout(Duration::from_secs(15), input.read_batch(&ctx))
        .await
        .expect("a message once the broker answers")
        .expect("read");
    assert_eq!(batch[0].as_bytes_ref().as_ref(), b"late");
    server.abort();
}

#[tokio::test]
async fn a_loopback_broker_is_refused_by_the_outbound_rule() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(false);
    let mut input = build(
        json!({"topics": ["t"], "urls": [format!("tcp://{addr}")]}),
        &ctx,
        &reg,
    );
    let err = input.connect(&ctx).await.expect_err("refused");
    assert!(matches!(err, CoreError::AccessDenied { .. }), "{err}");
}

fn build_output(conf: Value, ctx: &StreamContext, reg: &Registry) -> Arc<dyn Output> {
    reg.build_output(
        &ComponentConfig {
            label: String::new(),
            component_type: "mqtt".to_string(),
            config: conf,
            processors: Vec::new(),
        },
        ctx,
    )
    .expect("the output builds")
}

/// Every mqtt config the input or output refuses, with the words that tell the person what to
/// write instead; the fields it reads leniently (comma-split urls, a nanoid suffix) are kept.
#[test]
fn a_bad_mqtt_config_is_refused_naming_the_field() {
    let (reg, ctx, _stop) = context(true);
    let base = json!({ "urls": ["tcp://broker.example.org"], "topics": ["t"] });
    let with = |extra: Value| {
        let mut c = base.clone();
        c.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        c
    };
    for (conf, want) in [
        (json!("tcp://x"), "expected an object"),
        (json!({ "topics": ["t"] }), "at least one url is required"),
        (
            json!({ "urls": [" , "], "topics": ["t"] }),
            "at least one url is required",
        ),
        (
            json!({ "urls": ["::nope"], "topics": ["t"] }),
            "mqtt.urls: ",
        ),
        (
            json!({ "urls": ["http://x"], "topics": ["t"] }),
            "unsupported url scheme 'http'",
        ),
        (
            json!({ "urls": ["tcp://x"] }),
            "topics must name at least one",
        ),
        (
            json!({ "urls": ["tcp://x"], "topics": [""] }),
            "topics must name at least one",
        ),
        (with(json!({ "qos": 3 })), "qos must be 0, 1 or 2"),
        (
            with(json!({ "will": { "enabled": true } })),
            "mqtt.will is not supported",
        ),
        (
            with(json!({ "dynamic_client_id_suffix": "uuid" })),
            "unknown dynamic_client_id_suffix: uuid",
        ),
        (with(json!({ "connect_timeout": "soon" })), "soon"),
        (
            with(json!({ "tls": { "skip_cert_verify": true } })),
            "mqtt.tls.skip_cert_verify is not supported",
        ),
        (
            with(json!({ "tls": { "root_cas_file": "/etc/ca.pem" } })),
            "mqtt.tls.root_cas_file is not supported",
        ),
        (
            with(json!({ "tls": { "client_certs": [{ "cert": "c" }] } })),
            "mqtt.tls.client_certs is not supported",
        ),
        (
            with(json!({ "tls": { "root_cas": "not a certificate" } })),
            "root_cas",
        ),
    ] {
        let err = ekiben_io_http::mqtt::MqttConf::from_json(&conf)
            .err()
            .unwrap_or_else(|| panic!("{conf} must be refused"))
            .to_string();
        assert!(err.contains(want), "{conf}: {err}");
    }
    let lenient = ekiben_io_http::mqtt::MqttConf::from_json(&with(json!({
        "urls": ["tcp://a.example.org, ws://b.example.org", "wss://c.example.org"],
        "client_id": "jc-",
        "dynamic_client_id_suffix": "nanoid",
        "keepalive": 1_000_000,
        "tls": { "skip_cert_verify": false, "root_cas_file": "", "client_certs": [] },
    })))
    .expect("lenient fields");
    assert_eq!(lenient.urls.len(), 3);
    assert_eq!(lenient.client_id.len(), "jc-".len() + 21);
    assert_eq!(lenient.keepalive, u16::MAX);

    for (conf, want) in [
        (
            json!({ "urls": ["tcp://x"], "topic": "t", "qos": 2 }),
            "qos 2 is not supported",
        ),
        (
            json!({ "urls": ["tcp://x"], "topic": "" }),
            "field topic is required",
        ),
        (
            json!({ "urls": ["tcp://x"], "topic": "t", "retained_interpolated": "${! true }" }),
            "retained_interpolated is not supported",
        ),
        (json!({ "urls": ["tcp://x"], "topic": "${! nope( }" }), ""),
        (
            json!({ "urls": [], "topic": "t" }),
            "at least one url is required",
        ),
    ] {
        let built = reg.build_output(
            &ComponentConfig {
                label: String::new(),
                component_type: "mqtt".to_string(),
                config: conf.clone(),
                processors: Vec::new(),
            },
            &ctx,
        );
        let err = built
            .err()
            .unwrap_or_else(|| panic!("{conf} must be refused"))
            .to_string();
        assert!(err.contains(want), "{conf}: {err}");
    }
}

/// A broker for the output: answers CONNECT with `connack` and hands over the stream with the
/// CONNECT body.
async fn accept_publisher(listener: &TcpListener, connack: &[u8]) -> (TcpStream, Vec<u8>) {
    let (mut s, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("the output connects")
        .unwrap();
    let (first, connect) = read_packet(&mut s).await;
    assert_eq!(first, 0x10);
    s.write_all(connack).await.unwrap();
    (s, connect)
}

fn batch_of(msgs: &[(&str, &[u8])]) -> Vec<Part> {
    msgs.iter()
        .map(|(topic, payload)| {
            let mut p = Part::new(payload.to_vec());
            p.meta_set("t", BValue::String((*topic).to_string()));
            p
        })
        .collect()
}

/// QoS 1: each message goes to its interpolated topic with the retain flag and its own packet
/// id, and the batch is done only once every PUBACK is in; close sends DISCONNECT.
#[tokio::test]
async fn the_output_publishes_each_message_and_waits_for_its_puback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(true);
    let output = build_output(
        json!({ "urls": [format!("tcp://{addr}")], "client_id": "pub", "user": "u", "password": "p",
                "topic": "city/${! meta(\"t\") }", "qos": 1, "retained": true }),
        &ctx,
        &reg,
    );
    assert!(!output.is_connected());
    let (connected, (mut s, connect)) = tokio::join!(
        output.connect(&ctx),
        accept_publisher(&listener, &[0x20, 0x02, 0x00, 0x00])
    );
    connected.expect("connects");
    assert!(output.is_connected());
    output
        .connect(&ctx)
        .await
        .expect("a second connect keeps the connection");
    assert_eq!(
        connect[7],
        0x80 | 0x40 | 0x02,
        "user, password and clean session"
    );
    let broker = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (first, body) = read_packet(&mut s).await;
            // Topic `city/x`: its 2-byte length and 6 bytes, then the packet id.
            let id = [body[8], body[9]];
            seen.push((first, body));
            s.write_all(&[0x40, 0x02, id[0], id[1]]).await.unwrap();
        }
        let (first, _) = read_packet(&mut s).await;
        (seen, first)
    });
    output
        .write_batch(&ctx, batch_of(&[("a", b"pm1"), ("b", b"pm2")]))
        .await
        .expect("both acked");
    output.close().await.unwrap();
    let (seen, last) = broker.await.unwrap();
    assert_eq!(seen[0].0, 0x30 | 0x02 | 0x01, "PUBLISH, QoS 1, retained");
    assert_eq!(seen[0].1, b"\x00\x06city/a\x00\x01pm1");
    assert_eq!(seen[1].1, b"\x00\x06city/b\x00\x02pm2");
    assert_eq!(last, 0xe0, "DISCONNECT on close");
    assert!(!output.is_connected());
}

/// A topic that names a wildcard or nothing, a payload over the packet limit, and a broker that
/// never sends the PUBACK each fail the batch with the reason.
#[tokio::test]
async fn the_output_refuses_a_bad_topic_a_huge_message_and_a_missing_puback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(true);
    let output = build_output(
        json!({ "urls": [format!("tcp://{addr}")], "topic": "${! meta(\"t\") }", "qos": 1,
                "connect_timeout": "300ms" }),
        &ctx,
        &reg,
    );
    for (topic, want) in [
        ("a/+/b", "holds a wildcard or NUL"),
        ("a/#", "holds a wildcard or NUL"),
        (&"a".repeat(65536), "a topic must be 1 to 65535 bytes"),
    ] {
        let err = output
            .write_batch(&ctx, batch_of(&[(topic, b"x")]))
            .await
            .err()
            .expect("refused")
            .to_string();
        assert!(err.contains(want), "{topic:?}: {err}");
    }
    let huge = vec![b'x'; ekiben_io_http::mqtt::MAX_PACKET_BYTES + 1];
    let err = output
        .write_batch(&ctx, batch_of(&[("t", &huge)]))
        .await
        .err()
        .expect("too big")
        .to_string();
    assert!(err.contains("is over the"), "{err}");
    let numeric = build_output(
        json!({ "urls": [format!("tcp://{addr}")], "topic": "${! content().number() }" }),
        &ctx,
        &reg,
    );
    let err = numeric
        .write_batch(&ctx, batch_of(&[("t", b"not a number")]))
        .await
        .err()
        .expect("no topic")
        .to_string();
    assert!(err.contains("topic interpolation error"), "{err}");

    let broker = tokio::spawn(async move {
        let (mut s, _) = accept_publisher(&listener, &[0x20, 0x02, 0x00, 0x00]).await;
        let _ = read_packet(&mut s).await;
        // An unrelated PUBACK and a PINGRESP, never the one for this message.
        s.write_all(&[0x40, 0x02, 0x00, 0x09, 0xd0, 0x00])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let err = output
        .write_batch(&ctx, batch_of(&[("t", b"x")]))
        .await
        .err()
        .expect("no PUBACK")
        .to_string();
    assert!(err.contains("no PUBACK for a message to t"), "{err}");
    broker.abort();
}

/// A kept connection the broker dropped is dialled again once and the batch sent again; QoS 0
/// waits for nothing.
#[tokio::test]
async fn the_output_dials_again_when_a_kept_connection_was_lost() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (reg, ctx, _stop) = context(true);
    let output = build_output(
        json!({ "urls": [format!("tcp://{addr}")], "topic": "t", "qos": 1, "connect_timeout": "2s" }),
        &ctx,
        &reg,
    );
    let (connected, (first, _)) = tokio::join!(
        output.connect(&ctx),
        accept_publisher(&listener, &[0x20, 0x02, 0x00, 0x00])
    );
    connected.unwrap();
    drop(first);
    let broker = tokio::spawn(async move {
        let (mut s, _) = accept_publisher(&listener, &[0x20, 0x02, 0x00, 0x00]).await;
        let (first, body) = read_packet(&mut s).await;
        s.write_all(&[0x40, 0x02, body[3], body[4]]).await.unwrap();
        (first, body)
    });
    output
        .write_batch(&ctx, batch_of(&[("t", b"again")]))
        .await
        .expect("sent on the new connection");
    let (first, body) = broker.await.unwrap();
    assert_eq!(first, 0x32);
    assert!(body.ends_with(b"again"));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let qos0 = build_output(
        json!({ "urls": [format!("tcp://{addr}")], "topic": "t", "qos": 0 }),
        &ctx,
        &reg,
    );
    let broker = tokio::spawn(async move {
        let (mut s, _) = accept_publisher(&listener, &[0x20, 0x02, 0x00, 0x00]).await;
        read_packet(&mut s).await
    });
    qos0.write_batch(&ctx, batch_of(&[("t", b"fire")]))
        .await
        .expect("QoS 0 needs no PUBACK");
    assert_eq!(broker.await.unwrap(), (0x30, b"\x00\x01tfire".to_vec()));
}

/// A broker that refuses CONNECT or answers something else is reported; the output's connect
/// keeps the stream (the next write dials again) and the write names the refusal. A loopback
/// broker without the quota is an error at connect.
#[tokio::test]
async fn a_refused_connect_is_named_and_the_outbound_rule_holds_at_connect() {
    for (connack, want) in [
        (
            &[0x20u8, 0x02, 0x00, 0x05][..],
            "refused the connection (code 5)",
        ),
        (&[0xd0, 0x00], "did not answer CONNECT with CONNACK"),
        (&[], "closed the connection"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (reg, ctx, _stop) = context(true);
        let output = build_output(
            json!({ "urls": [format!("tcp://{addr}")], "topic": "t", "connect_timeout": "2s" }),
            &ctx,
            &reg,
        );
        let reply = connack.to_vec();
        tokio::spawn(async move {
            loop {
                let (s, _) = accept_publisher(&listener, &reply).await;
                drop(s);
            }
        });
        output
            .connect(&ctx)
            .await
            .expect("a refusing broker keeps the stream");
        assert!(!output.is_connected());
        let err = output
            .write_batch(&ctx, batch_of(&[("t", b"x")]))
            .await
            .err()
            .expect("refused")
            .to_string();
        assert!(err.contains(want), "{connack:?}: {err}");
    }

    let (reg, ctx, _stop) = context(false);
    let output = build_output(
        json!({ "urls": ["tcp://127.0.0.1:1883"], "topic": "t" }),
        &ctx,
        &reg,
    );
    assert!(matches!(
        output.connect(&ctx).await,
        Err(CoreError::AccessDenied { .. })
    ));
}
