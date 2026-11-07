// SPDX-License-Identifier: EUPL-1.2
//! Rules 1, 3, 4 and 9: a notification over the socket is the one every
//! binding is handed, after `pre_notify` under the subscriber, and only the
//! socket that made the Subscription receives it.

use crate::common::{
    broker, broker_asking, cars, connect, rendered, status, text, with, Client, USER,
};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ws::{Answer, Forwarded, Limits};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// No message within `wait`.
async fn silent(c: &mut Client, wait: Duration) -> Result<(), String> {
    match tokio::time::timeout(wait, c.next()).await {
        Err(_) => Ok(()),
        Ok(m) => Err(format!("expected nothing, got {m:?}")),
    }
}

/// A Subscription made over the socket notifies over the socket, with the
/// TS 104 243 envelope and `x-seq` from 1.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_subscription_made_over_the_socket_notifies_over_it() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    let made = c.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    assert_eq!(
        text(&made, "/body/notification/endpoint/uri"),
        format!("urn:antares:ws:{}", c.connection)
    );
    b.http.car(None, "urn:ngsi-ld:Car:1", 10).await?;
    let n = c.notification().await?;
    assert_eq!(n.pointer("/metadata/x-seq"), Some(&json!(1)), "{n}");
    assert_eq!(
        text(&n, "/metadata/x-subscription"),
        text(&made, "/body/id")
    );
    assert_eq!(text(&n, "/body/subscriptionId"), text(&made, "/body/id"));
    assert_eq!(
        n.pointer("/body/data/0/speed/value"),
        Some(&json!(10)),
        "{n}"
    );
    b.http.speed(None, "urn:ngsi-ld:Car:1", 11).await?;
    let n = c.notification().await?;
    assert_eq!(n.pointer("/metadata/x-seq"), Some(&json!(2)), "{n}");
    Ok(())
}

/// Drops every notification for the subject `eve`, delivers the rest.
struct NotForEve;

impl PolicyEngine for NotForEve {
    fn name(&self) -> &'static str {
        "not-for-eve"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Allow))
    }

    fn pre_notify(&self, s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        let eve = s
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(USER) && v == "eve");
        if eve {
            NotifyDecision::Drop
        } else {
            NotifyDecision::Deliver
        }
    }
}

/// W5: `pre_notify` runs under the subject of the upgrade, for every
/// notification: the same change reaches `ann`'s socket and not `eve`'s.
#[tokio::test(flavor = "multi_thread")]
async fn ws_notifications_pass_pre_notify_under_the_subscriber() -> Result<(), String> {
    let b = broker_asking(Limits::default(), Some(Arc::new(NotForEve))).await?;
    let mut ann = connect(b.addr, "/x/ws/ngsi-ld/v1", &[(USER, "ann")]).await?;
    let mut eve = connect(b.addr, "/x/ws/ngsi-ld/v1", &[(USER, "eve")]).await?;
    assert_eq!(status(&ann.subscribe(cars()).await?), 201);
    assert_eq!(status(&eve.subscribe(cars()).await?), 201);
    b.http.car(None, "urn:ngsi-ld:Car:1", 10).await?;
    ann.notification().await?;
    silent(&mut eve, Duration::from_millis(800)).await
}

/// Grants reads alone: no Subscription operation for anyone, and no read
/// for `eve`.
struct Reader;

