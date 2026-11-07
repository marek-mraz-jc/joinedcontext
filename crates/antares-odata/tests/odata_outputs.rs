// SPDX-License-Identifier: EUPL-1.2
//! Named OData outputs (docs API/11 EP-159, T-5225): each output of a view's
//! `representations.odata` is one more EntitySet over its one source's class, read only, its
//! filter a conjunct of every read: the collection, `$filter`, `$count`, by key and below a
//! key. A filtered output never holds an Entity outside its filter or the view.

mod writes;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use writes::{city, odata, send, stored, CITY};

const ROOT: &str = "/x/views/named/odata/v4";

/// The `city` fixture, Car:3 (speed 5, scope `/a`) and view `named` over Cars and Persons with
/// the outputs `fast` (speed over 2), `in-a` (scope `/a`) and `halls` (a class the view does
/// not show).
async fn named() -> Result<axum::Router, String> {
    let (b, app) = city().await?;
    std::mem::forget(b);
    let odata = json!([
        {"name": "fast", "title": {"en": "Fast cars"}, "filter": {"q": "speed>2"},
         "sources": [{"type": "Car"}]},
        {"name": "in-a", "filter": {"scopeQ": "/a"}, "sources": [{"type": "Car"}]},
        {"name": "halls", "sources": [{"type": "Hall"}]},
        {"name": "broken", "filter": {"q": "(("}, "sources": [{"type": "Car"}]}
    ]);
    let view = json!({"id": "named", "tenant": "city",
        "@context": {"city": CITY, "speed": format!("{CITY}speed")},
        "selector": ["city:Car", "city:Person"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:label"]},
                  {"types": ["city:Person"], "attributes": ["city:name"]}],
        "operations": ["query_entities", "retrieve_entity", "create_entity", "merge_entity", "delete_entity"],
        "representations": {"odata": odata}});
    let car = json!({"id": "urn:ngsi-ld:Car:3", "type": format!("{CITY}Car"), "scope": "/a",
        format!("{CITY}speed"): {"type": "Property", "value": 5.0},
        format!("{CITY}label"): {"type": "Property", "value": "three"}});
    for (uri, body) in [("/q/views/named", view), ("/ngsi-ld/v1/entities", car)] {
        let method = if uri.starts_with("/q") {
            Method::PUT
        } else {
            Method::POST
        };
        let (status, answer) = send(&app, method, uri, Some(&body.to_string())).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    Ok(app)
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(app, Method::GET, &format!("{ROOT}/{path}"), None).await
}

fn ids(body: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = body["value"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filtered_output_never_holds_an_entity_outside_its_filter_by_key_or_by_query() {
    let app = named().await.expect("fixture");
    let (status, all) = get(&app, "fast").await;
    assert_eq!(
        (status, ids(&all)),
        (StatusCode::OK, vec!["urn:ngsi-ld:Car:3"]),
        "{all}"
    );
    let (_, labelled) = get(&app, "fast?$filter=label%20eq%20'one'").await;
    assert_eq!(ids(&labelled), Vec::<&str>::new(), "{labelled}");
    let (_, count) = get(&app, "fast/$count").await;
    assert_eq!(count, json!(1));
    let (status, three) = get(&app, "fast('urn:ngsi-ld:Car:3')").await;
    assert_eq!(
        (status, three["speed"].as_f64()),
        (StatusCode::OK, Some(5.0)),
        "{three}"
    );
    for outside in [
        "fast('urn:ngsi-ld:Car:1')",
        "fast('urn:ngsi-ld:Car:1')/speed",
        "fast('urn:ngsi-ld:Car:1')/$ref",
        "fast('urn:ngsi-ld:Car:2')",
        "in_a('urn:ngsi-ld:Car:2')",
    ] {
        assert_eq!(
            get(&app, outside).await.0,
            StatusCode::NOT_FOUND,
            "{outside}"
        );
    }
    let (_, scoped) = get(&app, "in_a").await;
    assert_eq!(
        ids(&scoped),
        vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "{scoped}"
    );
    // the request's scopeQ joins the output's, never widens it
    let (_, elsewhere) = get(&app, "in_a?ngsild.scopeQ=/b").await;
    assert_eq!(ids(&elsewhere), Vec::<&str>::new(), "{elsewhere}");
    // a filter or a scope that cannot be read is the request's 400, never the whole set
    for refused in [
        "broken",
        "broken('urn:ngsi-ld:Car:3')",
        "in_a?ngsild.scopeQ=%ff",
        "in_a?ngsild.scopeQ=nowhere",
    ] {
        assert_eq!(
            get(&app, refused).await.0,
            StatusCode::BAD_REQUEST,
            "{refused}"
        );
    }
    // the class's own set is unchanged beside its outputs
    let (_, cars) = get(&app, "Car").await;
    assert_eq!(ids(&cars).len(), 3, "{cars}");
    // a view without a list serves what it served
    let (_, own) = odata(&app, Method::GET, "Car", None).await;
    assert_eq!(
        ids(&own),
        vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "{own}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_output_set_is_read_only_and_one_over_a_hidden_class_is_absent() {
    let app = named().await.expect("fixture");
    let (status, xml) = get(&app, "$metadata").await;
    let xml = xml.as_str().unwrap_or_default().to_owned();
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains(r#"<EntitySet Name="fast""#), "{xml}");
    assert!(xml.contains(r#"<EntitySet Name="in_a""#), "{xml}");
    assert!(!xml.contains("halls"), "{xml}");
    assert_eq!(get(&app, "halls").await.0, StatusCode::NOT_FOUND);
    let created = json!({"id": "urn:ngsi-ld:Car:8", "speed": 9.0}).to_string();
    let (status, body) = send(&app, Method::POST, &format!("{ROOT}/fast"), Some(&created)).await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_eq!(
        stored(&app, "urn:ngsi-ld:Car:8").await.0,
        StatusCode::NOT_FOUND
    );
    let (status, body) = send(
        &app,
        Method::DELETE,
        &format!("{ROOT}/fast('urn:ngsi-ld:Car:3')"),
        None,
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_eq!(stored(&app, "urn:ngsi-ld:Car:3").await.0, StatusCode::OK);
}
