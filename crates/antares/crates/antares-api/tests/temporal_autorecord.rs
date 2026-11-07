// SPDX-License-Identifier: EUPL-1.2
//! 5.6.4 + auto-recording: every Partial Attribute Update appends a new
//! attribute instance to the temporal evolution — the regression behind the
//! playground's flat history charts (create recorded, PATCHes silently not).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, String) {
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_update_appends_temporal_instances() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header(
                "Content-Length",
                r#"{"id":"urn:ngsi-ld:Rec:1","type":"Rec",
                    "v":{"type":"Property","value":1,"observedAt":"2026-08-08T14:00:00Z"}}"#
                    .len(),
            )
            .body(Body::from(
                r#"{"id":"urn:ngsi-ld:Rec:1","type":"Rec",
                    "v":{"type":"Property","value":1,"observedAt":"2026-08-08T14:00:00Z"}}"#,
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    for (i, ts) in [(2, "2026-08-08T14:00:10Z"), (3, "2026-08-08T14:00:20Z")] {
        let (status, body) = send(
            &st,
            Request::builder()
                .method("PATCH")
                .uri("/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:1/attrs/v")
                .header("Content-Type", "application/json")
                .header(
                    "Content-Length",
                    format!(r#"{{"type":"Property","value":{i},"observedAt":"{ts}"}}"#).len(),
                )
                .body(Body::from(format!(
                    r#"{{"type":"Property","value":{i},"observedAt":"{ts}"}}"#
                )))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:1")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");
    let instances = doc["v"].as_array().expect("v instance array");
    let values: Vec<i64> = instances
        .iter()
        .filter_map(|i| i["value"].as_i64())
        .collect();
    assert_eq!(
        values.len(),
        3,
        "each PATCH must append an instance, got {body}"
    );
    assert!(
        [1, 2, 3].iter().all(|v| values.contains(v)),
        "expected values 1..3, got {values:?}"
    );
}

/// 4.5.6: a scope change from the Core API is recorded in the temporal
/// evolution as a temporal Property instance whose observedAt "should be set
/// as a copy of the modifiedAt sub-Property".
#[tokio::test(flavor = "multi_thread")]
async fn scope_update_appends_temporal_property_instance() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:2","type":"Rec","scope":"/A",
        "v":{"type":"Property","value":1}}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let patch = r#"{"type":"Rec","scope":"/B"}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("PATCH")
            .uri("/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:2")
            .header("Content-Type", "application/json")
            .header("Content-Length", patch.len())
            .body(Body::from(patch))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:2?options=sysAttrs")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");
    let scope = doc["scope"].as_array().expect("scope instance array");
    let inst = scope
        .iter()
        .find(|i| i["value"] == serde_json::json!(["/B"]))
        .unwrap_or_else(|| panic!("no /B scope instance in {body}"));
    assert_eq!(inst["type"], "Property");
    assert_eq!(
        inst["observedAt"], inst["modifiedAt"],
        "observedAt must copy modifiedAt"
    );
}

/// 4.5.7: the temporal representation of a Property is an array of 4.5.2
/// instances; deletion records an instance with value "urn:ngsi-ld:null" and
/// deletedAt set; every instance carries an instanceId.
#[tokio::test(flavor = "multi_thread")]
async fn property_deletion_records_null_instance() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:3","type":"Rec",
        "v":{"type":"Property","value":7,"observedAt":"2026-08-10T00:00:00Z"}}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .method("DELETE")
            .uri("/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:3/attrs/v")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:3?timeproperty=deletedAt")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");
    let instances = doc["v"].as_array().expect("v instance array");
    let deleted = instances
        .iter()
        .find(|i| i["value"] == "urn:ngsi-ld:null")
        .unwrap_or_else(|| panic!("no null deletion instance in {body}"));
    assert!(deleted["deletedAt"].is_string(), "deletedAt must be set");
    assert!(
        deleted["instanceId"]
            .as_str()
            .is_some_and(|s| s.starts_with("urn:")),
        "instanceId must be maintained"
    );
}

