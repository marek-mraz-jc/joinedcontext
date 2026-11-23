// SPDX-License-Identifier: EUPL-1.2
//! `views::default_schema_max`: the schema bound a host names when its environment leaves
//! `ANTARES_SCHEMA_MAX_BYTES` unset (`jc` passes dev's value). A binary of its own, since the
//! default is process-wide and only its first setting counts.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Ops, Surface};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;

/// No route of its own: the schema writes go to the broker's `/q`.
struct Bare;

impl Surface for Bare {
    fn name(&self) -> &'static str {
        "bare"
    }
    fn prefix(&self) -> &'static str {
        "/x/bare"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: Ops) -> Router {
        Router::new()
    }
}

async fn put(app: &Router, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/q/schemas/city")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body.to_owned()))
        .unwrap_or_default();
    app.clone()
        .oneshot(req)
        .await
        .map_or(StatusCode::IM_A_TEAPOT, |r| r.status())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hosts_default_bounds_a_schema_and_only_its_first_setting_counts() {
    std::env::remove_var(antares_ops::views::SCHEMA_MAX_ENV);
    antares_ops::views::default_schema_max("64");
    antares_ops::views::default_schema_max("1");
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve(Bare).expect("app");
    let at_max = format!("\"{}\"", "a".repeat(62));
    assert_eq!(put(&app, &at_max).await, StatusCode::CREATED, "64 bytes");
    let over = format!("\"{}\"", "a".repeat(63));
    assert_eq!(
        put(&app, &over).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "65 bytes"
    );
}
