// SPDX-License-Identifier: EUPL-1.2
//! Inbound bridge Channels (ADR-0031, T-5079) over the loopback bridge: a
//! sample reaches its Attribute through the policy seam as the binder, a
//! relay notifies and stores nothing, a bind the Policy refuses is 403, a
//! broker's own notification coming back on the topic does not loop, and
//! the size and rate caps hold.

use antares_api::channels::{Bridge, Channels, Loopback};
use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const BUS: &str = "urn:ngsi-ld:Bus:ch1";

/// Refuses every Append Attributes (5.6.3) of an Attribute named `secret`,
/// and every write at all once `closed` is set.
#[derive(Default)]
struct NoSecret {
    closed: std::sync::atomic::AtomicBool,
}

impl PolicyEngine for NoSecret {
    fn name(&self) -> &str {
        "no-secret"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let closed = self.closed.load(std::sync::atomic::Ordering::SeqCst) && op.body.is_some();
        let answer =
            if closed || op.clause == "5.6.3" && op.attrs.iter().any(|a| a.ends_with("/secret")) {
                Decision::Deny("secret is read only".into())
            } else {
                Decision::Allow
            };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn broker(policy: bool) -> (AppState, Arc<Loopback>) {
    broker_with(policy.then(|| Arc::new(NoSecret::default()))).await
}

async fn broker_with(policy: Option<Arc<NoSecret>>) -> (AppState, Arc<Loopback>) {
    antares_jsonld::allow_private_egress(true);
    let lo = Arc::new(Loopback::new());
    let mut bridges: HashMap<String, Arc<dyn Bridge>> = HashMap::new();
    bridges.insert("lo".into(), lo.clone());
    let mut st = AppState::new("test".into())
        .with_surface(Box::new(Channels::new(bridges)))
        .expect("mount");
    if let Some(engine) = policy {
        st.policy = Some(engine);
    }
    antares_api::wire(&mut st).await;
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": BUS, "type": "Bus",
                    "speed": {"type": "Property", "value": 0},
                    "secret": {"type": "Property", "value": 0}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (st, lo)
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
            b.body(Body::from(payload)).expect("req")
        }
        None => b.body(Body::empty()).expect("req"),
    };
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn channel(attribute: &str, topic: &str, retention: &str) -> Value {
    json!({"entity": BUS, "attribute": attribute, "broker": "lo",
           "topic": topic, "retention": retention})
}

fn wait() -> Duration {
    Duration::from_secs(5 * antares_api::state::slow_factor())
}

/// Polls `GET uri` until `done` holds; the last answer either way.
async fn until(st: &AppState, uri: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + wait();
    loop {
        let (_, v) = send(st, "GET", uri, None).await;
        if done(&v) || tokio::time::Instant::now() > deadline {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn samples(v: &Value, name: &str) -> u64 {
    v["status"]["samples"][name].as_u64().unwrap_or(0)
}

/// An HTTP notification endpoint that hands each body to `rx` and, when
/// `echo` names a topic, publishes the notification on it as an MQTT
/// notification subscription on that topic would (clause 7 message).
async fn subscriber(
    echo: Option<(Arc<Loopback>, String)>,
) -> (String, tokio::sync::mpsc::Receiver<Value>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(16);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move |body: axum::body::Bytes| {
            let (tx, echo) = (tx.clone(), echo.clone());
            async move {
                let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                if let Some((lo, topic)) = echo {
                    lo.publish(&topic, json!({"metadata": {}, "body": v}).to_string());
                }
                let _ = tx.send(v).await;
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}/notify"), rx)
}

/// Creates the Subscription and returns once it notifies: the matcher
/// indexes a new Subscription after the create answers, and a change made
/// before that reaches nobody. The probes write `speed` through the API.
async fn subscribe(st: &AppState, uri: &str, rx: &mut tokio::sync::mpsc::Receiver<Value>) {
    let (status, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some(
            json!({"type": "Subscription", "entities": [{"type": "Bus"}],
                    "watchedAttributes": ["speed"],
                    "notification": {"attributes": ["speed"], "endpoint": {"uri": uri}}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deadline = tokio::time::Instant::now() + wait();
    for probe in 1_i64.. {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Subscription never notified"
        );
        let value = -probe;
        let (status, body) = send(
            st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{BUS}/attrs/speed"),
            Some(json!({"type": "Property", "value": value})),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        if let Ok(Some(n)) = tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            if n["data"][0]["speed"]["value"] == value {
                break;
            }
        }
    }
    while rx.try_recv().is_ok() {}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loopback_sample_round_trips_into_the_attribute_and_its_history() {
    let (st, lo) = broker(false).await;
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-speed",
        Some(channel("speed", "bus/1/speed", "mirror")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    lo.publish("bus/1/speed", "42");
    lo.publish("bus/2/speed", "99");
    let e = until(&st, &format!("/ngsi-ld/v1/entities/{BUS}"), |v| {
        v["speed"]["value"] == 42
    })
    .await;
    assert_eq!(e["speed"]["value"], 42, "{e}");
    assert!(e["speed"]["observedAt"].is_string(), "{e}");
    let ch = until(&st, "/x/channels/bus-speed", |v| samples(v, "written") == 1).await;
    assert_eq!(samples(&ch, "written"), 1, "{ch}");
    assert_eq!(ch["retention"], "mirror", "{ch}");
    let (_, t) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{BUS}?attrs=speed"),
        None,
    )
    .await;
    let values: Vec<&Value> = t["speed"]
        .as_array()
        .map(|a| a.iter().map(|i| &i["value"]).collect())
        .unwrap_or_default();
    assert!(
        values.contains(&&json!(42)),
        "the mirrored sample is history: {t}"
    );

    let (status, list) = send(&st, "GET", "/x/channels", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(1), "{list}");
    let (status, _) = send(&st, "DELETE", "/x/channels/bus-speed", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&st, "GET", "/x/channels/bus-speed", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_notifies_and_writes_no_entity_and_no_history_row() {
    let (st, lo) = broker(false).await;
    let (uri, mut rx) = subscriber(None).await;
    subscribe(&st, &uri, &mut rx).await;
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-relay",
        Some(channel("speed", "bus/1/relay", "relay")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    lo.publish("bus/1/relay", "7");
    let n = loop {
        let n = tokio::time::timeout(wait(), rx.recv())
            .await
            .expect("the relay notifies")
            .expect("a notification");
        if n["data"][0]["speed"]["value"] == 7 {
            break n;
        }
    };
    assert_eq!(n["data"][0]["id"], BUS, "{n}");
    let (_, e) = send(&st, "GET", &format!("/ngsi-ld/v1/entities/{BUS}"), None).await;
    assert_ne!(e["speed"]["value"], 7, "a relay stores nothing: {e}");
    let (_, t) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{BUS}?attrs=speed"),
        None,
    )
    .await;
    assert!(
        !t.to_string().contains("\"value\":7"),
        "a relay records no history: {t}"
    );
    let ch = until(&st, "/x/channels/bus-relay", |v| samples(v, "written") == 1).await;
    assert_eq!(samples(&ch, "written"), 1, "{ch}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bind_the_policy_refuses_is_403_and_binds_nothing() {
    let (st, _lo) = broker(true).await;
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-secret",
        Some(channel("secret", "bus/1/secret", "mirror")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(&st, "GET", "/x/channels/bus-secret", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // the same engine lets the other Attribute bind
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-speed",
        Some(channel("speed", "bus/1/speed", "mirror")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bind_names_what_is_wrong() {
    let (st, _lo) = broker(false).await;
    let absent = json!({"entity": "urn:ngsi-ld:Bus:absent", "attribute": "speed",
                        "broker": "lo", "topic": "t", "retention": "mirror"});
    for (body, status, word) in [
        (
            json!({"direction": "out"}),
            StatusCode::BAD_REQUEST,
            "direction",
        ),
        (json!({"broker": "nope"}), StatusCode::BAD_REQUEST, "broker"),
        (
            json!({"topic": "bus/+/speed"}),
            StatusCode::BAD_REQUEST,
            "topic",
        ),
        (json!({"maxRate": 1000}), StatusCode::BAD_REQUEST, "maxRate"),
        (json!({"maxBytes": 0}), StatusCode::BAD_REQUEST, "maxBytes"),
        (
            json!({"password": "x"}),
            StatusCode::BAD_REQUEST,
            "password",
        ),
        (
            json!({"attribute": "location"}),
            StatusCode::BAD_REQUEST,
            "attribute",
        ),
    ] {
        let mut ch = channel("speed", "bus/1/x", "mirror");
        for (k, v) in body.as_object().expect("object") {
            ch[k] = v.clone();
        }
        let (got, problem) = send(&st, "PUT", "/x/channels/bad", Some(ch)).await;
        assert_eq!(got, status, "{word}: {problem}");
        assert!(problem.to_string().contains(word), "{word}: {problem}");
    }
    let (got, problem) = send(&st, "PUT", "/x/channels/absent", Some(absent)).await;
    assert_eq!(got, StatusCode::NOT_FOUND, "{problem}");
}

/// The loop both ways: a Subscription notifying the Channel's own topic
/// hands every change back to the Channel. Mirror and relay each write the
/// sample once, and the echoed notification is dropped as `looped`.
#[tokio::test(flavor = "multi_thread")]
async fn a_notification_echoed_onto_the_topic_does_not_loop() {
    for (id, retention) in [("loop-mirror", "mirror"), ("loop-relay", "relay")] {
        let (st, lo) = broker(false).await;
        let topic = format!("bus/1/{id}");
        let (uri, mut rx) = subscriber(Some((lo.clone(), topic.clone()))).await;
        subscribe(&st, &uri, &mut rx).await;
        let (status, body) = send(
            &st,
            "PUT",
            &format!("/x/channels/{id}"),
            Some(channel("speed", &topic, retention)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        lo.publish(&topic, "5");
        let ch = until(&st, &format!("/x/channels/{id}"), |v| {
            samples(v, "looped") >= 1
        })
        .await;
        assert_eq!(samples(&ch, "written"), 1, "{retention}: {ch}");
        assert!(samples(&ch, "looped") >= 1, "{retention}: {ch}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut fives = 0;
        while let Ok(n) = rx.try_recv() {
            if n["data"][0]["speed"]["value"] == 5 {
                fives += 1;
            }
        }
        assert_eq!(fives, 1, "{retention}: one change, one notification");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_size_and_rate_caps_drop_samples() {
    let (st, lo) = broker(false).await;
    let mut ch = channel("speed", "bus/1/capped", "mirror");
    ch["maxRate"] = json!(2);
    ch["maxBytes"] = json!(8);
    let (status, body) = send(&st, "PUT", "/x/channels/capped", Some(ch)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    lo.publish("bus/1/capped", "123456789");
    for v in 1..=6 {
        lo.publish("bus/1/capped", v.to_string());
    }
    let ch = until(&st, "/x/channels/capped", |v| {
        samples(v, "written") + samples(v, "overRate") == 6
    })
    .await;
    assert_eq!(samples(&ch, "tooLarge"), 1, "{ch}");
    assert!(samples(&ch, "overRate") >= 2, "{ch}");
    assert!(samples(&ch, "written") <= 4, "{ch}");
}

/// The bind is not a licence: each sample is decided again as the binder,
/// so a grant withdrawn after the bind stops the samples.
#[tokio::test(flavor = "multi_thread")]
async fn a_sample_is_decided_again_after_the_bind() {
    let engine = Arc::new(NoSecret::default());
    let (st, lo) = broker_with(Some(engine.clone())).await;
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-speed",
        Some(channel("speed", "bus/1/speed", "relay")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    engine
        .closed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    lo.publish("bus/1/speed", "9");
    let ch = until(&st, "/x/channels/bus-speed", |v| samples(v, "refused") == 1).await;
    assert_eq!(samples(&ch, "refused"), 1, "{ch}");
    assert_eq!(samples(&ch, "written"), 0, "{ch}");
}

/// F-ANT3-011: a rebind whose store write fails leaves the old binding
/// running: the new Channel replaces the running one only once it is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_rebind_the_store_refuses_keeps_the_old_binding_running() {
    let (mut st, lo) = broker(false).await;
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-speed",
        Some(channel("speed", "bus/1/speed", "mirror")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let key = format!("{}:bus-speed", antares_model::TenantId::DEFAULT);
    st.store = Arc::new(crate::common::Double::refusing_create(
        st.store.clone(),
        antares_store::Kind::View,
        &key,
    ));
    let (status, body) = send(
        &st,
        "PUT",
        "/x/channels/bus-speed",
        Some(channel("speed", "bus/9/speed", "mirror")),
    )
    .await;
    assert!(status.is_server_error(), "{status} {body}");
    lo.publish("bus/1/speed", "42");
    let e = until(&st, &format!("/ngsi-ld/v1/entities/{BUS}"), |v| {
        v["speed"]["value"] == 42
    })
    .await;
    assert_eq!(e["speed"]["value"], 42, "the old binding still runs: {e}");
}
