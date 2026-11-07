// SPDX-License-Identifier: EUPL-1.2
//! `Ops::view` and `ViewOps::run` (ADR-0023 rules 4 and 10): a loaded view
//! by id and nothing else, its allowlist, its one Tenant, and the narrowed
//! handle following a reload; the names a caller writes beside a standard's
//! own query read in the view's @context (T-3758).

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// `/x/probe/{view}/types` lists the types through the view;
/// `/x/probe/{view}/entities` queries the Cars through it.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/probe"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        let run = |ops: Ops, op: OperationId, path: &'static str| {
            move |Path(id): Path<String>, caller: Caller| async move {
                let Some(view) = ops.view(&id) else {
                    return antares_ops::Refusal::NotFound.ngsi_ld();
                };
                match view
                    .run(op, &caller, OpsRequest::new(Method::GET, path))
                    .await
                {
                    Ok(r) => r,
                    Err(refusal) => refusal.ngsi_ld(),
                }
            }
        };
        let mapped = ops.clone();
        let reader = ops.clone();
        let named = ops.clone();
        Router::new()
            .route(
                "/{view}/read",
                get(move |Path(id): Path<String>, caller: Caller| {
                    read_whole(reader.clone(), id, caller)
                }),
            )
            .route(
                "/{view}/names",
                get(move |Path(id): Path<String>, RawQuery(query): RawQuery| {
                    let ops = named.clone();
                    async move {
                        let Some(view) = ops.view(&id) else {
                            return antares_ops::Refusal::NotFound.ngsi_ld();
                        };
                        // the raw (still percent-encoded) `q=` and `names=`, as a surface reads them
                        let query = query.unwrap_or_default();
                        let raw = |key: &str| {
                            query
                                .split('&')
                                .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
                                .unwrap_or_default()
                                .to_owned()
                        };
                        let mut minted: Vec<String> = Vec::new();
                        let q = view.rename_q(&raw("q"), &mut |iri| {
                            minted.push(iri.to_owned());
                            format!("t{}", minted.len())
                        });
                        let expanded = view.expand_names(&raw("names"));
                        axum::Json(json!({
                            "speed": view.expand_name("speed"),
                            "q": q.map_err(|r| r.ngsi_ld().status().as_u16()),
                            "minted": minted,
                            "names": expanded.map_err(|r| r.ngsi_ld().status().as_u16()),
                        }))
                        .into_response()
                    }
                }),
            )
            .route(
                "/{view}/mapping",
                get(move |Path(id): Path<String>, caller: Caller| {
                    let ops = mapped.clone();
                    async move {
                        let view = ops.mapped_view(&id);
                        // an operation the view does not allow, mapped or not
                        let types = match &view {
                            Some(v) => {
                                let req = OpsRequest::new(Method::GET, "/ngsi-ld/v1/types");
                                let run = v.run(OperationId::RetrieveEntityTypes, &caller, req);
                                Some(run.await.map_or_else(|r| r.ngsi_ld().status(), |a| a.status()).as_u16())
                            }
                            None => None,
                        };
                        axum::Json(json!({
                            "view": ops.view(&id).is_some(),
                            "mapped": view.as_ref().and_then(|v| v.mapping().map(|m| m.source_class().to_owned())),
                            "names": view.as_ref().and_then(antares_ops::ViewOps::target_names),
                            "types": types,
                            // the ceiling a surface writing a file itself stays under (T-4465)
                            "maxAnswerBytes": ops.max_answer_bytes(),
                        }))
                    }
                }),
            )
            .route(
                "/{view}/types",
                get(run(
                    ops.clone(),
                    OperationId::RetrieveEntityTypes,
                    "/ngsi-ld/v1/types",
                )),
            )
            .route(
                "/{view}/entities",
                get(run(
                    ops,
                    OperationId::QueryEntities,
                    "/ngsi-ld/v1/entities?type=Car",
                )),
            )
    }
}

