// SPDX-License-Identifier: EUPL-1.2
//! The view's guarantees as properties over generated Cars, views and engine
//! narrowings (surfaces-test-plan sections 4 and 5, ADR-0023 rules 1, 2 and
//! 11): composed with an engine a view never serves more than either side
//! alone; a caller's `q` over a hidden Attribute answers as if it were
//! absent; no answer carries an Attribute the view hides.
//!
//! `PROPTEST_CASES` sets the number of cases (default 32: each case builds
//! a broker).

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower::ServiceExt;

fn runtime() -> Result<tokio::runtime::Runtime, TestCaseError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| TestCaseError::fail(e.to_string()))
}

/// A Car: its number, and its `speed` and `colour` if it has them.
type Car = (u8, Option<u8>, Option<bool>);

fn cars() -> impl Strategy<Value = Vec<Car>> {
    proptest::collection::btree_map(
        0u8..20,
        (
            proptest::option::of(0u8..100),
            proptest::option::of(any::<bool>()),
        ),
        1..6,
    )
    .prop_map(|m| m.into_iter().map(|(n, (s, c))| (n, s, c)).collect())
}

fn car_doc((n, speed, red): Car) -> Value {
    let mut e = serde_json::Map::new();
    e.insert("id".into(), json!(format!("urn:ngsi-ld:Car:{n}")));
    e.insert("type".into(), json!("Car"));
    if let Some(s) = speed {
        e.insert("speed".into(), json!({"type": "Property", "value": s}));
    }
    if let Some(r) = red {
        let colour = if r { "red" } else { "blue" };
        e.insert(
            "colour".into(),
            json!({"type": "Property", "value": colour}),
        );
    }
    Value::Object(e)
}

/// Narrows every query to `speed<{0}` when it has one.
struct SlowOnly(Option<u8>);

impl PolicyEngine for SlowOnly {
    fn name(&self) -> &'static str {
        "slow-only"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = match self.0 {
            Some(b) => match antares_ql::parse_q(&format!("speed<{b}")) {
                Ok(q) => Decision::Filter(Filter {
                    q: Some(Box::new(q)),
                    ..Filter::default()
                }),
                Err(e) => Decision::Deny(e.to_string()),
            },
            None => Decision::Allow,
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

struct Nothing;

impl antares_ops::Surface for Nothing {
    fn name(&self) -> &'static str {
        "nothing"
    }
    fn prefix(&self) -> &'static str {
        "/x/nothing"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: antares_ops::Ops) -> Router {
        Router::new()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let bytes = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", tenant);
    if !bytes.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `id → shown member names` of a query's answer.
async fn query(
    app: &Router,
    tenant: &str,
    q: Option<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, TestCaseError> {
    let mut uri = "/ngsi-ld/v1/entities?type=Car&limit=100".to_owned();
    if let Some(q) = q {
        uri.push_str("&q=");
        uri.extend(q.bytes().map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        }));
    }
    let (s, body) = send(app, Method::GET, &uri, tenant, None).await;
    prop_assert_eq!(s, StatusCode::OK, "{} {}", uri, body);
    let mut out = BTreeMap::new();
    for e in body.as_array().cloned().unwrap_or_default() {
        let id = e
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let members = e
            .as_object()
            .map(|m| {
                m.keys()
                    .filter(|k| !["id", "type", "@context"].contains(&k.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.insert(id, members);
    }
    Ok(out)
}

async fn seed(
    app: &Router,
    tenant: &str,
    cars: &[Car],
    strip_speed: bool,
) -> Result<(), TestCaseError> {
    for car in cars {
        let mut doc = car_doc(*car);
        if strip_speed {
            doc.as_object_mut().map(|m| m.remove("speed"));
        }
        let (s, body) = send(app, Method::POST, "/ngsi-ld/v1/entities", tenant, Some(doc)).await;
        prop_assert_eq!(s, StatusCode::CREATED, "{}", body);
    }
    Ok(())
}

/// A broker holding `cars` in `city` and a loaded view `v` over them.
async fn with_view(cars: &[Car], view: &Value) -> Result<(TestBroker, Router), TestCaseError> {
    let b = TestBroker::new().await.map_err(TestCaseError::fail)?;
    let app = b.serve(Nothing).map_err(TestCaseError::fail)?;
    seed(&app, "city", cars, false).await?;
    let (s, body) = send(&app, Method::PUT, "/q/views/v", "city", Some(view.clone())).await;
    prop_assert_eq!(s, StatusCode::CREATED, "{}", body);
    Ok((b, app))
}

fn view(q: Option<u8>, attributes: &[&str]) -> Value {
    let mut v = json!({
        "id": "v", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": attributes}],
        "operations": ["query_entities"]
    });
    if let Some(a) = q {
        v.as_object_mut()
            .map(|m| m.insert("q".into(), json!(format!("speed>{a}"))));
    }
    v
}

fn attributes() -> impl Strategy<Value = Vec<&'static str>> {
    proptest::sample::subsequence(vec!["speed", "colour"], 0..=2)
}

fn caller_q() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        (0u8..100).prop_map(|n| Some(format!("speed>{n}"))),
        (0u8..100).prop_map(|n| Some(format!("speed<{n}"))),
        Just(Some("speed".to_owned())),
        Just(Some("!speed".to_owned())),
        Just(Some("colour==\"red\"".to_owned())),
        (0u8..100).prop_map(|n| Some(format!("speed>{n}|colour==\"blue\""))),
    ]
}

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32),
    )
}

