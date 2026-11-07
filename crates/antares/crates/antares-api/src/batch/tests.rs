// SPDX-License-Identifier: EUPL-1.2
//! Unit tests of the batch operations.

/// T-3531: a remote error status is taken only when it is one.
#[test]
fn remote_status_takes_only_an_http_error_status() {
    use serde_json::json;
    let e = |s: Value| json!({"error": {"status": s}});
    assert_eq!(super::remote_status(&e(json!(404)), 207), 404);
    assert_eq!(super::remote_status(&e(json!(65_736)), 207), 207);
    assert_eq!(super::remote_status(&e(json!(200)), 207), 207);
    assert_eq!(super::remote_status(&e(json!("404")), 207), 207);
    assert_eq!(super::remote_status(&json!({}), 500), 500);
}

use super::*;
use axum::body::Body;
use axum::http::Request;
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> Router {
    crate::router(AppState::new("antares-test".into()))
}

async fn post(app: &Router, uri: &str, body: Value) -> Response {
    let s = body.to_string();
    app.clone()
        .oneshot(
            Request::post(uri)
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len())
                .body(Body::from(s))
                .expect("req"),
        )
        .await
        .expect("resp")
}

async fn get_status(app: &Router, uri: &str) -> StatusCode {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
        .await
        .expect("resp")
        .status()
}

async fn body_json(resp: Response) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

/// 5.7.2.4 (p. 201): "At least one of the following input data shall be
/// provided: a) selector of Entity Types; b) list of Attribute names,
/// including at least one non-system Attribute; c) NGSI-LD Query,
/// including at least one non-system Attribute; d) NGSI-LD GeoQuery;
/// e) local scope. If none of the above is provided, then an error of
/// type BadRequestData shall be raised (too wide query)." Query Entities
/// is one operation, so the resource that carries the Query in a body is
/// bound by it exactly as the one that carries it in the URI.
#[tokio::test]
async fn a_too_wide_query_body_is_refused_like_its_uri_twin() {
    let app = app();
    for wide in [
        json!({"type": "Query"}),
        // ids and an id pattern alone are the case the clause names as
        // insufficient ("it is not possible to retrieve a set of entities
        // by only specifying desired Entity identifiers")
        json!({"type": "Query", "entities": [{"id": "urn:ngsi-ld:Vehicle:1"}]}),
        json!({"type": "Query", "entities": [{"idPattern": ".*"}]}),
        // a system Attribute qualifies neither as an attrs list nor as q
        json!({"type": "Query", "attrs": ["createdAt"]}),
        json!({"type": "Query", "q": "createdAt>\"2020-01-01T00:00:00Z\""}),
    ] {
        let resp = post(&app, "/ngsi-ld/v1/entityOperations/query", wide.clone()).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "too wide query accepted: {wide}"
        );
        let body = body_json(resp).await;
        assert_eq!(
            body["type"], "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
            "{body}"
        );
    }

    // Each of the five qualifying inputs on its own is still served.
    for ok in [
        json!({"type": "Query", "entities": [{"type": "Vehicle"}]}),
        json!({"type": "Query", "attrs": ["speed"]}),
        json!({"type": "Query", "q": "speed>100"}),
        json!({"type": "Query", "geoQ": {"georel": "near;maxDistance==2000",
               "geometry": "Point", "coordinates": "[1,2]"}}),
    ] {
        let resp = post(&app, "/ngsi-ld/v1/entityOperations/query", ok.clone()).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "qualifying query refused: {ok}"
        );
    }
}

/// A Query member lifted out of the body becomes the parameter its GET
/// twin carries in the URI, where 6.3.4 caps it — the POST form must not
/// be the cheap way past that cap.
#[tokio::test]
async fn query_body_members_are_capped_like_the_uri() {
    let app = app();
    let huge = "a".repeat(crate::bounds::MAX_URI_BYTES + 1);
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}], "q": format!("name==\"{huge}\"")}),
    ).await
    ;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert!(
        body["title"].as_str().is_some_and(|t| t.contains("Bad")),
        "{body}"
    );

    // the same member just inside the cap is still served
    let ok = "a".repeat(64);
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}], "q": format!("name==\"{ok}\"")}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "a normal query still works");

    // and the array members are assembled under the same cap
    let many: Vec<Value> = (0..600)
        .map(|i| Value::String(format!("attribute-with-a-long-name-{i:04}")))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}], "attrs": many}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// The same cap again, for the members `ordering` and `aggrParams` are