/// The view's Cars read whole through [`antares_ops::ViewOps::read_answer`] (T-3773).
async fn read_whole(ops: Ops, id: String, caller: Caller) -> Response {
    let Some(view) = ops.view(&id) else {
        return antares_ops::Refusal::NotFound.ngsi_ld();
    };
    let req = OpsRequest::new(Method::GET, "/ngsi-ld/v1/entities?type=Car");
    let read = match view.run(OperationId::QueryEntities, &caller, req).await {
        Ok(answer) => view.read_answer(answer.into_body(), "test").await,
        Err(refusal) => Err(refusal),
    };
    match read {
        Ok(bytes) => bytes.into_bytes().into_response(),
        Err(refusal) => refusal.ngsi_ld(),
    }
}

struct Answer {
    status: StatusCode,
    tenant: Option<HeaderValue>,
    json: Value,
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenants: &[HeaderValue],
    body: Option<&Value>,
) -> Answer {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
    for t in tenants {
        req = req.header("NGSILD-Tenant", t.clone());
    }
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let tenant = resp.headers().get("ngsild-tenant").cloned();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        tenant,
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn view(attributes: &[&str], operations: &[&str]) -> Value {
    json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": attributes}],
        "operations": operations
    })
}

async fn broker() -> Result<(TestBroker, Router), String> {
    broker_on(TestBroker::new().await?).await
}

