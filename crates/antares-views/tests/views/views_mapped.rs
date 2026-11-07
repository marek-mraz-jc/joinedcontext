// SPDX-License-Identifier: EUPL-1.2
//! A view of another model (gap G18, `docs/analysis/view-mapping.md`):
//! `vehicles` serves the fixture's Cars as `Vehicle`s, through the same
//! rules as `cars`. Car:1 (speed 10, red) and Car:2 (speed 50, blue) are in
//! it; Car:3 (speed 90) is not; `secret` is never shown.

use crate::common::{fixture, get, put_view, send, Answer, Fixture};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

pub const ROOT: &str = "/x/views/vehicles/ngsi-ld/v1";

/// `vehicles`: Cars below 80 show `speed`, `colour` and `owner`, mapped.
pub fn vehicles_view() -> Value {
    json!({
        "id": "vehicles", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "q": "speed<80", "attributes": ["speed", "colour", "owner"]}],
        "operations": antares_ops::mapping::READS.iter().map(|op| op.id()).collect::<Vec<_>>(),
        "mapping": {
            "version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
            "slots": [
                {"target": "velocity", "source": "speed", "kind": "unitConversion", "factor": 2, "filterable": true},
                {"target": "paint", "source": "colour", "kind": "valueMappings",
                 "forward": {"red": "rot", "blue": "blau"}, "filterable": true},
                {"target": "ownedBy", "source": "owner", "kind": "rename", "filterable": true},
                {"target": "hidden", "source": "secret", "kind": "rename", "filterable": true},
                {"target": "maker", "kind": "constant", "value": "acme"},
                {"target": "label", "kind": "expr",
                 "expression": {"binary": "+", "left": {"slot": "colour"}, "right": {"const": "!"}}}
            ]
        }
    })
}

pub async fn vehicles() -> Result<Fixture, String> {
    let f = fixture().await?;
    put_view(&f.app, &vehicles_view()).await?;
    Ok(f)
}

