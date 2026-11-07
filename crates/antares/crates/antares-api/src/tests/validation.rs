// SPDX-License-Identifier: EUPL-1.2
//! HTTP-level request validation: purge, retrieve, query, CSR, type=*, Content-Length and Accept (5.6.21.4, 5.7, 5.9.2.4, 6.3.4).

use super::*;

// ---- 5.6.21.4 Purge: the five qualifying conditions -------------------

async fn create(app: &Router, id: &str, ty: &str) {
    let entity = serde_json::json!({"id": id, "type": ty,
        "name": {"type": "Property", "value": "x"}});
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (entity.to_string()).len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
}

async fn purge(app: &Router, query: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::delete(format!("/ngsi-ld/v1/entities?{query}"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp")
        .status()
}

/// 4.3.6.1 on batch distribution: "all constraints specified in the
/// registration shall be respected" — a registration scoped to entity
/// type A must not swallow a batch item of type B. The B item is purely
/// local: it gets created; only the A item earns the Conflict part
/// (read-only registration), so the batch is a 207.
#[tokio::test]
async fn batch_items_outside_a_registrations_types_stay_local() {
    crate::allow_private();
    let app = app();
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:type-scope",
        "type": "ContextSourceRegistration",
        "mode": "redirect",
        "operations": ["retrieveEntity", "queryEntity"],
        "information": [{"entities": [{"type": "ScopedType"}]}],
        "endpoint": "http://127.0.0.1:1",
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", reg.to_string().len())
                .body(Body::from(reg.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let batch = serde_json::json!([
        {"id": "urn:ngsi-ld:ScopedType:remote", "type": "ScopedType",
         "name": {"type": "Property", "value": "x"}},
        {"id": "urn:ngsi-ld:Other:local", "type": "OtherType",
         "name": {"type": "Property", "value": "y"}},
    ]);
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header("Content-Length", batch.to_string().len())
                .body(Body::from(batch.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::MULTI_STATUS, "one conflict part");

    // the out-of-scope item was created locally…
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Other:local?local=true")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK, "non-matching item is local");
    // …and the in-scope one was refused everywhere (redirect, read-only)
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:ScopedType:remote?local=true")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn purge_rejects_id_and_idpattern_as_the_only_filter() {
    // 5.6.21.4: id/idPattern are legal input data (5.6.21.3) but are not
    // among the five qualifying conditions — "If none of the above is
    // provided, then an error of type BadRequestData shall be raised (too
    // wide query)". `idPattern=.*` alone used to delete the whole tenant.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:purge1", "Building").await;

    assert_eq!(purge(&app, "idPattern=.%2A").await, StatusCode::BAD_REQUEST);
    assert_eq!(
        purge(&app, "id=urn:ngsi-ld:Building:purge1").await,
        StatusCode::BAD_REQUEST
    );

    // the entity is still there — the guard ran before any deletion
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:purge1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn purge_requires_a_non_system_attribute_in_attrs_and_q() {
    // 5.6.21.4 b) and c): the Attribute list / query must include "at
    // least one non-system Attribute".
    let app = app();
    assert_eq!(
        purge(&app, "attrs=createdAt").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        purge(&app, "q=modifiedAt%3E%222020-01-01T00:00:00Z%22").await,
        StatusCode::BAD_REQUEST
    );
    // a real attribute qualifies
    assert_ne!(purge(&app, "attrs=name").await, StatusCode::BAD_REQUEST);
    assert_ne!(
        purge(&app, "q=name%3D%3D%22x%22").await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn purge_accepts_each_qualifying_condition() {
    // a) type, d) georel, e) local — none may 400 (5.6.21.4)
    let app = app();
    assert_ne!(purge(&app, "type=Building").await, StatusCode::BAD_REQUEST);
    assert_ne!(purge(&app, "local=true").await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn purge_rejects_linked_entity_paths() {
    // 5.6.21.4: "If projection attributes are present and indicate the
    // use of Linked Entity retrieval" and "If the filter conditions
    // specified by the query includes Linked Entity attributes" →
    // BadRequestData. `owner{name}` is the 4.9 LinkedEntityRelation form.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:lep1", "Building").await;
    assert_eq!(
        purge(&app, "q=owner%7Bname%7D%3D%3D%22x%22").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        purge(&app, "attrs=owner%7Bname%7D").await,
        StatusCode::BAD_REQUEST
    );
    // the entity must survive the rejected purges
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:lep1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn purge_validates_csf_syntax() {
    // 5.6.21.4: "if the query, geoquery or context source filter are not
    // syntactically valid ... an error of type BadRequestData shall be
    // raised". csf is a 4.9 query over Context Source properties.
    let app = app();
    assert_eq!(
        purge(&app, "type=Building&csf=%29%29bad%28%28").await,
        StatusCode::BAD_REQUEST
    );
    // a well-formed csf is accepted
    assert_ne!(
        purge(&app, "type=Building&csf=endpoint%3D%3D%22x%22").await,
        StatusCode::BAD_REQUEST
    );
}

// ---- 5.7.1.4 Retrieve Entity ------------------------------------------

async fn get_status(app: &Router, uri: &str, accept: Option<&str>) -> StatusCode {
    let mut req = Request::get(uri);
    if let Some(a) = accept {
        req = req.header("Accept", a);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).expect("req"))
        .await
        .expect("resp")
        .status()
}

#[tokio::test]
async fn retrieve_honors_the_type_selector() {
    // 5.7.1.4: ResourceNotFound when no entity "whose id (URI), and
    // where specified type, is equivalent" exists — ?type narrows the
    // retrieve target (4.17); a matching or wildcard selector passes.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:rt1", "Building").await;
    let uri = "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rt1";
    assert_eq!(
        get_status(&app, &format!("{uri}?type=Vehicle"), None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_status(&app, &format!("{uri}?type=Building"), None).await,
        StatusCode::OK
    );
    assert_eq!(
        get_status(&app, &format!("{uri}?type=%2A"), None).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn retrieve_geometry_property_needs_geojson_accept() {
    // 5.7.1.4: "If geometryProperty parameter is present and the Accept
    // Header is not set to application/geo+json ... BadRequestData".
    let app = app();
    create(&app, "urn:ngsi-ld:Building:rg1", "Building").await;
    let uri = "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rg1?geometryProperty=location";
    assert_eq!(get_status(&app, uri, None).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        get_status(&app, uri, Some("application/json")).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_status(&app, uri, Some("application/geo+json")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn retrieve_linked_projection_requires_join_and_depth() {
    // 5.7.1.4: projection attributes that "indicate the use of Linked
    // Entity retrieval" without join, or that project deeper than
    // joinLevel, are BadRequestData.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:rl1", "Building").await;
    let uri = "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rl1";
    // {…} selection without join
    assert_eq!(
        get_status(&app, &format!("{uri}?pick=owner%7Bname%7D"), None).await,
        StatusCode::BAD_REQUEST
    );
    // join=@none is "Linked Entity retrieval not specified"
    assert_eq!(
        get_status(
            &app,
            &format!("{uri}?pick=owner%7Bname%7D&join=%40none"),
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    // depth 2 projection over joinLevel=1
    assert_eq!(
        get_status(
            &app,
            &format!("{uri}?pick=owner%7Bworks%7Bname%7D%7D&join=inline&joinLevel=1"),
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    // well-formed: depth 1 within joinLevel=1, plain member kept
    assert_eq!(
        get_status(
            &app,
            &format!("{uri}?pick=id,type,name,owner%7Bname%7D&join=inline&joinLevel=1"),
            None
        )
        .await,
        StatusCode::OK
    );
}

// ---- 5.7.2.4 Query Entities validation --------------------------------

#[tokio::test]
async fn query_requires_a_non_system_attribute_in_attrs_and_q() {
    // 5.7.2.4 b/c: the Attribute list / query only qualifies the
    // too-wide guard when it includes "at least one non-system
    // Attribute" — system names alone are still a too-wide 400.
    let app = app();
    assert_eq!(
        get_status(&app, "/ngsi-ld/v1/entities?attrs=createdAt", None).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_status(
            &app,
            "/ngsi-ld/v1/entities?q=modifiedAt%3E%222020-01-01T00:00:00Z%22",
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_ne!(
        get_status(&app, "/ngsi-ld/v1/entities?attrs=name", None).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn query_linked_entity_guards() {
    // 5.7.2.4: projection or filter conditions that use Linked Entity
    // paths require join, and their depth may not exceed joinLevel
    // ("too deep query"); csf must be syntactically valid.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:ql1", "Building").await;
    let base = "/ngsi-ld/v1/entities?type=Building";
    // {…} projection without join
    assert_eq!(
        get_status(&app, &format!("{base}&pick=owner%7Bname%7D"), None).await,
        StatusCode::BAD_REQUEST
    );
    // linked q term without join
    assert_eq!(
        get_status(
            &app,
            &format!("{base}&q=owner%7Bname%7D%3D%3D%22x%22"),
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    // linked q term two hops deep over joinLevel=1
    assert_eq!(
        get_status(
            &app,
            &format!("{base}&q=owner%7Bworks%7Bname%7D%7D%3D%3D%22x%22&join=inline&joinLevel=1"),
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    // invalid csf
    assert_eq!(
        get_status(&app, &format!("{base}&csf=%29%29bad%28%28"), None).await,
        StatusCode::BAD_REQUEST
    );
    // well-formed linked q within depth is accepted
    assert_eq!(
        get_status(
            &app,
            &format!("{base}&q=owner%7Bname%7D%3D%3D%22x%22&join=inline&joinLevel=1"),
            None
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn temporal_retrieve_rejects_linked_projection() {
    // 5.7.3.4: "If projection attributes are present and indicate the
    // use of Linked Entity retrieval, an error of type BadRequestData
    // shall be raised" — unconditional, temporal has no join.
    let app = app();
    assert_eq!(
        get_status(
            &app,
            "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Building:tl1?pick=owner%7Bname%7D",
            None
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn temporal_query_validation_edges() {
    // 5.7.4.4: non-system rule for attrs/q; Linked Entity projection or
    // filter is an unconditional 400; invalid id URI 400; csf syntax
    // 400; orderBy may only name "id" on the temporal query.
    let app = app();
    let w = "timerel=after&timeAt=2020-01-01T00:00:00Z";
    let cases: &[(&str, StatusCode, &str)] = &[
        (
            "attrs=createdAt",
            StatusCode::BAD_REQUEST,
            "system attrs alone are too wide",
        ),
        (
            "type=Building&pick=owner%7Bname%7D",
            StatusCode::BAD_REQUEST,
            "linked projection",
        ),
        (
            "type=Building&q=owner%7Bname%7D%3D%3D%22x%22",
            StatusCode::BAD_REQUEST,
            "linked filter",
        ),
        (
            "type=Building&id=not%20a%20uri",
            StatusCode::BAD_REQUEST,
            "invalid id URI",
        ),
        (
            "type=Building&csf=%29%29bad%28%28",
            StatusCode::BAD_REQUEST,
            "invalid csf",
        ),
        (
            "type=Building&orderBy=name",
            StatusCode::BAD_REQUEST,
            "orderBy other than id",
        ),
        (
            "type=Building&orderBy=id",
            StatusCode::OK,
            "orderBy id is legal",
        ),
    ];
    for (qs, want, why) in cases {
        let uri = format!("/ngsi-ld/v1/temporal/entities?{w}&{qs}");
        assert_eq!(get_status(&app, &uri, None).await, *want, "{why}");
    }
}

#[tokio::test]
async fn temporal_query_values_filter_respects_the_window() {
    // 5.7.4.4: "the values filter query shall be checked against all
    // the Attribute instances resulting from the initial filtering
    // performed by the temporal query" — an instance OUTSIDE the
    // interval must not satisfy q.
    let app = app();
    let body = serde_json::json!({
        "id": "urn:ngsi-ld:Vehicle:tw1", "type": "Vehicle",
        "speed": [
            {"type": "Property", "value": 5,
             "observedAt": "2026-01-01T00:00:00Z"},
            {"type": "Property", "value": 99,
             "observedAt": "2026-03-01T00:00:00Z"}
        ]
    })
    .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert!(resp.status().is_success(), "seed: {}", resp.status());
    // window covers ONLY the value-5 instance; q asks for 99
    let uri = "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=between&timeAt=2025-12-01T00:00:00Z&endTimeAt=2026-02-01T00:00:00Z&q=speed%3D%3D99";
    let resp = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    let status = resp.status();
    let docs = body_json(resp).await;
    assert!(status.is_success(), "query: {status}");
    assert_eq!(
        docs.as_array().map(Vec::len),
        Some(0),
        "out-of-window instance must not satisfy q: {docs}"
    );
}

// ---- 5.9.2.4 Create Context Source Registration -----------------------

async fn post_json(app: &Router, uri: &str, body: serde_json::Value) -> StatusCode {
    let body = body.to_string();
    app.clone()
        .oneshot(
            Request::post(uri)
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp")
        .status()
}

#[tokio::test]
async fn csr_create_mode_restrictions() {
    // 5.9.2.4: auxiliary registrations may only offer retrieveOps /
    // retrieveEntity / queryEntity (or a combination); an exclusive
    // registration conflicts with an existing entity carrying any of
    // its Attributes; a redirect registration conflicts with any
    // existing matching entity.
    let app = app();
    let reg = |id: &str, mode: &str, ops: serde_json::Value, ent: serde_json::Value| {
        serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:{id}"),
            "type": "ContextSourceRegistration",
            "mode": mode,
            "operations": ops,
            "information": [ent],
            "endpoint": "http://source.example.com"
        })
    };
    let uri = "/ngsi-ld/v1/csourceRegistrations";

    // auxiliary with a write op → 400; retrieve/query combination → 201
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "aux-bad",
                "auxiliary",
                serde_json::json!(["updateEntity"]),
                serde_json::json!({"entities": [{"type": "Building"}]})
            )
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "aux-ok",
                "auxiliary",
                serde_json::json!(["retrieveOps", "queryEntity"]),
                serde_json::json!({"entities": [{"type": "Building"}]})
            )
        )
        .await,
        StatusCode::CREATED
    );

    // exclusive vs existing entity carrying the registered attribute
    create(&app, "urn:ngsi-ld:Building:csr1", "Building").await; // has "name"
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "exc-conflict",
                "exclusive",
                serde_json::json!(["retrieveOps"]),
                serde_json::json!({
                    "entities": [{"type": "Building", "id": "urn:ngsi-ld:Building:csr1"}],
                    "propertyNames": ["name"]
                })
            )
        )
        .await,
        StatusCode::CONFLICT
    );
    // exclusive over an attr the entity does NOT have is fine
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "exc-ok",
                "exclusive",
                serde_json::json!(["retrieveOps"]),
                serde_json::json!({
                    "entities": [{"type": "Building", "id": "urn:ngsi-ld:Building:csr1"}],
                    "propertyNames": ["capacity"]
                })
            )
        )
        .await,
        StatusCode::CREATED
    );

    // redirect vs any existing matching entity
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "red-conflict",
                "redirect",
                serde_json::json!(["retrieveOps"]),
                serde_json::json!({
                    "entities": [{"type": "Building", "id": "urn:ngsi-ld:Building:csr1"}]
                })
            )
        )
        .await,
        StatusCode::CONFLICT
    );
    // redirect for an untouched id is fine
    assert_eq!(
        post_json(
            &app,
            uri,
            reg(
                "red-ok",
                "redirect",
                serde_json::json!(["retrieveOps"]),
                serde_json::json!({
                    "entities": [{"type": "Building", "id": "urn:ngsi-ld:Building:other"}]
                })
            )
        )
        .await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn csr_expires_at_deletes_the_registration() {
    // 5.9.2.4: "If expiresAt is a date and time in the future,
    // implementations shall delete the Registration when this point in
    // time is reached" — after expiry the registration is gone from
    // retrieve and query.
    let app = app();
    let soon = (chrono::Utc::now() + chrono::Duration::milliseconds(1100))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:expiring",
        "type": "ContextSourceRegistration",
        "expiresAt": soon,
        "information": [{"entities": [{"type": "Building"}]}],
        "endpoint": "http://source.example.com"
    });
    assert_eq!(
        post_json(&app, "/ngsi-ld/v1/csourceRegistrations", reg).await,
        StatusCode::CREATED
    );
    let uri = "/ngsi-ld/v1/csourceRegistrations/urn:ngsi-ld:ContextSourceRegistration:expiring";
    assert_eq!(get_status(&app, uri, None).await, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
    assert_eq!(
        get_status(&app, uri, None).await,
        StatusCode::NOT_FOUND,
        "expired registration must be gone"
    );
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/csourceRegistrations?type=Building")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let docs = body_json(resp).await;
    assert!(
        !docs.to_string().contains("expiring"),
        "expired registration must not be listed: {docs}"
    );
}

/// 5.9.2.4 with RFC 9110 §9.2.1: the reads that hide an expired
/// registration write nothing, and `sweep_expired_docs` is what frees
/// the row — for every kind that carries its own expiry, over every
/// tenant the driver will name.
#[tokio::test]
async fn expired_documents_are_freed_by_the_sweep_and_by_no_read() {
    use antares_store::Kind;
    let st = AppState::new("antares-sweep".into());
    let app = router(st.clone());
    let t = antares_model::TenantId::default();
    let id = "urn:ngsi-ld:ContextSourceRegistration:swept";
    st.store
        .create(
            &t,
            Kind::Registration,
            id,
            serde_json::json!({
                "id": id,
                "type": "ContextSourceRegistration",
                "expiresAt": "2000-01-01T00:00:00.000Z",
                "information": [{"entities": [{"type": "Building"}]}],
                "endpoint": "http://source.example.com",
            }),
        )
        .await
        .expect("seed");

    let uri = format!("/ngsi-ld/v1/csourceRegistrations/{id}");
    assert_eq!(
        get_status(&app, &uri, None).await,
        StatusCode::NOT_FOUND,
        "5.9.2.4: an expired registration is gone to a reader"
    );
    assert!(
        st.store
            .get(&t, Kind::Registration, id)
            .await
            .expect("store")
            .is_some(),
        "the read deleted the row: a GET must be safe (RFC 9110 9.2.1)"
    );

    assert_eq!(sweep_expired_docs(&st).await, 1, "the sweep reaps it");
    assert!(
        st.store
            .get(&t, Kind::Registration, id)
            .await
            .expect("store")
            .is_none(),
        "the sweep left the row behind"
    );
    assert_eq!(sweep_expired_docs(&st).await, 0, "the sweep is idempotent");
}

/// ADR-0026 rule 7a (T-4543): a socket Subscription whose lease passed
/// belongs to a connection no process holds, and the sweep deletes it;
/// a live lease, and a client's own expired Subscription, stay.
#[tokio::test]
async fn the_sweep_deletes_a_socket_subscription_whose_lease_passed() {
    use antares_store::Kind;
    let st = AppState::new("antares-sweep".into());
    let t = antares_model::TenantId::default();
    let sub = |id: &str, uri: &str, expires: &str| {
        serde_json::json!({
            "id": id,
            "type": "Subscription",
            "entities": [{"type": "Car"}],
            "expiresAt": expires,
            "notification": {"endpoint": {"uri": uri}},
        })
    };
    let seeds = [
        (
            "urn:ngsi-ld:Subscription:ws-dead",
            "urn:antares:ws:gone",
            "2000-01-01T00:00:00Z",
        ),
        (
            "urn:ngsi-ld:Subscription:ws-live",
            "urn:antares:ws:here",
            "2999-01-01T00:00:00Z",
        ),
        (
            "urn:ngsi-ld:Subscription:http",
            "http://n.example.com/n",
            "2000-01-01T00:00:00Z",
        ),
    ];
    for (id, uri, expires) in seeds {
        st.store
            .create(&t, Kind::Subscription, id, sub(id, uri, expires))
            .await
            .expect("seed");
    }
    assert_eq!(
        sweep_expired_docs(&st).await,
        1,
        "only the lapsed lease goes"
    );
    for (id, _, _) in seeds {
        let kept = st
            .store
            .get(&t, Kind::Subscription, id)
            .await
            .expect("store");
        assert_eq!(kept.is_some(), id != seeds[0].0, "{id}");
    }
    assert_eq!(sweep_expired_docs(&st).await, 0, "the sweep is idempotent");
}

#[tokio::test]
async fn csr_update_reapplies_mode_restrictions() {
    // 5.9.3.4: the exclusive/redirect entity-conflict rules and the
    // auxiliary ops restriction apply to the post-merge document.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:csru1", "Building").await;
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:upd1",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "information": [{"entities": [{"type": "Building", "id": "urn:ngsi-ld:Building:csru1"}]}],
        "endpoint": "http://source.example.com"
    });
    assert_eq!(
        post_json(&app, "/ngsi-ld/v1/csourceRegistrations", reg).await,
        StatusCode::CREATED
    );
    // flipping the mode to redirect now collides with the entity
    let patch = serde_json::json!({"mode": "redirect"}).to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::patch(
                "/ngsi-ld/v1/csourceRegistrations/urn:ngsi-ld:ContextSourceRegistration:upd1",
            )
            .header("Content-Type", "application/json")
            .header("Content-Length", patch.len())
            .body(Body::from(patch))
            .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    // flipping to auxiliary while operations carry writes is 400
    let patch =
        serde_json::json!({"mode": "auxiliary", "operations": ["updateEntity"]}).to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::patch(
                "/ngsi-ld/v1/csourceRegistrations/urn:ngsi-ld:ContextSourceRegistration:upd1",
            )
            .header("Content-Type", "application/json")
            .header("Content-Length", patch.len())
            .body(Body::from(patch))
            .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn csr_query_csf_scope_and_geo_filters() {
    // 5.10.2.4: the context source filter (csf) matches the
    // registration's own Context Source Properties; the Scope query
    // matches its scope; the geoquery matches its location.
    let app = app();
    let reg_a = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:disc-a",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Building"}]}],
        "endpoint": "http://a.example.com",
        "scope": "/Madrid/Centro",
        "location": {"type": "Point", "coordinates": [8.68, 49.41]}
    });
    let reg_b = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:disc-b",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Building"}]}],
        "endpoint": "http://b.example.com",
        "scope": "/Berlin"
    });
    for r in [reg_a, reg_b] {
        assert_eq!(
            post_json(&app, "/ngsi-ld/v1/csourceRegistrations", r).await,
            StatusCode::CREATED
        );
    }
    let list = |q: String| {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::get(format!(
                        "/ngsi-ld/v1/csourceRegistrations?type=Building&{q}"
                    ))
                    .body(Body::empty())
                    .expect("req"),
                )
                .await
                .expect("resp");
            assert_eq!(resp.status(), StatusCode::OK, "query {q}");
            body_json(resp).await.to_string()
        }
    };
    // csf on a Context Source Property
    let got = list("csf=endpoint%3D%3D%22http%3A%2F%2Fa.example.com%22".into()).await;
    assert!(
        got.contains("disc-a") && !got.contains("disc-b"),
        "csf: {got}"
    );
    // Scope query against the registration scope
    let got = list("scopeQ=%2FMadrid%2F%23".into()).await;
    assert!(
        got.contains("disc-a") && !got.contains("disc-b"),
        "scopeQ: {got}"
    );
    // geoquery against the registration location
    let got = list(
        "georel=near%3BmaxDistance%3D%3D2000&geometry=Point&coordinates=%5B8.68%2C49.41%5D".into(),
    )
    .await;
    assert!(
        got.contains("disc-a") && !got.contains("disc-b"),
        "geo: {got}"
    );
}

// ---- Table 6.4.3.2-1: type=* ------------------------------------------

#[tokio::test]
async fn type_wildcard_selects_every_type() {
    // "\"*\" is also allowed as a value and local is implicitly set to
    // true". Expanding "*" as a term produced an IRI nothing matched, so
    // the query returned 200 with an empty array.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:star1", "Building").await;
    create(&app, "urn:ngsi-ld:Vehicle:star2", "Vehicle").await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=%2A")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let docs = body_json(resp).await;
    let ids: Vec<&str> = docs
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    assert!(ids.contains(&"urn:ngsi-ld:Building:star1"), "got {ids:?}");
    assert!(ids.contains(&"urn:ngsi-ld:Vehicle:star2"), "got {ids:?}");
}

#[tokio::test]
async fn type_wildcard_conflicts_with_explicit_local_false() {
    // "…and shall not be explicitly set to false" (Table 6.4.3.2-1)
    let app = app();
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=%2A&local=false")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---- 5.7.2.4 validation bullets ---------------------------------------

#[tokio::test]
async fn ordering_is_rejected_only_for_distributed_execution() {
    // 5.7.2.4: "If the ordering parameter is present and the execution of
    // the operation is not limited to the local scope … BadRequestData",
    // with 4.23.1 "Sort ordering is never applied to distributed
    // operations". The subject is the EXECUTION — a query no registration
    // matches runs locally whether or not the client passed local=true.
    // Reading it as "local=true is mandatory" fails ETSI 019_19, which
    // orders without it.
    let app = app();

    // no registrations → local execution → ordering is fine
    for q in [
        "/ngsi-ld/v1/entities?type=Building&orderBy=name",
        "/ngsi-ld/v1/entities?type=Building&orderBy=name&local=true",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::get(q).body(Body::empty()).expect("req"))
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::OK, "{q}");
    }

    // a matching registration makes it a distributed operation
    let csr = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:ord1",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Building"}]}],
        "endpoint": "http://peer.invalid:9090"
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", (csr.to_string()).len())
                .body(Body::from(csr.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&orderBy=name")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // …and local=true brings it back into scope
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&orderBy=name&local=true")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn geometry_property_requires_geojson_accept() {
    // 5.7.2.4: "If geometryProperty parameter is present and the Accept
    // Header is not set to \"application/geo+json\" … BadRequestData"
    let app = app();
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&geometryProperty=location")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&geometryProperty=location&local=true")
                .header("Accept", "application/geo+json")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---- 6.3.4: Content-Length precondition -------------------------------

#[tokio::test]
async fn missing_content_length_is_a_bare_411() {
    // 6.3.4: "For HTTP POST, PATCH and PUT HTTP requests implementations
    // shall check … Content-Length header shall include the length of the
    // request payload body", and its absence "shall result in just a 411
    // HTTP status code (without any payload body)". No exemption is given
    // for chunked transfer.
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:Building:cl1", "type": "Building"});

    for (method, uri) in [
        ("POST", "/ngsi-ld/v1/entities"),
        (
            "PATCH",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:cl1/attrs",
        ),
        ("PUT", "/ngsi-ld/v1/entities/urn:ngsi-ld:Building:cl1"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("Content-Type", "application/json")
                    .body(Body::from(entity.to_string()))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::LENGTH_REQUIRED, "{method} {uri}");
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        assert!(bytes.is_empty(), "411 carries no payload body");
    }

    // GET/DELETE are outside the clause's scope
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---- Table 5.2.40-1: Context Source Identity --------------------------

#[tokio::test]
async fn source_identity_carries_the_mandated_members() {
    // contextSourceAlias / contextSourceUptime / contextSourceTimeAt are
    // all cardinality 1. The old payload used hostAlias/uptime, which are
    // not core-context terms at all.
    let resp = app()
        .oneshot(
            Request::get("/ngsi-ld/v1/info/sourceIdentity")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(doc["type"], "ContextSourceIdentity");
    assert!(doc["id"].as_str().is_some_and(|s| s.starts_with("urn:")));
    assert_eq!(doc["contextSourceAlias"], "antares-test");
    assert!(
        doc["contextSourceUptime"]
            .as_str()
            .is_some_and(|s| s.starts_with("PT") && s.ends_with('S')),
        "uptime must be an ISO 8601 duration, got {:?}",
        doc["contextSourceUptime"]
    );
    assert!(
        doc["contextSourceTimeAt"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z')),
        "timeAt must be a 4.6.3 DateTime, got {:?}",
        doc["contextSourceTimeAt"]
    );
    assert!(
        doc.get("hostAlias").is_none(),
        "hostAlias is not a spec member"
    );
    assert!(doc.get("uptime").is_none(), "uptime is not a spec member");
}

// ---- 6.3.4: Accept precedence -----------------------------------------

#[tokio::test]
async fn accept_precedence_follows_the_spec_list_not_header_order() {
    // "The order of the list above is significant … the first one of the
    // list shall be selected, unless amended by … a q parameter."
    // json > ld+json > geo+json, regardless of how the client orders them.
    let app = app();
    create(&app, "urn:ngsi-ld:Building:acc1", "Building").await;

    for (accept, want) in [
        ("application/ld+json, application/json", "application/json"),
        ("application/geo+json, application/json", "application/json"),
        ("application/ld+json", "application/ld+json"),
        // an explicit q still wins over list order
        (
            "application/json;q=0.1, application/ld+json;q=0.9",
            "application/ld+json",
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:acc1")
                    .header("Accept", accept)
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        let ct = resp
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(
            ct.starts_with(want),
            "Accept: {accept} → Content-Type {ct}, expected {want}"
        );
    }
}

/// 6.3.10 p.275: "At least, the type Link Target Attribute shall be
/// included ... and its value shall be exactly equal to the media type
/// resulting from the original request" — previously emitted only for
/// ld+json, never for plain application/json.
#[tokio::test]
async fn pagination_links_carry_the_type_attribute_for_plain_json() {
    let app = app();
    for i in 0..3 {
        let entity = serde_json::json!({
            "id": format!("urn:ngsi-ld:Building:pg{i}"),
            "type": "Building"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::post("/ngsi-ld/v1/entities")
                    .header("Content-Type", "application/json")
                    .header("Content-Length", (entity.to_string()).len())
                    .body(Body::from(entity.to_string()))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&limit=1&offset=1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let links: Vec<String> = resp
        .headers()
        .get_all("Link")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(", "))
        .filter(|l| l.contains("rel=\"next\"") || l.contains("rel=\"prev\""))
        .map(str::to_owned)
        .collect();
    assert_eq!(links.len(), 2, "expected next+prev, got {links:?}");
    for l in &links {
        assert!(
            l.contains(";type=\"application/json\""),
            "pagination link lacks the mandatory type attribute: {l}"
        );
    }
}

/// 4.5.9 p.63/65: in the simplified temporal representation
/// a ListProperty pairs a BARE ordered array with its timestamp under
/// `valueLists` (EXAMPLE 3), and a ListRelationship the same under
/// `objectLists` — not a {"valueList"/"objectList"} wrapper object.
#[tokio::test]
async fn temporal_values_list_types_use_bare_arrays() {
    let app = app();
    let doc = serde_json::json!({
        "id": "urn:ngsi-ld:Meeting:tv1",
        "type": "Meeting",
        "period": [
            {"type": "ListProperty", "valueList": ["First", "Second"],
             "observedAt": "2023-01-01T00:00:00Z"},
            {"type": "ListProperty", "valueList": ["1st", "2nd"],
             "observedAt": "2023-01-02T00:00:00Z"}
        ],
        "membersPresent": [
            {"type": "ListRelationship",
             "objectList": ["urn:ngsi-ld:Person:Alice", "urn:ngsi-ld:Person:Bob"],
             "observedAt": "2023-01-01T00:00:00Z"}
        ]
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (doc.to_string()).len())
                .body(Body::from(doc.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "temporal upsert failed: {}",
        resp.status()
    );
    let resp = app
        .clone()
        .oneshot(
            Request::get(
                "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Meeting:tv1?format=temporalValues",
            )
            .body(Body::empty())
            .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let vl = &body["period"]["valueLists"];
    assert!(vl.is_array(), "period.valueLists missing: {body}");
    assert_eq!(
        vl[0][0],
        serde_json::json!(["First", "Second"]),
        "first pair element must be the BARE ordered array: {vl}"
    );
    assert_eq!(vl[0][1], "2023-01-01T00:00:00Z");
    let ol = &body["membersPresent"]["objectLists"];
    assert_eq!(
        ol[0][0],
        serde_json::json!(["urn:ngsi-ld:Person:Alice", "urn:ngsi-ld:Person:Bob"]),
        "objectLists pairs carry the bare URI array: {ol}"
    );
}

/// 4.5.19.1 Table -1 + 5.7.4.4 p.211: string-valued Properties
/// aggregate min/max lexicographically ("first/last value in
/// lexicographical order"); a method the datatype is not eligible for
/// ("sum" on strings is N/A) raises InvalidRequest; and numeric folds
/// never leak f64::INFINITY (serialized as null) into the payload.
#[tokio::test]
async fn aggregation_dispatches_on_datatype_and_rejects_ineligible() {
    let app = app();
    let doc = serde_json::json!({
        "id": "urn:ngsi-ld:Building:agg1",
        "type": "Building",
        "operator": [
            {"type": "Property", "value": "alpha", "observedAt": "2023-01-01T00:00:00Z"},
            {"type": "Property", "value": "zulu",  "observedAt": "2023-01-02T00:00:00Z"},
            {"type": "Property", "value": "mike",  "observedAt": "2023-01-03T00:00:00Z"}
        ]
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (doc.to_string()).len())
                .body(Body::from(doc.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let base = "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Building:agg1\
                ?options=aggregatedValues&timerel=after&timeAt=2022-01-01T00:00:00Z";
    // eligible: lexicographic min/max on strings
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("{base}&aggrMethods=min,max"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["operator"]["min"][0][0], "alpha", "{body}");
    assert_eq!(body["operator"]["max"][0][0], "zulu", "{body}");
    // ineligible: sum over strings is N/A → InvalidRequest (400)
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("{base}&aggrMethods=sum"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(
        body["type"], "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest",
        "{body}"
    );
}
