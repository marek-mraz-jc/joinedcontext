// SPDX-License-Identifier: EUPL-1.2
//! Rule 8: a socket opened on a view lives as long as the view; removing
//! the view closes it with 1008 and nothing more is delivered on it.

use crate::common::{broker, cars, connect, status, Broker};
use antares_ws::Limits;
use serde_json::json;
use std::time::Duration;

/// Load view `cars` of tenant `city`: Cars show `speed`, Subscriptions
/// may be made.
async fn cars_view(b: &Broker) -> Result<(), String> {
    let view = json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities", "retrieve_entity", "create_subscription",
                       "retrieve_subscription", "delete_subscription", "update_subscription"]
    });
    let (code, answer) = b
        .http
        .call("PUT", "/q/views/cars", &[], Some(&view))
        .await?;
    (code == 201 || code == 204)
        .then_some(())
        .ok_or(format!("view: {code} {answer}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_removed_view_stops_delivery() -> Result<(), String> {
    let b = broker(Limits {
        ping_every: Duration::from_millis(200),
        ..Limits::default()
    })
    .await?;
    cars_view(&b).await?;
    let city = Some("city");
    let mut c = connect(b.addr, "/x/views/cars/ws", &[]).await?;
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    b.http.car(city, "urn:ngsi-ld:Car:v", 1).await?;
    let n = c.notification().await?;
    assert_eq!(
        n.pointer("/body/data/0/speed/value"),
        Some(&json!(1)),
        "{n}"
    );

    // a few heartbeats while the view stands and the socket holds a
    // Subscription
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (code, answer) = b.http.call("DELETE", "/q/views/cars", &[], None).await?;
    assert_eq!(code, 204, "{answer}");
    b.http.speed(city, "urn:ngsi-ld:Car:v", 2).await?;
    let (close, seen) = c.closed().await?;
    assert_eq!(close, 1008, "{seen:?}");
    assert!(
        seen.iter()
            .all(|m| m.pointer("/metadata/x-op") != Some(&json!("x-notification"))),
        "delivered after the view was removed: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|m| m.pointer("/metadata/x-reason") == Some(&json!("view-removed"))),
        "{seen:?}"
    );
    let refused = crate::common::open(b.addr, "/x/views/cars/ws", &[])
        .await?
        .err()
        .ok_or("a socket opened on the removed view")?;
    assert_eq!(refused.status(), 404);
    Ok(())
}

/// T-4084: an x-subscribe whose `q` is no NGSI-LD q is answered 400, as
/// POST /subscriptions is. A write through another view reaching the socket
/// is antares-broker's `views_matrix` test, which mounts the views surface.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_bad_q_is_refused() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    cars_view(&b).await?;
    let mut c = connect(b.addr, "/x/views/cars/ws", &[]).await?;
    let bad = c
        .subscribe(json!({"type": "Subscription", "entities": [{"type": "Car"}], "q": "speed>>"}))
        .await?;
    assert_eq!(status(&bad), 400, "{bad}");
    assert_eq!(
        bad.pointer("/body/type"),
        Some(&json!("https://uri.etsi.org/ngsi-ld/errors/BadRequestData")),
        "{bad}"
    );
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    Ok(())
}
