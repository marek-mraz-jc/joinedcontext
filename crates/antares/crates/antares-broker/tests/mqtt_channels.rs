// SPDX-License-Identifier: EUPL-1.2
//! Inbound MQTT Channel e2e (ADR-0031, T-5079): the real binary binds a
//! topic of a live MQTT server to an Attribute, writes what is published
//! there, and does not loop when an MQTT notification Subscription
//! publishes that Attribute's changes on the same topic. Gated on
//! ANTARES_TEST_MQTT_URL as `mqtt_notify` is: without a live server the
//! test is skipped, not green-lied.
#![cfg(all(unix, feature = "mqtt", feature = "channels"))]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

mod common;
use common::free_port;

struct Broker(Child);
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(port: u16, method: &str, path: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("read");
    out
}

fn json_body(resp: &str) -> serde_json::Value {
    let body = resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or_default();
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

fn until(port: u16, path: &str, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let v = json_body(&http(port, "GET", path, ""));
        if done(&v) || Instant::now() > deadline {
            return v;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn samples(v: &serde_json::Value, name: &str) -> u64 {
    v["status"]["samples"][name].as_u64().unwrap_or(0)
}

#[test]
fn an_mqtt_channel_writes_its_samples_and_does_not_loop() {
    let Ok(mqtt_url) = std::env::var("ANTARES_TEST_MQTT_URL") else {
        eprintln!(
            "SKIP: ANTARES_TEST_MQTT_URL not set (mqtt://127.0.0.1:1883, live server needed)"
        );
        return;
    };
    let hostport = mqtt_url
        .strip_prefix("mqtt://")
        .expect("ANTARES_TEST_MQTT_URL must be mqtt://host:port");
    let (host, port_s) = hostport.split_once(':').expect("host:port");
    let mqtt_port: u16 = port_s.parse().expect("port");
    let topic = format!("antares-channel-e2e-{}", std::process::id());
    let http_port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env_remove("ANTARES_TEST_DATABASE_URL")
            .env_remove("ANTARES_TEST_MQTT_URL")
            .env("ANTARES_HTTP_PORT", http_port.to_string())
            .env("ANTARES_EGRESS_ALLOW_PRIVATE", "true")
            .env("ANTARES_API_SURFACES", "admin,channels")
            .env("ANTARES_CHANNEL_BROKERS", format!("m={mqtt_url}"))
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while TcpStream::connect(("127.0.0.1", http_port)).is_err()
        || !http(http_port, "GET", "/q/health", "").contains("200")
    {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
    // the outbound half on the same topic: every change of `speed` is
    // published where the Channel listens
    let sub = format!(
        r#"{{"type": "Subscription", "entities": [{{"type": "Bus"}}],
             "watchedAttributes": ["speed"],
             "notification": {{"endpoint": {{"uri": "{mqtt_url}/{topic}"}}}}}}"#
    );
    let resp = http(http_port, "POST", "/ngsi-ld/v1/subscriptions", &sub);
    assert!(resp.contains("201"), "subscription: {resp}");
    let entity = r#"{"id": "urn:ngsi-ld:Bus:e2e", "type": "Bus",
                     "speed": {"type": "Property", "value": 0}}"#;
    let resp = http(http_port, "POST", "/ngsi-ld/v1/entities", entity);
    assert!(resp.contains("201"), "entity: {resp}");
    let channel = format!(
        r#"{{"entity": "urn:ngsi-ld:Bus:e2e", "attribute": "speed", "broker": "m",
             "topic": "{topic}", "retention": "mirror"}}"#
    );
    let resp = http(http_port, "PUT", "/x/channels/bus-e2e", &channel);
    assert!(resp.contains("201"), "channel: {resp}");
    // the shared subscription is live once the bind answered; give the
    // server a moment to hold it before publishing
    std::thread::sleep(Duration::from_millis(500));

    let mut opts = rumqttc::MqttOptions::new(
        format!("antares-e2e-pub-{}", std::process::id()),
        host,
        mqtt_port,
    );
    opts.set_keep_alive(Duration::from_secs(10));
    let (client, mut conn) = rumqttc::Client::new(opts, 16);
    client
        .publish(&topic, rumqttc::QoS::AtLeastOnce, false, "42")
        .expect("publish");
    for event in conn.iter() {
        if matches!(
            event.expect("mqtt"),
            rumqttc::Event::Incoming(rumqttc::Packet::PubAck(_))
        ) {
            break;
        }
    }

    let e = until(http_port, "/ngsi-ld/v1/entities/urn:ngsi-ld:Bus:e2e", |v| {
        v["speed"]["value"] == 42
    });
    assert_eq!(e["speed"]["value"], 42, "{e}");
    let ch = until(http_port, "/x/channels/bus-e2e", |v| {
        samples(v, "looped") >= 1
    });
    assert!(
        samples(&ch, "looped") >= 1,
        "the echo came back and was dropped: {ch}"
    );
    std::thread::sleep(Duration::from_secs(1));
    let ch = json_body(&http(http_port, "GET", "/x/channels/bus-e2e", ""));
    assert_eq!(samples(&ch, "written"), 1, "one sample, one write: {ch}");
}
