// SPDX-License-Identifier: EUPL-1.2
//! The fixture of the write tests: a Tenant `city` with Cars and Persons,
//! view `cars` that offers the writes and view `reads` that does not.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

pub const CITY: &str = "https://example.org/city/";
pub const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"},
               "linkml": {"prefix_prefix": "linkml", "prefix_reference": "https://w3id.org/linkml/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "label", "owner", "drivers", "garage", "location", "notes", "colour",
                                "route", "tags", "ownerRef", "address", "reading"]},
              "Person": {"slots": ["name"]}, "Garage": {"slots": ["name"]}, "Address": {"slots": ["street"]},
              "Any": {"class_uri": "linkml:Any"}},
  "slots": {"speed": {"range": "float"}, "label": {"range": "string"}, "owner": {"range": "Person", "annotations": {"odata_referential_constraint":
                      {"tag": "odata_referential_constraint", "value": "ownerRef"}}},
            "ownerRef": {"range": "string"}, "street": {},
            "address": {"range": "Address", "inlined": true},
            "reading": {"range": "Any", "inlined": true},
            "name": {}, "garage": {"range": "Garage"}, "drivers": {"range": "Person", "multivalued": true},
            "location": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
            "notes": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "JsonProperty"}}},
            "colour": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "VocabProperty"}}},
            "route": {"range": "Person", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}},
            "tags": {"range": "string", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}}}
}"#;

pub async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city")
        // A 4.0 model has no `Edm.Untyped` member (`notes`).
        .header("OData-MaxVersion", "4.01");
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

pub fn person(n: u32, scope: &str) -> Value {
    json!({"id": format!("urn:ngsi-ld:Person:{n}"), "type": format!("{CITY}Person"), "scope": scope,
           format!("{CITY}name"): {"type": "Property", "value": format!("p{n}")}})
}

/// View `cars` shows Cars and Persons in scope `/a` and offers the entity
/// writes; view `reads` shows the same and offers reads only. Person:1 is
/// in view, Person:2 out of its scope; Car:1 in view, Car:2 out of it.
pub async fn city() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    city_on(TestBroker::new().await?).await
}

/// [`city`] on a broker the caller built (a policy engine of its own),
/// with `ANTARES_SCHEMA_MAX_BYTES` set before it was.
pub async fn city_on(b: TestBroker) -> Result<(TestBroker, Router), String> {
    let app = b.serve(antares_odata::OData)?;
    let reads = ["query_entities", "retrieve_entity"];
    let writes = [
        "create_entity",
        "merge_entity",
        "replace_entity",
        "delete_entity",
        "replace_attribute",
        "partial_attribute_update",
        "delete_attribute",
        "append_attributes",
    ];
    let mut puts = Vec::new();
    for (id, operations) in [
        ("cars", reads.iter().chain(&writes).collect::<Vec<_>>()),
        ("reads", reads.iter().collect()),
    ] {
        let view = json!({
            "id": id, "tenant": "city", "@context": {"city": CITY},
            "selector": ["city:Car", "city:Person"],
            "scopeQ": "/a",
            "rules": [{"types": ["city:Car"],
                       "attributes": ["city:speed", "city:label", "city:owner", "city:drivers", "city:garage",
                                      "city:location", "city:notes", "city:colour", "city:route", "city:tags",
                                      "city:ownerRef", "city:address", "city:reading"]},
                      {"types": ["city:Person"], "attributes": ["city:name"]}],
            "operations": operations
        });
        puts.push((format!("/q/views/{id}"), view.to_string()));
    }
    puts.push(("/q/schemas/city".into(), SCHEMA.into()));
    for (uri, body) in &puts {
        let (status, answer) = send(&app, Method::PUT, uri, Some(body)).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    let car = |n: u32, scope: &str| {
        json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"), "scope": scope,
               format!("{CITY}speed"): {"type": "Property", "value": 1.0},
               format!("{CITY}label"): {"type": "Property", "value": "one"}})
    };
    for entity in [person(1, "/a"), person(2, "/b"), car(1, "/a"), car(2, "/b")] {
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&entity.to_string()),
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!("{:?}: {status} {answer}", entity.get("id")));
        }
    }
    Ok((b, app))
}

/// The Entity as the broker holds it, outside any view.
pub async fn stored(app: &Router, id: &str) -> (StatusCode, Value) {
    send(
        app,
        Method::GET,
        &format!("/ngsi-ld/v1/entities/{id}"),
        None,
    )
    .await
}

pub async fn odata(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    send(app, method, &format!("{ROOT}/{path}"), body).await
}
