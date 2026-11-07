// SPDX-License-Identifier: EUPL-1.2
//! Error hygiene (surfaces-threat-model O20, odata-linkml-mapping S3): an
//! OData error names neither a hidden type or Attribute nor a literal of
//! the request, and carries no inner NGSI-LD `ProblemDetails`.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "secret"]}, "Vault": {"slots": ["secret"]}},
  "slots": {"speed": {"range": "float"}, "secret": {"range": "integer"}}
}"#;

async fn send(app: &Router, uri: &str) -> (StatusCode, String) {
    let req = Request::get(uri).body(Body::empty()).unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": "https://example.org/city/"},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string();
    for (uri, body) in [
        ("/q/views/cars", view.as_str()),
        ("/q/schemas/city", SCHEMA),
    ] {
        let req = Request::put(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body.to_owned()))
            .map_err(|e| e.to_string())?;
        let Ok(resp) = app.clone().oneshot(req).await;
        if resp.status() != StatusCode::CREATED {
            return Err(format!("{uri}: {}", resp.status()));
        }
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_errors_carry_no_hidden_name_and_no_inner_detail() {
    let (_b, app) = broker().await.expect("broker");
    for path in [
        "Vault",
        "Vault('urn:ngsi-ld:Vault:1')",
        "Car?$select=secret",
        "Car?$filter=secret%20eq%204711",
        "Car?$orderby=secret",
        "Car?$filter=speed%20eq%209007199254740993",
        "Car('not%20a%20uri%204711')",
        "Car('urn:ngsi-ld:Car:4711')",
        "Car?$filter=contains(speed,'4711')",
        "Car?$top=-4711",
        "Car?$filter=speed%20eq%20'4711",
        "Car?colour=4711",
    ] {
        let (status, body) = send(&app, &format!("{ROOT}/{path}")).await;
        assert!(
            status.is_client_error() || status.is_server_error(),
            "{path}: {status} {body}"
        );
        let error: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let keys: Vec<&str> = error
            .get("error")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["code", "message"],
            "{path}: an OData error, nothing else: {body}"
        );
        for leak in [
            "secret",
            "Vault",
            "4711",
            "https://",
            "ngsi-ld/errors",
            "detail",
        ] {
            assert!(!body.contains(leak), "{path}: {leak} in {body}");
        }
    }
}
