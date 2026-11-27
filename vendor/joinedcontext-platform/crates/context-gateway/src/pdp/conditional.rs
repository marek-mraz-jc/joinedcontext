//! The reads the gateway makes before a write, on its own account (R45, GW16).
//!
//! The broker's view decides a write and compares its entity tag (T-3644, T-3789). What the
//! gateway still reads first are the entities a write's Relationships name, so it can refuse a
//! link to one the caller cannot see, and [`state_dependent`] says whether a grant decides from
//! a stored entity rather than from the payload alone.

use crate::pdp::evaluator::Constraints;
use crate::proxy::Broker;
use axum::body::Body;
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, HeaderValue, Method};
use jc_core::ProblemDetails;
use serde_json::Value;

/// The largest answer a pre-write read will hold. One entity, generously.
const MAX_PROBE: usize = 1024 * 1024;

/// Whether the grants decide from the entity as it is stored, rather than from the payload
/// the caller sent alone (R45).
pub fn state_dependent(constraints: &Constraints) -> bool {
    constraints.q.is_some() || !constraints.geo_areas.is_empty()
}

/// One answer to a read the gateway made on its own account.
pub(crate) struct Read {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

/// Reads from the broker with the pinned tenant and nothing else the caller sent.
///
/// A fresh header map rather than the request's: the write's `Content-Length`, its media
/// type and its own `If-Match` all describe a body this read does not send.
pub(crate) async fn retrieve(
    broker: &Broker,
    target: &str,
    headers: &HeaderMap,
) -> Result<Read, Box<ProblemDetails>> {
    let mut probe = HeaderMap::new();
    // The tenant the gateway pinned, which the reads before a write carry too (GW20).
    crate::middleware::tenancy::copy_tenant(headers, &mut probe);
    probe.insert(ACCEPT, HeaderValue::from_static("application/json"));
    crate::middleware::tenancy::as_gateway(&mut probe);

    let answer = broker
        .send(Method::GET, target, probe, Body::empty())
        .await
        .map_err(|error| Box::new(ProblemDetails::from(error)))?;
    let status = answer.status().as_u16();
    let Ok(bytes) = axum::body::to_bytes(answer.into_body(), MAX_PROBE).await else {
        tracing::error!("the entity read before a write is larger than the gateway holds");
        return Err(Box::new(ProblemDetails::internal()));
    };
    Ok(Read {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::IntoFuture;

    /// A broker on a free port answering every GET with `status`, `content_type` and `body`.
    async fn broker_answering(status: u16, body: Vec<u8>) -> Broker {
        let app = axum::Router::new().fallback(axum::routing::get(move || {
            let body = body.clone();
            async move {
                (
                    axum::http::StatusCode::from_u16(status).expect("a status"),
                    [("content-type", "application/json")],
                    body,
                )
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("the bound address");
        tokio::spawn(axum::serve(listener, app).into_future());
        Broker::new(format!("http://{address}"))
    }

    /// The pre-write read answers the broker's status and JSON, a body that is no JSON as null.
    #[tokio::test]
    async fn a_read_before_a_write_answers_what_the_broker_said() {
        let found = broker_answering(200, br#"{"id":"urn:ngsi-ld:A:1"}"#.to_vec()).await;
        let read = retrieve(
            &found,
            "/ngsi-ld/v1/entities/urn:ngsi-ld:A:1",
            &HeaderMap::new(),
        )
        .await
        .expect("an answer");
        assert_eq!(read.status, 200);
        assert_eq!(read.body["id"], "urn:ngsi-ld:A:1");

        let gone = broker_answering(404, b"not json".to_vec()).await;
        let read = retrieve(
            &gone,
            "/ngsi-ld/v1/entities/urn:ngsi-ld:A:2",
            &HeaderMap::new(),
        )
        .await
        .expect("an answer");
        assert_eq!((read.status, read.body), (404, Value::Null));
    }

    /// A broker nothing listens on is the gateway's own failure answer, and one that answers more
    /// than a single entity can be is a 500 rather than held whole.
    #[tokio::test]
    async fn a_broker_that_is_down_or_answers_without_end_is_an_error_not_an_entity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let closed = listener.local_addr().expect("the bound address");
        drop(listener);
        let down = Broker::new(format!("http://{closed}"));
        let refused = retrieve(
            &down,
            "/ngsi-ld/v1/entities/urn:ngsi-ld:A:1",
            &HeaderMap::new(),
        )
        .await
        .err()
        .expect("the broker is down");
        assert!(refused.status >= 500, "{}", refused.status);

        let huge = broker_answering(200, vec![b' '; MAX_PROBE + 1]).await;
        let refused = retrieve(
            &huge,
            "/ngsi-ld/v1/entities/urn:ngsi-ld:A:1",
            &HeaderMap::new(),
        )
        .await
        .err()
        .expect("too large");
        assert_eq!(refused.status, 500);
    }
}