/// lifted into. 5.2.43 makes `orderBy` a String[] and 5.2.44 makes
/// `aggrMethods` one, and both are joined into the flat parameter the GET
/// twin carries in its URI — where 6.3.4's bare 414 caps them. They went
/// into the map uncapped, so the POST form handed the 4.23 ordering and
/// the 4.5.19 aggregation parsers a string the GET form cannot express:
/// one body inside `MAX_BODY_BYTES` holds ~150 000 order keys, each
/// expanded once per Entity comparison.
#[tokio::test]
async fn ordering_and_aggregation_members_are_capped_like_the_uri() {
    let app = app();
    let many: Vec<Value> = (0..600)
        .map(|i| Value::String(format!("attributeWithALongOrderKeyName{i:04}")))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}],
               "ordering": {"orderBy": many}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an over-cap orderBy is refused"
    );

    // the temporal twin carries aggrParams; its aggrMethods is joined the
    // same way and takes the same cap
    let methods: Vec<Value> = (0..600)
        .map(|i| Value::String(format!("totallyUnknownAggregationMethod{i:04}")))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/temporal/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}],
               "temporalQ": {"timerel": "before", "timeAt": "2026-01-01T00:00:00Z"},
               "aggrParams": {"aggrMethods": methods}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an over-cap aggrMethods is refused"
    );

    // an ordinary ordering is untouched
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}],
               "ordering": {"orderBy": ["name", "!speed"], "collation": "sk"}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a normal ordering still works"
    );
}

/// The same cap, for the three parameters the `entities` selectors are
/// lifted into. 5.2.33 makes `entities` an array of EntitySelectors, and
/// `query_doc_params` folds their `type`, `id` and `idPattern` members
/// into the flat parameters the GET twin carries in its URI — where
/// 6.3.4's bare 414 caps them at `MAX_URI_BYTES`. They went into the map
/// before the cap existed in the function, so the POST form handed the
/// expansion and store layers a string the GET form cannot express:
/// ~320 000 selectors fit inside `MAX_BODY_BYTES`, one expanded IRI and
/// one bind parameter each, past PostgreSQL's 65 535-parameter limit.
#[tokio::test]
async fn entity_selector_members_are_capped_like_the_uri() {
    let app = app();
    let many_types: Vec<Value> = (0..600)
        .map(|i| json!({"type": format!("VeryLongTypeNameForTheCap{i:04}")}))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": many_types}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the joined type parameter is capped like its GET twin"
    );

    let many_ids: Vec<Value> = (0..400)
        .map(|i| json!({"id": format!("urn:ngsi-ld:Vehicle:a-fairly-long-identifier-{i:06}")}))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": many_ids}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the joined id parameter is capped like its GET twin"
    );

    let many_pats: Vec<Value> = (0..400)
        .map(|i| json!({"idPattern": format!("^urn:ngsi-ld:Vehicle:pattern-{i:06}.*$")}))
        .collect();
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": many_pats}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the joined idPattern parameter is capped like its GET twin"
    );

    // The GeoQuery strings are lifted the same way (5.2.13).
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}],
               "geoQ": {"georel": format!("near;maxDistance=={}", "9".repeat(
                   crate::bounds::MAX_URI_BYTES + 1)),
                        "geometry": "Point", "coordinates": [0, 0]}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "geoQ georel is capped like its GET twin"
    );

    // …but `coordinates` keeps its own ceiling: MAX_GEO_VERTICES, which a
    // legal polygon can spend more than MAX_URI_BYTES on. Capping it in
    // bytes too would refuse a geometry the broker advertises support for.
    // A circle: a valid ring, as the area relations ask (T-3912).
    let ring: Vec<Value> = (0..=600)
        .map(|i| f64::from(i % 600) * std::f64::consts::TAU / 600.0)
        .map(|a| json!([a.cos() / 100.0, a.sin() / 100.0]))
        .collect();
    let coords = json!([ring]);
    assert!(
        coords.to_string().len() > crate::bounds::MAX_URI_BYTES,
        "the polygon has to exceed the byte cap for this to prove anything"
    );
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [{"type": "Vehicle"}],
               "geoQ": {"georel": "within", "geometry": "Polygon",
                        "coordinates": coords}}),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a polygon under MAX_GEO_VERTICES is served whatever it weighs"
    );

    // The cap may not cost an ordinary multi-selector query: 5.2.33's
    // union of a handful of selectors stays well inside it.
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/query",
        json!({"type": "Query", "entities": [
            {"type": "Vehicle", "id": "urn:ngsi-ld:Vehicle:1"},
            {"type": "Vehicle", "id": "urn:ngsi-ld:Vehicle:2"}
        ]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "a normal union still works");
}