async fn broker_on(b: TestBroker) -> Result<(TestBroker, Router), String> {
    let app = b.serve(Probe)?;
    for (tenant, id) in [
        ("city", "urn:ngsi-ld:Car:1"),
        ("other", "urn:ngsi-ld:Car:9"),
    ] {
        let car = json!({"id": id, "type": "Car",
            "speed": {"type": "Property", "value": 1}, "colour": {"type": "Property", "value": "red"}});
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            &[HeaderValue::from_static(tenant)],
            Some(&car),
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED);
    }
    let a = send(
        &app,
        Method::PUT,
        "/q/views/cars",
        &[],
        Some(&view(
            &["speed"],
            &["retrieve_entity_types", "query_entities"],
        )),
    )
    .await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.json);
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_runs_in_its_one_tenant() {
    let (_b, app) = broker().await.expect("broker");
    let city = HeaderValue::from_static("city");
    let none = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert_eq!(none.status, StatusCode::OK, "{}", none.json);
    assert_eq!(
        none.json.pointer("/0/id"),
        Some(&json!("urn:ngsi-ld:Car:1")),
        "{}",
        none.json
    );
    assert_eq!(
        none.tenant, None,
        "the pinned tenant is named to a caller who never sent it"
    );
    let same = send(
        &app,
        Method::GET,
        "/x/probe/cars/entities",
        std::slice::from_ref(&city),
        None,
    )
    .await;
    assert_eq!(same.status, StatusCode::OK);
    assert_eq!(same.tenant, Some(city.clone()));
    for tenants in [
        vec![HeaderValue::from_static("other")],
        vec![HeaderValue::from_static("not a tenant")],
        vec![HeaderValue::from_bytes(b"\xffcity").expect("opaque bytes")],
        vec![city.clone(), city],
    ] {
        let a = send(&app, Method::GET, "/x/probe/cars/entities", &tenants, None).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{tenants:?}: {}", a.json);
        assert_eq!(
            a.json.get("detail"),
            Some(&json!("a view serves one tenant"))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_loaded_view_and_its_allowed_operations_run() {
    let (b, app) = broker().await.expect("broker");
    let a = send(&app, Method::GET, "/x/probe/nosuch/types", &[], None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    // a view that allows types but not the query
    let a = send(
        &app,
        Method::PUT,
        "/q/views/cars",
        &[],
        Some(&view(&["speed"], &["retrieve_entity_types"])),
    )
    .await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.json);
    let a = send(&app, Method::GET, "/x/probe/cars/types", &[], None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
    let a = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED, "{}", a.json);
    // a view that failed validation is no view
    let mut broken = view(&["speed"], &["query_entities"]);
    broken["operations"] = json!(["drop_tables"]);
    b.store_view_doc("broken", broken).await.expect("stored");
    let a = send(&app, Method::GET, "/x/probe/broken/types", &[], None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
}

/// The narrowed handle is kept per view and follows a reload: a changed
/// view serves its new narrowing, a removed one nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_handle_follows_the_view_table() {
    let (_b, app) = broker().await.expect("broker");
    let shows = |a: &Answer, attr: &str| a.json.pointer(&format!("/0/{attr}")).is_some();
    let a = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert!(shows(&a, "speed") && !shows(&a, "colour"), "{}", a.json);
    let again = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert_eq!(again.json, a.json, "served from the kept handle");

    let changed = view(&["colour"], &["retrieve_entity_types", "query_entities"]);
    let put = send(&app, Method::PUT, "/q/views/cars", &[], Some(&changed)).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT);
    let a = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert!(
        shows(&a, "colour") && !shows(&a, "speed"),
        "the old narrowing is still served: {}",
        a.json
    );

    let mut other = changed.clone();
    other["id"] = json!("trucks");
    let put = send(&app, Method::PUT, "/q/views/trucks", &[], Some(&other)).await;
    assert_eq!(put.status, StatusCode::CREATED);
    let del = send(&app, Method::DELETE, "/q/views/cars", &[], None).await;
    assert_eq!(del.status, StatusCode::NO_CONTENT);
    let a = send(&app, Method::GET, "/x/probe/cars/entities", &[], None).await;
    assert_eq!(
        a.status,
        StatusCode::NOT_FOUND,
        "a removed view is served: {}",
        a.json
    );
    let a = send(&app, Method::GET, "/x/probe/trucks/entities", &[], None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.json);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_of_another_model_reaches_only_the_surface_that_asks_for_its_mapping() {
    let (_b, app) = broker().await.expect("broker");
    let mapped = json!({
        "id": "mapped", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities"],
        "mapping": {"version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
                    "slots": [{"target": "velocity", "source": "speed", "kind": "rename"}]}
    });
    let a = send(&app, Method::PUT, "/q/views/mapped", &[], Some(&mapped)).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.json);
    let a = send(&app, Method::GET, "/x/probe/mapped/entities", &[], None).await;
    assert_eq!(
        a.status,
        StatusCode::NOT_FOUND,
        "Ops::view: an unknown view to every other surface"
    );
    let a = send(&app, Method::GET, "/x/probe/mapped/mapping", &[], None).await;
    assert_eq!(
        a.json,
        json!({"view": false, "mapped": "https://uri.etsi.org/ngsi-ld/default-context/Car",
               "names": [["Vehicle"], ["velocity"]], "types": 405,
               "maxAnswerBytes": antares_ops::bounds::MAX_ANSWER_BYTES})
    );
    let a = send(&app, Method::GET, "/x/probe/cars/mapping", &[], None).await;
    assert_eq!(
        a.json["names"],
        Value::Null,
        "a view of the store's own model: {}",
        a.json
    );
    assert_eq!(a.json["mapped"], Value::Null);
}

/// T-3817: a mapped view names the slots it can produce from what its rules
/// show of the source class: none shown names the constants alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_mapped_view_names_the_slots_its_rules_can_produce() {
    let (_b, app) = broker().await.expect("broker");
    let slots = json!([
        {"target": "velocity", "source": "speed", "kind": "rename"},
        {"target": "hidden", "source": "secret", "kind": "rename"},
        {"target": "maker", "kind": "constant", "value": "acme"}
    ]);
    for (id, rules, names) in [
        (
            "narrow",
            json!([{"types": ["Car"], "attributes": ["speed"]}]),
            json!(["maker", "velocity"]),
        ),
        (
            "bare",
            json!([{"types": ["Car"], "attributes": []}]),
            json!(["maker"]),
        ),
        (
            "both",
            json!([{"types": ["Car"], "attributes": ["speed", "secret"]}]),
            json!(["hidden", "maker", "velocity"]),
        ),
    ] {
        let view = json!({
            "id": id, "tenant": "city", "selector": ["Car"],
            "rules": rules,
            "operations": ["query_entities"],
            "mapping": {"version": 2, "sourceClass": "Car", "targetClass": "Vehicle", "slots": slots}
        });
        let a = send(
            &app,
            Method::PUT,
            &format!("/q/views/{id}"),
            &[],
            Some(&view),
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.json);
        let a = send(
            &app,
            Method::GET,
            &format!("/x/probe/{id}/mapping"),
            &[],
            None,
        )
        .await;
        assert_eq!(a.json["names"], json!([["Vehicle"], names]), "{id}");
    }
}

/// T-3773: a view's answer is read up to the broker's buffer bound; past it
/// the surface gets `Refusal::Limit`, 413, never the whole body.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_answer_past_the_buffer_bound_is_413() {
    let (_b, app) = broker().await.expect("broker");
    let whole = send(&app, Method::GET, "/x/probe/cars/read", &[], None).await;
    assert_eq!(whole.status, StatusCode::OK, "{}", whole.json);
    assert_eq!(
        whole.json.pointer("/0/id"),
        Some(&json!("urn:ngsi-ld:Car:1"))
    );
    let small = TestBroker::with_max_answer_bytes(16).await.expect("broker");
    let (_b, app) = broker_on(small).await.expect("broker");
    let a = send(&app, Method::GET, "/x/probe/cars/read", &[], None).await;
    assert_eq!(a.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", a.json);
    assert_eq!(
        a.json.get("detail"),
        Some(&json!(antares_ops::bounds::ANSWER_TOO_LARGE))
    );
}

/// T-5895: a view's answer the broker's buffered-answer pool has no room
/// for is `Refusal::Overloaded`: 503 with Retry-After, no body.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_answer_past_the_answer_pool_is_503() {
    let small = TestBroker::with_answer_pool(16).await.expect("broker");
    let (_b, app) = broker_on(small).await.expect("broker");
    let a = send(&app, Method::GET, "/x/probe/cars/read", &[], None).await;
    assert_eq!(a.status, StatusCode::SERVICE_UNAVAILABLE, "{}", a.json);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_callers_names_read_in_the_views_context() {
    let (_b, app) = broker().await.expect("broker");
    let mut named = view(&["speed"], &["query_entities"]);
    named["@context"] = json!({"speed": "https://example.org/speed"});
    let a = send(&app, Method::PUT, "/q/views/cars", &[], Some(&named)).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.json);
    let a = send(
        &app,
        Method::GET,
        "/x/probe/cars/names?q=speed%3E1;colour.observedAt%3E2026-01-01T00:00:00Z&names=speed%2Ccolour",
        &[],
        None,
    )
    .await;
    assert_eq!(
        a.json,
        json!({
            "speed": "https://example.org/speed",
            "q": {"Ok": "t1>1;t2.observedAt>\"2026-01-01T00:00:00Z\""},
            "minted": ["https://example.org/speed", "https://uri.etsi.org/ngsi-ld/default-context/colour"],
            "names": {"Ok": ["https://example.org/speed", "https://uri.etsi.org/ngsi-ld/default-context/colour"]},
        }),
        "{}",
        a.json
    );
    let a = send(
        &app,
        Method::GET,
        "/x/probe/cars/names?q=%28speed&names=a,,b",
        &[],
        None,
    )
    .await;
    assert_eq!(
        (a.json.get("q"), a.json.get("names")),
        (Some(&json!({"Err": 400})), Some(&json!({"Err": 400}))),
        "{}",
        a.json
    );
    let a = send(&app, Method::GET, "/x/probe/nosuch/names", &[], None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
}
