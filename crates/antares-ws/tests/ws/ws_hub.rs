// SPDX-License-Identifier: EUPL-1.2
//! The hub's seams without the notifier in front: the sink's refusals, a
//! delivery another pod relays (rule 9), and the queue under offers no
//! socket drains in between (rule 6). The queue tests run on one thread:
//! offers made without an `.await` between them all land before the
//! session drains any.

use crate::common::{broker, broker_alone, cars, connect, outbound, rendered, status, text, until};
use antares_ops::{NotificationSink, Surface};
use antares_ws::{
    Answer, Forwarded, Hub, Limits, Relay, Sink, StaMqtt, ViewWs, Ws, ENDPOINT_PREFIX,
};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const ROOT: &str = "/x/ws/ngsi-ld/v1";
const TIMEOUT: Duration = Duration::from_secs(1);

/// A serialized [`Forwarded`] of `subscription`, entity and value given.
fn forwarded(subscription: &str, entity: Option<&str>, n: i64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Forwarded {
        tenant: "default".into(),
        subscription: subscription.into(),
        entity: entity.map(str::to_owned),
        changes: false,
        rendered: rendered(subscription, n),
    })
    .map_err(|e| e.to_string())
}

fn answer(bytes: &[u8]) -> Result<Answer, String> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// A relay that hands every request to the hub that listened last, and
/// records what it was asked to listen to and stop listening to.
#[derive(Default)]
struct Loopback {
    hub: Mutex<Option<Weak<Hub>>>,
    seen: Mutex<Vec<String>>,
}

impl Loopback {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Relay for Loopback {
    fn request<'a>(
        &'a self,
        connection: &'a str,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        let hub = self
            .hub
            .lock()
            .ok()
            .and_then(|h| h.as_ref().and_then(Weak::upgrade));
        Box::pin(async move { hub.map(|h| h.answer(connection, &payload)) })
    }

    fn listen(&self, hub: Weak<Hub>, connection: &str) {
        if let Ok(mut h) = self.hub.lock() {
            *h = Some(hub);
        }
        if let Ok(mut s) = self.seen.lock() {
            s.push(format!("listen {connection}"));
        }
    }

