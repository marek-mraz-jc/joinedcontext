// SPDX-License-Identifier: EUPL-1.2
//! T-5911: the SensorThings MQTT extension of a view (OGC 18-088 section 14, read side) end to
//! end. An NGSI-LD write of a Datastream's Attribute reaches an MQTT subscriber of its
//! Observations topic within a second, the message the HTTP read of that Observation byte for
//! byte; a hidden Thing's topic and a wildcard are refused and get nothing; an unsubscribed
//! topic gets nothing more; a caller the policy grants nothing is refused; a client PUBLISH
//! closes the connection.
//!
//! The client is the few lines of RFC 6455 and MQTT 3.1.1 these tests need, so the broker gains
//! no dependency, development ones included: its tree is the vanilla gate's.
#![cfg(all(feature = "full", feature = "jc-policy"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tower::Service;

/// The service account the tests write as, outside every view.
const SEEDER: &str = "mqtt-seed";
/// The read and subscription clauses the public role holds whole, so the view alone decides.
const READS: [&str; 15] = [
    "5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.7.5", "5.7.6", "5.7.7", "5.7.8", "5.7.9", "5.7.10",
    "5.8.1", "5.8.2", "5.8.3", "5.8.4", "5.8.5",
];
const TENANT: &str = "mqtt-city";
const VIEW: &str = "mqtt-cars";

async fn broker() -> (Router, u16) {
    std::env::set_var(
        "ANTARES_POLICY_SUBJECT_HEADERS",
        joinedcontext_policy::subject::HEADERS.join(","),
    );
    let mut state = AppState::new("sta-mqtt".into())
        .with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())))
        .with_policy(std::sync::Arc::new(
            joinedcontext_policy::JoinedcontextPolicy::new(),
        ));
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_sta::Sta),
        antares_ops::mount(antares_ws::StaMqtt::process()),
    ];
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let _reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    install(&app).await;
    (app, port)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (StatusCode, Vec<u8>) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("a request"))
        .await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, bytes.to_vec())
}

/// The view (Car `speed` and `name`; Radar stored and hidden), the public role reading through
/// it, the seeder writing on the Tenant root, and the Entities.
async fn install(app: &Router) {
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let view = json!({"id": VIEW, "tenant": TENANT, "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "name"]}], "operations": ops});
    let policy = json!({"policies": [
        {"id": "open", "assignee": {"kind": "role", "id": "public"}, "operations": READS},
        {"id": "seed", "assignee": {"kind": "serviceAccount", "id": SEEDER},
         "operations": ["5.6.1", "5.6.2", "5.6.3", "5.6.8", "5.6.10"]}
    ]});
    for (uri, doc) in [
        (format!("/q/views/{VIEW}"), view),
        (format!("/q/policy/{TENANT}"), policy),
    ] {
        let (status, body) = send(app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{uri}: {status} {body:?}");
    }
    write(
        app,
        &json!([car(10, "2026-01-01T00:00:00Z"),
            {"id": "urn:ngsi-ld:Radar:1", "type": "Radar", "speed": {"type": "Property", "value": 1}}]),
    )
    .await;
}

fn car(speed: i64, at: &str) -> Value {
    json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
           "name": {"type": "Property", "value": format!("car at {speed}")},
           "speed": {"type": "Property", "value": speed, "observedAt": at}})
}

async fn write(app: &Router, entities: &Value) {
    let headers = [("x-jc-client", SEEDER), ("NGSILD-Tenant", TENANT)];
    let uri = "/ngsi-ld/v1/entityOperations/upsert?options=update";
    let (status, body) = send(app, "POST", uri, &headers, Some(entities)).await;
    assert!(status.is_success(), "write: {status} {body:?}");
}

/// The HTTP read of `rest` below the view's STA root, as the public role.
async fn read(app: &Router, rest: &str) -> (StatusCode, Vec<u8>) {
    let uri = format!("/x/views/{VIEW}/sta/v1.1/{rest}");
    send(app, "GET", &uri, &[("x-jc-roles", "public")], None).await
}

/// An MQTT 3.1.1 client over a raw WebSocket.
struct Mqtt(TcpStream);

/// A string of MQTT 3.1.1 section 1.5.3.
fn string(s: &str) -> Vec<u8> {
    let mut out = u16::try_from(s.len()).unwrap_or(0).to_be_bytes().to_vec();
    out.extend_from_slice(s.as_bytes());
    out
}

fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![first];
    let mut n = body.len();
    loop {
        let byte = u8::try_from(n & 0x7f).unwrap_or(0);
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(body);
    out
}

impl Mqtt {
    /// Upgrade with the `mqtt` subprotocol and `headers`: the socket, or the answer's head.
    fn open(port: u16, headers: &[(&str, &str)]) -> Result<Self, String> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_millis(1500)))
            .map_err(|e| e.to_string())?;
        let req = format!(
            "GET /x/views/{VIEW}/sta/mqtt HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n{}\r\n",
            headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>()
        );
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut byte).map_err(|e| e.to_string())?;
            head.extend_from_slice(&byte);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        if head.starts_with("HTTP/1.1 101") && head.contains("ec-websocket-protocol: mqtt") {
            Ok(Self(s))
        } else {
            Err(head)
        }
    }

    /// One MQTT packet in one masked binary frame.
    fn send(&mut self, payload: &[u8]) {
        let key = [0x12u8, 0x34, 0x56, 0x78];
        let mut frame = vec![0x82u8];
        let n = u16::try_from(payload.len()).expect("a small packet");
        if n < 126 {
            frame.push(0x80 | u8::try_from(n).unwrap_or(0));
        } else {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&n.to_be_bytes());
        }
        frame.extend_from_slice(&key);
        frame.extend(payload.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
        let _sent = self.0.write_all(&frame);
    }

    /// The next MQTT packet (one per frame from this server); `None` on silence or an end.
    fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            let mut h = [0u8; 2];
            self.0.read_exact(&mut h).ok()?;
            let [first, second] = h;
            let len = match second & 0x7f {
                126 => {
                    let mut n = [0u8; 2];
                    self.0.read_exact(&mut n).ok()?;
                    usize::from(u16::from_be_bytes(n))
                }
                127 => {
                    let mut n = [0u8; 8];
                    self.0.read_exact(&mut n).ok()?;
                    usize::try_from(u64::from_be_bytes(n)).ok()?
                }
                n => usize::from(n),
            };
            let mut payload = vec![0u8; len];
            self.0.read_exact(&mut payload).ok()?;
            match first & 0x0f {
                0x2 => return Some(payload),
                0x8 => return None,
                _ => {}
            }
        }
    }

    fn connect(&mut self) -> Option<Vec<u8>> {
        let mut body = string("MQTT");
        body.extend_from_slice(&[4, 0x02, 0, 30]);
        body.extend(string("t-5911"));
        self.send(&packet(0x10, &body));
        self.next()
    }

    fn subscribe(&mut self, id: u16, topics: &[&str]) -> Option<Vec<u8>> {
        let mut body = id.to_be_bytes().to_vec();
        for t in topics {
            body.extend(string(t));
            body.push(0);
        }
        self.send(&packet(0x82, &body));
        self.next()
    }
}

