//! T-4484, API/01 §16a: a public Endpoint's standards page is built from one real entity read
//! anonymously through the gateway, and shows only examples that answered with an entity.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "bbsk";

fn mirror(slug: &str, audience: &str) -> Mirror {
    let mirror = Mirror::new();
    mirror.upsert(common::envelope(
        "Endpoint",
        "air",
        PROJECT,
        json!({
            "slug": slug,
            "audience": audience,
            "contextSpaceRef": {"name": "air"},
            "enabledRepresentations": ["ngsi-ld", "ogc-features", "odata", "sta", "csv"],
            "catalog": {"license": "CC_BY_4_0"}
        }),
    ));
    mirror.upsert(common::envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({"dataModelRef": {"name": "air-quality"}}),
    ));
    mirror.upsert(common::envelope(
        "DataModel",
        "air-quality",
        PROJECT,
        json!({"classes": ["EmptyType", "AirQualityObserved"]}),
    ));
    mirror
}

/// A gateway that answers the reads a correct page makes and nothing else: a malformed example
/// finds no mock, answers 404, and is dropped.
async fn gateway(slug: &str) -> MockServer {
    let server = MockServer::start().await;
    let at = |rest: &str| format!("/api/endpoint/{slug}/{rest}");
    let entity = json!({
        "id": "urn:ngsi-ld:AirQualityObserved:zv1", "type": "AirQualityObserved",
        "name": "Zvolen", "address": "Hlavná 1", "no2": 21.5
    });
    let mounts = [
        Mock::given(method("GET"))
            .and(path(at("ngsi-ld/v1/entities")))
            .and(query_param("type", "EmptyType"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))),
        Mock::given(method("GET"))
            .and(path(at("ngsi-ld/v1/entities")))
            .and(query_param("type", "AirQualityObserved"))
            .and(query_param("limit", "1"))
            .and(query_param("options", "keyValues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity]))),
        Mock::given(method("GET"))
            .and(path(at("ngsi-ld/v1/entities")))
            .and(query_param("type", "AirQualityObserved"))
            .and(query_param("q", "name==\"Zvolen\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity]))),
        Mock::given(method("GET"))
            .and(path(at("ngsi-ld/v1/entities")))
            .and(query_param("type", "AirQualityObserved"))
            .and(query_param("q", "no2>=21.5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity, entity]))),
        Mock::given(method("GET"))
            .and(path(at("ogc/features/collections")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"collections": [{"id": "AirQualityObserved"}]})),
            ),
        Mock::given(method("GET"))
            .and(path(at(
                "ogc/features/collections/AirQualityObserved/queryables",
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "properties": {"id": {}, "geometry": {}, "name": {}, "no2": {}}
            }))),
        Mock::given(method("GET"))
            .and(path(at(
                "ogc/features/collections/AirQualityObserved/items",
            )))
            .and(query_param("name", "Zvolen"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"features": [{}]}))),
        Mock::given(method("GET"))
            .and(path(at(
                "ogc/features/collections/AirQualityObserved/items",
            )))
            .and(query_param("filter", "no2 >= 21.5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"features": [{}]}))),
        Mock::given(method("GET"))
            .and(path(at("odata/v4/")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"name": "AirQualityObserved", "url": "AirQualityObserved"}]
            }))),
        Mock::given(method("GET"))
            .and(path(at("odata/v4/$metadata")))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<edmx:Edmx><Schema><EntityType Name="AirQualityObserved">
                <Property Name="name" Type="Edm.String"/><Property Name="no2" Type="Edm.Double"/>
                </EntityType></Schema></edmx:Edmx>"#,
            )),
        Mock::given(method("GET"))
            .and(path(at("odata/v4/AirQualityObserved")))
            .and(query_param("$filter", "name eq 'Zvolen'"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{}]}))),
        // the range answers no entity: that example is not shown
        Mock::given(method("GET"))
            .and(path(at("odata/v4/AirQualityObserved")))
            .and(query_param("$filter", "no2 ge 21.5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []}))),
        // The first pages of five (T-5111): OData's answers nothing and is left out.
        Mock::given(method("GET"))
            .and(path(at("ngsi-ld/v1/entities")))
            .and(query_param("type", "AirQualityObserved"))
            .and(query_param("limit", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity]))),
        Mock::given(method("GET"))
            .and(path(at(
                "ogc/features/collections/AirQualityObserved/items",
            )))
            .and(query_param("limit", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"features": [{}]}))),
        Mock::given(method("GET"))
            .and(path(at("odata/v4/AirQualityObserved")))
            .and(query_param("$top", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []}))),
    ];
    for mock in mounts {
        mock.mount(&server).await;
    }
    server
}

async fn get(state: AppState, uri: &str) -> (StatusCode, Option<String>, Value) {
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .map(|v| v.to_str().expect("ascii").to_owned());
    let body = to_bytes(response.into_body(), 1 << 20).await.expect("body");
    (
        status,
        cache,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}

fn state(gateway: &MockServer, mirror: Mirror) -> AppState {
    let mut config = Config::for_tests();
    config.gateway_url = Some(gateway.uri());
    AppState::new(config, None).with_mirror(Arc::new(mirror))
}

fn standard<'a>(answer: &'a Value, name: &str) -> &'a Value {
    answer["standards"]
        .as_array()
        .and_then(|list| list.iter().find(|s| s["representation"] == name))
        .unwrap_or_else(|| panic!("{name} in {answer}"))
}