    fn unlisten(&self, connection: &str) {
        if let Ok(mut s) = self.seen.lock() {
            s.push(format!("unlisten {connection}"));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_the_sink_delivers_only_to_a_known_connection() -> Result<(), String> {
    let hub = Hub::new(Limits::default());
    let sink = Sink::new(hub.clone());
    let unknown = format!("{ENDPOINT_PREFIX}nobody");
    let out = outbound(Some("urn:ngsi-ld:Subscription:s"), None);
    assert!(sink.deliver("http://x/n", &out, TIMEOUT).await.is_err());
    assert!(sink
        .deliver(&unknown, &outbound(None, None), TIMEOUT)
        .await
        .is_err());
    assert!(sink.deliver(&unknown, &out, TIMEOUT).await.is_err());
    // what another pod asks for a connection this one does not hold
    assert_eq!(answer(&hub.answer("nobody", b"not json"))?, Answer::Unknown);
    let n = forwarded("urn:ngsi-ld:Subscription:s", None, 1)?;
    assert_eq!(answer(&hub.answer("nobody", &n))?, Answer::Unknown);
    Ok(())
}

#[test]
fn ws_both_surfaces_report_their_limits() {
    for (name, info, needs_view) in [
        (
            Ws::process().name(),
            Ws::process().version_info(),
            Ws::process().needs_view(),
        ),
        (
            ViewWs::process().name(),
            ViewWs::process().version_info(),
            ViewWs::process().needs_view(),
        ),
        (
            StaMqtt::process().name(),
            StaMqtt::process().version_info(),
            StaMqtt::process().needs_view(),
        ),
    ] {
        assert_eq!(
            info.pointer("/limits/queue"),
            Some(&json!(256)),
            "{name}: {info}"
        );
        assert_eq!(info.pointer("/connections"), Some(&json!(0)), "{name}");
        assert_eq!(info.pointer("/queued"), Some(&json!(0)), "{name}");
        assert_eq!(needs_view, name != "ws");
    }
    assert_eq!(Ws::process().prefix(), "/x/ws");
}

/// `showChanges` from a notification whose Attribute holds instances.
#[test]
fn ws_previous_values_are_seen_in_multi_instance_attributes() {
    let mut out = outbound(Some("s"), Some("city"));
    out.body = json!({"subscriptionId": "s", "data": [{"id": "urn:ngsi-ld:Car:m", "type": "Car",
        "speed": [{"type": "Property", "value": 2, "previousValue": 1}]}]});
    let n = Forwarded::of(&out);
    assert_eq!(n.as_ref().map(|n| n.changes), Some(true), "{n:?}");
    assert_eq!(n.map(|n| n.tenant), Some("city".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_held_connection_answers_another_pods_relay() -> Result<(), String> {
    let b = broker(Limits {
        resume_grace: Duration::from_millis(300),
        ..Limits::default()
    })
    .await?;
    let relay = Arc::new(Loopback::default());
    b.hub.set_relay(relay.clone());
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let made = c.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    let sub = text(&made, "/body/id");
    let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);

    // the pod without the socket delivers through the relay
    let other = Hub::new(Limits::default());
    other.set_relay(relay.clone());
    let sink = Sink::new(other);
    sink.deliver(&endpoint, &outbound(Some(&sub), None), TIMEOUT)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let n = c.notification().await?;
    assert_eq!(
        n.pointer("/body/data/0/speed/value"),
        Some(&json!(7)),
        "{n}"
    );
    // rule 9: the holding pod compares the Tenants; an unknown
    // Subscription is not written either
    let city = outbound(Some(&sub), Some("city"));
    assert!(sink.deliver(&endpoint, &city, TIMEOUT).await.is_err());
    let foreign = outbound(Some("urn:ngsi-ld:Subscription:other"), None);
    assert!(sink.deliver(&endpoint, &foreign, TIMEOUT).await.is_err());
    // past the notification cap nothing is written, here or relayed
    let mut huge = outbound(Some(&sub), None);
    if let Some(car) = huge
        .body
        .pointer_mut("/data/0")
        .and_then(Value::as_object_mut)
    {
        car.insert(
            "blob".into(),
            json!({"type": "Property", "value": "b".repeat(1 << 20)}),
        );
    }
    let here = Sink::new(b.hub.clone());
    assert!(here.deliver(&endpoint, &huge, TIMEOUT).await.is_err());
    assert!(sink.deliver(&endpoint, &huge, TIMEOUT).await.is_err());
    // no pod answers
    let deaf = Hub::new(Limits::default());
    deaf.set_relay(Arc::new(Loopback::default()));
    let out = outbound(Some(&sub), None);
    assert!(Sink::new(deaf)
        .deliver(&endpoint, &out, TIMEOUT)
        .await
        .is_err());

    // the grace ends: the Subscription is deleted and the pod stops
    // answering for the connection
    let id = c.connection.clone();
    c.close().await;
    until(&b.hub, |h| h.connections() == 0).await?;
    assert_eq!(
        relay.seen(),
        vec![format!("listen {id}"), format!("unlisten {id}")]
    );
    let path = format!("/ngsi-ld/v1/subscriptions/{sub}");
    let (code, body) = b.http.call("GET", &path, &[], None).await?;
    assert_eq!(code, 404, "{body}");
    Ok(())
}

/// Two Subscriptions on one connection; their ids.
async fn two(c: &mut crate::common::Client) -> Result<(String, String), String> {
    let one = c.subscribe(cars()).await?;
    let two = c.subscribe(cars()).await?;
    assert_eq!((status(&one), status(&two)), (201, 201), "{one} {two}");
    Ok((text(&one, "/body/id"), text(&two, "/body/id")))
}

#[tokio::test]
async fn ws_coalescing_keeps_subscriptions_and_entities_apart() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let (s1, s2) = two(&mut c).await?;
    // the first carries several Entities and is never coalesced
    let offers = [
        (&s2, None, 0),
        (&s1, Some("a"), 1),
        (&s1, Some("b"), 2),
        (&s2, Some("a"), 3),
        (&s1, Some("a"), 4),
    ];
    let mut answers = Vec::new();
    for (sub, entity, n) in offers {
        answers.push(answer(
            &b.hub.answer(&c.connection, &forwarded(sub, entity, n)?),
        )?);
    }
    assert_eq!(answers, vec![Answer::Queued; 5]);
    let mut got = Vec::new();
    for _ in 0..4 {
        let n = c.notification().await?;
        got.push((text(&n, "/metadata/x-subscription"), text(&n, "/body/id")));
    }
    // the fourth replaced the first in place
    assert_eq!(
        got,
        vec![
            (s2.clone(), "urn:ngsi-ld:Notification:0".to_owned()),
            (s1.clone(), "urn:ngsi-ld:Notification:4".to_owned()),
            (s1, "urn:ngsi-ld:Notification:2".to_owned()),
            (s2, "urn:ngsi-ld:Notification:3".to_owned())
        ]
    );
    Ok(())
}

#[tokio::test]
async fn ws_an_overflow_while_attached_sends_a_gap_and_closes() -> Result<(), String> {
    let b = broker_alone(Limits {
        queue: 2,
        ..Limits::default()
    })
    .await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let (sub, _) = two(&mut c).await?;
    // two Entities each, so none is coalesced; the sink queues without
    // yielding, so the session drains none of them in between
    let sink = Sink::new(b.hub.clone());
    let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);
    let mut out = outbound(Some(&sub), None);
    if let Some(data) = out.body.get_mut("data").and_then(Value::as_array_mut) {
        data.push(json!({"id": "urn:ngsi-ld:Car:s", "type": "Car"}));
    }
    let mut delivered = Vec::new();
    for _ in 1..=3 {
        delivered.push(sink.deliver(&endpoint, &out, TIMEOUT).await.is_ok());
    }
    assert_eq!(delivered, vec![true, true, false]);
    assert_eq!(b.hub.overflows(), 1);
    let (code, seen) = c.closed().await?;
    assert_eq!(code, 1013, "{seen:?}");
    let gap = seen
        .iter()
        .find(|m| m.pointer("/metadata/x-op") == Some(&json!("x-gap")))
        .ok_or(format!("no x-gap in {seen:?}"))?;
    assert_eq!(
        (
            gap.pointer("/metadata/x-from"),
            gap.pointer("/metadata/x-to")
        ),
        (Some(&json!(1)), Some(&json!(3))),
        "{gap}"
    );
    Ok(())
}

#[tokio::test]
async fn ws_an_overflow_while_away_is_a_gap_on_resume() -> Result<(), String> {
    let b = broker(Limits {
        queue: 2,
        ..Limits::default()
    })
    .await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let (sub, _) = two(&mut c).await?;
    let (id, token) = (c.connection.clone(), c.token.clone());
    c.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    let mut answers = Vec::new();
    for n in 1..=3 {
        answers.push(answer(&b.hub.answer(&id, &forwarded(&sub, None, n)?))?);
    }
    assert_eq!(answers.last(), Some(&Answer::Overflow), "{answers:?}");

    let mut again = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&again.resume(&id, &token, 0).await?), 200);
    let gap = again.next().await?;
    assert_eq!(
        gap.pointer("/metadata/x-op"),
        Some(&json!("x-gap")),
        "{gap}"
    );
    assert_eq!(gap.pointer("/metadata/x-to"), Some(&json!(3)), "{gap}");
    Ok(())
}

/// 4.3: a notification whose message would reach the cap with the longest
/// seq is written, one byte more is refused before it takes a seq. The
/// message is rendered once with the seq it takes (T-3626), so the cap is
/// judged on that text plus the digits a larger seq would add.
#[tokio::test]
async fn ws_the_notification_cap_holds_to_the_byte() -> Result<(), String> {
    let b = broker_alone(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let (sub, _) = two(&mut c).await?;
    let padded = |n: usize| {
        let mut r = rendered(&sub, 1);
        if let Some(body) = r.get_mut("body").and_then(Value::as_object_mut) {
            body.insert("pad".into(), json!("x".repeat(n)));
        }
        r
    };
    let base = antares_ws::envelope::notification(&padded(0), u64::MAX, &sub, 0).len();
    let fits = antares_ws::MAX_NOTIFICATION_BYTES
        .checked_sub(base)
        .ok_or("an empty notification is over the cap")?;
    let offer = |n: usize| {
        serde_json::to_vec(&Forwarded {
            tenant: "default".into(),
            subscription: sub.clone(),
            entity: None,
            changes: false,
            rendered: padded(n),
        })
        .map_err(|e| e.to_string())
    };
    assert_eq!(
        answer(&b.hub.answer(&c.connection, &offer(fits + 1)?))?,
        Answer::TooLarge
    );
    assert_eq!(
        answer(&b.hub.answer(&c.connection, &offer(fits)?))?,
        Answer::Queued
    );
    let n = c.notification().await?;
    // the refused one took no seq
    assert_eq!(n.pointer("/metadata/x-seq"), Some(&json!(1)), "{n}");
    assert_eq!(
        n.pointer("/body/pad").and_then(Value::as_str).map(str::len),
        Some(fits)
    );
    Ok(())
}

/// T-4921: a notification the broker dropped before the queue (a full
/// delivery lane) takes its seq and reaches the consumer as an `x-gap` in
/// order; losses in a row share one, and the socket stays open.
#[tokio::test]
async fn ws_a_loss_before_the_queue_is_a_gap_in_order() -> Result<(), String> {
    let b = broker_alone(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let (sub, _) = two(&mut c).await?;
    let sink = Sink::new(b.hub.clone());
    let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);
    let out = outbound(Some(&sub), None);
    assert!(sink.deliver(&endpoint, &out, TIMEOUT).await.is_ok());
    sink.lost(&endpoint, "default", &sub);
    sink.lost(&endpoint, "default", &sub);
    // another Tenant's loss and an unknown Subscription's are not this one's
    assert_eq!(b.hub.lose(&c.connection, "other", &sub), Answer::Unknown);
    assert_eq!(
        b.hub.lose(&c.connection, "default", "urn:x"),
        Answer::Unknown
    );
    assert!(sink.deliver(&endpoint, &out, TIMEOUT).await.is_ok());
    let mut seen = Vec::new();
    for _ in 0..3 {
        let m = c.next().await.map_err(|e| format!("{e} after {seen:?}"))?;
        seen.push((
            text(&m, "/metadata/x-op"),
            m.pointer("/metadata/x-seq").cloned(),
            m.pointer("/metadata/x-from").cloned(),
            m.pointer("/metadata/x-to").cloned(),
        ));
    }
    assert_eq!(
        seen,
        vec![
            ("x-notification".to_owned(), Some(json!(1)), None, None),
            ("x-gap".to_owned(), None, Some(json!(2)), Some(json!(3))),
            ("x-notification".to_owned(), Some(json!(4)), None, None),
        ]
    );
    assert_eq!(b.hub.overflows(), 0);
    assert_eq!(b.hub.attached(), 1, "the socket stays open");
    Ok(())
}

/// T-4921: a loss into a full queue is an overflow like any other, and a
/// full queue that holds a gap reports the gap's whole range as lost.
#[tokio::test]
async fn ws_a_loss_into_a_full_queue_overflows() -> Result<(), String> {
    for gap_first in [false, true] {
        let b = broker_alone(Limits {
            queue: 1,
            ..Limits::default()
        })
        .await?;
        let mut c = connect(b.addr, ROOT, &[]).await?;
        let (sub, _) = two(&mut c).await?;
        let sink = Sink::new(b.hub.clone());
        let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);
        let mut out = outbound(Some(&sub), None);
        if let Some(data) = out.body.get_mut("data").and_then(Value::as_array_mut) {
            data.push(json!({"id": "urn:ngsi-ld:Car:s", "type": "Car"}));
        }
        // a binding's URI that names no connection is not this hub's
        sink.lost("urn:other", "default", &sub);
        assert_eq!(b.hub.lose("nobody", "default", &sub), Answer::Unknown);
        if gap_first {
            assert_eq!(b.hub.lose(&c.connection, "default", &sub), Answer::Queued);
            assert!(sink.deliver(&endpoint, &out, TIMEOUT).await.is_err());
        } else {
            assert!(sink.deliver(&endpoint, &out, TIMEOUT).await.is_ok());
            assert_eq!(b.hub.lose(&c.connection, "default", &sub), Answer::Overflow);
        }
        assert_eq!(b.hub.overflows(), 1);
        let (code, seen) = c.closed().await?;
        assert_eq!(code, 1013, "{seen:?}");
        let gap = seen
            .iter()
            .find(|m| m.pointer("/metadata/x-op") == Some(&json!("x-gap")))
            .ok_or(format!("no x-gap in {seen:?}"))?;
        assert_eq!(
            (
                gap.pointer("/metadata/x-from"),
                gap.pointer("/metadata/x-to")
            ),
            (Some(&json!(1)), Some(&json!(2))),
            "{gap}"
        );
    }
    Ok(())
}