impl PolicyEngine for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn decide<'a>(&'a self, s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let eve = s
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(USER) && v == "eve");
        let refused = op.clause.starts_with("5.8.") || (eve && op.clause == "5.7.2");
        Box::pin(std::future::ready(if refused {
            Decision::Deny(format!("{} is not granted", op.clause))
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// T-4755: a socket's own Subscription is decided as Query Entities: a
/// read grant watches, updates and ends it over the socket, while the same
/// caller makes and reads no Subscription over HTTP, and a caller who may
/// not read watches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_read_grant_watches_over_its_own_socket_alone() -> Result<(), String> {
    let b = broker_asking(Limits::default(), Some(Arc::new(Reader))).await?;
    let mut ann = connect(b.addr, "/x/ws/ngsi-ld/v1", &[(USER, "ann")]).await?;
    let made = ann.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    let id = text(&made, "/body/id");
    b.http.car(None, "urn:ngsi-ld:Car:1", 10).await?;
    ann.notification().await?;
    ann.send(
        json!({"x-op": "x-update", "x-subscription": id, "x-id": "u"}),
        Some(json!({"throttling": 5})),
    )
    .await?;
    let updated = ann.result().await?;
    assert_eq!(status(&updated), 204, "{updated}");
    let path = format!("/ngsi-ld/v1/subscriptions/{id}");
    let (code, answer) = b.http.call("GET", &path, &[(USER, "ann")], None).await?;
    assert_eq!(code, 403, "{answer}");
    let fragment = json!({"throttling": 1});
    let (code, answer) = b
        .http
        .call("PATCH", &path, &[(USER, "ann")], Some(&fragment))
        .await?;
    assert_eq!(code, 403, "{answer}");
    let (code, answer) = b.http.call("DELETE", &path, &[(USER, "ann")], None).await?;
    assert_eq!(code, 403, "{answer}");
    let http = with(
        cars(),
        &["notification", "endpoint"],
        json!({"uri": "http://127.0.0.1:9/n"}),
    );
    let (code, answer) = b
        .http
        .call(
            "POST",
            "/ngsi-ld/v1/subscriptions",
            &[(USER, "ann")],
            Some(&http),
        )
        .await?;
    assert_eq!(code, 403, "{answer}");
    ann.send(json!({"x-op": "x-unsubscribe", "x-subscription": id}), None)
        .await?;
    assert_eq!(status(&ann.result().await?), 204);
    let mut eve = connect(b.addr, "/x/ws/ngsi-ld/v1", &[(USER, "eve")]).await?;
    let refused = eve.subscribe(cars()).await?;
    assert_eq!(status(&refused), 403, "{refused}");
    Ok(())
}

/// `throttling` and `cooldown` are applied before the sink is asked: a
/// second change inside the throttling window is not sent, and a cooldown,
/// which only follows a failed notification (5.2.12), lets the second change
/// of a delivered one through.
#[tokio::test(flavor = "multi_thread")]
async fn ws_throttling_and_cooldown_are_honoured() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    for (member, second) in [("throttling", false), ("cooldown", true)] {
        let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
        let id = format!("urn:ngsi-ld:Car:{member}");
        let sub = with(cars(), &["entities"], json!([{"type": "Car", "id": id}]));
        let sub = if second {
            with(
                sub,
                &["notification", "endpoint"],
                json!({"cooldown": 30_000}),
            )
        } else {
            with(sub, &[member], json!(30))
        };
        let made = c.subscribe(sub).await?;
        assert_eq!(status(&made), 201, "{made}");
        b.http.car(None, &id, 1).await?;
        c.notification().await?;
        b.http.speed(None, &id, 2).await?;
        if second {
            let n = c.notification().await?;
            assert_eq!(
                n.pointer("/body/data/0/speed/value"),
                Some(&json!(2)),
                "{member}: {n}"
            );
        } else {
            silent(&mut c, Duration::from_millis(800)).await?;
        }
    }
    Ok(())
}

/// A tiny HTTP endpoint: the body of the first POST it receives.
async fn http_receiver() -> Result<(String, tokio::sync::oneshot::Receiver<Value>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        let body = loop {
            let Ok(n) = s.read(&mut buf).await else {
                return;
            };
            if n == 0 {
                return;
            }
            raw.extend_from_slice(buf.get(..n).unwrap_or_default());
            let text = String::from_utf8_lossy(&raw).into_owned();
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().to_owned())
                })
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= length {
                break body.to_owned();
            }
        };
        let _ok = s
            .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
            .await;
        if let Ok(v) = serde_json::from_str(&body) {
            let _sent = tx.send(v);
        }
    });
    Ok((format!("http://{addr}/notify"), rx))
}

