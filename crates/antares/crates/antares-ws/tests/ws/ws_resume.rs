// SPDX-License-Identifier: EUPL-1.2
//! Rule 7: a connection outlives its socket for `resume_grace`; a resume by
//! the same subject replays after the last `x-seq` it saw, with `x-gap` for
//! what the ring lost, and nobody else can resume it.

use crate::common::{broker, cars, connect, status, until, Client, USER};
use antares_ws::Limits;
use serde_json::{json, Value};
use std::time::Duration;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

/// The `x-seq` of the next `count` notifications.
async fn seqs(c: &mut Client, count: usize) -> Result<Vec<u64>, String> {
    let mut out = Vec::new();
    while out.len() < count {
        let n = c.notification().await?;
        out.push(
            n.pointer("/metadata/x-seq")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
    Ok(out)
}

/// A connection with one Subscription to Cars that has seen `seen`
/// notifications (speeds 1..=seen on `Car:r`), then dropped its socket.
async fn seen_then_dropped(
    b: &crate::common::Broker,
    headers: &[(&str, &str)],
    seen: u64,
) -> Result<(String, String), String> {
    b.http.car(None, "urn:ngsi-ld:Car:r", 0).await?;
    let mut c = connect(b.addr, ROOT, headers).await?;
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    // each read before the next change: two changes queued together
    // would coalesce into one notification (rule 6)
    let mut got = Vec::new();
    for v in 1..=seen {
        b.http
            .speed(
                None,
                "urn:ngsi-ld:Car:r",
                i64::try_from(v).map_err(|e| e.to_string())?,
            )
            .await?;
        got.extend(seqs(&mut c, 1).await?);
    }
    assert_eq!(got, (1..=seen).collect::<Vec<_>>());
    let ids = (c.connection.clone(), c.token.clone());
    c.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    Ok(ids)
}

/// A resume after `x-seq` 2 replays 3 and 4 from the ring, then what was
/// queued while away, in order, without a gap.
#[tokio::test(flavor = "multi_thread")]
async fn ws_resume_replays_after_the_last_seq() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let (connection, token) = seen_then_dropped(&b, &[], 4).await?;
    b.http.speed(None, "urn:ngsi-ld:Car:r", 50).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let r = c.resume(&connection, &token, 2).await?;
    assert_eq!(status(&r), 200, "{r}");
    assert_eq!(seqs(&mut c, 3).await?, [3, 4, 5]);
    assert_eq!(
        b.hub.connections(),
        1,
        "the socket's own connection is closed on resume"
    );
    Ok(())
}

/// The grace of a socket that a resume replaced ends nothing: the Subscription
/// lives until the grace of the last socket to detach runs out.
#[tokio::test(flavor = "multi_thread")]
async fn ws_only_the_last_detached_socket_s_grace_deletes() -> Result<(), String> {
    let b = broker(Limits {
        resume_grace: Duration::from_millis(1500),
        ..Limits::default()
    })
    .await?;
    let mut first = connect(b.addr, ROOT, &[]).await?;
    let made = first.subscribe(cars()).await?;
    assert_eq!(status(&made), 201, "{made}");
    let sub = made
        .pointer("/body/id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let ids = (first.connection.clone(), first.token.clone());
    first.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    let start = std::time::Instant::now();
    let mut second = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&second.resume(&ids.0, &ids.1, 0).await?), 200);
    // the second socket detaches 1 s in, so its grace runs to about 2.5 s
    tokio::time::sleep(Duration::from_secs(1).saturating_sub(start.elapsed())).await;
    second.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    // past the first grace (1.5 s), well before the second's end
    tokio::time::sleep(Duration::from_millis(1900).saturating_sub(start.elapsed())).await;
    let path = format!("/ngsi-ld/v1/subscriptions/{sub}");
    let (code, body) = b.http.call("GET", &path, &[], None).await?;
    assert_eq!(code, 200, "{body}");
    Ok(())
}

/// A resume from another socket while the first is still open takes the
/// connection over: the first socket is ended, not left open beside it.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_resume_elsewhere_ends_the_open_socket() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut old = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&old.subscribe(cars()).await?), 201);
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let r = c.resume(&old.connection, &old.token, 0).await?;
    assert_eq!(status(&r), 200, "{r}");
    let ended = old.closed().await;
    assert!(
        !matches!(&ended, Err(e) if e.contains("not closed")),
        "{ended:?}"
    );
    Ok(())
}

/// The ring keeps the last `queue` notifications: resuming after 0 when 6
/// were sent with a ring of 4 starts with `x-gap` 1..2, then 3..6.
#[tokio::test(flavor = "multi_thread")]
async fn ws_resume_past_the_ring_sends_a_gap() -> Result<(), String> {
    let b = broker(Limits {
        queue: 4,
        ..Limits::default()
    })
    .await?;
    let (connection, token) = seen_then_dropped(&b, &[], 6).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&c.resume(&connection, &token, 0).await?), 200);
    let gap = c.next().await?;
    assert_eq!(
        gap.pointer("/metadata/x-op"),
        Some(&json!("x-gap")),
        "{gap}"
    );
    assert_eq!(gap.pointer("/metadata/x-from"), Some(&json!(1)), "{gap}");
    assert_eq!(gap.pointer("/metadata/x-to"), Some(&json!(2)), "{gap}");
    assert_eq!(seqs(&mut c, 4).await?, [3, 4, 5, 6]);
    Ok(())
}

/// The connection and its token are not enough: another subject, a wrong
/// token, an unknown connection and a resume that is not the first message
/// are each answered 404, and the owner can still resume afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn ws_resume_with_another_subject_is_not_found() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let alice = [(USER, "alice")];
    let (connection, token) = seen_then_dropped(&b, &alice, 1).await?;
    for (who, conn, tok) in [
        ("bob", connection.as_str(), token.as_str()),
        ("", connection.as_str(), token.as_str()),
        ("alice", connection.as_str(), "x"),
        ("alice", "nope", token.as_str()),
    ] {
        let headers: Vec<(&str, &str)> = (!who.is_empty())
            .then_some((USER, who))
            .into_iter()
            .collect();
        let mut c = connect(b.addr, ROOT, &headers).await?;
        let r = c.resume(conn, tok, 0).await?;
        assert_eq!(status(&r), 404, "{who} {conn}: {r}");
    }
    let mut late = connect(b.addr, ROOT, &alice).await?;
    assert_eq!(status(&late.subscribe(cars()).await?), 201);
    assert_eq!(
        status(&late.resume(&connection, &token, 0).await?),
        404,
        "not the first message"
    );
    let mut c = connect(b.addr, ROOT, &alice).await?;
    assert_eq!(status(&c.resume(&connection, &token, 1).await?), 200);
    Ok(())
}

/// A socket open for `max_lifetime` is closed with `x-closing` "lifetime"
/// and 1000, and the connection can be resumed on a new one.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_connection_ends_at_its_max_lifetime() -> Result<(), String> {
    let b = broker(Limits {
        max_lifetime: Duration::from_secs(1),
        ..Limits::default()
    })
    .await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    let (code, seen) = c.closed().await?;
    assert_eq!(code, 1000);
    assert!(
        seen.iter()
            .any(|m| m.pointer("/metadata/x-op") == Some(&json!("x-closing"))
                && m.pointer("/metadata/x-reason") == Some(&json!("lifetime"))),
        "{seen:?}"
    );
    until(&b.hub, |h| h.attached() == 0).await?;
    let mut again = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(
        status(&again.resume(&c.connection, &c.token, 0).await?),
        200
    );
    Ok(())
}
