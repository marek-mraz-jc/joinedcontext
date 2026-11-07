// SPDX-License-Identifier: EUPL-1.2
//! Set Attribute Value (TS 104 175 10.2.6, TS 104 176 7.6.3.2, T-3450):
//! `PUT /entities/{entityId}/attrs/{attrId}/value` with "a value only
//! Attribute Fragment as the HTTP payload request body". The type and every
//! other sub-Attribute of the stored Attribute are kept; observedAt follows
//! the request's `observedAt` parameter or goes.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const CAR: &str = "urn:ngsi-ld:Vehicle:set-value";
const REMOTE: &str = "urn:ngsi-ld:Vehicle:set-value-remote";

async fn send(st: &AppState, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(p) => b
            .header("Content-Type", "application/json")
            .header("Content-Length", p.len())
            .body(Body::from(p.to_owned()))
            .expect("request"),
        None => b.body(Body::empty()).expect("request"),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn seeded() -> AppState {
    let st = AppState::new("me".into());
    let car = json!({
        "id": CAR,
        "type": "Vehicle",
        "speed": [
            {"type": "Property", "value": 1, "unitCode": "KMH",
             "observedAt": "2020-01-01T00:00:00Z"},
            {"type": "Property", "value": 2, "datasetId": "urn:ngsi-ld:Dataset:gps"},
        ],
        "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:a"},
        "label": {"type": "LanguageProperty", "languageMap": {"en": "car"}},
        "config": {"type": "JsonProperty", "json": {"a": 1}},
    });
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(&car.to_string())).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    st
}

fn value_uri(id: &str, attr: &str, query: &str) -> String {
    format!("/ngsi-ld/v1/entities/{id}/attrs/{attr}/value{query}")
}