/// The members two notifications of one change share: all but the
/// notification's own id, time and Subscription.
fn shared(n: &Value) -> Value {
    let mut n = n.clone();
    if let Some(o) = n.as_object_mut() {
        for own in ["id", "notifiedAt", "subscriptionId"] {
            o.remove(own);
        }
    }
    n
}

/// Rule 3: the body over the socket is what the HTTP binding POSTs for the
/// same change.
#[tokio::test(flavor = "multi_thread")]
async fn ws_notification_body_equals_the_http_body() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let (uri, posted) = http_receiver().await?;
    let http_sub = with(
        cars(),
        &["notification", "endpoint"],
        json!({"uri": uri, "accept": "application/json"}),
    );
    let (code, answer) = b
        .http
        .call("POST", "/ngsi-ld/v1/subscriptions", &[], Some(&http_sub))
        .await?;
    assert_eq!(code, 201, "{answer}");
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    b.http.car(None, "urn:ngsi-ld:Car:1", 10).await?;
    let over_ws = c.notification().await?;
    let over_http = tokio::time::timeout(crate::common::WAIT, posted)
        .await
        .map_err(|_| "no HTTP notification".to_owned())?
        .map_err(|e| e.to_string())?;
    let ws_body = over_ws.get("body").cloned().unwrap_or_default();
    assert_eq!(shared(&ws_body), shared(&over_http));
    Ok(())
}

/// Rule 3: the `metadata` is the MQTT binding's for the same parts:
/// `Content-Type`, `Link` and one entry per `receiverInfo` key, plus the
/// `x-` members.
#[tokio::test(flavor = "multi_thread")]
async fn ws_metadata_equals_the_mqtt_metadata() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[("NGSILD-Tenant", "city")]).await?;
    let sub = with(
        cars(),
        &["notification", "endpoint"],
        json!({"receiverInfo": [{"key": "K", "value": "V"}]}),
    );
    assert_eq!(status(&c.subscribe(sub).await?), 201);
    b.http.car(Some("city"), "urn:ngsi-ld:Car:1", 10).await?;
    let n = c.notification().await?;
    let body = n.get("body").cloned().unwrap_or_default();
    let link = text(&n, "/metadata/Link");
    assert!(
        link.contains("rel=\"http://www.w3.org/ns/json-ld#context\""),
        "{n}"
    );
    let mqtt = antares_ops::build_message(
        &body,
        "application/json",
        Some(&link),
        &[
            ("K".into(), "V".into()),
            ("NGSILD-Tenant".into(), "city".into()),
        ],
    );
    let mut metadata = n.get("metadata").cloned().unwrap_or_default();
    if let Some(m) = metadata.as_object_mut() {
        m.retain(|k, _| !k.starts_with("x-"));
    }
    assert_eq!(Some(&metadata), mqtt.get("metadata"));
    Ok(())
}

/// W6: a notification reaches only the socket whose Subscription it is.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_subscription_is_delivered_only_to_the_connection_that_created_it(
) -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut maker = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    let mut other = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    assert_eq!(status(&maker.subscribe(cars()).await?), 201);
    // the other socket naming the maker's endpoint is refused
    let stolen = with(
        cars(),
        &["notification", "endpoint"],
        json!({"uri": format!("urn:antares:ws:{}", maker.connection)}),
    );
    let refused = other.subscribe(stolen).await?;
    assert_eq!(status(&refused), 400, "{refused}");
    b.http.car(None, "urn:ngsi-ld:Car:1", 10).await?;
    maker.notification().await?;
    silent(&mut other, Duration::from_millis(800)).await
}

