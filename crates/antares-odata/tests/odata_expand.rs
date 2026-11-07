// SPDX-License-Identifier: EUPL-1.2
//! `$expand` (odata-linkml-mapping 5.1 rows Q3, Q6 to Q8, S1;
//! surfaces-threat-model O5, O6): one level of NGSI-LD `join=inline`
//! through the view, a target the view hides read like an absent one,
//! `/$ref` from the Relationship's object, the deeper forms 501, and a
//! join walk past the broker's lookup budget a 400, never a partial answer.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const ROOT: &str = "/x/views/cars/odata/v4";
/// The base a request without `Host` is answered under (no
/// `ANTARES_PUBLIC_URL` in tests): context URLs are absolute.
const PUBLIC: &str = "http://localhost:9090";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "owner", "drivers"]}, "Person": {"slots": ["name", "pin"]}},
  "slots": {"speed": {"range": "float"}, "owner": {"range": "Person"}, "name": {}, "pin": {"range": "integer"},
            "drivers": {"range": "Person",
                        "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}}}
}"#;

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
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

async fn get(app: &Router, path: &str) -> (StatusCode, Value) {
    send(app, Method::GET, &format!("{ROOT}/{path}"), None).await
}

fn person(n: u32, scope: &str, name: &str) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:Person:{n}"), "type": format!("{CITY}Person"), "scope": scope,
        format!("{CITY}name"): {"type": "Property", "value": name},
        format!("{CITY}pin"): {"type": "Property", "value": 4711},
    })
}

fn car(n: u32, owner: &str, drivers: &[&str]) -> Value {
    let list: Vec<Value> = drivers.iter().map(|d| json!({"object": d})).collect();
    json!({
        "id": format!("urn:ngsi-ld:Car:{n}"), "type": format!("{CITY}Car"), "scope": "/a",
        format!("{CITY}speed"): {"type": "Property", "value": f64::from(n)},
        format!("{CITY}owner"): {"type": "Relationship", "object": owner},
        format!("{CITY}drivers"): {"type": "ListRelationship", "objectList": list},
    })
}

/// Person:1 in the view, Person:2 out of its scope, Person:9 absent; Car:1
/// owned by 1 and driven by 1 and 2, Car:2 owned by the hidden 2, Car:3 by
/// the absent 9.
async fn broker(entities: &[Value]) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car", "city:Person"],
        "scopeQ": "/a",
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:owner", "city:drivers"]},
                  {"types": ["city:Person"], "attributes": ["city:name"]}],
        "operations": ["query_entities", "retrieve_entity", "retrieve_attribute_value"]
    })
    .to_string();
    for (uri, body) in [
        ("/q/views/cars", view.as_str()),
        ("/q/schemas/city", SCHEMA),
    ] {
        let (status, answer) = send(&app, Method::PUT, uri, Some(body)).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    for entity in entities {
        let (status, answer) = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&entity.to_string()),
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!("{}: {status} {answer}", entity["id"]));
        }
    }
    Ok((b, app))
}

async fn city() -> Result<(TestBroker, Router), String> {
    broker(&[
        person(1, "/a", "Ann"),
        person(2, "/b", "Bob"),
        car(
            1,
            "urn:ngsi-ld:Person:1",
            &["urn:ngsi-ld:Person:1", "urn:ngsi-ld:Person:2"],
        ),
        car(2, "urn:ngsi-ld:Person:2", &[]),
        car(3, "urn:ngsi-ld:Person:9", &["urn:ngsi-ld:Person:9"]),
    ])
    .await
}