async fn attribute(st: &AppState, attr: &str) -> Value {
    let (code, body) = send(
        &st.clone(),
        "GET",
        &format!("/ngsi-ld/v1/entities/{CAR}"),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    body[attr].clone()
}

/// 10.2.6.4: the value is replaced; the type and the other sub-Attributes
/// stay; observedAt takes the parameter, and without it is removed.
#[tokio::test(flavor = "multi_thread")]
async fn ts104176_7_6_3_2_set_attribute_value_replaces_the_value() {
    let st = seeded().await;
    let (code, body) = send(
        &st,
        "PUT",
        &value_uri(CAR, "speed", "?observedAt=2026-10-10T10:00:00Z"),
        Some("5"),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let speed = attribute(&st, "speed").await;
    let default = speed
        .as_array()
        .and_then(|a| a.iter().find(|i| i.get("datasetId").is_none()))
        .cloned()
        .expect("the default instance");
    assert_eq!(default["value"], json!(5), "{speed}");
    assert_eq!(default["type"], "Property");
    assert_eq!(
        default["unitCode"], "KMH",
        "a sub-Attribute was dropped: {speed}"
    );
    assert_eq!(default["observedAt"], "2026-10-10T10:00:00Z");
    let gps = speed
        .as_array()
        .and_then(|a| a.iter().find(|i| i.get("datasetId").is_some()))
        .cloned()
        .expect("the gps instance");
    assert_eq!(gps["value"], json!(2), "another instance changed: {speed}");

    // no observedAt parameter: the observedAt sub-Attribute goes
    let (code, _) = send(&st, "PUT", &value_uri(CAR, "speed", ""), Some("6")).await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let (_, value) = send(&st, "GET", &value_uri(CAR, "speed", ""), None).await;
    assert!(value.to_string().contains('6'), "{value}");
    let default = attribute(&st, "speed").await;
    let default = default
        .as_array()
        .and_then(|a| a.iter().find(|i| i.get("datasetId").is_none()))
        .cloned()
        .expect("the default instance");
    assert!(default.get("observedAt").is_none(), "{default}");
    assert_eq!(default["unitCode"], "KMH");

    // the datasetId parameter selects its instance
    let (code, _) = send(
        &st,
        "PUT",
        &value_uri(CAR, "speed", "?datasetId=urn:ngsi-ld:Dataset:gps"),
        Some("7"),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    let speed = attribute(&st, "speed").await;
    let gps = speed
        .as_array()
        .and_then(|a| a.iter().find(|i| i.get("datasetId").is_some()))
        .cloned()
        .expect("the gps instance");
    assert_eq!(gps["value"], json!(7), "{speed}");

    // the member follows the stored type
    let (code, body) = send(
        &st,
        "PUT",
        &value_uri(CAR, "owner", ""),
        Some("\"urn:ngsi-ld:Person:b\""),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let owner = attribute(&st, "owner").await;
    assert_eq!(owner["type"], "Relationship");
    assert_eq!(owner["object"], "urn:ngsi-ld:Person:b", "{owner}");
    let (code, _) = send(
        &st,
        "PUT",
        &value_uri(CAR, "label", ""),
        Some(r#"{"en": "auto", "de": "Wagen"}"#),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT);
    assert_eq!(
        attribute(&st, "label").await["languageMap"],
        json!({"en": "auto", "de": "Wagen"})
    );
    // a JsonProperty may hold null (the one exception)
    let (code, body) = send(
        &st,
        "PUT",
        &value_uri(CAR, "config", ""),
        Some(r#"{"a": null}"#),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(attribute(&st, "config").await["json"], json!({"a": null}));
}

/// One case per 10.2.6.4 error bullet.
#[tokio::test(flavor = "multi_thread")]
async fn set_attribute_value_errors() {
    let st = seeded().await;
    for (uri, body, code, title) in [
        // the target Entity id is not a URI
        (
            value_uri("not-a-uri", "speed", ""),
            "5",
            400,
            "BadRequestData",
        ),
        // the target Attribute name is not valid
        (value_uri(CAR, "sp%3Ceed", ""), "5", 400, "BadRequestData"),
        // the target is scope
        (value_uri(CAR, "scope", ""), "\"/a\"", 400, "BadRequestData"),
        // not JSON, null, an NGSI-LD Null anywhere
        (value_uri(CAR, "speed", ""), "{", 400, "InvalidRequest"),
        (value_uri(CAR, "speed", ""), "null", 400, "InvalidRequest"),
        (
            value_uri(CAR, "speed", ""),
            "\"urn:ngsi-ld:null\"",
            400,
            "InvalidRequest",
        ),
        (
            value_uri(CAR, "speed", ""),
            r#"{"x": [null]}"#,
            400,
            "InvalidRequest",
        ),
        // an observedAt that is not a DateTime
        (
            value_uri(CAR, "speed", "?observedAt=yesterday"),
            "5",
            400,
            "BadRequestData",
        ),
        // the Entity, the Attribute, the instance or the type is not there
        (
            value_uri("urn:ngsi-ld:Vehicle:absent", "speed", ""),
            "5",
            404,
            "ResourceNotFound",
        ),
        (value_uri(CAR, "weight", ""), "5", 404, "ResourceNotFound"),
        (
            value_uri(CAR, "speed", "?datasetId=urn:ngsi-ld:Dataset:none"),
            "5",
            404,
            "ResourceNotFound",
        ),
        (
            value_uri(CAR, "speed", "?type=Bus"),
            "5",
            404,
            "ResourceNotFound",
        ),
        // a value the stored type cannot hold
        (value_uri(CAR, "owner", ""), "5", 400, "BadRequestData"),
    ] {
        let (got, problem) = send(&st, "PUT", &uri, Some(body)).await;
        assert_eq!(got.as_u16(), code, "{uri} {body}: {problem}");
        assert_eq!(
            problem["type"],
            format!("https://uri.etsi.org/ngsi-ld/errors/{title}"),
            "{uri} {body}: {problem}"
        );
    }
    // nothing above changed the stored value
    let speed = attribute(&st, "speed").await;
    assert!(
        speed
            .as_array()
            .is_some_and(|a| a.iter().any(|i| i["value"] == json!(1))),
        "{speed}"
    );
}

/// A Context Source recording the request line and body of every request,
/// answering 204.
fn source() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            if let Ok(mut v) = log.lock() {
                v.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
            let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        }
    });
    (port, seen)
}

/// 10.2.6.4: an exclusive registration for the target receives the value
/// only body on its own Set Attribute Value resource; nothing is local.
#[tokio::test(flavor = "multi_thread")]
async fn set_attribute_value_is_forwarded_to_an_exclusive_registration() {
    antares_jsonld::allow_private_egress(true);
    let st = AppState::new("me".into());
    let (port, seen) = source();
    let reg = json!({
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"id": REMOTE, "type": "Vehicle"}],
                         "propertyNames": ["speed"]}],
        "mode": "exclusive",
        "operations": ["replaceAttrs", "retrieveEntity"],
        "endpoint": format!("http://127.0.0.1:{port}"),
    });
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some(&reg.to_string()),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(&st, "PUT", &value_uri(REMOTE, "speed", ""), Some("42")).await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let log = seen.lock().expect("log").clone();
    let put = log
        .iter()
        .find(|r| r.starts_with("PUT "))
        .unwrap_or_else(|| panic!("nothing forwarded: {log:?}"));
    assert!(
        put.starts_with(&format!(
            "PUT /ngsi-ld/v1/entities/{REMOTE}/attrs/speed/value"
        )),
        "{put}"
    );
    assert!(put.ends_with("42"), "the value only body: {put}");
}
