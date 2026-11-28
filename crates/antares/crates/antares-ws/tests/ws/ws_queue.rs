// SPDX-License-Identifier: EUPL-1.2
//! Rule 6: one bounded queue per connection. A consumer that falls behind
//! is closed with `x-gap` and 1013, never buffered without bound, and a
//! single-entity notification without `showChanges` replaces its queued
//! predecessor.

use crate::common::{
    broker, broker_alone, cars, connect, connect_small, outbound, status, text, until, with,
    Client, WAIT,
};
use antares_ops::NotificationSink;
use antares_ws::{Limits, Sink, ENDPOINT_PREFIX};
use serde_json::{json, Value};
use std::time::Duration;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

/// Limits whose `resume_grace` outlasts a test that writes five changes
/// while detached, each waited for up to [`crate::common::SETTLE`]: under
/// CI load the default 30 s passed and the connection, its queue
/// included, was gone before the resume (T-5245).
fn away() -> Limits {
    Limits {
        resume_grace: Duration::from_secs(600),
        ..Limits::default()
    }
}

/// A Subscription to `Car:c`'s `speed`, `showChanges` as given.
fn one_car(show_changes: bool) -> Value {
    json!({"type": "Subscription", "entities": [{"type": "Car", "id": "urn:ngsi-ld:Car:c"}],
           "watchedAttributes": ["speed"], "notification": {"attributes": ["speed"], "showChanges": show_changes}})
}

/// The `speed` values of the notifications that follow until `count` are
/// read.
async fn speeds(c: &mut Client, count: usize) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    while out.len() < count {
        let n = c.notification().await?;
        out.push(
            n.pointer("/body/data/0/speed/value")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    Ok(out)
}

/// A client that stops reading while 120 notifications of 200 KiB each
/// are offered loses its socket, either because its queue overflowed
/// (`x-gap`, 1013) or because a write stayed stuck for `pong_within`,
/// whichever comes first; and it reads far fewer notifications than were
/// offered: the server held at most `queue` of them beyond what the socket
/// took. The offers go through the sink, as the notifier makes them, so
/// how fast the notifier matches does not decide the outcome.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_slow_consumer_is_disconnected_not_buffered() -> Result<(), String> {
    const OFFERS: usize = 120;
    let b = broker_alone(Limits {
        queue: 4,
        pong_within: Duration::from_secs(2),
        ..Limits::default()
    })
    .await?;
    let mut c = connect_small(b.addr, 16 * 1024).await?;
    let made = c.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    let mut out = outbound(Some(&text(&made, "/body/id")), None);
    // two Entities, so no offer replaces a queued one
    out.body = with(
        out.body,
        &["data"],
        json!([{"id": "urn:ngsi-ld:Car:s1", "type": "Car", "blob": {"type": "Property", "value": "b".repeat(200 * 1024)}},
               {"id": "urn:ngsi-ld:Car:s2", "type": "Car"}]),
    );
    let sink = Sink::new(b.hub.clone());
    let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);
    let mut refused = 0usize;
    for _ in 0..OFFERS {
        if sink.deliver(&endpoint, &out, WAIT).await.is_err() {
            refused = refused.saturating_add(1);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    until(&b.hub, |h| h.attached() == 0).await?;
    assert!(refused > 0, "every offer was taken");
    let mut read = 0usize;
    loop {
        match tokio::time::timeout(WAIT, c.next()).await {
            Ok(Ok(m)) if m.pointer("/metadata/x-op") == Some(&json!("x-notification")) => {
                read = read.saturating_add(1);
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => return Err("the socket stayed open".into()),
        }
    }
    assert!(read < OFFERS, "all {read} notifications were buffered");
    Ok(())
}

/// A client reading nothing on a small receive buffer, subscribed to Cars, with twenty
/// notifications of 900 KiB offered: its socket is stuck in a write the client never takes.
async fn stuck(b: &crate::common::Broker) -> Result<Client, String> {
    let mut c = connect_small(b.addr, 16 * 1024).await?;
    let made = c.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    let mut out = outbound(Some(&text(&made, "/body/id")), None);
    out.body = with(
        out.body,
        &["data"],
        json!([{"id": "urn:ngsi-ld:Car:w1", "type": "Car", "blob": {"type": "Property", "value": "w".repeat(900 * 1024)}},
               {"id": "urn:ngsi-ld:Car:w2", "type": "Car"}]),
    );
    let sink = Sink::new(b.hub.clone());
    let endpoint = format!("{ENDPOINT_PREFIX}{}", c.connection);
    for _ in 0..20 {
        let _offered = sink.deliver(&endpoint, &out, WAIT).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(c)
}

/// Read what `c` is sent for `d`, so its own socket never stalls.
async fn read_for(c: &mut Client, d: Duration) {
    let now = tokio::time::Instant::now();
    let until = now.checked_add(d).unwrap_or(now);
    while let Ok(Ok(_)) = tokio::time::timeout_at(until, c.next()).await {}
}

/// Rule 7: a socket stuck in a write when a resume takes its connection over ends as gone
/// after `pong_within`. The connection is the resumed socket's by then, so the stuck one
/// neither detaches it nor starts its grace: past the grace it still holds its Subscription.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_socket_stuck_writing_leaves_its_connection_to_the_resume() -> Result<(), String> {
    let b = broker_alone(Limits {
        pong_within: Duration::from_secs(2),
        resume_grace: Duration::from_secs(1),
        ..Limits::default()
    })
    .await?;
    let first = stuck(&b).await?;
    let mut again = connect(b.addr, ROOT, &[]).await?;
    let r = again.resume(&first.connection, &first.token, 0).await?;
    assert_eq!(status(&r), 200, "{r}");
    // the stuck write times out at 2 s, the grace it must not start would end at 3 s
    read_for(&mut again, Duration::from_secs(5)).await;
    assert_eq!((b.hub.connections(), b.hub.attached()), (1, 1));
    drop(first);
    Ok(())
}

/// Rule 7: a socket whose connection was taken over while it was stuck in a write, and whose
/// write then goes through, ends at its next heartbeat tick, the takeover's wake long taken by
/// the resumed socket; the connection stays the resumed one's.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_socket_taken_over_while_writing_ends_at_its_next_tick() -> Result<(), String> {
    let b = broker_alone(Limits {
        ping_every: Duration::from_secs(1),
        ..Limits::default()
    })
    .await?;
    let mut first = stuck(&b).await?;
    let mut again = connect(b.addr, ROOT, &[]).await?;
    let r = again.resume(&first.connection, &first.token, 0).await?;
    assert_eq!(status(&r), 200, "{r}");
    // past a ping period, the stuck client reads again: its socket ends, taken over
    read_for(&mut again, Duration::from_millis(1500)).await;
    let ended = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Err(why) = first.next().await {
                break why;
            }
        }
    })
    .await;
    assert!(
        ended
            .as_ref()
            .is_ok_and(|why| !why.starts_with("no message")),
        "the taken-over socket stayed open: {ended:?}"
    );
    assert_eq!((b.hub.connections(), b.hub.attached()), (1, 1));
    Ok(())
}

