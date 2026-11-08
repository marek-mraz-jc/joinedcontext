//! The `mqtt` input against a small MQTT 3.1.1 broker written here (packets by hand), as the
//! Portal renders it for an MqttBroker source (crates/portal `render.rs` `datasource_input`).
#![cfg(feature = "websocket")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ekiben_bloblang::{Environment, Value as BValue};
use ekiben_core::{
    ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Registry, ResourceManager, StreamContext,
    StreamMetrics,
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
