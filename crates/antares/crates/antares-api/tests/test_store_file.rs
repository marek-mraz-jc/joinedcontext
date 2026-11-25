// SPDX-License-Identifier: EUPL-1.2
//! The harness switch `ANTARES_TEST_STORE=file`: `AppState::new` composes a
//! fresh on-disk redb store instead of the in-memory one, and the broker
//! serves from it as from any store. A binary of its own: the variable is
//! read by every `AppState::new` of the process.
#![cfg(feature = "test-kit")]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn the_file_harness_store_serves_what_it_was_given() {
    std::env::set_var("ANTARES_TEST_STORE", "file");
    let st = AppState::new("file-harness".into());
    assert_eq!(st.store_name, "file");
    let body = json!({"id": "urn:ngsi-ld:V:file", "type": "V",
                      "speed": {"type": "Property", "value": 3}})
    .to_string();
    let created = antares_api::router(st.clone())
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(created.status(), StatusCode::CREATED);
    let read = antares_api::router(st)
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:V:file")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(read.status(), StatusCode::OK);
}