fn examples(answer: &Value, name: &str) -> Vec<(String, String, u64)> {
    standard(answer, name)["examples"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            (
                e["kind"].as_str().unwrap_or_default().to_owned(),
                e["url"].as_str().unwrap_or_default().to_owned(),
                e["answered"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_examples_are_built_from_one_real_entity_and_each_answered() {
    let slug = "standards-real-entity";
    let gateway = gateway(slug).await;
    let (status, cache, answer) = get(
        state(&gateway, mirror(slug, "public")),
        &format!("/api/v1/catalogue/endpoints/{slug}/standards"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(cache.as_deref(), Some("public, max-age=3600"));
    assert_eq!(
        answer["entityType"], "AirQualityObserved",
        "the first type with an entity"
    );
    assert_eq!(answer["types"], json!(["EmptyType", "AirQualityObserved"]));
    let order: Vec<&str> = answer["standards"]
        .as_array()
        .expect("standards")
        .iter()
        .filter_map(|s| s["representation"].as_str())
        .collect();
    assert_eq!(
        order,
        ["ngsi-ld", "ogc-features", "odata", "sta"],
        "csv is no query standard"
    );

    let shown = Config::for_tests().public_base_url;
    let base = format!(
        "{}api/endpoint/{slug}/",
        shown.origin().ascii_serialization() + "/"
    );
    let ngsi = examples(&answer, "ngsi-ld");
    assert_eq!(ngsi.len(), 2, "{answer}");
    assert_eq!(ngsi[0].0, "equality");
    assert_eq!(ngsi[0].2, 1);
    assert_eq!((ngsi[1].0.as_str(), ngsi[1].2), ("range", 2));
    for (_, url, _) in ngsi.iter().chain(&examples(&answer, "ogc-features")) {
        assert!(
            url.starts_with(&base),
            "a visitor's URL, never the gateway's: {url}"
        );
        assert!(!url.contains(&gateway.uri()), "{url}");
    }
    assert_eq!(
        standard(&answer, "ogc-features")["fields"],
        json!(["name", "no2"]),
        "the queryables, without id and geometry"
    );
    assert_eq!(examples(&answer, "ogc-features").len(), 2, "{answer}");
    let odata = examples(&answer, "odata");
    assert_eq!(
        odata.len(),
        1,
        "the range answered nothing and is left out: {answer}"
    );
    assert_eq!(odata[0].0, "equality");
    assert_eq!(standard(&answer, "odata")["fields"], json!(["name", "no2"]));
    assert!(
        examples(&answer, "sta").is_empty(),
        "a surface that does not answer shows no example"
    );
    assert!(
        !answer.to_string().contains("Hlavn"),
        "a value with a space is never quoted into an example"
    );

    // T-5111: what a developer page needs beyond the standards.
    assert_eq!(answer["base"], base);
    assert_eq!(
        answer["served"],
        json!(["ngsi-ld", "ogc-features", "odata", "sta", "csv", "mcp"])
    );
    assert_eq!(
        answer["licence"],
        json!({
            "code": "CC_BY_4_0",
            "title": "Creative Commons Attribution 4.0",
            "url": "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0"
        })
    );
    assert_eq!(
        answer["requestsPerMinute"], 5000,
        "the catalog's default for an Organization that sets none"
    );
    assert_eq!(
        standard(&answer, "ngsi-ld")["first"],
        format!("{base}ngsi-ld/v1/entities?type=AirQualityObserved&limit=5")
    );
    assert_eq!(
        standard(&answer, "ogc-features")["first"],
        format!("{base}ogc/features/collections/AirQualityObserved/items?limit=5")
    );
    assert_eq!(
        standard(&answer, "odata")["first"],
        Value::Null,
        "a first page with no entity is no sample"
    );
}

#[tokio::test]
async fn an_endpoint_that_is_not_public_or_unknown_is_a_404() {
    let slug = "standards-restricted";
    let gateway = gateway(slug).await;
    for uri in [
        format!("/api/v1/catalogue/endpoints/{slug}/standards"),
        "/api/v1/catalogue/endpoints/no-such-endpoint/standards".to_owned(),
    ] {
        let (status, _, _) = get(state(&gateway, mirror(slug, "restricted")), &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    assert!(
        gateway
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "nothing is read for an Endpoint a visitor may not see"
    );
}

#[tokio::test]
async fn an_endpoint_whose_gateway_is_down_lists_its_standards_without_examples() {
    let slug = "standards-gateway-down";
    let mut config = Config::for_tests();
    config.gateway_url = Some("http://127.0.0.1:9".to_owned());
    let state = AppState::new(config, None).with_mirror(Arc::new(mirror(slug, "public")));
    let (status, _, answer) = get(
        state,
        &format!("/api/v1/catalogue/endpoints/{slug}/standards"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["entityType"], Value::Null);
    for name in ["ngsi-ld", "ogc-features", "odata", "sta"] {
        assert!(examples(&answer, name).is_empty(), "{name}: {answer}");
    }
}