/// 4.5.8: the temporal representation of a Relationship is an array of 4.5.3
/// instances; deletion records an instance with object "urn:ngsi-ld:null" and
/// deletedAt set; every instance carries an instanceId.
#[tokio::test(flavor = "multi_thread")]
async fn relationship_deletion_records_null_instance() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:4","type":"Rec",
        "r":{"type":"Relationship","object":"urn:ngsi-ld:X:1",
             "observedAt":"2026-08-10T00:00:00Z"}}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .method("DELETE")
            .uri("/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:4/attrs/r")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:4?timeproperty=deletedAt")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");
    let instances = doc["r"].as_array().expect("r instance array");
    let deleted = instances
        .iter()
        .find(|i| i["object"] == "urn:ngsi-ld:null")
        .unwrap_or_else(|| panic!("no null deletion instance in {body}"));
    assert_eq!(deleted["type"], "Relationship");
    assert!(
        deleted.get("value").is_none(),
        "a Relationship deletion instance must not carry a Property value"
    );
    assert!(deleted["deletedAt"].is_string(), "deletedAt must be set");
    assert!(
        deleted["instanceId"]
            .as_str()
            .is_some_and(|s| s.starts_with("urn:")),
        "instanceId must be maintained"
    );
}

/// 4.5.9: simplified temporal representation — per-type member names
/// (values/objects/languageMaps/valueLists/objectLists), [value, time] pairs,
/// bare ordered arrays for the List types (Examples 3 and 7), wrapped
/// {"languageMap": …} for LanguageProperty (Example 2).
#[tokio::test(flavor = "multi_thread")]
async fn simplified_temporal_pairs_per_attribute_type() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:5","type":"Rec",
        "speed":[{"type":"Property","value":1,"observedAt":"2026-08-10T00:00:01Z"},
                 {"type":"Property","value":2,"observedAt":"2026-08-10T00:00:02Z"}],
        "location":{"type":"GeoProperty","observedAt":"2026-08-10T00:00:01Z",
                    "value":{"type":"Point","coordinates":[1.0,2.0]}},
        "says":{"type":"LanguageProperty","observedAt":"2026-08-10T00:00:01Z",
                "languageMap":{"en":"yes"}},
        "steps":{"type":"ListProperty","observedAt":"2026-08-10T00:00:01Z",
                 "valueList":["a","b"]},
        "spouse":{"type":"Relationship","observedAt":"2026-08-10T00:00:01Z",
                  "object":"urn:ngsi-ld:P:1"},
        "route":{"type":"ListRelationship","observedAt":"2026-08-10T00:00:01Z",
                 "objectList":["urn:ngsi-ld:R:1","urn:ngsi-ld:R:2"]}}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:5?options=temporalValues")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");

    let speed = &doc["speed"];
    assert_eq!(speed["type"], "Property");
    let pairs = speed["values"].as_array().expect("values pairs");
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].as_array().expect("pair").len(), 2);
    assert_eq!(pairs[0][0].as_f64(), Some(1.0));
    assert_eq!(pairs[0][1], "2026-08-10T00:00:01Z");

    assert_eq!(doc["location"]["type"], "GeoProperty");
    assert_eq!(doc["location"]["values"][0][0]["type"], "Point");

    assert_eq!(
        doc["says"]["languageMaps"][0][0],
        serde_json::json!({"languageMap": {"en": "yes"}})
    );

    // Lists carry the BARE ordered array as the pair's first element
    assert_eq!(doc["steps"]["type"], "ListProperty");
    assert_eq!(
        doc["steps"]["valueLists"][0][0],
        serde_json::json!(["a", "b"])
    );
    assert_eq!(doc["spouse"]["objects"][0][0], "urn:ngsi-ld:P:1");
    assert_eq!(doc["route"]["type"], "ListRelationship");
    assert_eq!(
        doc["route"]["objectLists"][0][0],
        serde_json::json!(["urn:ngsi-ld:R:1", "urn:ngsi-ld:R:2"])
    );

    // the simplified object shall ONLY contain type + the per-type member
    // (datasetId when grouped): no reified value/object/instanceId leakage
    for (attr, member) in [
        ("speed", "values"),
        ("says", "languageMaps"),
        ("steps", "valueLists"),
        ("spouse", "objects"),
        ("route", "objectLists"),
    ] {
        let extra: Vec<&String> = doc[attr]
            .as_object()
            .expect("object")
            .keys()
            .filter(|k| !["type", "datasetId", member].contains(&k.as_str()))
            .collect();
        assert!(extra.is_empty(), "{attr} leaks members: {extra:?}");
    }
}

