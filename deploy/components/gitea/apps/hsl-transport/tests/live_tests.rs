//! The fleet from the Endpoint's WebSocket (T-3581, API/07).
//!
//! A stand-in Endpoint serves the REST read and the socket the way the gateway relays the
//! broker's view: a welcome, the subscription's result, numbered notifications, a close and a
//! resume, a gap. What is asserted is what the app sends and what the map then holds. Which
//! entities and attributes a view lets through is the broker's and the gateway's (Antares
//! T-3517, gateway T-3885), tested there.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use hsl_transport::{App, Config};
use serde_json::{json, Value};

const BUS: &str = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-01";
const TRAM: &str = "urn:ngsi-ld:Vehicle:hsl.fi:transport:tram-07";

/// What each socket the app opens is sent, frame by frame; `None` closes it without a word.
type Script = Vec<Vec<Option<Value>>>;

#[derive(Default)]
struct Endpoint {
    script: Mutex<Script>,
    /// The first message of each socket the app opened.
    first: Mutex<Vec<Value>>,
    reads: AtomicUsize,
}

fn op(op: &str, extra: Value, body: Option<Value>) -> Option<Value> {
    let mut metadata = json!({ "x-op": op });
    if let (Some(fields), Some(target)) = (extra.as_object(), metadata.as_object_mut()) {
        target.extend(fields.clone());
    }
    Some(match body {
        Some(body) => json!({ "metadata": metadata, "body": body }),
        None => json!({ "metadata": metadata }),
    })
}

fn moved(seq: u64, id: &str, longitude: f64) -> Option<Value> {
    op(
        "x-notification",
        json!({ "x-seq": seq }),
        Some(json!({ "type": "Notification", "data": [{
            "id": id, "type": "Vehicle",
            "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [longitude, 60.2] },
                          "observedAt": "2026-10-12T08:00:00Z" },
            "route": { "type": "Property", "value": "550" }
        }] })),
    )
}

async fn socket(
    State(endpoint): State<Arc<Endpoint>>,
    upgrade: WebSocketUpgrade,
) -> axum::response::Response {
    upgrade
        .protocols(["ngsi-ld-ws"])
        .on_upgrade(move |socket| converse(endpoint, socket))
}

async fn converse(endpoint: Arc<Endpoint>, mut socket: WebSocket) {
    let n = endpoint.first.lock().unwrap().len();
    let welcome = op(
        "x-welcome",
        json!({ "x-connection": format!("c{n}"), "x-resume-token": format!("t{n}") }),
        None,
    );
    let _ = socket
        .send(Message::text(welcome.unwrap().to_string()))
        .await;
    let Some(Ok(Message::Text(first))) = socket.recv().await else {
        return;
    };
    let first: Value = serde_json::from_str(&first).unwrap();
    let answer = if first["metadata"]["x-op"] == "x-resume" {
        op(
            "x-result",
            json!({ "x-id": "resume", "x-status": 200 }),
            None,
        )
    } else {
        op(
            "x-result",
            json!({ "x-id": "fleet", "x-status": 201 }),
            Some(json!({ "id": "urn:ngsi-ld:Subscription:1" })),
        )
    };
    endpoint.first.lock().unwrap().push(first);
    let _ = socket
        .send(Message::text(answer.unwrap().to_string()))
        .await;
    let frames = {
        let mut script = endpoint.script.lock().unwrap();
        if script.is_empty() {
            Vec::new()
        } else {
            script.remove(0)
        }
    };
    for frame in frames {
        match frame {
            Some(frame) => {
                let _ = socket.send(Message::text(frame.to_string())).await;
            }
            None => return,
        }
    }
    // Hold the socket open, as a live feed does.
    while socket.recv().await.is_some() {}
}

async fn entities(State(endpoint): State<Arc<Endpoint>>) -> Json<Value> {
    endpoint.reads.fetch_add(1, Ordering::SeqCst);
    Json(json!({ "type": "FeatureCollection", "features": [{
        "id": TRAM, "type": "Feature",
        "geometry": { "type": "Point", "coordinates": [24.9, 60.1] },
        "properties": { "type": "Vehicle", "transportMode": { "type": "Property", "value": "tram" } }
    }] }))
}

