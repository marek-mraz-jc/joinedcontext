// SPDX-License-Identifier: EUPL-1.2
//! The `SensorThings` MQTT extension (T-5911) over a real socket: CONNECT, the topics and their
//! refusals, what a write publishes, UNSUBSCRIBE, and every bound that closes a connection.
//! [`crate::common::StubSta`] scripts what each `SensorThings` read answers; the Subscriptions,
//! the notifier and the queue are the broker's own.

use crate::common::{broker, sta, until, Broker, WAIT};
use antares_ws::Limits;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const READS: [&str; 5] = [
    "query_entities",
    "create_subscription",
    "retrieve_subscription",
    "update_subscription",
    "delete_subscription",
];

/// Load view `id` of tenant `city` showing Car `speed`, allowing `ops`; its STA root answers.
async fn view(b: &Broker, id: &str, ops: &[&str]) -> Result<(), String> {
    let doc = json!({"id": id, "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}], "operations": ops});
    let (code, answer) = b
        .http
        .call("PUT", &format!("/q/views/{id}"), &[], Some(&doc))
        .await?;
    sta(&format!("{id}/"), 200, &json!({"value": []}));
    (code == 201 || code == 204)
        .then_some(())
        .ok_or(format!("view: {code} {answer}"))
}

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

/// CONNECT at `level` with `flags` and `keep_alive` seconds.
fn connect(level: u8, flags: u8, keep_alive: u16) -> Vec<u8> {
    let mut body = string("MQTT");
    body.push(level);
    body.push(flags);
    body.extend_from_slice(&keep_alive.to_be_bytes());
    body.extend(string("t-5911"));
    if flags & 0x04 != 0 {
        body.extend(string("v1.1/Things"));
        body.extend(string("gone"));
    }
    packet(0x10, &body)
}

fn subscribe(id: u16, topics: &[&str]) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    for t in topics {
        body.extend(string(t));
        body.push(1);
    }
    packet(0x82, &body)
}

fn unsubscribe(id: u16, topics: &[&str]) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    for t in topics {
        body.extend(string(t));
    }
    packet(0xa2, &body)
}

