// SPDX-License-Identifier: EUPL-1.2
//! `$ref` writes (odata-linkml-mapping section 6 rows W16 to W20,
//! surfaces-threat-model O15): setting a single-valued navigation, or the
//! full list of a collection-valued one in the Relationship's own kind, is
//! Append Attributes, removing one Delete
//! Attribute; every target is read under the view first, so a hidden one
//! answers like an absent one and nothing is written. Adding to or
//! removing from a collection has no NGSI-LD operation: 501.

mod writes;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use writes::{city, odata, stored, CITY};

const ONE: &str = "Car('urn:ngsi-ld:Car:1')";

fn reference(person: &str) -> String {
    json!({"@odata.id": format!("Person('{person}')")}).to_string()
}

fn references(people: &[&str]) -> String {
    let items: Vec<Value> = people
        .iter()
        .map(|p| json!({"@odata.id": format!("Person('{p}')")}))
        .collect();
    json!({"value": items}).to_string()
}

async fn attr(app: &axum::Router, name: &str) -> Value {
    let (_, car) = stored(app, "urn:ngsi-ld:Car:1").await;
    car.get(format!("{CITY}{name}"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_ref_sets_replaces_and_removes_a_relationship() {
    let (_b, app) = city().await.expect("broker");
    let person1 = "urn:ngsi-ld:Person:1";
    let (status, body) = odata(
        &app,
        Method::PUT,
        &format!("{ONE}/owner/$ref"),
        Some(&reference(person1)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        attr(&app, "owner").await,
        json!({"type": "Relationship", "object": person1})
    );
    let (status, body) = odata(&app, Method::GET, &format!("{ONE}/owner/$ref"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = odata(
        &app,
        Method::PUT,
        &format!("{ONE}/route/$ref"),
        Some(&references(&[person1, person1])),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let route = attr(&app, "route").await;
    assert_eq!(
        route["type"], "ListRelationship",
        "the kind is kept: {route}"
    );
    assert_eq!(
        route["objectList"].as_array().map(Vec::len),
        Some(2),
        "{route}"
    );

    let (status, body) = odata(
        &app,
        Method::PUT,
        &format!("{ONE}/drivers/$ref"),
        Some(&references(&[person1])),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let drivers = attr(&app, "drivers").await;
    assert_eq!(drivers["type"], "Relationship", "{drivers}");

    let (status, body) = odata(&app, Method::DELETE, &format!("{ONE}/owner/$ref"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(attr(&app, "owner").await, Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_bind_to_an_absent_target_answers_like_a_hidden_one() {
    let (_b, app) = city().await.expect("broker");
    let mut answers = Vec::new();
    for person in ["urn:ngsi-ld:Person:2", "urn:ngsi-ld:Person:9"] {
        for (nav, body) in [
            ("owner", reference(person)),
            ("route", references(&["urn:ngsi-ld:Person:1", person])),
        ] {
            let (status, answer) =
                odata(&app, Method::PUT, &format!("{ONE}/{nav}/$ref"), Some(&body)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{nav} {person}: {answer}");
            answers.push(answer);
        }
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");
    for nav in ["owner", "route"] {
        assert_eq!(attr(&app, nav).await, Value::Null, "{nav}: nothing written");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_ref_on_a_collection_adds_or_removes_nothing() {
    let (_b, app) = city().await.expect("broker");
    let one = reference("urn:ngsi-ld:Person:1");
    for (method, path, body) in [
        (
            Method::POST,
            format!("{ONE}/drivers/$ref"),
            Some(one.as_str()),
        ),
        (Method::DELETE, format!("{ONE}/drivers/$ref"), None),
        (
            Method::DELETE,
            format!("{ONE}/drivers/$ref?$id=Person('urn:ngsi-ld:Person:1')"),
            None,
        ),
        (
            Method::PUT,
            format!("{ONE}/owner/$ref?$select=id"),
            Some(one.as_str()),
        ),
    ] {
        let (status, answer) = odata(&app, method.clone(), &path, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "{method} {path}: {answer}"
        );
    }
    let (status, answer) = send_reads(&app, &format!("{ONE}/owner/$ref"), &one).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{answer}");
    for nav in ["owner", "drivers"] {
        assert_eq!(attr(&app, nav).await, Value::Null, "{nav}: nothing written");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_ref_with_a_malformed_body_is_400_and_writes_nothing() {
    let (_b, app) = city().await.expect("broker");
    for body in [r#"{"@odata.id": 5}"#, "{", r#"{"other": "x"}"#] {
        let (status, answer) =
            odata(&app, Method::PUT, &format!("{ONE}/owner/$ref"), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }
    assert_eq!(attr(&app, "owner").await, Value::Null, "nothing written");
}

/// A `PUT` through view `reads`, which offers no write.
async fn send_reads(app: &axum::Router, path: &str, body: &str) -> (StatusCode, Value) {
    writes::send(
        app,
        Method::PUT,
        &format!("/x/views/reads/odata/v4/{path}"),
        Some(body),
    )
    .await
}