/// Five changes to one Car while its connection is detached reach the
/// resumed socket as one notification carrying the last value.
#[tokio::test(flavor = "multi_thread")]
async fn ws_coalescing_keeps_the_latest_single_entity_notification() -> Result<(), String> {
    let b = broker(away()).await?;
    b.http.car(None, "urn:ngsi-ld:Car:c", 0).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&c.subscribe(one_car(false)).await?), 201);
    let (connection, token) = (c.connection.clone(), c.token.clone());
    c.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    // each change reaches the detached queue before the next is written: a
    // notifier lane that falls behind folds waiting changes into one
    // notification, and one still on its way at the resume meets the new
    // socket (T-5245)
    for v in 1..=5u8 {
        b.http.speed(None, "urn:ngsi-ld:Car:c", v.into()).await?;
        until(&b.hub, |h| h.queued() == u64::from(v)).await?;
    }
    let mut again = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&again.resume(&connection, &token, 0).await?), 200);
    let n = again.notification().await?;
    assert_eq!(
        n.pointer("/body/data/0/speed/value"),
        Some(&json!(5)),
        "{n}"
    );
    assert_eq!(n.pointer("/metadata/x-seq"), Some(&json!(1)), "{n}");
    assert_eq!(
        n.pointer("/metadata/x-coalesced"),
        Some(&json!(4)),
        "the coalesced notification says how many changes it replaced: {n}"
    );
    b.http.speed(None, "urn:ngsi-ld:Car:c", 6).await?;
    assert_eq!(
        speeds(&mut again, 1).await?,
        [json!(6)],
        "the next change after the coalesced one"
    );
    Ok(())
}

/// A consumer that keeps up gets every change of a per-second writer as
/// its own notification, none coalesced and none marked (T-5050).
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_consumer_that_keeps_up_gets_every_change() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    b.http.car(None, "urn:ngsi-ld:Car:c", 0).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&c.subscribe(one_car(false)).await?), 201);
    for v in 1..=5 {
        b.http.speed(None, "urn:ngsi-ld:Car:c", v).await?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let mut seen = Vec::new();
    while seen.len() < 5 {
        let n = c.notification().await?;
        assert_eq!(n.pointer("/metadata/x-coalesced"), None, "{n}");
        seen.push(
            n.pointer("/body/data/0/speed/value")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    assert_eq!(seen, [json!(1), json!(2), json!(3), json!(4), json!(5)]);
    Ok(())
}

/// With `showChanges` every change is its own notification: each carries
/// a `previousValue` the next one needs.
#[tokio::test(flavor = "multi_thread")]
async fn ws_show_changes_is_never_coalesced() -> Result<(), String> {
    let b = broker(away()).await?;
    b.http.car(None, "urn:ngsi-ld:Car:c", 0).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let sub = with(
        one_car(true),
        &["notification", "format"],
        json!("normalized"),
    );
    assert_eq!(status(&c.subscribe(sub).await?), 201);
    let (connection, token) = (c.connection.clone(), c.token.clone());
    c.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    // each change reaches the detached queue before the next is written: a
    // notifier lane that falls behind folds waiting changes into one
    // notification, and one still on its way at the resume meets the new
    // socket (T-5245)
    for v in 1..=5u8 {
        b.http.speed(None, "urn:ngsi-ld:Car:c", v.into()).await?;
        until(&b.hub, |h| h.queued() == u64::from(v)).await?;
    }
    let mut again = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&again.resume(&connection, &token, 0).await?), 200);
    assert_eq!(
        speeds(&mut again, 5).await?,
        [1, 2, 3, 4, 5].map(|v| json!(v))
    );
    Ok(())
}