/// Upgrade `/x/views/{view}/sta/mqtt`, offering `mqtt` when `offer`.
async fn open(b: &Broker, view: &str, offer: bool) -> Result<Result<Socket, u16>, String> {
    let mut req = format!("ws://{}/x/views/{view}/sta/mqtt", b.addr)
        .into_client_request()
        .map_err(|e| e.to_string())?;
    if offer {
        req.headers_mut()
            .insert("sec-websocket-protocol", HeaderValue::from_static("mqtt"));
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(Ok(ws)),
        Err(tokio_tungstenite::tungstenite::Error::Http(refused)) => {
            Ok(Err(refused.status().as_u16()))
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn send(ws: &mut Socket, bytes: Vec<u8>) -> Result<(), String> {
    ws.send(Message::Binary(bytes.into()))
        .await
        .map_err(|e| e.to_string())
}

/// The next binary message, `None` once the server closed; an error on silence.
async fn next(ws: &mut Socket, wait: Duration) -> Result<Option<Vec<u8>>, String> {
    loop {
        match tokio::time::timeout(wait, ws.next()).await {
            Err(_) => return Err("silence".into()),
            Ok(Some(Ok(Message::Binary(b)))) => return Ok(Some(b.to_vec())),
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => return Ok(None),
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// An open, connected socket on `view`.
async fn connected(b: &Broker, view: &str, keep_alive: u16) -> Result<Socket, String> {
    let mut ws = open(b, view, true)
        .await?
        .map_err(|s| format!("refused {s}"))?;
    send(&mut ws, connect(4, 0x02, keep_alive)).await?;
    let ack = next(&mut ws, WAIT).await?;
    (ack == Some(vec![0x20, 2, 0, 0]))
        .then_some(ws)
        .ok_or(format!("CONNACK {ack:?}"))
}

/// The topic and payload of a PUBLISH.
fn published(p: &[u8]) -> (String, Value) {
    let took = p
        .iter()
        .skip(1)
        .position(|b| b & 0x80 == 0)
        .unwrap_or(0)
        .saturating_add(2);
    let body = p.get(took..).unwrap_or_default();
    let n = usize::from(u16::from_be_bytes([
        body.first().copied().unwrap_or(0),
        body.get(1).copied().unwrap_or(0),
    ]));
    let topic = String::from_utf8_lossy(body.get(2..2 + n).unwrap_or_default()).into_owned();
    let payload = serde_json::from_slice(body.get(2 + n..).unwrap_or_default()).unwrap_or_default();
    (topic, payload)
}

/// Whatever arrives within a second: nothing is expected.
async fn quiet(ws: &mut Socket) -> Result<(), String> {
    match next(ws, Duration::from_secs(1)).await {
        Err(_) => Ok(()),
        Ok(m) => Err(format!("unexpected {m:?}")),
    }
}

async fn car(b: &Broker, id: &str, speed: i64) -> Result<(), String> {
    b.http.car(Some("city"), id, speed).await
}

async fn speed(b: &Broker, id: &str, speed: i64) -> Result<(), String> {
    b.http.speed(Some("city"), id, speed).await
}

#[tokio::test(flavor = "multi_thread")]
async fn sta_mqtt_a_write_publishes_what_it_changed_on_each_topic() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, "m1", &READS).await?;
    car(&b, "urn:ngsi-ld:Car:1", 1).await?;
    let thing = "v1.1/Things('urn:ngsi-ld:Car:1')?$select=name";
    let obs = "v1.1/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations";
    let page = "m1/Datastreams('urn:ngsi-ld:Car:1/speed')/Observations?$orderby=phenomenonTime%20desc&$top=10";
    let link = |set: &str, id: &str| format!("http://h/x/views/m1/sta/v1.1/{set}('{id}')");
    sta(
        "m1/Things('urn:ngsi-ld:Car:1')?$select=name",
        200,
        &json!({"name": "a"}),
    );
    sta(page, 200, &json!({"value": []}));
    sta("m1/Things?$top=1", 200, &json!({"value": []}));
    sta("m1/Datastreams?$top=1", 200, &json!({"value": []}));
    let mut ws = connected(&b, "m1", 0).await?;
    let topics = [
        thing,
        obs,
        "v1.1/Things",
        "v1.1/Datastreams",
        "v1.1/Things('urn:ngsi-ld:Car:9')",
        "v1.1/#",
        thing,
    ];
    send(&mut ws, subscribe(1, &topics)).await?;
    let suback = next(&mut ws, WAIT).await?;
    assert_eq!(
        suback,
        Some(vec![0x90, 9, 0, 1, 0, 0, 0, 0, 0x80, 0x80, 0]),
        "the hidden Thing and the wildcard are refused"
    );
    assert_eq!(b.hub.connections(), 1);

    // the write changes the Thing, adds an Observation, and the Thing and its Datastream of
    // the whole-set topics are read for the first time
    sta(
        "m1/Things('urn:ngsi-ld:Car:1')?$select=name",
        200,
        &json!({"name": "b"}),
    );
    sta(
        page,
        200,
        &json!({"value": [{"@iot.id": "o1", "@iot.selfLink": link("Observations", "o1")}, {"@iot.id": "o0"}]}),
    );
    sta("m1/Observations('o1')", 200, &json!({"result": 2}));
    sta(
        "m1/Things('urn:ngsi-ld:Car:1')",
        200,
        &json!({"@iot.id": "urn:ngsi-ld:Car:1"}),
    );
    sta(
        "m1/Things('urn:ngsi-ld:Car:1')/Datastreams?$top=100",
        200,
        &json!({"value": [{"@iot.id": "d1", "@iot.selfLink": link("Datastreams", "d1")}]}),
    );
    sta("m1/Datastreams('d1')", 200, &json!({"name": "speed"}));
    speed(&b, "urn:ngsi-ld:Car:1", 2).await?;
    let mut seen = Vec::new();
    while seen.len() < 4 {
        let p = next(&mut ws, WAIT).await?.ok_or("closed")?;
        seen.push(published(&p));
    }
    seen.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = vec![
        (obs.to_owned(), json!({"result": 2})),
        ("v1.1/Datastreams".to_owned(), json!({"name": "speed"})),
        (
            "v1.1/Things".to_owned(),
            json!({"@iot.id": "urn:ngsi-ld:Car:1"}),
        ),
        (thing.to_owned(), json!({"name": "b"})),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(seen, want);

    // the same answers again publish nothing; a Car the set topic cannot read neither
    speed(&b, "urn:ngsi-ld:Car:1", 3).await?;
    car(&b, "urn:ngsi-ld:Car:2", 3).await?;
    quiet(&mut ws).await?;

    // a Ping frame is answered by the socket, PINGREQ by MQTT; an UNSUBSCRIBE split over two
    // frames is one packet
    ws.send(Message::Ping(Vec::new().into()))
        .await
        .map_err(|e| e.to_string())?;
    send(&mut ws, vec![0xc0, 0]).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0xd0, 0]));
    let unsub = unsubscribe(2, &[obs, "v1.1/Sensors"]);
    let (head, tail) = unsub.split_at(3);
    send(&mut ws, head.to_vec()).await?;
    send(&mut ws, tail.to_vec()).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0xb0, 2, 0, 2]));
    sta(
        page,
        200,
        &json!({"value": [{"@iot.id": "o2", "@iot.selfLink": link("Observations", "o2")}]}),
    );
    sta("m1/Observations('o2')", 200, &json!({"result": 4}));
    speed(&b, "urn:ngsi-ld:Car:1", 4).await?;
    quiet(&mut ws).await?;

    send(&mut ws, vec![0xe0, 0]).await?;
    assert_eq!(next(&mut ws, WAIT).await?, None, "DISCONNECT ends it");
    until(&b.hub, |h| h.connections() == 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn sta_mqtt_refuses_what_is_no_reading_client() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, "m2", &READS).await?;
    view(&b, "m3", &["query_entities"]).await?;
    car(&b, "urn:ngsi-ld:Car:1", 1).await?;
    assert_eq!(
        open(&b, "m2", false).await?.err(),
        Some(400),
        "no subprotocol"
    );

    // the CONNECT: a level other than 3.1.1, a Will, a caller who cannot read the root
    for (view_id, first, ack) in [
        ("m2", connect(3, 0x02, 0), vec![0x20, 2, 0, 1]),
        ("m2", connect(4, 0x06, 0), vec![0x20, 2, 0, 5]),
        ("m4", connect(4, 0x02, 0), vec![0x20, 2, 0, 5]),
    ] {
        if view_id == "m4" {
            let doc = json!({"id": "m4", "tenant": "city", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed"]}], "operations": READS});
            b.http.call("PUT", "/q/views/m4", &[], Some(&doc)).await?;
        }
        let mut ws = open(&b, view_id, true).await?.map_err(|s| s.to_string())?;
        send(&mut ws, first).await?;
        assert_eq!(next(&mut ws, WAIT).await?, Some(ack));
        assert_eq!(next(&mut ws, WAIT).await?, None, "closed after the refusal");
    }

    // a packet before the CONNECT, a second CONNECT, a PUBLISH, a text frame, a malformed
    // packet, a packet past 64 KiB: each closes the connection
    let mut ws = open(&b, "m2", true).await?.map_err(|s| s.to_string())?;
    send(&mut ws, vec![0xc0, 0]).await?;
    assert_eq!(next(&mut ws, WAIT).await?, None, "no CONNECT first");
    for (bytes, what) in [
        (connect(4, 0x02, 0), "a second CONNECT"),
        (
            packet(0x30, &[&string("v1.1/Things")[..], b"{}"].concat()),
            "a PUBLISH",
        ),
        (vec![0x10, 2, 0, 0], "a malformed packet"),
        (vec![0x82, 0x80, 0x80, 0x10], "a packet past 64 KiB"),
    ] {
        let mut ws = connected(&b, "m2", 0).await?;
        send(&mut ws, bytes).await?;
        assert_eq!(next(&mut ws, WAIT).await?, None, "{what}");
    }
    let mut ws = connected(&b, "m2", 0).await?;
    ws.send(Message::Text("{}".into()))
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(next(&mut ws, WAIT).await?, None, "a text frame");

    // a client that closes ends its session: the server drops the stream
    let mut ws = connected(&b, "m2", 0).await?;
    ws.close(None).await.map_err(|e| e.to_string())?;
    while tokio::time::timeout(WAIT, ws.next())
        .await
        .map_err(|_| "the server kept the socket after the client's Close".to_string())?
        .is_some()
    {}

    // a view of every type over a Tenant that holds nothing has no type to watch
    let doc = json!({"id": "m7", "tenant": "void", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}], "operations": READS});
    b.http.call("PUT", "/q/views/m7", &[], Some(&doc)).await?;
    sta("m7/", 200, &json!({}));
    sta("m7/Things('urn:ngsi-ld:Car:1')", 200, &json!({}));
    let mut ws = connected(&b, "m7", 0).await?;
    send(&mut ws, subscribe(4, &["v1.1/Things('urn:ngsi-ld:Car:1')"])).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0x90, 3, 0, 4, 0x80]));

    // a view that takes no Subscription refuses every topic
    sta("m3/Things('urn:ngsi-ld:Car:1')", 200, &json!({}));
    let mut ws = connected(&b, "m3", 0).await?;
    send(&mut ws, subscribe(5, &["v1.1/Things('urn:ngsi-ld:Car:1')"])).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0x90, 3, 0, 5, 0x80]));
    // ... and a whole set it cannot read
    send(&mut ws, subscribe(6, &["v1.1/Datastreams"])).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0x90, 3, 0, 6, 0x80]));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sta_mqtt_bounds_close_a_connection() -> Result<(), String> {
    let b = broker(Limits {
        max_lifetime: Duration::from_secs(4),
        subscription_lease: Duration::from_secs(3),
        message_burst: 3,
        messages_per_sec: 1,
        ..Limits::default()
    })
    .await?;
    view(&b, "m5", &READS).await?;
    car(&b, "urn:ngsi-ld:Car:1", 1).await?;
    sta("m5/Things('urn:ngsi-ld:Car:1')", 200, &json!({}));

    // past the frame rate
    let mut ws = connected(&b, "m5", 0).await?;
    for _ in 0..4 {
        send(&mut ws, vec![0xc0, 0]).await?;
    }
    let mut got = Vec::new();
    while let Some(p) = next(&mut ws, WAIT).await? {
        got.push(p);
    }
    assert!(got.len() < 4, "{got:?}");

    // keep alive 1 s: silent for 1.5 s ends it
    let mut ws = connected(&b, "m5", 1).await?;
    assert_eq!(next(&mut ws, WAIT).await?, None, "keep alive");

    // a subscribed connection renews its lease and ends at its lifetime
    let mut ws = connected(&b, "m5", 0).await?;
    send(&mut ws, subscribe(1, &["v1.1/Things('urn:ngsi-ld:Car:1')"])).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0x90, 3, 0, 1, 0]));
    assert_eq!(next(&mut ws, WAIT).await?, None, "lifetime");

    // a removed view closes its connections
    let mut ws = connected(&b, "m5", 0).await?;
    let (code, answer) = b.http.call("DELETE", "/q/views/m5", &[], None).await?;
    assert_eq!(code, 204, "{answer}");
    assert_eq!(next(&mut ws, WAIT).await?, None, "view removed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sta_mqtt_a_connection_without_connect_closes() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, "m6", &READS).await?;
    let mut ws = open(&b, "m6", true).await?.map_err(|s| s.to_string())?;
    assert_eq!(
        next(&mut ws, Duration::from_secs(15)).await?,
        None,
        "no CONNECT in 10 s"
    );
    Ok(())
}

