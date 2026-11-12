// SPDX-License-Identifier: EUPL-1.2
//! T-6823: a file the store counts past `ANTARES_EXPORT_FILE_ROWS` is `413` before its first
//! byte, never a download cut at its time; a file within it comes whole. Its own binary: the
//! ceiling is read from the environment when the router is built, and the shared `ogc` binary
//! builds routers on many threads at once.
#![cfg(feature = "tabular")]

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

type Outcome = Result<(), Box<dyn std::error::Error>>;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "dc",
  "classes": {"Car": {"slots": ["speed"]}},
  "slots": {"speed": {"range": "float"}}
}"#;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items?f=csv";
const ZIP: &str = "/x/views/city/ogc/features/collections?f=zip&collections=Car";

/// The status and the bytes of one request.
async fn call(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Length", body.len())
        .header("Content-Type", "application/json")
        .header("NGSILD-Tenant", "city")
        .body(Body::from(body.to_owned()))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    (status, bytes)
}

async fn car(app: &Router, n: u32) -> Outcome {
    let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
                     "speed": {"type": "Property", "value": n}});
    let (status, body) = call(app, "POST", "/ngsi-ld/v1/entities", &car.to_string()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_past_its_rows_is_refused_before_its_first_byte() -> Outcome {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    std::env::set_var(antares_ogc::EXPORT_FILE_ROWS_ENV, "5");
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_ogc::Ogc)?;
    let view = json!({"id": "city", "tenant": "city", "selector": ["Car"],
                      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                      "operations": ["query_entities"]});
    let (status, body) = call(&app, "PUT", "/q/views/city", &view.to_string()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (status, body) = call(&app, "PUT", "/q/schemas/city", SCHEMA).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    for n in 1..=5 {
        car(&app, n).await?;
    }
    // At the ceiling: the whole file, its header and five rows.
    let (status, body) = call(&app, "GET", CARS, "").await;
    let csv = String::from_utf8(body)?;
    assert_eq!(status, StatusCode::OK, "{csv}");
    assert_eq!(
        csv.lines()
            .filter(|l| l.contains("urn:ngsi-ld:Car:"))
            .count(),
        5,
        "{csv}"
    );
    assert!(!csv.contains("# the file stopped"), "{csv}");

    car(&app, 6).await?;
    // One past it: refused before any byte of the file, with a reason a person can act on.
    for uri in [CARS, ZIP] {
        let (status, body) = call(&app, "GET", uri, "").await;
        let problem: Value = serde_json::from_slice(&body)?;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}: {problem}");
        assert!(
            problem
                .to_string()
                .contains("narrow it (bbox, datetime, filter)"),
            "{uri}: {problem}"
        );
    }
    // A narrower file of the same collection is written.
    let (status, body) = call(&app, "GET", &format!("{CARS}&filter=speed%3C6"), "").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    Ok(())
}