async fn start(script: Script) -> (Arc<App>, Arc<Endpoint>) {
    let endpoint = Arc::new(Endpoint {
        script: Mutex::new(script),
        ..Endpoint::default()
    });
    let routes = Router::new()
        .route("/api/endpoint/s/ws", get(socket))
        .route("/api/endpoint/s/ngsi-ld/v1/entities", get(entities))
        .with_state(Arc::clone(&endpoint));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, routes).await });
    let app = Arc::new(App::new(Config {
        base_path: "/".to_owned(),
        endpoint_url: format!("http://{address}/api/endpoint/s/"),
        page_config: None,
    }));
    tokio::spawn(Arc::clone(&app).follow());
    (app, endpoint)
}

/// Waits until `done` holds, at most five seconds.
async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

fn at(app: &App, id: &str) -> Option<[f64; 2]> {
    app.snapshot()
        .into_iter()
        .find(|v| v.id == id)
        .map(|v| v.coordinates)
}

#[tokio::test]
async fn the_socket_subscribes_reads_once_and_moves_the_buses() {
    let (app, endpoint) = start(vec![vec![moved(1, BUS, 25.0), moved(2, BUS, 25.1)]]).await;
    until("the bus moved twice", || {
        at(&app, BUS) == Some([25.1, 60.2])
    })
    .await;
    let first = endpoint.first.lock().unwrap()[0].clone();
    assert_eq!(first["metadata"]["x-op"], "x-subscribe");
    assert_eq!(first["body"]["entities"], json!([{ "type": "Vehicle" }]));
    assert_eq!(first["body"]["watchedAttributes"], json!(["location"]));
    // One full read after the subscription, so a vehicle that has not moved yet is drawn.
    assert_eq!(at(&app, TRAM), Some([24.9, 60.1]));
    assert_eq!(endpoint.reads.load(Ordering::SeqCst), 1);
    assert_eq!(app.feed_lost_since(), None);
    assert!(app.metrics.text(0).contains("hsl_ws_notifications_total 2"));
}

#[tokio::test]
async fn a_dropped_socket_resumes_from_the_last_seq_and_reads_nothing_again() {
    let (app, endpoint) = start(vec![
        vec![moved(1, BUS, 25.0), moved(2, BUS, 25.1), None],
        vec![moved(1, BUS, 25.2)],
    ])
    .await;
    until("the bus moved after the resume", || {
        at(&app, BUS) == Some([25.2, 60.2])
    })
    .await;
    let resume = endpoint.first.lock().unwrap()[1].clone();
    assert_eq!(resume["metadata"]["x-op"], "x-resume");
    assert_eq!(resume["metadata"]["x-connection"], "c0");
    assert_eq!(resume["metadata"]["x-resume-token"], "t0");
    assert_eq!(resume["metadata"]["x-seq"], 2);
    assert_eq!(
        endpoint.reads.load(Ordering::SeqCst),
        1,
        "a resume reads nothing again"
    );
    until("the feed is live again", || app.feed_lost_since().is_none()).await;
}

#[tokio::test]
async fn a_gap_reads_the_fleet_again() {
    let (app, endpoint) = start(vec![vec![
        moved(1, BUS, 25.0),
        op("x-gap", json!({ "x-from": 2, "x-to": 9 }), None),
    ]])
    .await;
    until("the gap read", || {
        endpoint.reads.load(Ordering::SeqCst) == 2
    })
    .await;
    assert_eq!(at(&app, BUS), Some([25.0, 60.2]));
    until("the feed is live again", || app.feed_lost_since().is_none()).await;
    assert!(app.metrics.text(0).contains("hsl_ws_gaps_total 1"));
}

#[tokio::test]
async fn no_endpoint_is_a_lost_feed_with_its_time() {
    let app = Arc::new(App::new(Config {
        base_path: "/".to_owned(),
        endpoint_url: "http://127.0.0.1:9/api/endpoint/s/".to_owned(),
        page_config: None,
    }));
    tokio::spawn(Arc::clone(&app).follow());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let since = app.feed_lost_since().expect("lost");
    assert!(since.ends_with('Z'), "{since}");
    assert!(app.snapshot().is_empty());
}