/// A PUBLISH packet's topic and payload.
fn published(p: &[u8]) -> Option<(String, Vec<u8>)> {
    let (&first, rest) = p.split_first()?;
    if first >> 4 != 3 {
        return None;
    }
    let took = rest.iter().position(|b| b & 0x80 == 0)?.checked_add(1)?;
    let body = rest.get(took..)?;
    let n = usize::from(u16::from_be_bytes([*body.first()?, *body.get(1)?]));
    let topic = String::from_utf8(body.get(2..2 + n)?.to_vec()).ok()?;
    Some((topic, body.get(2 + n..)?.to_vec()))
}

/// The path below the STA root of a resource's `@iot.selfLink`.
fn below_root(resource: &Value) -> String {
    let link = resource["@iot.selfLink"].as_str().unwrap_or_default();
    link.split_once("/sta/v1.1/")
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or_default()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("the client")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sta_mqtt_an_ngsi_ld_write_reaches_the_observations_topic_as_the_http_read() {
    let (app, port) = broker().await;
    let (status, streams) = read(&app, "Things('urn:ngsi-ld:Car:1')/Datastreams").await;
    assert_eq!(status, StatusCode::OK);
    let streams: Value = serde_json::from_slice(&streams).expect("json");
    let speed = streams["value"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["name"].as_str().is_some_and(|n| n.contains("speed")))
        .map(below_root)
        .expect("the speed Datastream");
    let observations = format!("v1.1/{speed}/Observations");
    let topics = [
        observations.clone(),
        "v1.1/Things('urn:ngsi-ld:Radar:1')".to_owned(),
        "v1.1/#".to_owned(),
        "v1.1/Things".to_owned(),
    ];
    let mut client = blocking(move || {
        let mut c = Mqtt::open(
            port,
            &[("Sec-WebSocket-Protocol", "mqtt"), ("x-jc-roles", "public")],
        )
        .expect("the socket");
        assert_eq!(c.connect(), Some(vec![0x20, 2, 0, 0]), "CONNACK accepted");
        let refs: Vec<&str> = topics.iter().map(String::as_str).collect();
        let suback = c.subscribe(1, &refs);
        assert_eq!(
            suback,
            Some(vec![0x90, 6, 0, 1, 0, 0x80, 0x80, 0]),
            "SUBACK"
        );
        c
    })
    .await;

    // one write: the Observation on its topic, the Thing on the set topic, within a second
    let started = Instant::now();
    write(&app, &json!([car(11, "2026-01-02T00:00:00Z")])).await;
    let (mut client, messages, took) = blocking(move || {
        let mut seen = Vec::new();
        let mut took = None;
        while seen.len() < 2 {
            let Some(p) = client.next() else { break };
            if let Some(m) = published(&p) {
                if m.0.ends_with("/Observations") {
                    took = Some(started.elapsed());
                }
                seen.push(m);
            }
        }
        (client, seen, took)
    })
    .await;
    let took = took.expect("a message on the Observations topic");
    assert!(took < Duration::from_secs(1), "{took:?}");
    let on = |topic: &str| {
        messages
            .iter()
            .find(|(t, _)| t == topic)
            .map(|(_, p)| p.clone())
    };
    let obs = on(&observations).expect("the Observation");
    let obs_json: Value = serde_json::from_slice(&obs).expect("json");
    assert_eq!(obs_json["result"], json!(11), "{obs_json}");
    let (status, http) = read(&app, &below_root(&obs_json)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        String::from_utf8_lossy(&obs),
        String::from_utf8_lossy(&http),
        "the message is the HTTP read byte for byte"
    );
    let thing = on("v1.1/Things").expect("the Thing on the set topic");
    let (_, http) = read(&app, "Things('urn:ngsi-ld:Car:1')").await;
    assert_eq!(
        String::from_utf8_lossy(&thing),
        String::from_utf8_lossy(&http)
    );

    // the hidden Radar changes: nothing; unsubscribed: nothing more
    write(&app, &json!([{"id": "urn:ngsi-ld:Radar:1", "type": "Radar", "speed": {"type": "Property", "value": 2}}])).await;
    let (mut client, nothing) = blocking(move || {
        let n = client.next();
        (client, n)
    })
    .await;
    assert_eq!(nothing, None, "a hidden Entity reaches no topic");
    let unsub = {
        let mut body = 2u16.to_be_bytes().to_vec();
        body.extend(string(&observations));
        body.extend(string("v1.1/Things"));
        packet(0xa2, &body)
    };
    let mut client = blocking(move || {
        client.send(&unsub);
        assert_eq!(client.next(), Some(vec![0xb0, 2, 0, 2]), "UNSUBACK");
        client
    })
    .await;
    write(&app, &json!([car(12, "2026-01-03T00:00:00Z")])).await;
    let client = blocking(move || {
        assert_eq!(client.next(), None, "an unsubscribed topic gets nothing");
        client
    })
    .await;

    // a PUBLISH closes the connection: the extension is read only
    blocking(move || {
        let mut client = client;
        client.send(&packet(0x30, &[&string("v1.1/Things")[..], b"{}"].concat()));
        assert_eq!(client.next(), None, "closed");
        client.send(&[0xc0, 0]);
        assert_eq!(client.next(), None, "no PINGRESP after the close");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sta_mqtt_a_caller_the_policy_refuses_gets_nothing() {
    let (app, port) = broker().await;
    let (status, _) = read(&app, "Things('urn:ngsi-ld:Car:1')").await;
    assert_eq!(status, StatusCode::OK, "the public role reads it");
    // no subprotocol: not an MQTT client
    let refused = blocking(move || Mqtt::open(port, &[("x-jc-roles", "public")]).err()).await;
    assert!(
        refused
            .as_deref()
            .is_some_and(|h| h.starts_with("HTTP/1.1 400")),
        "{refused:?}"
    );
    // no grant: refused at CONNECT or at every SUBSCRIBE
    let answers = blocking(move || {
        let mut c = Mqtt::open(port, &[("Sec-WebSocket-Protocol", "mqtt")]).expect("the socket");
        let connack = c.connect();
        let suback = c.subscribe(1, &["v1.1/Things('urn:ngsi-ld:Car:1')", "v1.1/Things"]);
        (connack, suback)
    })
    .await;
    let refused_at_connect = answers.0 == Some(vec![0x20, 2, 0, 5]) && answers.1.is_none();
    let refused_per_topic = answers.0 == Some(vec![0x20, 2, 0, 0])
        && answers.1 == Some(vec![0x90, 4, 0, 1, 0x80, 0x80]);
    assert!(refused_at_connect || refused_per_topic, "{answers:?}");
    write(&app, &json!([car(13, "2026-01-04T00:00:00Z")])).await;
}