fn by_id(body: &Value, id: &str) -> Value {
    body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_expand_omits_out_of_view_targets() {
    let (_b, app) = city().await.expect("broker");
    let (status, body) = get(&app, "Car?$expand=owner,drivers&$filter=speed%20gt%200").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let one = by_id(&body, "urn:ngsi-ld:Car:1");
    assert_eq!(
        one["owner"],
        json!({"id": "urn:ngsi-ld:Person:1", "_types": ["Person"], "scope": ["/a"], "name": "Ann"}),
        "the target through the view: no pin: {one}"
    );
    assert_eq!(
        one["drivers"],
        json!([{"id": "urn:ngsi-ld:Person:1", "_types": ["Person"], "scope": ["/a"], "name": "Ann"}]),
        "a hidden target is left out of the collection: {one}"
    );
    let hidden = by_id(&body, "urn:ngsi-ld:Car:2");
    let absent = by_id(&body, "urn:ngsi-ld:Car:3");
    assert_eq!(hidden["owner"], Value::Null, "{hidden}");
    assert_eq!(
        (&hidden["owner"], &hidden["drivers"]),
        (&absent["owner"], &absent["drivers"]),
        "hidden reads like absent"
    );
    let text = body.to_string();
    for leak in ["Bob", "4711", "Person:2"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    let (status, one) = get(&app, "Car('urn:ngsi-ld:Car:2')?$expand=owner").await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["owner"], Value::Null, "{one}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_expand_ref_star_and_a_nested_select() {
    let (_b, app) = city().await.expect("broker");
    let (status, body) = get(&app, "Car?$expand=owner/$ref,drivers/$ref").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let one = by_id(&body, "urn:ngsi-ld:Car:1");
    assert_eq!(
        one["owner"],
        json!({"@odata.id": "Person('urn:ngsi-ld:Person:1')"}),
        "{one}"
    );
    assert_eq!(
        one["drivers"],
        json!([
            {"@odata.id": "Person('urn:ngsi-ld:Person:1')"},
            {"@odata.id": "Person('urn:ngsi-ld:Person:2')"}
        ]),
        "a reference is the Relationship's object (Q7): {one}"
    );
    assert_eq!(
        by_id(&body, "urn:ngsi-ld:Car:2")["drivers"],
        json!([]),
        "{body}"
    );

    let (status, body) = send(
        &app,
        Method::GET,
        &format!("{ROOT}/Car?$expand=*&$select=speed"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["@odata.context"],
        format!("{PUBLIC}{ROOT}/$metadata#Car(speed)")
    );
    let one = by_id(&body, "urn:ngsi-ld:Car:1");
    let mut keys: Vec<&str> = one
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["drivers", "id", "owner", "speed"], "{one}");
    assert_eq!(one["owner"]["name"], "Ann", "{one}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_expand_context_url_names_the_expansions() {
    let (_b, app) = city().await.expect("broker");
    let req = Request::get(format!(
        "{ROOT}/Car('urn:ngsi-ld:Car:1')?$expand=owner($select=id),*/$ref"
    ))
    .header("OData-MaxVersion", "4.01")
    .body(Body::empty())
    .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let one: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        one["@context"],
        format!("{PUBLIC}{ROOT}/$metadata#Car(owner(id))/$entity"),
        "{one}"
    );
    assert_eq!(
        one["owner"],
        json!({"id": "urn:ngsi-ld:Person:1"}),
        "named overrides the star: {one}"
    );
    assert_eq!(
        one["drivers"][1],
        json!({"@id": "Person('urn:ngsi-ld:Person:2')"}),
        "{one}"
    );

    let (status, one) = get(
        &app,
        "Car('urn:ngsi-ld:Car:1')?$expand=owner($select=id,createdAt)",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert!(one["owner"]["createdAt"].is_string(), "{one}");
    assert!(
        one.get("createdAt").is_none(),
        "a system member only where it is named (E6): {one}"
    );

    let req = Request::get(format!("{ROOT}/Car?$expand=owner&$orderby=id&$top=1"))
        .header("OData-MaxVersion", "4.01")
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let page: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        page["@context"],
        format!("{PUBLIC}{ROOT}/$metadata#Car(owner())"),
        "{page}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_expand_beyond_one_level_is_501() {
    let (_b, app) = city().await.expect("broker");
    for path in [
        "Car?$expand=owner($levels=2)",
        "Car?$expand=owner($expand=*)",
        "Car('urn:ngsi-ld:Car:1')?$expand=owner($levels=max)",
    ] {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}: {body}");
        assert_eq!(body["error"]["code"], "NotImplemented", "{path}: {body}");
    }
    let (status, body) = get(&app, "Car?$expand=pin").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a name the view hides: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_expand_stops_at_the_join_lookup_budget() {
    let drivers: Vec<String> = (0..1001)
        .map(|n| format!("urn:ngsi-ld:Person:{n}"))
        .collect();
    let drivers: Vec<&str> = drivers.iter().map(String::as_str).collect();
    let (_b, app) = broker(&[
        person(1, "/a", "Ann"),
        car(1, "urn:ngsi-ld:Person:1", &drivers),
    ])
    .await
    .expect("broker");
    let (status, body) = get(&app, "Car('urn:ngsi-ld:Car:1')?$expand=owner").await;
    assert_eq!(status, StatusCode::OK, "within the budget: {body}");
    for path in [
        "Car('urn:ngsi-ld:Car:1')?$expand=drivers",
        "Car?$expand=drivers",
        "Car('urn:ngsi-ld:Car:1')/drivers",
    ] {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("$filter")),
            "{path}: names the way to narrow it: {body}"
        );
    }
}

fn ids(body: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_filter_navigation_path_reads_hidden_targets_as_absent() {
    let (_b, app) = city().await.expect("broker");
    for (filter, want) in [
        ("owner/name%20eq%20'Ann'", vec!["urn:ngsi-ld:Car:1"]),
        ("contains(owner/name,'n')", vec!["urn:ngsi-ld:Car:1"]),
        ("owner/name%20eq%20'Bob'", vec![]),
        ("owner/name%20eq%20'Nobody'", vec![]),
        (
            "owner/name%20eq%20'Ann'%20and%20speed%20gt%200",
            vec!["urn:ngsi-ld:Car:1"],
        ),
    ] {
        let (status, body) = get(&app, &format!("Car?$filter={filter}")).await;
        assert_eq!(status, StatusCode::OK, "{filter}: {body}");
        assert_eq!(ids(&body), want, "{filter}: {body}");
    }
    let (status, body) = get(&app, "Car?$filter=owner/name%20ne%20'Ann'").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let others = ids(&body);
    assert_eq!(
        others.contains(&"urn:ngsi-ld:Car:2"),
        others.contains(&"urn:ngsi-ld:Car:3"),
        "the hidden owner of Car:2 answers like the absent one of Car:3: {body}"
    );
    for (path, want) in [
        (
            "Car?$filter=drivers/name%20eq%20'Ann'",
            StatusCode::BAD_REQUEST,
        ),
        ("Car?$filter=owner/pin%20eq%204711", StatusCode::BAD_REQUEST),
    ] {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, want, "{path}: {body}");
    }
    // T-5906: sorted by the joined target's value; the hidden owner of Car:2 sorts with the
    // absent one of Car:3, null first ascending
    let (status, body) = get(&app, "Car?$orderby=owner/name").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ordered: Vec<&str> = body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert_eq!(
        ordered,
        [
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:1"
        ],
        "{body}"
    );
}

async fn text(app: &Router, path: &str) -> (StatusCode, String, String) {
    let req = Request::get(format!("{ROOT}/{path}"))
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let kind = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (status, kind, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_reads_a_property_its_value_and_the_entities_a_navigation_names() {
    let (_b, app) = city().await.expect("broker");
    let one = "Car('urn:ngsi-ld:Car:1')";
    let (status, mut body) = get(&app, &format!("{one}/speed")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tag = body
        .as_object_mut()
        .and_then(|o| o.remove("@odata.metadataEtag"));
    assert!(tag.is_some_and(|t| t.is_string()), "{body}");
    assert_eq!(
        body,
        json!({"@odata.context": format!("{PUBLIC}{ROOT}/$metadata#{one}/speed"), "value": 1.0})
    );
    for (headers, want) in [
        (
            ("OData-MaxVersion", "4.01"),
            json!({"@context": format!("{PUBLIC}{ROOT}/$metadata#{one}/speed"), "value": 1.0}),
        ),
        (
            ("Accept", "application/json;odata.metadata=none"),
            json!({"value": 1.0}),
        ),
    ] {
        let req = Request::get(format!("{ROOT}/{one}/speed"))
            .header(headers.0, headers.1)
            .body(Body::empty())
            .unwrap_or_default();
        let Ok(resp) = app.clone().oneshot(req).await;
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        let mut got: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(o) = got.as_object_mut() {
            o.remove("@metadataEtag");
        }
        assert_eq!(got, want, "{headers:?}");
    }
    let (status, kind, raw) = text(&app, &format!("{one}/speed/$value")).await;
    assert_eq!(
        (status, kind.as_str(), raw.as_str()),
        (StatusCode::OK, "text/plain", "1.0")
    );
    let (status, owner) = get(&app, &format!("{one}/owner")).await;
    assert_eq!(status, StatusCode::OK, "{owner}");
    assert_eq!(
        owner["@odata.context"],
        format!("{PUBLIC}{ROOT}/$metadata#Person/$entity")
    );
    assert_eq!(
        (&owner["id"], &owner["name"]),
        (&json!("urn:ngsi-ld:Person:1"), &json!("Ann"))
    );
    let (status, named) = get(&app, &format!("{one}/owner?$select=name")).await;
    assert_eq!(status, StatusCode::OK, "{named}");
    assert!(
        named.get("scope").is_none() && named["name"] == "Ann",
        "{named}"
    );
    let (status, drivers) = get(&app, &format!("{one}/drivers")).await;
    assert_eq!(status, StatusCode::OK, "{drivers}");
    assert_eq!(
        ids(&drivers),
        ["urn:ngsi-ld:Person:1"],
        "the hidden driver is left out: {drivers}"
    );
    let (status, refs) = get(&app, &format!("{one}/drivers/$ref")).await;
    assert_eq!(status, StatusCode::OK, "{refs}");
    assert_eq!(
        refs["value"],
        json!([
            {"@odata.id": format!("{PUBLIC}{ROOT}/Person('urn:ngsi-ld:Person:1')")},
            {"@odata.id": format!("{PUBLIC}{ROOT}/Person('urn:ngsi-ld:Person:2')")}
        ]),
        "references are the Relationship's objects (Q33): {refs}"
    );
    let (status, owner_ref) = get(&app, &format!("{one}/owner/$ref")).await;
    assert_eq!(status, StatusCode::OK, "{owner_ref}");
    assert_eq!(
        owner_ref["@odata.id"],
        format!("{PUBLIC}{ROOT}/Person('urn:ngsi-ld:Person:1')")
    );
    let (status, none) = get(&app, "Car('urn:ngsi-ld:Car:2')/drivers").await;
    assert_eq!(
        (status, &none["value"]),
        (StatusCode::OK, &json!([])),
        "{none}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_a_hidden_target_or_value_reads_like_an_absent_one() {
    let (_b, app) = city().await.expect("broker");
    let hidden = text(&app, "Car('urn:ngsi-ld:Car:2')/owner").await;
    let absent = text(&app, "Car('urn:ngsi-ld:Car:3')/owner").await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND, "{}", hidden.2);
    assert_eq!(hidden, absent, "a hidden owner answers like a dangling one");
    let (status, body) = get(&app, "Car('urn:ngsi-ld:Car:3')/drivers").await;
    assert_eq!(
        (status, &body["value"]),
        (StatusCode::OK, &json!([])),
        "one absent driver: an empty collection: {body}"
    );
    let hidden = text(&app, "Person('urn:ngsi-ld:Person:2')/name").await;
    let absent = text(&app, "Person('urn:ngsi-ld:Person:9')/name").await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND, "{}", hidden.2);
    assert_eq!(hidden, absent);
    assert!(!hidden.2.contains("Bob"), "{}", hidden.2);
    let pin = text(&app, "Person('urn:ngsi-ld:Person:1')/pin").await;
    let unknown = text(&app, "Person('urn:ngsi-ld:Person:1')/nosuch").await;
    assert_eq!(pin.0, StatusCode::NOT_FOUND, "{}", pin.2);
    assert_eq!(pin, unknown, "a hidden member is an unknown one");
    let bare = json!({"id": "urn:ngsi-ld:Car:7", "type": format!("{CITY}Car"), "scope": "/a"});
    let (status, made) = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&bare.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let (status, body) = get(&app, "Car('urn:ngsi-ld:Car:7')/owner").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no owner: {body}");
    let (status, body) = get(&app, "Car('urn:ngsi-ld:Car:7')/drivers").await;
    assert_eq!(
        (status, &body["value"]),
        (StatusCode::OK, &json!([])),
        "{body}"
    );
    for path in [
        "Car('urn:ngsi-ld:Car:1')/drivers?$top=1",
        "Car('urn:ngsi-ld:Car:1')/drivers?$skip=1",
        "Car('urn:ngsi-ld:Car:1')/drivers?$count=true",
        "Car('urn:ngsi-ld:Car:1')/drivers?$orderby=name",
        "Car('urn:ngsi-ld:Car:1')/drivers?$skiptoken=x",
        "Car('urn:ngsi-ld:Car:1')/drivers?$filter=name%20eq%20'Ann'",
        "Car('urn:ngsi-ld:Car:1')/drivers/$count",
        "Car('urn:ngsi-ld:Car:1')/owner?$expand=*",
        "Car('urn:ngsi-ld:Car:1')/scope",
    ] {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}: {body}");
    }
}
