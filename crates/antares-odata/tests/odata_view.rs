// SPDX-License-Identifier: EUPL-1.2
//! A `$skiptoken` belongs to the view and the query it was issued for
//! (odata-linkml-mapping S5.2, surfaces-threat-model O4): replayed on
//! another view, carried to another query or edited beside `$skip`/`$top`,
//! it is a 400.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed"]}},
  "slots": {"speed": {"range": "float"}}
}"#;

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    for id in ["fleet-a", "fleet-b"] {
        let view = json!({
            "id": id, "tenant": "city", "@context": {"city": "https://example.org/city/"},
            "selector": ["city:Car"],
            "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
            "operations": ["query_entities", "retrieve_entity"]
        })
        .to_string();
        let (s, b) = send(&app, "PUT", &format!("/q/views/{id}"), &view, &[]).await;
        if s != StatusCode::CREATED {
            return Err(format!("{id}: {s} {b}"));
        }
    }
    let (s, body) = send(&app, "PUT", "/q/schemas/city", SCHEMA, &[]).await;
    if s != StatusCode::CREATED {
        return Err(format!("schema: {s} {body}"));
    }
    for n in 1..=3 {
        let car =
            json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "https://example.org/city/Car",
            "https://example.org/city/speed": {"type": "Property", "value": n}})
            .to_string();
        let (s, body) = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            &car,
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        if s != StatusCode::CREATED {
            return Err(format!("car {n}: {s} {body}"));
        }
    }
    Ok((b, app))
}

/// The `$skiptoken` value of the first page's next link.
async fn token(app: &Router, view: &str, query: &str) -> String {
    let uri = format!("/x/views/{view}/odata/v4/Car?{query}");
    let (s, body) = send(app, "GET", &uri, "", &[("Prefer", "odata.maxpagesize=1")]).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let next = body
        .get("@odata.nextLink")
        .and_then(Value::as_str)
        .unwrap_or_default();
    next.split("$skiptoken=")
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_a_skiptoken_of_another_view_is_400() {
    let (_b, app) = broker().await.expect("broker");
    let issued = token(&app, "fleet-a", "$count=true").await;
    let (s, body) = send(
        &app,
        "GET",
        &format!("/x/views/fleet-a/odata/v4/Car?$count=true&$skiptoken={issued}"),
        "",
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "its own view reads it: {body}");
    let (s, body) = send(
        &app,
        "GET",
        &format!("/x/views/fleet-b/odata/v4/Car?$count=true&$skiptoken={issued}"),
        "",
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "BadRequest");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_skiptoken_from_another_query_is_400() {
    let (_b, app) = broker().await.expect("broker");
    let issued = token(&app, "fleet-a", "$filter=speed%20gt%200").await;
    let root = "/x/views/fleet-a/odata/v4/Car";
    for (query, status) in [
        (
            format!("$filter=speed%20gt%200&$skiptoken={issued}"),
            StatusCode::OK,
        ),
        (
            format!("$filter=speed%20gt%201&$skiptoken={issued}"),
            StatusCode::BAD_REQUEST,
        ),
        (format!("$skiptoken={issued}"), StatusCode::BAD_REQUEST),
        (
            format!("$filter=speed%20gt%200&$skiptoken={issued}&$skip=0"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("$filter=speed%20gt%200&$top=5&$skiptoken={issued}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "$filter=speed%20gt%200&$skiptoken=garbage".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (s, body) = send(&app, "GET", &format!("{root}?{query}"), "", &[]).await;
        assert_eq!(s, status, "{query}: {body}");
    }
    // `2.{cursor}.{skip}.{top}.{digest}` (T-3758): the cursor of the first page is 1
    let edited = format!("2.2.{}", issued.trim_start_matches("2.1."));
    let (s, body) = send(
        &app,
        "GET",
        &format!("{root}?$filter=speed%20gt%200&$skiptoken={edited}"),
        "",
        &[],
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "an edited offset is a $skip, no more: {body}"
    );
    assert_eq!(body["value"].as_array().map(Vec::len), Some(1), "{body}");
}

/// OData 4.01 Part 2 section 4 and the ABNF `resourcePath`: a path segment
/// is never empty, so `Car/` is not `Car` but a malformed URL (T-3731, the
/// external compliance suite's `11.1_resource_path`). The broker's
/// trailing-slash trim, which the NGSI-LD API keeps, leaves the OData
/// service's paths as sent; the service root still answers with and without
/// its slash.
#[tokio::test(flavor = "multi_thread")]
async fn odata_an_empty_path_segment_is_400_and_the_service_root_stays() {
    let (_b, app) = broker().await.expect("broker");
    let root = "/x/views/fleet-a/odata/v4";
    for path in ["Car/", "Car//", "Car('urn:ngsi-ld:Car:1')/", "Car//speed"] {
        let (s, body) = send(&app, "GET", &format!("{root}/{path}"), "", &[]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["error"]["code"], "BadRequest", "{path}: {body}");
    }
    for uri in [root.to_owned(), format!("{root}/"), format!("{root}/Car")] {
        let (s, body) = send(&app, "GET", &uri, "", &[]).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {body}");
    }
    // the NGSI-LD API beside it keeps reading a trailing slash as absent
    let (s, body) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/?type=https://example.org/city/Car",
        "",
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
}
