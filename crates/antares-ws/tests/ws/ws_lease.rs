// SPDX-License-Identifier: EUPL-1.2
//! Rule 7a (T-4543): a Subscription made over a socket is leased, an
//! earlier `expiresAt` the client asks is kept, the heartbeat renews it, and
//! sockets that come and go leave no Subscription behind.

use crate::common::{broker, cars, connect, status, text, until, with, Broker, Client};
use antares_ws::Limits;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

fn at(v: &Value, pointer: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&text(v, pointer))
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| format!("{pointer} of {v}: {e}"))
}

async fn stored(b: &Broker, sub: &str) -> Result<Value, String> {
    let path = format!("/ngsi-ld/v1/subscriptions/{sub}");
    let (code, body) = b.http.call("GET", &path, &[], None).await?;
    assert_eq!(code, 200, "{body}");
    Ok(body)
}

async fn count(b: &Broker) -> Result<usize, String> {
    let (code, all) = b
        .http
        .call("GET", "/ngsi-ld/v1/subscriptions", &[], None)
        .await?;
    assert_eq!(code, 200, "{all}");
    Ok(all.as_array().map_or(0, Vec::len))
}

fn short() -> Limits {
    Limits {
        subscription_lease: Duration::from_millis(1500),
        ping_every: Duration::from_millis(100),
        resume_grace: Duration::from_millis(200),
        ..Limits::default()
    }
}

async fn update(c: &mut Client, sub: &str, fragment: Value) -> Result<u64, String> {
    c.send(
        json!({"x-op": "x-update", "x-subscription": sub}),
        Some(fragment),
    )
    .await?;
    Ok(status(&c.result().await?))
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_socket_subscription_is_leased_and_renewed() -> Result<(), String> {
    let b = broker(short()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let made = Utc::now();
    let r = c.subscribe(cars()).await?;
    assert_eq!(status(&r), 201, "{r}");
    let sub = text(&r, "/body/id");
    let first = at(&r, "/body/expiresAt")?;
    assert!(first > made, "{r}");
    assert!(
        first <= Utc::now() + chrono::Duration::milliseconds(1500),
        "{r}"
    );

    // a third of the lease later the heartbeat has moved it on
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let renewed = at(&stored(&b, &sub).await?, "/expiresAt")?;
    assert!(renewed > first, "{renewed} after {first}");
    assert!(renewed > Utc::now(), "a live socket's lease never lapses");

    // a client's far expiresAt is held to the lease, an earlier one kept
    let far = with(cars(), &["expiresAt"], json!("2999-01-01T00:00:00Z"));
    let r = c.subscribe(far).await?;
    assert!(
        at(&r, "/body/expiresAt")? < Utc::now() + chrono::Duration::seconds(2),
        "{r}"
    );
    let soon = (Utc::now() + chrono::Duration::milliseconds(900))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let r = c
        .subscribe(with(cars(), &["expiresAt"], json!(soon)))
        .await?;
    assert_eq!(text(&r, "/body/expiresAt"), soon, "{r}");
    let wished = text(&r, "/body/id");
    // renewal keeps the wish, never past it
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(text(&stored(&b, &wished).await?, "/expiresAt"), soon);

    // x-update: a far wish is held to the lease, removal leaves the lease
    let far = json!({"expiresAt": "2999-01-01T00:00:00Z"});
    assert_eq!(update(&mut c, &wished, far).await?, 204);
    let after = at(&stored(&b, &wished).await?, "/expiresAt")?;
    assert!(after < Utc::now() + chrono::Duration::seconds(2), "{after}");
    let removed = json!({"expiresAt": "urn:ngsi-ld:null"});
    assert_eq!(update(&mut c, &wished, removed).await?, 204);
    let after = at(&stored(&b, &wished).await?, "/expiresAt")?;
    assert!(after < Utc::now() + chrono::Duration::seconds(2), "{after}");
    assert_eq!(
        update(&mut c, &wished, json!({"expiresAt": null})).await?,
        204
    );
    // deleted over HTTP: the next renewal forgets it
    let path = format!("/ngsi-ld/v1/subscriptions/{wished}");
    let (code, body) = b.http.call("DELETE", &path, &[], None).await?;
    assert_eq!(code, 204, "{body}");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(update(&mut c, &wished, json!({})).await?, 404);
    // no DateTime is refused
    assert_eq!(update(&mut c, &sub, json!({"expiresAt": 5})).await?, 400);
    let r = c.subscribe(with(cars(), &["expiresAt"], json!(5))).await?;
    assert_eq!(status(&r), 400, "{r}");
    Ok(())
}

/// Three sockets that each subscribe and go (a gateway restarted three
/// times) leave only the live one's Subscription.
#[tokio::test(flavor = "multi_thread")]
async fn ws_sockets_that_come_and_go_leave_one_subscription() -> Result<(), String> {
    let b = broker(short()).await?;
    for _ in 0..3 {
        let mut c = connect(b.addr, ROOT, &[]).await?;
        assert_eq!(status(&c.subscribe(cars()).await?), 201);
        c.close().await;
    }
    let mut live = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&live.subscribe(cars()).await?), 201);
    // the grace deletes a connection's Subscriptions before it leaves the table
    until(&b.hub, |h| h.connections() == 1).await?;
    assert_eq!(count(&b).await?, 1);
    Ok(())
}