/// F-R2ANT4-005: one notification makes at most 100 reads, its page and members together; a
/// changed member past them is published by the next notification of its topic, not lost.
#[tokio::test(flavor = "multi_thread")]
async fn sta_mqtt_one_notification_reads_a_bounded_number_of_resources() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, "m8", &READS).await?;
    car(&b, "urn:ngsi-ld:Car:1", 1).await?;
    let link = |id: &str| format!("http://h/x/views/m8/sta/v1.1/Datastreams('{id}')");
    let members: Vec<Value> = (0..100)
        .map(|i| {
            let id = format!("d{i}");
            sta(
                &format!("m8/Datastreams('{id}')"),
                200,
                &json!({"name": id}),
            );
            json!({"@iot.id": id, "@iot.selfLink": link(&id)})
        })
        .collect();
    sta(
        "m8/Things('urn:ngsi-ld:Car:1')/Datastreams?$top=100",
        200,
        &json!({"value": members}),
    );
    sta("m8/Datastreams?$top=1", 200, &json!({"value": []}));
    let mut ws = connected(&b, "m8", 0).await?;
    send(&mut ws, subscribe(1, &["v1.1/Datastreams"])).await?;
    assert_eq!(next(&mut ws, WAIT).await?, Some(vec![0x90, 3, 0, 1, 0]));
    speed(&b, "urn:ngsi-ld:Car:1", 2).await?;
    assert_eq!(drained(&mut ws).await, 99, "the page took one of 100 reads");
    speed(&b, "urn:ngsi-ld:Car:1", 3).await?;
    assert_eq!(drained(&mut ws).await, 1, "the member left over comes next");
    Ok(())
}

/// How many messages arrive before two seconds of silence.
async fn drained(ws: &mut Socket) -> usize {
    let mut n = 0_usize;
    while let Ok(Some(_)) = next(ws, Duration::from_secs(2)).await {
        n = n.saturating_add(1);
    }
    n
}
