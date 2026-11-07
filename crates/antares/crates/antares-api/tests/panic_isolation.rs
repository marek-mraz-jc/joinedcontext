// SPDX-License-Identifier: EUPL-1.2
//! Panic isolation (T-3531): a handler that panics answers 500
//! InternalError, its message stays out of the body, and the same router
//! goes on serving. The notification pipeline and the policy engine carry
//! their own guards and tests (`notify.rs`
//! `panicking_change_does_not_stop_the_next_notification`, `policy.rs`
//! `an_engine_that_panics_denies_rather_than_allows`).

use antares_api::{ApiSurface, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &str = "stored-value-that-must-not-leak";

/// A surface whose one route panics, mounted on the broker's own router.
struct Boom;

impl ApiSurface for Boom {
    fn name(&self) -> &str {
        "boom"
    }
    fn prefix(&self) -> &str {
        "/x"
    }
    fn router(&self, _st: AppState) -> Router<AppState> {
        Router::new().route("/boom", get(boom))
    }
    fn version_info(&self) -> Value {
        json!({})
    }
}

async fn boom() -> &'static str {
    panic!("{SECRET}")
}

async fn send(app: &Router, path: &str) -> (StatusCode, String) {
    let req = Request::get(path).body(Body::empty()).expect("req");
    let resp = app.clone().oneshot(req).await.expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_handler_answers_500_and_the_broker_keeps_serving() {
    let st = AppState::new("panic".into())
        .with_surface(Box::new(Boom))
        .expect("/x is a reserved prefix");
    let app = antares_api::router(st);
    for _ in 0..2 {
        let (code, body) = send(&app, "/x/boom").await;
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(body.contains("InternalError"), "{body}");
        assert!(!body.contains(SECRET), "the panic message leaked: {body}");
        let (code, body) = send(&app, "/ngsi-ld/v1/entities?type=Vehicle").await;
        assert_eq!(code, StatusCode::OK, "{body}");
    }
}
