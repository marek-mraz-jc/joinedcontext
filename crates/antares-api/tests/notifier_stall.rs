// SPDX-License-Identifier: EUPL-1.2
//! T-4102: a delivery that never finishes holds its own Subscription, never
//! the notifier. A drain waits for every delivery of its pass and the next
//! drain for it, so before the step deadline one stuck sink stopped every
//! later notification of the process (T-4084 on dev: 227 000 changes
//! dropped, timesSent 0).

use antares_api::AppState;
use antares_model::NgsiError;
use antares_notifier::{DeliveryError, NotificationSink, Outbound};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tower::ServiceExt;

/// `stuck://` never answers; `memory://` hands each notification over.
struct Sinks(mpsc::UnboundedSender<Value>);

impl NotificationSink for Sinks {
    fn schemes(&self) -> &'static [&'static str] {
        &["stuck", "memory"]
    }

    fn network(&self) -> bool {
        false
    }

    fn parse_endpoint(&self, _uri: &str, _info: &[(&str, &str)]) -> Result<(), NgsiError> {
        Ok(())
    }

    fn deliver<'a>(
        &'a self,
        uri: &'a str,
        out: &'a Outbound,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<(), DeliveryError>> + Send + 'a>> {
        Box::pin(async move {
            if uri.starts_with("stuck://") {
                std::future::pending::<()>().await;
            }
            self.0
                .send(out.body.clone())
                .map_err(|e| DeliveryError::failed(e.to_string()))
        })
    }
}

async fn post(st: &AppState, path: &str, doc: Value) -> StatusCode {
    let body = doc.to_string();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("req");
    antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp")
        .status()
}

fn vehicle(n: u32) -> Value {
    json!({"id": format!("urn:ngsi-ld:Vehicle:{n}"), "type": "Vehicle",
           "speed": {"type": "Property", "value": n}})
}

/// The paused clock runs to the deadline as soon as only the stuck delivery
/// is left waiting, so the test takes no minute.
#[tokio::test(start_paused = true)]
async fn a_stuck_delivery_holds_no_later_notification_of_another_subscription() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut st = AppState::new("stall".into()).with_sink(Box::new(Sinks(tx)));
    antares_api::wire(&mut st).await;
    for uri in ["stuck://a", "memory://b"] {
        let sub = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
                         "notification": {"endpoint": {"uri": uri}}});
        assert_eq!(post(&st, "subscriptions", sub).await, StatusCode::CREATED);
    }
    let mut ids = Vec::new();
    for n in [1, 2] {
        assert_eq!(post(&st, "entities", vehicle(n)).await, StatusCode::CREATED);
        let notified = tokio::time::timeout(Duration::from_secs(300), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("Vehicle {n} was never notified to memory://b"))
            .expect("a notification");
        ids.push(notified["data"][0]["id"].clone());
    }
    assert_eq!(
        ids,
        [
            json!("urn:ngsi-ld:Vehicle:1"),
            json!("urn:ngsi-ld:Vehicle:2")
        ]
    );
}
