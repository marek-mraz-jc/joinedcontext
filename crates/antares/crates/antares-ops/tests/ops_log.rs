// SPDX-License-Identifier: EUPL-1.2
//! Threat G10: an operation a surface runs is traced to the surface and the
//! view that ran it, one log line each, and never to the subject (ADR-0020
//! keeps it out of what the broker records). A process of its own: tracing
//! caches each callsite's interest per process.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Where this test's log lines go.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `POST /x/writer/{view}` appends `colour` to `Car:1` through the view,
/// `POST /x/writer` the same on the root of the surface's state.
struct Writer;

const APPEND: &str = "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1/attrs";

fn append() -> OpsRequest {
    let body = json!({"colour": {"type": "Property", "value": "red"}}).to_string();
    OpsRequest::new(Method::POST, APPEND)
        .header(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )
        .header(
            axum::http::header::CONTENT_LENGTH,
            axum::http::HeaderValue::from(body.len()),
        )
        .body(body)
}

impl Surface for Writer {
    fn name(&self) -> &'static str {
        "writer"
    }
    fn prefix(&self) -> &'static str {
        "/x/writer"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        let root = ops.clone();
        Router::new()
            .route(
                "/{view}",
                post(move |Path(id): Path<String>, caller: Caller| async move {
                    let Some(view) = ops.view(&id) else {
                        return StatusCode::NOT_FOUND;
                    };
                    match view
                        .run(OperationId::AppendAttributes, &caller, append())
                        .await
                    {
                        Ok(r) => r.status(),
                        Err(_) => StatusCode::FORBIDDEN,
                    }
                }),
            )
            .route(
                "/",
                post(move |caller: Caller| async move {
                    match root
                        .run(OperationId::AppendAttributes, &caller, append())
                        .await
                    {
                        Ok(r) => r.status(),
                        Err(_) => StatusCode::FORBIDDEN,
                    }
                }),
            )
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> StatusCode {
    let bytes = body.to_string();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", bytes.len());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let _ = resp.into_body().collect().await;
    status
}

#[tokio::test]
async fn a_surface_operation_is_logged_with_the_surface_and_the_view() -> Result<(), String> {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-user");
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let broker = TestBroker::new().await?;
    let app = broker.serve(Writer)?;
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 1}});
    let city = [("NGSILD-Tenant", "city")];
    assert_eq!(
        send(&app, Method::POST, "/ngsi-ld/v1/entities", &city, &car).await,
        StatusCode::CREATED
    );
    let view = json!({"id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "colour"]}],
        "operations": ["append_attributes"]});
    assert_eq!(
        send(&app, Method::PUT, "/q/views/cars", &[], &view).await,
        StatusCode::CREATED
    );

    let alice = [("x-user", "alice-7f3")];
    let through = send(&app, Method::POST, "/x/writer/cars", &alice, &json!({})).await;
    assert_eq!(through, StatusCode::NO_CONTENT);
    let root = [("x-user", "alice-7f3"), ("NGSILD-Tenant", "city")];
    assert_eq!(
        send(&app, Method::POST, "/x/writer", &root, &json!({})).await,
        StatusCode::NO_CONTENT
    );

    let log = lines
        .0
        .lock()
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let ran: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("surface operation"))
        .collect();
    assert_eq!(ran.len(), 2, "{log}");
    for want in [
        "surface=\"writer\"",
        "view=\"cars\"",
        "tenant=\"city\"",
        "operation=\"append_attributes\"",
        "status=204",
    ] {
        assert!(ran[0].contains(want), "{want} in {}", ran[0]);
    }
    assert!(
        ran[1].contains("surface=\"writer\"") && !ran[1].contains("view="),
        "{}",
        ran[1]
    );
    assert!(!log.contains("alice-7f3"), "the subject stays out: {log}");
    Ok(())
}
