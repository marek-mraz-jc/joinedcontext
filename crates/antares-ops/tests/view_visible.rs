// SPDX-License-Identifier: EUPL-1.2
//! `ViewOps::visible` (T-3511): what a caller may see through a view is the
//! view's rules composed with the engine's answer for the operation, asked
//! with the names it is about.

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
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

const CAR: &str = "https://uri.etsi.org/ngsi-ld/default-context/Car";
const SPEED: &str = "https://uri.etsi.org/ngsi-ld/default-context/speed";
const COLOUR: &str = "https://uri.etsi.org/ngsi-ld/default-context/colour";

/// `/x/visible/{view}/{op}` answers what `ViewOps::visible` shows of Car,
/// speed and colour for operation `op`, asked about Car and speed.
struct Shown;

impl Surface for Shown {
    fn name(&self) -> &'static str {
        "visible"
    }
    fn prefix(&self) -> &'static str {
        "/x/visible"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/{op}",
            get(
                move |Path((id, op)): Path<(String, String)>, caller: Caller| async move {
                    let Some(view) = ops.view(&id) else {
                        return axum::Json(json!("no view"));
                    };
                    let op = OperationId::from_id(&op).unwrap_or(OperationId::Notification);
                    let asked = view
                        .visible(&caller, op, &[CAR.to_owned()], &[SPEED.to_owned()])
                        .await;
                    axum::Json(match asked {
                        Ok(v) => json!([
                            v.shows_type("Car"),
                            v.shows_attribute(CAR, SPEED),
                            v.shows_attribute(CAR, COLOUR),
                        ]),
                        Err(refusal) => json!(refusal.status().as_u16()),
                    })
                },
            ),
        )
    }
}

/// Omits colour from 5.7.7 when it is asked about Car and speed, denies
/// 5.7.10, allows the rest.
struct Narrowing;

impl PolicyEngine for Narrowing {
    fn name(&self) -> &'static str {
        "narrowing"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let named = op.types == [CAR] && op.attrs == [SPEED];
        let d = match op.clause {
            "5.7.7" if named => Decision::Filter(Filter {
                omit: vec![COLOUR.into()],
                ..Filter::default()
            }),
            "5.7.10" => Decision::Deny("no".into()),
            _ => Decision::Allow,
        };
        Box::pin(std::future::ready(d))
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
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// A broker asking `engine` (none when `None`) with view `cars`: Cars show
/// speed and colour, through the type list and the type and attribute
/// information reads.
async fn broker(engine: Option<Arc<dyn PolicyEngine>>) -> Result<Router, String> {
    let mut b = TestBroker::new().await?;
    if let Some(engine) = engine {
        b = b.with_policy(engine);
    }
    let app = b.serve(Shown)?;
    let view = json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "colour"]}],
        "operations": [
            "retrieve_entity_types",
            "retrieve_entity_type_info",
            "retrieve_attribute_info"
        ]
    });
    let body = view.to_string();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/q/views/cars")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .map_err(|e| e.to_string())?;
    let Ok(resp) = app.clone().oneshot(req).await;
    if resp.status() == StatusCode::CREATED {
        Ok(app)
    } else {
        Err(format!("the view did not load: {}", resp.status()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_engine_the_view_alone_decides() {
    let app = broker(None).await.expect("a broker");
    assert_eq!(
        get_json(&app, "/x/visible/cars/retrieve_entity_type_info").await,
        json!([true, true, true])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_narrows_or_refuses_what_it_is_asked_about() {
    let app = broker(Some(Arc::new(Narrowing))).await.expect("a broker");
    assert_eq!(
        get_json(&app, "/x/visible/cars/retrieve_entity_type_info").await,
        json!([true, true, false]),
        "view ∧ engine, the engine asked about Car and speed"
    );
    assert_eq!(
        get_json(&app, "/x/visible/cars/retrieve_attribute_info").await,
        json!(403)
    );
    assert_eq!(
        get_json(&app, "/x/visible/cars/retrieve_entity_types").await,
        json!([true, true, true]),
        "an engine that allows leaves the view's rules as they are"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_operation_the_view_leaves_out_is_not_allowed() {
    let app = broker(None).await.expect("a broker");
    for op in ["retrieve_entity_type_details", "notification"] {
        assert_eq!(
            get_json(&app, &format!("/x/visible/cars/{op}")).await,
            json!(405),
            "{op}"
        );
    }
}