async fn get_entity(app: &Router, id: &str) -> Value {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/ngsi-ld/v1/entities/{id}"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK, "entity {id} readable");
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

/// 6.3.7: `options` is a comma separated list of strings, so the
/// 6.15.3.1 "update" upsert mode applies whenever it is one member of
/// the list — existing Entity content is updated, not replaced.
#[tokio::test]
async fn upsert_update_mode_in_option_list_merges() {
    let app = app();
    let id = "urn:ngsi-ld:Building:optlist-upsert";
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/create",
        json!([{"id": id, "type": "Building",
                "speed": {"type": "Property", "value": 1},
                "brand": {"type": "Property", "value": "acme"}}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/upsert?options=update,sysAttrs",
        json!([{"id": id, "type": "Building",
                "speed": {"type": "Property", "value": 2}}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let doc = get_entity(&app, id).await;
    assert_eq!(doc["speed"]["value"], 2, "the payload attribute is applied");
    // update mode must NOT destroy attributes absent from the payload
    assert_eq!(
        doc["brand"]["value"], "acme",
        "update mode keeps attributes not in the payload: {doc}"
    );
}

/// A member that restates what is stored writes nothing (T-5050): its
/// `modifiedAt` stays where the last real change left it, and the next
/// real change still lands and moves it.
#[tokio::test]
async fn an_unchanged_upsert_member_writes_nothing() {
    let app = app();
    let id = "urn:ngsi-ld:Vehicle:parked";
    let v = |speed: i64| {
        json!([{"id": id, "type": "Vehicle",
                "speed": {"type": "Property", "value": speed,
                          "observedAt": "2026-10-21T18:00:00Z"}}])
    };
    let uri = "/ngsi-ld/v1/entityOperations/upsert?options=update";
    assert_eq!(post(&app, uri, v(0)).await.status(), StatusCode::CREATED);
    let stamp = |d: &Value| d["modifiedAt"].clone();
    let first = get_entity(&app, &format!("{id}?options=sysAttrs")).await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_eq!(post(&app, uri, v(0)).await.status(), StatusCode::NO_CONTENT);
    let again = get_entity(&app, &format!("{id}?options=sysAttrs")).await;
    assert_eq!(stamp(&again), stamp(&first), "no change, no write: {again}");
    assert_eq!(again["speed"]["modifiedAt"], first["speed"]["modifiedAt"]);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_eq!(post(&app, uri, v(7)).await.status(), StatusCode::NO_CONTENT);
    let moved = get_entity(&app, &format!("{id}?options=sysAttrs")).await;
    assert_eq!(moved["speed"]["value"], 7);
    assert_ne!(
        stamp(&moved),
        stamp(&first),
        "a real change is written: {moved}"
    );
}

/// Only the Entity's and the instances' own stamps are skipped; a
/// `modifiedAt` inside a value is data.
#[test]
fn same_but_stamps_skips_only_the_merge_stamps() {
    let doc = |m: &str, inner: &str| {
        json!({"id": "urn:x", "type": ["T"], "modifiedAt": m,
               "p": [{"type": "Property", "modifiedAt": m,
                      "value": {"modifiedAt": inner}}]})
    };
    assert!(super::write::same_but_stamps(
        &doc("1", "a"),
        &doc("2", "a")
    ));
    assert!(!super::write::same_but_stamps(
        &doc("1", "a"),
        &doc("1", "b")
    ));
    let mut more = doc("1", "a");
    more["q"] = json!([{"type": "Property", "value": 1}]);
    assert!(!super::write::same_but_stamps(&doc("1", "a"), &more));
    assert!(!super::write::same_but_stamps(&more, &doc("1", "a")));
}

/// 4.22 + 5.6.9.4 end to end. 5.6.9.4 (PDF p.175): "For each of the
/// NGSI-LD Entities included in the input Array execute the behaviour
/// defined by clause 5.6.3, but limited to a local operation… If the
/// Entity update failed, then a new BatchEntityError shall be added to E
/// containing the failed Entity ID and the ProblemDetails associated."
/// 4.22 makes an entity past its `expiresAt` invalid, so 5.6.3 answers
/// ResourceNotFound and the batch has to report it in E — not in S, and
/// not by writing to it.
#[tokio::test]
async fn batch_update_of_an_expired_entity_is_an_error_not_a_success() {
    let app = app();
    let id = "urn:ngsi-ld:Building:expired-batch-update";
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/create",
        json!([{"id": id, "type": "Building", "expiresAt": "2020-01-01T00:00:00Z",
                "speed": {"type": "Property", "value": 1}}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    // The premise the client can see: it is already absent to a read.
    assert_eq!(
        get_status(&app, &format!("/ngsi-ld/v1/entities/{id}")).await,
        StatusCode::NOT_FOUND,
        "4.22: an expired entity does not exist"
    );

    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/update",
        json!([{"id": id, "type": "Building",
                "speed": {"type": "Property", "value": 2}}]),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::MULTI_STATUS,
        "5.6.9.5: none updated, so S and E are reported"
    );
    let outcome = body_json(resp).await;
    assert!(
        outcome["success"].as_array().is_none_or(|a| a.is_empty()),
        "an entity every read refuses may not be reported as updated: {outcome}"
    );
    let errors = outcome["errors"].as_array().expect("E array");
    assert_eq!(errors.len(), 1, "{outcome}");
    assert_eq!(errors[0]["entityId"], id, "{outcome}");
    assert!(
        errors[0]["error"]["type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceNotFound")),
        "5.6.3 on an absent entity is ResourceNotFound: {outcome}"
    );

    // and the write did not happen behind the error
    assert_eq!(
        get_status(&app, &format!("/ngsi-ld/v1/entities/{id}")).await,
        StatusCode::NOT_FOUND,
        "still absent"
    );
}

/// 6.3.7 + 6.16.3.1: "noOverwrite" as one member of the `options` list
/// disables Attribute overwrite for Batch Entity Update (5.6.9).
#[tokio::test]
async fn batch_update_no_overwrite_in_option_list_is_honoured() {
    let app = app();
    let id = "urn:ngsi-ld:Building:optlist-update";
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/create",
        json!([{"id": id, "type": "Building",
                "speed": {"type": "Property", "value": 1}}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/update?options=noOverwrite,sysAttrs",
        json!([{"id": id, "type": "Building",
                "speed": {"type": "Property", "value": 2},
                "brand": {"type": "Property", "value": "acme"}}]),
    )
    .await;
    // `speed` already existed, so the skip is a partial failure: 5.6.9.5
    // makes 207 with the S and E arrays the only correct answer.
    assert_eq!(
        resp.status(),
        StatusCode::MULTI_STATUS,
        "the skipped instance must be reported"
    );
    let outcome = body_json(resp).await;
    let errs = outcome["errors"].as_array().expect("E array");
    assert_eq!(errs.len(), 1, "{outcome}");
    assert_eq!(errs[0]["entityId"], id, "E names the entity: {outcome}");
    assert!(
        !outcome["success"]
            .as_array()
            .expect("S array")
            .iter()
            .any(|v| v.as_str() == Some(id)),
        "a partially skipped entity must not also be in S: {outcome}"
    );
    let doc = get_entity(&app, id).await;
    assert_eq!(
        doc["speed"]["value"], 1,
        "noOverwrite leaves the existing instance alone: {doc}"
    );
    assert_ne!(doc["speed"]["value"], 2, "the payload must not overwrite");
    assert_eq!(doc["brand"]["value"], "acme", "new attributes are appended");
}

/// 5.6.7.4: what merges into the client's S and E arrays is the outcome
/// of the Entities this broker forwarded. Ids a Context Source invents
/// are dropped, and its error text is never relayed verbatim.
#[tokio::test]
async fn remote_batch_results_are_confined_to_forwarded_ids() {
    let sent = vec!["urn:ngsi-ld:Building:mine".to_owned()];
    let (mut ok, mut err) = (Vec::new(), Vec::new());
    merge_remote_batch(
        207,
        &json!({
            "success": ["urn:ngsi-ld:Building:mine", "urn:ngsi-ld:Secret:peer"],
            "errors": [
                {"entityId": "urn:ngsi-ld:Building:mine",
                 "error": {"status": 404, "detail": "row 42 of table peer_secrets"}},
                {"entityId": "urn:ngsi-ld:Secret:other",
                 "error": {"status": 409, "detail": "peer internals"}}
            ]
        }),
        &sent,
        false,
        &mut ok,
        &mut err,
    );
    assert_eq!(
        ok,
        vec![("urn:ngsi-ld:Building:mine".to_owned(), false)],
        "only forwarded ids reach S"
    );
    assert_eq!(err.len(), 1, "only forwarded ids reach E: {err:?}");
    assert_eq!(err[0]["entityId"], "urn:ngsi-ld:Building:mine");
    let dump = Value::Array(err.clone()).to_string();
    assert!(
        !dump.contains("Secret") && !dump.contains("peer"),
        "peer ids and error text must not be relayed: {dump}"
    );
    assert_eq!(err[0]["error"]["status"], 404, "the remote status travels");
    // a 2xx id list is confined the same way
    let (mut ok, mut err) = (Vec::new(), Vec::new());
    merge_remote_batch(
        201,
        &json!(["urn:ngsi-ld:Building:mine", "urn:ngsi-ld:Secret:peer"]),
        &sent,
        true,
        &mut ok,
        &mut err,
    );
    assert_eq!(ok, vec![("urn:ngsi-ld:Building:mine".to_owned(), true)]);
    assert!(err.is_empty());
}

/// 5.6.10.3: the Batch Entity Delete input is "an array of Entity IDs"
/// with no per-operation exemption from this broker's batch ceiling —
/// an over-cap array is rejected whole and deletes nothing.
#[tokio::test]
async fn batch_delete_over_the_item_cap_deletes_nothing() {
    let app = app();
    let id = "urn:ngsi-ld:Building:delete-cap";
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/create",
        json!([{"id": id, "type": "Building"}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let mut ids: Vec<Value> = (0..*crate::bounds::MAX_BATCH_ITEMS)
        .map(|i| Value::String(format!("urn:ngsi-ld:Building:cap-{i}")))
        .collect();
    ids.push(Value::String(id.to_owned()));
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/delete",
        Value::Array(ids),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "over-cap delete");
    let doc = body_json(resp).await;
    assert!(
        doc["type"]
            .as_str()
            .expect("problem type")
            .ends_with("BadRequestData"),
        "{doc}"
    );
    // rejected whole: the in-range entity of the same array survives
    get_entity(&app, id).await;
}

/// 5.6.8.5 third bullet: when only some Entities succeeded, S is "the
/// list of Entities successfully created **or updated**" — the
/// created-only list of the second bullet is the all-succeeded case.
#[tokio::test]
async fn upsert_207_success_carries_updated_entities_too() {
    let app = app();
    let existing = "urn:ngsi-ld:Building:upsert-207-old";
    let fresh = "urn:ngsi-ld:Building:upsert-207-new";
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/create",
        json!([{"id": existing, "type": "Building"}]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = post(
        &app,
        "/ngsi-ld/v1/entityOperations/upsert",
        json!([
            {"id": existing, "type": "Building",
             "speed": {"type": "Property", "value": 1}},
            {"id": fresh, "type": "Building"},
            "not an entity"
        ]),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
    let doc = body_json(resp).await;
    let s: Vec<&str> = doc["success"]
        .as_array()
        .expect("S array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(s.contains(&existing), "the updated Entity is in S: {doc}");
    assert!(s.contains(&fresh), "the created Entity is in S: {doc}");
    assert_eq!(doc["errors"].as_array().expect("E array").len(), 1, "{doc}");
}

/// The stored Entity before (create) and the upsert?options=update fragment
/// of the T-5657 cases: two instances of `p1`, each with two sub-Attributes,
/// and a fragment that names each instance with only one of them.
fn two_instance_docs(id: &str) -> (Value, Value) {
    let sub = |v: i64| json!({"type": "Property", "value": v});
    let created = json!([{"id": id, "type": "Building",
            "p1": [{"type": "Property", "value": 1, "a": sub(1), "b": sub(2)},
                   {"type": "Property", "value": 11, "datasetId": "urn:ds:1",
                    "x": sub(1), "y": sub(2)}]}]);
    let fragment = json!([{"id": id, "type": "Building",
            "p1": [{"type": "Property", "value": 3, "a": sub(3)},
                   {"type": "Property", "value": 13, "datasetId": "urn:ds:1",
                    "x": sub(3)}]}]);
    (created, fragment)
}

/// Each named instance holds only what the fragment supplied and keeps its
/// `createdAt` (10.2.3.4).
fn assert_instances_replaced(before: &Value, doc: &Value) {
    let instances = doc["p1"].as_array().expect("two instances");
    assert_eq!(instances.len(), 2, "{doc}");
    let default = instances.iter().find(|i| i.get("datasetId").is_none());
    let ds1 = instances.iter().find(|i| i["datasetId"] == "urn:ds:1");
    let (Some(default), Some(ds1)) = (default, ds1) else {
        panic!("both instances kept: {doc}");
    };
    assert_eq!(default["value"], 3);
    assert_eq!(default["a"]["value"], 3);
    assert_eq!(default.get("b"), None, "b left the default instance: {doc}");
    assert_eq!(ds1["value"], 13);
    assert_eq!(ds1["x"]["value"], 3);
    assert_eq!(ds1.get("y"), None, "y left the urn:ds:1 instance: {doc}");
    let created = |d: &Value| {
        d["p1"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i.get("datasetId").is_none()))
            .map(|i| i["createdAt"].clone())
    };
    assert_ne!(created(before), Some(Value::Null), "{before}");
    assert_eq!(created(doc), created(before), "createdAt kept: {doc}");
}

/// 10.3.3.4: an upsert in "update" mode runs Update Attributes (10.2.3.4)
/// on an existing Entity, and there the instance the fragment names "shall
/// be replaced by the new one supplied", its `createdAt` kept: a
/// sub-Attribute the fragment leaves out leaves the instance (T-5657).
#[tokio::test]
async fn upsert_update_mode_replaces_the_named_instance() {
    let app = app();
    let id = "urn:ngsi-ld:Building:upsert-replaces-instance";
    let (created, fragment) = two_instance_docs(id);
    let resp = post(&app, "/ngsi-ld/v1/entityOperations/create", created).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let before = get_entity(&app, &format!("{id}?options=sysAttrs")).await;
    let uri = "/ngsi-ld/v1/entityOperations/upsert?options=update";
    let resp = post(&app, uri, fragment).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let doc = get_entity(&app, &format!("{id}?options=sysAttrs")).await;
    assert_instances_replaced(&before, &doc);
}

/// The same through a view (ADR-0023): the view path (`upsert_in_view`)
/// replaces each named instance whole too (T-5657).
#[cfg(feature = "views")]
#[tokio::test]
async fn upsert_update_mode_through_a_view_replaces_the_named_instance() {
    use crate::policy::Rule;
    use crate::views::CompiledView;
    let st = AppState::new("antares-test".into());
    let plain = crate::router(st.clone());
    let id = "urn:ngsi-ld:Building:view-upsert-replaces-instance";
    let (created, fragment) = two_instance_docs(id);
    let resp = post(&plain, "/ngsi-ld/v1/entityOperations/create", created).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let before = get_entity(&plain, &format!("{id}?options=sysAttrs")).await;
    let view = CompiledView::new(
        &["Building".into()],
        None,
        None,
        None,
        vec![Rule {
            types: vec!["Building".into()],
            attributes: vec!["p1".into()],
            ..Rule::default()
        }],
    )
    .expect("a view");
    let viewed = crate::router(st.with_view(std::sync::Arc::new(view)));
    let uri = "/ngsi-ld/v1/entityOperations/upsert?options=update";
    let resp = post(&viewed, uri, fragment).await;
    let status = resp.status();
    assert_eq!(status, StatusCode::NO_CONTENT, "{}", body_json(resp).await);
    let doc = get_entity(&plain, &format!("{id}?options=sysAttrs")).await;
    assert_instances_replaced(&before, &doc);
}