/// No answer of a mapped view names a source Attribute or the view's
/// generated terms, whatever it says.
pub fn speaks_the_target_model(a: &Answer) {
    for leak in ["speed", "colour", "secret", "owner\"", "jcSrc", "Car\""] {
        assert!(!a.text.contains(leak), "{leak} in {}", a.text);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_answers_in_the_target_model_what_the_view_serves() -> Result<(), String> {
    let f = vehicles().await?;
    let a = get(&f.app, &format!("{ROOT}/entities?type=Vehicle")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.ids(),
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"],
        "Car:3 is out of the view"
    );
    let car1 = a.json.get(0).cloned().unwrap_or_default();
    assert_eq!(
        car1,
        json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Vehicle",
            "velocity": {"type": "Property", "value": 20.0},
            "paint": {"type": "Property", "value": "rot"},
            "ownedBy": {"type": "Relationship", "object": "urn:ngsi-ld:Person:p"},
            "maker": {"type": "Property", "value": "acme"},
            "label": {"type": "Property", "value": "red!"}
        }),
        "`hidden` reads `secret`, which the view does not show: it is absent"
    );
    speaks_the_target_model(&a);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_in_the_target_model_selects_in_the_source_model() -> Result<(), String> {
    let f = vehicles().await?;
    for (q, ids) in [
        ("velocity%3C50", vec!["urn:ngsi-ld:Car:1"]),
        ("velocity==100", vec!["urn:ngsi-ld:Car:2"]),
        ("paint==%22blau%22", vec!["urn:ngsi-ld:Car:2"]),
        ("paint!=%22rot%22", vec!["urn:ngsi-ld:Car:2"]),
        ("ownedBy", vec!["urn:ngsi-ld:Car:1"]),
        // the source value of Car:3 matches, the view does not serve it
        ("velocity%3E150", vec![]),
        // the view hides `secret`: a filter on it holds for nobody
        ("hidden==1", vec![]),
        ("hidden", vec![]),
    ] {
        let a = get(&f.app, &format!("{ROOT}/entities?type=Vehicle&q={q}")).await;
        assert_eq!(a.status, StatusCode::OK, "{q}: {}", a.text);
        assert_eq!(a.ids(), ids, "{q}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn another_type_is_an_empty_answer_without_the_store() -> Result<(), String> {
    let f = vehicles().await?;
    for ty in ["Car", "Person", "Other"] {
        let a = get(&f.app, &format!("{ROOT}/entities?type={ty}&count=true")).await;
        assert_eq!(
            (a.status, a.json.clone()),
            (StatusCode::OK, json!([])),
            "{ty}"
        );
        assert_eq!(
            a.header("ngsild-results-count").as_deref(),
            Some("0"),
            "{ty}"
        );
        assert_eq!(
            a.header("content-type").as_deref(),
            Some("application/json")
        );
    }
    let a = send(
        &f.app,
        Method::GET,
        &format!("{ROOT}/entities?type=Car"),
        &[("Accept", "application/ld+json")],
        None,
    )
    .await;
    assert_eq!(
        a.header("content-type").as_deref(),
        Some("application/ld+json")
    );
    assert!(
        a.header("ngsild-results-count").is_none(),
        "count was not asked"
    );
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1?type=Car"),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn retrieve_entity_and_attribute_rebuild_and_keep_the_view() -> Result<(), String> {
    let f = vehicles().await?;
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:2?attrs=label"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"id": "urn:ngsi-ld:Car:2", "type": "Vehicle",
               "label": {"type": "Property", "value": "blue!"}}),
        "only what attrs asked, though the expression fetched `colour`"
    );
    speaks_the_target_model(&a);
    let a = get(&f.app, &format!("{ROOT}/entities/urn:ngsi-ld:Car:3")).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "out of the view");
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:2/attrs/velocity"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json.get("value"), Some(&json!(100.0)), "{}", a.json);
    speaks_the_target_model(&a);
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1/attrs/hidden"),
    )
    .await;
    assert_eq!(
        a.status,
        StatusCode::NOT_FOUND,
        "a hidden attribute does not exist: {}",
        a.text
    );
    speaks_the_target_model(&a);
    for computed in ["maker", "label"] {
        let a = get(
            &f.app,
            &format!("{ROOT}/entities/urn:ngsi-ld:Car:1/attrs/{computed}"),
        )
        .await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{computed}: {}", a.text);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn every_representation_is_rebuilt_and_carries_the_views_context() -> Result<(), String> {
    let f = vehicles().await?;
    let uri = format!("{ROOT}/entities/urn:ngsi-ld:Car:1?pick=id,velocity,paint");
    let a = send(
        &f.app,
        Method::GET,
        &uri,
        &[("Accept", "application/ld+json")],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"@context": "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld",
               "id": "urn:ngsi-ld:Car:1",
               "velocity": {"type": "Property", "value": 20.0},
               "paint": {"type": "Property", "value": "rot"}}),
        "never the inner @context of generated terms"
    );
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1?options=keyValues&omit=ownedBy,label"),
    )
    .await;
    assert_eq!(
        a.json,
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Vehicle", "velocity": 20.0, "paint": "rot", "maker": "acme"})
    );
    let a = get(
        &f.app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1?format=concise&attrs=velocity"),
    )
    .await;
    assert_eq!(
        a.json,
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Vehicle", "velocity": 20.0}),
        "concise: a Property with nothing but its value is the value"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ordering_and_paging_follow_the_source_model() -> Result<(), String> {
    let f = vehicles().await?;
    let a = get(
        &f.app,
        &format!("{ROOT}/entities?type=Vehicle&orderBy=velocity;desc&limit=1&count=true"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.ids(), ["urn:ngsi-ld:Car:2"]);
    assert_eq!(
        a.header("ngsild-results-count").as_deref(),
        Some("2"),
        "the store's count stays"
    );
    let next = a.link("next").unwrap_or_default();
    assert!(
        next.starts_with("/x/views/vehicles/ngsi-ld/v1/entities?"),
        "{next}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_mapped_view_does_not_serve_is_refused_without_naming_the_source(
) -> Result<(), String> {
    let f = vehicles().await?;
    for (uri, headers, status) in [
        (
            format!("{ROOT}/entities?type=Vehicle&q=speed%3C5"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&q=label==%22x%22"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&q=paint%3E%22a%22"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&q=ownedBy%7Bname%7D==1"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&lang=en"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&attrs=speed"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&type=Vehicle"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle&format=geojson"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle"),
            vec![("Accept", "application/geo+json")],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities?type=Vehicle"),
            vec![(
                "Link",
                "<https://example.org/ctx.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"",
            )],
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/entities/urn:ngsi-ld:Car:1/attrs/speed"),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let a = send(&f.app, Method::GET, &uri, &headers, None).await;
        assert_eq!(a.status, status, "{uri}: {}", a.text);
        speaks_the_target_model(&a);
    }
    let a = send(
        &f.app,
        Method::POST,
        &format!("{ROOT}/entities"),
        &[],
        Some(&json!({"id": "urn:ngsi-ld:Car:7", "type": "Vehicle"})),
    )
    .await;
    assert_eq!(
        a.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "read only: {}",
        a.text
    );
    assert_eq!(a.header("allow").as_deref(), Some("GET, HEAD"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_head_request_answers_without_a_body() -> Result<(), String> {
    let f = vehicles().await?;
    let a = send(
        &f.app,
        Method::HEAD,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1"),
        &[],
        None,
    )
    .await;
    assert_eq!((a.status, a.text.as_str()), (StatusCode::OK, ""));
    Ok(())
}