/// W6: the endpoint is never client-settable over HTTP, on create or
/// update, and an endpoint of another `urn` form is refused too.
#[tokio::test(flavor = "multi_thread")]
async fn ws_an_http_create_naming_a_ws_endpoint_is_refused() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    let made = c.subscribe(cars()).await?;
    let id = text(&made, "/body/id");
    for uri in [
        format!("urn:antares:ws:{}", c.connection),
        "urn:antares:ws:nobody".to_owned(),
        "urn:example:other".to_owned(),
    ] {
        let sub = with(cars(), &["notification", "endpoint"], json!({"uri": uri}));
        let (code, answer) = b
            .http
            .call("POST", "/ngsi-ld/v1/subscriptions", &[], Some(&sub))
            .await?;
        assert_eq!(code, 400, "{uri}: {answer}");
        let fragment = json!({"notification": {"endpoint": {"uri": uri}}});
        let (code, answer) = b
            .http
            .call(
                "PATCH",
                &format!("/ngsi-ld/v1/subscriptions/{id}"),
                &[],
                Some(&fragment),
            )
            .await?;
        assert_eq!(code, 400, "{uri}: {answer}");
    }
    Ok(())
}

/// Rule 4: the Subscription is an ordinary one: readable and deletable
/// over HTTP.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_subscription_is_readable_over_http() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[]).await?;
    let made = c.subscribe(cars()).await?;
    let id = text(&made, "/body/id");
    let (code, read) = b
        .http
        .call("GET", &format!("/ngsi-ld/v1/subscriptions/{id}"), &[], None)
        .await?;
    assert_eq!(code, 200, "{read}");
    assert_eq!(
        text(&read, "/notification/endpoint/uri"),
        format!("urn:antares:ws:{}", c.connection)
    );
    let (code, list) = b
        .http
        .call("GET", "/ngsi-ld/v1/subscriptions", &[], None)
        .await?;
    assert_eq!(code, 200);
    assert!(
        list.as_array()
            .is_some_and(|l| l.iter().any(|s| text(s, "/id") == id)),
        "{list}"
    );
    // updated over the socket, then deleted over it
    c.send(
        json!({"x-op": "x-update", "x-subscription": id, "x-id": "u"}),
        Some(json!({"throttling": 5})),
    )
    .await?;
    let updated = c.result().await?;
    assert_eq!(status(&updated), 204, "{updated}");
    assert_eq!(
        updated.pointer("/body/throttling"),
        Some(&json!(5)),
        "{updated}"
    );
    c.send(json!({"x-op": "x-unsubscribe", "x-subscription": id}), None)
        .await?;
    assert_eq!(status(&c.result().await?), 204);
    let (code, _) = b
        .http
        .call("GET", &format!("/ngsi-ld/v1/subscriptions/{id}"), &[], None)
        .await?;
    assert_eq!(code, 404);
    // a Subscription this socket did not make is not found over it
    c.send(
        json!({"x-op": "x-unsubscribe", "x-subscription": "urn:ngsi-ld:Subscription:x"}),
        None,
    )
    .await?;
    assert_eq!(status(&c.result().await?), 404);
    Ok(())
}

/// Rule 9: a write compares the Tenants; a notification of another Tenant
/// handed to a connection is never written.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_tenant_mismatch_is_never_written() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, "/x/ws/ngsi-ld/v1", &[("NGSILD-Tenant", "city")]).await?;
    let made = c.subscribe(cars()).await?;
    let n = Forwarded {
        tenant: "elsewhere".into(),
        subscription: text(&made, "/body/id"),
        entity: None,
        changes: false,
        rendered: rendered(&text(&made, "/body/id"), 1),
    };
    let payload = serde_json::to_vec(&n).map_err(|e| e.to_string())?;
    let answer: Answer = serde_json::from_slice(&b.hub.answer(&c.connection, &payload))
        .map_err(|e| e.to_string())?;
    assert_eq!(answer, Answer::Unknown);
    // the same notification of its own Tenant is written
    let own = Forwarded {
        tenant: "city".into(),
        ..n
    };
    let payload = serde_json::to_vec(&own).map_err(|e| e.to_string())?;
    let answer: Answer = serde_json::from_slice(&b.hub.answer(&c.connection, &payload))
        .map_err(|e| e.to_string())?;
    assert_eq!(answer, Answer::Queued);
    assert_eq!(
        c.notification().await?.pointer("/body/type"),
        Some(&json!("Notification"))
    );
    silent(&mut c, Duration::from_millis(300)).await
}