/// 4.5.19: aggregated temporal representation — per-method members of
/// [value, periodStart, periodEnd] triples; Properties labelled "Property",
/// Relationships "Relationship"; PT0S = one whole-range period; only the
/// requested methods (plus type) appear.
#[tokio::test(flavor = "multi_thread")]
async fn aggregated_temporal_representation() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:6","type":"Rec",
        "speed":[{"type":"Property","value":1,"observedAt":"2026-08-10T00:00:01Z"},
                 {"type":"Property","value":2,"observedAt":"2026-08-10T00:00:02Z"},
                 {"type":"Property","value":3,"observedAt":"2026-08-10T00:00:03Z"}],
        "spouse":[{"type":"Relationship","object":"urn:ngsi-ld:P:1",
                   "observedAt":"2026-08-10T00:00:01Z"},
                  {"type":"Relationship","object":"urn:ngsi-ld:P:1",
                   "observedAt":"2026-08-10T00:00:02Z"}]}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:6?options=aggregatedValues&aggrMethods=sum,avg,totalCount&aggrPeriodDuration=PT0S&attrs=speed&timerel=after&timeAt=2026-08-10T00:00:00Z")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");

    let speed = &doc["speed"];
    assert_eq!(speed["type"], "Property");
    let sum = speed["sum"].as_array().expect("sum rows");
    assert_eq!(sum.len(), 1, "PT0S = one whole-range period: {body}");
    let row = sum[0].as_array().expect("triple");
    assert_eq!(row.len(), 3, "[value, start, end]");
    assert_eq!(row[0].as_f64(), Some(6.0));
    assert!(row[1].as_str().is_some_and(|s| s.contains('T')));
    assert_eq!(speed["avg"][0][0].as_f64(), Some(2.0));
    assert_eq!(speed["totalCount"][0][0], 3);
    let extra: Vec<&String> = speed
        .as_object()
        .expect("object")
        .keys()
        .filter(|k| !["type", "sum", "avg", "totalCount"].contains(&k.as_str()))
        .collect();
    assert!(extra.is_empty(), "only requested methods: {extra:?}");

    // Relationship label + totalCount eligibility
    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:6?options=aggregatedValues&aggrMethods=totalCount&attrs=spouse&timerel=after&timeAt=2026-08-10T00:00:00Z")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let doc: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(doc["spouse"]["type"], "Relationship");
    assert_eq!(doc["spouse"]["totalCount"][0][0], 2);

    // ineligible method on a Relationship → 400 InvalidRequest
    let (status, body) = send(
        &st,
        Request::builder()
            .uri("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:6?options=aggregatedValues&aggrMethods=sum&attrs=spouse&timerel=after&timeAt=2026-08-10T00:00:00Z")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("InvalidRequest"), "{body}");
}

