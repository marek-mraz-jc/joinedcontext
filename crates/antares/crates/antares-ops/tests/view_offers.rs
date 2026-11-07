// SPDX-License-Identifier: EUPL-1.2
//! `ViewOps::offered`, `ViewOps::shown_names`, `ViewOps::shown_by_type` and
//! `ViewOps::shows_every_name` (MCP D-3, D-4, `OData` 3.1, ADR-0023 rule 12):
//! what a
//! surface may list for a caller is the view's allowlist filtered by one
//! fail-closed engine probe, and the names it may offer are the view's, or
//! every name for a view of `*`.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// The operations every probe asks about, in this order.
const ASKED: [OperationId; 4] = [
    OperationId::RetrieveEntityTypes,
    OperationId::QueryEntities,
    OperationId::RetrieveEntity,
    OperationId::Notification,
];

/// `/x/offers/{view}/offered` answers `ViewOps::offered` for [`ASKED`],
/// `/x/offers/{view}/names` answers `ViewOps::shown_names`,
/// `/x/offers/{view}/by-type` `ViewOps::shown_by_type` and
/// `/x/offers/{view}/every` `ViewOps::shows_every_name`.
struct Offers;

impl Surface for Offers {
    fn name(&self) -> &'static str {
        "offers"
    }
    fn prefix(&self) -> &'static str {
        "/x/offers"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        let names = ops.clone();
        let every = ops.clone();
        let by_type = ops.clone();
        Router::new()
            .route(
                "/{view}/every",
                get(move |Path(id): Path<String>| async move {
                    axum::Json(json!(every.view(&id).map(|v| v.shows_every_name())))
                }),
            )
            .route(
                "/{view}/by-type",
                get(move |Path(id): Path<String>| async move {
                    axum::Json(json!(by_type.view(&id).map(|v| v.shown_by_type())))
                }),
            )
            .route(
                "/{view}/offered",
                get(move |Path(id): Path<String>, caller: Caller| async move {
                    match ops.view(&id) {
                        Some(v) => axum::Json(json!(v.offered(&caller, &ASKED).await)),
                        None => axum::Json(Value::Null),
                    }
                }),
            )
            .route(
                "/{view}/names",
                get(move |Path(id): Path<String>| async move {
                    axum::Json(json!(names.view(&id).map(|v| v.shown_names())))
                }),
            )
    }
}

/// Denies clause 5.7.5 (Retrieve Entity Types), allows the rest.
struct NoTypes;

/// Panics on every decision.
struct Broken;

impl PolicyEngine for NoTypes {
    fn name(&self) -> &'static str {
        "no-types"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if op.clause == "5.7.5" {
            Decision::Deny("no".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

impl PolicyEngine for Broken {
    fn name(&self) -> &'static str {
        "broken"
    }
    #[expect(clippy::panic, reason = "the engine under test fails")]
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        panic!("broken engine")
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn get_json(app: &Router, uri: &str) -> Value {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// A broker asking `engine` (none when `None`) with view `cars` loaded:
/// Cars show `speed` and `https://example.org/odd`, and the view allows
/// listing types and querying, not retrieving one Entity.
async fn broker(engine: Option<Arc<dyn PolicyEngine>>) -> Result<(TestBroker, Router), String> {
    let mut b = TestBroker::new().await?;
    if let Some(engine) = engine {
        b = b.with_policy(engine);
    }
    let app = b.serve(Offers)?;
    let cars = json!({
        "id": "cars", "tenant": "city", "selector": ["Car", "Bike"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "https://example.org/odd", "speed"]}],
        "operations": ["retrieve_entity_types", "query_entities", "notification"]
    });
    let all = json!({
        "id": "all", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["query_entities"]
    });
    for (id, view) in [("cars", cars), ("all", all)] {
        let body = view.to_string();
        let req = Request::builder()
            .method(Method::PUT)
            .uri(format!("/q/views/{id}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .map_err(|e| e.to_string())?;
        let Ok(resp) = app.clone().oneshot(req).await;
        if resp.status() != StatusCode::CREATED {
            return Err(format!("view: {}", resp.status()));
        }
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_and_the_engine_decide_what_is_offered() -> Result<(), String> {
    let (_b, app) = broker(None).await?;
    assert_eq!(
        get_json(&app, "/x/offers/cars/offered").await,
        json!([true, true, false, false]),
        "no engine: the allowlist, and a row no request reaches is never offered"
    );
    let (_b, app) = broker(Some(Arc::new(NoTypes))).await?;
    assert_eq!(
        get_json(&app, "/x/offers/cars/offered").await,
        json!([false, true, false, false])
    );
    assert_eq!(
        get_json(&app, "/x/offers/nosuch/offered").await,
        Value::Null
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_that_fails_the_probe_offers_nothing() -> Result<(), String> {
    let (_b, app) = broker(Some(Arc::new(Broken))).await?;
    assert_eq!(
        get_json(&app, "/x/offers/cars/offered").await,
        json!([false, false, false, false])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_names_offered_are_the_ones_the_view_shows() -> Result<(), String> {
    let (_b, app) = broker(None).await?;
    assert_eq!(
        get_json(&app, "/x/offers/cars/names").await,
        json!([["Car"], ["https://example.org/odd", "speed"]]),
        "a type no rule shows is not offered; names once each, sorted"
    );
    assert_eq!(get_json(&app, "/x/offers/cars/every").await, json!(false));
    assert_eq!(get_json(&app, "/x/offers/all/every").await, json!(true));
    assert_eq!(
        get_json(&app, "/x/offers/all/names").await,
        json!([[], []]),
        "a view of `*` lists no name: a surface offers every one by its syntax"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn each_type_shows_the_union_of_its_rules_as_iris() -> Result<(), String> {
    let (_b, app) = broker(None).await?;
    let core = "https://uri.etsi.org/ngsi-ld/default-context/";
    assert_eq!(
        get_json(&app, "/x/offers/cars/by-type").await,
        json!({format!("{core}Car"): ["https://example.org/odd", format!("{core}speed")]}),
        "expanded, each name once; Bike has no rule and is absent"
    );
    assert_eq!(
        get_json(&app, "/x/offers/nosuch/by-type").await,
        Value::Null
    );
    Ok(())
}