proptest! {
    #![proptest_config(config())]

    /// `view ∧ engine` serves no Entity and no member that the view alone
    /// would not for the same query, nor any the engine alone does not admit.
    #[test]
    fn the_conjunction_never_widens(
        cars in cars(),
        view_q in proptest::option::of(0u8..100),
        engine_q in proptest::option::of(0u8..100),
        shown in attributes(),
        q in caller_q(),
    ) {
        runtime()?.block_on(async {
            let (a, _) = with_view(&cars, &view(view_q, &shown)).await?;
            let b = a.sibling().await.map_err(TestCaseError::fail)?.with_policy(Arc::new(SlowOnly(engine_q)));
            let view_only = query(&a.serve_view("v").map_err(TestCaseError::fail)?, "city", q.as_deref()).await?;
            // The engine alone, over everything it admits: the caller's q is
            // read on the Entity as shown, so through the view a hidden
            // Attribute is absent and `!speed` can match there what the
            // engine's full Entity does not (rule 2), without serving
            // anything the engine would not.
            let engine_only = query(&b.serve(Nothing).map_err(TestCaseError::fail)?, "city", None).await?;
            let both = query(&b.serve_view("v").map_err(TestCaseError::fail)?, "city", q.as_deref()).await?;
            for (id, members) in &both {
                let (Some(v), Some(e)) = (view_only.get(id), engine_only.get(id)) else {
                    return Err(TestCaseError::fail(format!(
                        "{id} served by view ∧ engine and not by each: view {view_only:?}, engine {engine_only:?}"
                    )));
                };
                prop_assert!(members.is_subset(v) && members.is_subset(e), "{} shows {:?}; view {:?}, engine {:?}", id, members, v, e);
            }
            Ok(())
        })?;
    }

    /// With `speed` hidden, every caller `q` answers through the view what
    /// it answers over the same Cars stored without `speed` (rule 2).
    #[test]
    fn q_over_hidden_equals_q_over_absent(cars in cars(), q in caller_q()) {
        runtime()?.block_on(async {
            let (a, app) = with_view(&cars, &view(None, &["colour"])).await?;
            seed(&app, "stripped", &cars, true).await?;
            let hidden = query(&a.serve_view("v").map_err(TestCaseError::fail)?, "city", q.as_deref()).await?;
            let absent = query(&app, "stripped", q.as_deref()).await?;
            prop_assert_eq!(hidden.keys().collect::<Vec<_>>(), absent.keys().collect::<Vec<_>>(), "q {:?}", q);
            Ok(())
        })?;
    }

    /// No answer through the view carries a member its rule does not show.
    #[test]
    fn projection_never_shows_a_hidden_attribute(
        cars in cars(),
        view_q in proptest::option::of(0u8..100),
        shown in attributes(),
        q in caller_q(),
    ) {
        runtime()?.block_on(async {
            let (a, _) = with_view(&cars, &view(view_q, &shown)).await?;
            let answer = query(&a.serve_view("v").map_err(TestCaseError::fail)?, "city", q.as_deref()).await?;
            for (id, members) in &answer {
                prop_assert!(members.iter().all(|m| shown.contains(&m.as_str())), "{} shows {:?}, the view {:?}", id, members, shown);
            }
            Ok(())
        })?;
    }
}