/// 4.11: aggrPeriodDuration is an ISO 8601 duration; a magnitude far beyond
/// any representable time range violates the parameter's value space and
/// answers 400 BadRequestData — it must never reach chrono duration
/// arithmetic, which panics (a remote 500) on such values.
#[tokio::test(flavor = "multi_thread")]
async fn absurd_aggr_period_duration_is_400_not_a_panic() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;

    let create = r#"{"id":"urn:ngsi-ld:Rec:7","type":"Rec",
        "speed":[{"type":"Property","value":1,"observedAt":"2026-08-10T00:00:01Z"},
                 {"type":"Property","value":2,"observedAt":"2026-08-10T00:00:02Z"}]}"#;
    let (status, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", create.len())
            .body(Body::from(create))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    for period in ["PT99999999999999999S", "P99999999999W"] {
        let (status, body) = send(
            &st,
            Request::builder()
                .uri(format!(
                    "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:7\
                     ?options=aggregatedValues&aggrMethods=sum&attrs=speed\
                     &timerel=after&timeAt=2026-08-10T00:00:00Z\
                     &aggrPeriodDuration={period}"
                ))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{period}: {body}");
        assert!(body.contains("BadRequestData"), "{period}: {body}");
        assert!(
            !body.contains("InternalError"),
            "must not be a 500-class answer: {body}"
        );
    }

    // a sane sub-range period still aggregates
    let (status, body) = send(
        &st,
        Request::builder()
            .uri(
                "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:7\
                 ?options=aggregatedValues&aggrMethods=sum&attrs=speed\
                 &timerel=after&timeAt=2026-08-10T00:00:00Z&aggrPeriodDuration=PT1H",
            )
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn req(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri);
    match body {
        Some(s) => b
            .header("Content-Type", "application/json")
            .header("Content-Length", s.len())
            .body(Body::from(s.to_owned())),
        None => b.body(Body::empty()),
    }
    .expect("request")
}

/// 5.6.5.4: an Entity held only in the temporal store is not held locally,
/// so Delete Attribute answers ResourceNotFound and records nothing; its
/// evolution belongs to 5.6.13 (forge suite fix f7c78f94, TP 020_17).
#[tokio::test(flavor = "multi_thread")]
async fn delete_attribute_of_a_temporal_only_entity_is_not_found() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:t1","type":"Rec",
        "speed":[{"type":"Property","value":1,"observedAt":"2020-01-01T00:00:00Z"}]}"#;
    let (s, b) = send(
        &st,
        req("POST", "/ngsi-ld/v1/temporal/entities", Some(body)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = send(
        &st,
        req(
            "DELETE",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:t1/attrs/speed",
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
    let (s, b) = send(
        &st,
        req(
            "GET",
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:t1",
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(!b.contains("deletedAt"), "nothing recorded: {b}");
}

/// 5.6.5.4: "If the target Entity does not contain the target Attribute then
/// an error of type ResourceNotFound shall be raised." The Attribute's
/// recorded history is no Attribute of the Entity: deleting it twice, or a
/// scope the Entity never had, answers 404 and records no second deletion
/// (Coraine entity_attr_crud, entity_member_delete_and_null).
#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_attribute_the_entity_no_longer_has_is_not_found() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:d1","type":"Rec",
        "brand":{"type":"Property","value":"A"},"speed":{"type":"Property","value":1}}"#;
    let (s, b) = send(&st, req("POST", "/ngsi-ld/v1/entities", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let brand = "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:d1/attrs/brand";
    let (s, b) = send(&st, req("DELETE", brand, None)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (s, b) = send(&st, req("DELETE", brand, None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
    let scope = "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:d1/attrs/scope";
    let (s, b) = send(&st, req("DELETE", scope, None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
    let (s, b) = send(
        &st,
        req(
            "GET",
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:d1",
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        b.matches("deletedAt").count(),
        1,
        "one deletion recorded: {b}"
    );
}

/// 4.18 and 4.5.0: a deleted scope reads as the NGSI-LD Null in the
/// temporal representation, not as an empty array (forge fix fad5ab03,
/// TP 020_19 and 020_20).
#[tokio::test(flavor = "multi_thread")]
async fn deleted_scope_is_the_ngsi_ld_null_in_the_temporal_representation() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:s1","type":"Rec","scope":"/A",
        "speed":{"type":"Property","value":1}}"#;
    let (s, b) = send(&st, req("POST", "/ngsi-ld/v1/entities", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = send(
        &st,
        req(
            "DELETE",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:s1/attrs/scope",
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (s, b) = send(
        &st,
        req(
            "GET",
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:s1?timeproperty=deletedAt",
            None,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let doc: serde_json::Value = serde_json::from_str(&b).expect("json");
    let deleted: Vec<&serde_json::Value> = doc["scope"]
        .as_array()
        .map(|a| a.iter().filter(|i| i.get("deletedAt").is_some()).collect())
        .unwrap_or_default();
    assert_eq!(deleted.len(), 1, "{b}");
    assert_eq!(deleted[0]["value"], "urn:ngsi-ld:null", "{b}");
}

/// 5.7.4.4, S9: the temporal representation holds only the instances that
/// meet the query, not every instance of an Entity the query selected
/// (forge fix 9ca4213e, TP 021_08). An instance meets `q` when `q` holds on
/// the Entity with that Attribute narrowed to that instance, so a term on
/// another Attribute keeps acting at Entity level.
#[tokio::test(flavor = "multi_thread")]
async fn temporal_query_keeps_only_the_instances_that_meet_q() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:q1","type":"Rec",
        "speed":[{"type":"Property","value":120,"observedAt":"2020-07-01T12:05:00Z"},
                 {"type":"Property","value":80,"observedAt":"2020-07-01T12:07:00Z"},
                 {"type":"Property","value":100,"observedAt":"2020-07-01T12:09:00Z"}],
        "brand":[{"type":"Property","value":"x","observedAt":"2020-07-01T12:05:00Z"}]}"#;
    let (s, b) = send(
        &st,
        req("POST", "/ngsi-ld/v1/temporal/entities", Some(body)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let values = |b: &str, attr: &str| -> Vec<i64> {
        let list: serde_json::Value = serde_json::from_str(b).expect("json");
        let mut v: Vec<i64> = list[0][attr]
            .as_array()
            .map(|a| a.iter().filter_map(|i| i["value"].as_i64()).collect())
            .unwrap_or_default();
        v.sort_unstable();
        v
    };
    let uri = "/ngsi-ld/v1/temporal/entities?type=Rec&q=speed%3E90\
               &timerel=after&timeAt=2020-07-01T12:00:00Z";
    let (s, b) = send(&st, req("GET", uri, None)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(values(&b, "speed"), [100, 120], "80 fails q: {b}");
    // an OR with a term on another Attribute: that term holds for the
    // Entity, so every speed instance meets the query
    let uri = "/ngsi-ld/v1/temporal/entities?type=Rec&q=speed%3E90%7Cbrand%3D%3D%22x%22\
               &timerel=after&timeAt=2020-07-01T12:00:00Z";
    let (s, b) = send(&st, req("GET", uri, None)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(values(&b, "speed"), [80, 100, 120], "{b}");
}

async fn temporal_doc(st: &AppState, uri: &str) -> serde_json::Value {
    let (s, b) = send(st, req("GET", uri, None)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    serde_json::from_str(&b).expect("json")
}

/// TS 104 175 5.2.4 (CIM 009 4.8): "modifiedAt" is "the temporal Property
/// at which the Entity or Attribute was last modified". An Attribute update
/// modifies the Entity, so the temporal representation's Entity modifiedAt
/// is the newest instance's, not the create's (Coraine
/// troe_entity_modifiedat_attr_update).
#[tokio::test(flavor = "multi_thread")]
async fn temporal_entity_modified_at_follows_attribute_updates() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:m1","type":"Rec","a":{"type":"Property","value":1}}"#;
    let (s, b) = send(&st, req("POST", "/ngsi-ld/v1/entities", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    for v in [2, 3] {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let patch = format!(r#"{{"type":"Property","value":{v}}}"#);
        let (s, b) = send(
            &st,
            req(
                "PATCH",
                "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:m1/attrs/a",
                Some(&patch),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    }
    for uri in [
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:m1?options=sysAttrs",
        "/ngsi-ld/v1/temporal/entities?type=Rec&timerel=after&timeAt=2000-01-01T00:00:00Z\
         &timeproperty=modifiedAt&options=sysAttrs",
    ] {
        let doc = temporal_doc(&st, uri).await;
        let e = doc.as_array().and_then(|a| a.first()).unwrap_or(&doc);
        let newest = e["a"]
            .as_array()
            .expect("a instances")
            .iter()
            .filter_map(|i| i["modifiedAt"].as_str())
            .max()
            .expect("instance modifiedAt");
        assert_eq!(e["modifiedAt"].as_str(), Some(newest), "{uri}: {e}");
        assert_ne!(e["modifiedAt"], e["createdAt"], "{uri}: {e}");
    }
}

/// CIM 009 4.5.7 (TS 104 175 5.3.2.5): "In case the Property is deleted, an
/// instance of the Property is recorded with its value set to the URI
/// "urn:ngsi-ld:null" and the deletedAt Temporal Property set." A replace
/// upsert that drops an instance (urn:ds:1) and an Attribute (P2) deletes
/// them, and a timeproperty=modifiedAt read finds the record (Coraine
/// troe_timescale_routes_batch step 02).
#[tokio::test(flavor = "multi_thread")]
async fn replace_upsert_records_each_removed_instance_as_deleted() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:u1","type":"Rec",
        "P1":[{"type":"Property","value":1},
              {"type":"Property","value":2,"datasetId":"urn:ds:1"}],
        "P2":{"type":"Property","value":9}}"#;
    let (s, b) = send(&st, req("POST", "/ngsi-ld/v1/entities", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let upsert = r#"[{"id":"urn:ngsi-ld:Rec:u1","type":"Rec",
        "P1":{"type":"Property","value":10}}]"#;
    let (s, b) = send(
        &st,
        req("POST", "/ngsi-ld/v1/entityOperations/upsert", Some(upsert)),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let doc = temporal_doc(
        &st,
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:u1?timerel=after\
         &timeAt=2020-01-01T00:00:00Z&timeproperty=modifiedAt",
    )
    .await;
    let deleted = |attr: &str| -> Vec<serde_json::Value> {
        doc[attr]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|i| i.get("deletedAt").is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };
    let p1 = deleted("P1");
    assert_eq!(p1.len(), 1, "{doc}");
    assert_eq!(p1[0]["datasetId"], "urn:ds:1", "{doc}");
    assert_eq!(p1[0]["value"], "urn:ngsi-ld:null", "{doc}");
    let p2 = deleted("P2");
    assert_eq!(p2.len(), 1, "{doc}");
    assert_eq!(p2[0]["value"], "urn:ngsi-ld:null", "{doc}");
    assert_eq!(p2[0].get("datasetId"), None, "{doc}");
}

/// CIM 009 4.5.8 (TS 104 175 5.3.2.5): "In case the Relationship is deleted,
/// an instance of the Relationship is recorded with its object set to the
/// URI "urn:ngsi-ld:null" and the deletedAt Temporal Property set." A
/// Merge Entity NGSI-LD Null deletes the Relationship as Delete Attribute
/// does (Coraine troe_timescale_temporal_deleted_kinds step 04).
#[tokio::test(flavor = "multi_thread")]
async fn merge_patch_null_records_the_relationship_as_deleted() {
    let mut st = AppState::new("test".into());
    antares_api::wire(&mut st).await;
    let body = r#"{"id":"urn:ngsi-ld:Rec:n1","type":"Rec",
        "locatedAt":{"type":"Relationship","object":"urn:ngsi-ld:Place:1",
                     "observedAt":"2026-06-01T09:00:00Z"}}"#;
    let (s, b) = send(&st, req("POST", "/ngsi-ld/v1/entities", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = send(
        &st,
        req(
            "PATCH",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Rec:n1",
            Some(r#"{"locatedAt":"urn:ngsi-ld:null"}"#),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let doc = temporal_doc(
        &st,
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Rec:n1?timeproperty=deletedAt",
    )
    .await;
    let inst = doc["locatedAt"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i.get("deletedAt").is_some()))
        .unwrap_or_else(|| panic!("no deletion recorded: {doc}"));
    assert_eq!(inst["type"], "Relationship", "{doc}");
    assert_eq!(inst["object"], "urn:ngsi-ld:null", "{doc}");
}
