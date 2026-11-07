// SPDX-License-Identifier: EUPL-1.2
//! `ViewOps::scan` (T-3757, ADR-0028 rules 3 to 5): the residual runs on what the view and the
//! policy answered, so it never adds an Entity; pages filled from the upstream cursor concatenate
//! to the whole answer; the scan reads at most its budget; and what only the residual read never
//! leaves it. A Probe surface runs the scan as the outer request's caller over 23 Cars, of which
//! the policy shows the 15 red ones and the view never the `secret` Attribute.

use antares_ops::policy::Subject;
use antares_ops::policy::{Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine};
use antares_ops::residual::{compile, Cmp, DateTime, Expr, Lit, Path, Program};
use antares_ops::test_kit::{q_filter, scan_within, TestBroker};
use antares_ops::{Caller, Ops, Page, Pushdown, Scanned, Surface};
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;

const CARS: usize = 23;
const TENANT: &str = "scan";

/// Shows the red Cars only.
struct RedOnly;

impl PolicyEngine for RedOnly {
    fn name(&self) -> &'static str {
        "red-only"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = match q_filter("colour==\"red\"") {
            Ok(f) => Decision::Filter(f),
            Err(e) => Decision::Deny(e),
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// What the next probe scans with.
type Plan = Arc<Mutex<(Pushdown, Option<Expr>)>>;

/// `GET /x/scan/{view}?start&size[&whole][&budget&upstream]`: the scan as JSON, `read` only
/// under an explicit budget.
struct Probe(Plan);

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/scan"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        let plan = self.0.clone();
        Router::new().route(
            "/{view}",
            get(
                move |caller: Caller,
                      UrlPath(id): UrlPath<String>,
                      Query(q): Query<HashMap<String, usize>>| async move {
                    let (pushdown, expr) =
                        plan.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let Ok(program) = expr.as_ref().map(program).transpose() else {
                        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
                    };
                    let Some(view) = ops.view(&id) else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    let at = |k: &str| q.get(k).copied().unwrap_or(0);
                    let page = Page {
                        start: at("start"),
                        size: at("size"),
                        whole: q.contains_key("whole"),
                    };
                    let scanned = match (q.get("budget"), &program) {
                        (Some(&budget), Some(p)) => {
                            let bound = (budget, at("upstream"));
                            scan_within(&view, &caller, &pushdown, p, page, bound)
                                .await
                                .map(|(s, read)| (s, Some(read)))
                        }
                        _ => view
                            .scan(&caller, &pushdown, program.as_ref(), page)
                            .await
                            .map(|s| (s, None)),
                    };
                    match scanned {
                        Ok((
                            Scanned::Page {
                                entities,
                                next,
                                count,
                                truncated,
                            },
                            read,
                        )) => Json(json!({
                            "entities": entities, "next": next, "count": count, "read": read,
                            "truncated": truncated, "pageSize": view.page_size()
                        }))
                        .into_response(),
                        Ok((Scanned::Answer(answer), _)) => answer,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                },
            ),
        )
    }
}

struct Rig {
    app: Router,
    plan: Plan,
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let Ok(resp): Result<Response, _> = app.clone().oneshot(req).await;
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

fn with_body(req: axum::http::request::Builder, body: &Value) -> Request<Body> {
    let text = body.to_string();
    req.header("Content-Type", "application/json")
        .header("NGSILD-Tenant", TENANT)
        .header("Content-Length", text.len())
        .body(Body::from(text))
        .unwrap_or_default()
}

/// Car `n`: red unless `n` is a multiple of 3, speed `n`, a secret, and 128 tags; Car 1's
/// speed has a second instance.
fn car(n: usize) -> Value {
    let colour = if n.is_multiple_of(3) { "blue" } else { "red" };
    let speed = json!({"type": "Property", "value": n});
    let speed = match n {
        1 => {
            json!([speed, {"type": "Property", "value": 100, "datasetId": "urn:ngsi-ld:Dataset:b"}])
        }
        _ => speed,
    };
    json!({
        "id": format!("urn:ngsi-ld:Car:{n:02}"), "type": "Car",
        "colour": {"type": "Property", "value": colour},
        "speed": speed,
        "secret": {"type": "Property", "value": "x"},
        "tags": {"type": "ListProperty", "valueList": (0..128).collect::<Vec<_>>()}
    })
}

async fn rig() -> Result<Rig, String> {
    rig_on(TestBroker::new().await?).await
}

async fn rig_on(broker: TestBroker) -> Result<Rig, String> {
    let admin = broker.router();
    let view = json!({"id": "cars", "tenant": TENANT, "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["colour", "speed", "tags"]}],
        "operations": ["query_entities"]});
    let (s, b) = send(&admin, with_body(Request::put("/q/views/cars"), &view)).await;
    assert_eq!(s, StatusCode::CREATED, "the view loads: {b}");
    for n in 0..CARS {
        let req = with_body(Request::post("/ngsi-ld/v1/entities"), &car(n));
        let (s, b) = send(&admin, req).await;
        assert_eq!(s, StatusCode::CREATED, "car {n}: {b}");
    }
    let plan = Plan::default();
    let app = broker
        .with_policy(Arc::new(RedOnly))
        .serve(Probe(plan.clone()))?;
    Ok(Rig { app, plan })
}

fn pushdown() -> Pushdown {
    Pushdown {
        query: "type=Car".into(),
        ..Pushdown::default()
    }
}

fn program(expr: &Expr) -> Result<Program, String> {
    let now = DateTime::parse("2026-10-11T15:00:00Z").unwrap_or(DateTime::MIN);
    compile(expr, now).map_err(|e| format!("{e:?}"))
}

fn odd() -> Expr {
    let odd = (1..40).step_by(2).map(Lit::Int).collect();
    Expr::In(Box::new(Expr::Path(Path::value("speed"))), odd)
}

fn secret_is_x() -> Expr {
    Expr::Cmp(
        Cmp::Eq,
        Box::new(Expr::Path(Path::value("secret"))),
        Box::new(Expr::Lit(Lit::Str("x".into()))),
    )
}

impl Rig {
    async fn scan(&self, plan: (Pushdown, Option<Expr>), query: &str) -> (StatusCode, Value) {
        *self.plan.lock().unwrap_or_else(PoisonError::into_inner) = plan;
        let req = Request::get(format!("/x/scan/cars?{query}"))
            .header("NGSILD-Tenant", TENANT)
            .body(Body::empty())
            .unwrap_or_default();
        send(&self.app, req).await
    }
}

fn ids(page: &Value) -> Vec<String> {
    page["entities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of the red Cars `keep` holds for, in id order.
fn expected(keep: impl Fn(usize) -> bool) -> Vec<String> {
    (0..CARS)
        .filter(|n| n % 3 != 0 && keep(*n))
        .map(|n| format!("urn:ngsi-ld:Car:{n:02}"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn residual_never_widens() {
    let rig = rig().await.expect("rig");
    let (s, plain) = rig.scan((pushdown(), None), "size=1000&whole=1").await;
    assert_eq!(s, StatusCode::OK, "{plain}");
    assert_eq!(
        ids(&plain),
        expected(|_| true),
        "the view and policy answer"
    );
    assert_eq!(plain["count"], 15);
    let shown = ids(&plain);
    let cases: [(Expr, Vec<String>); 4] = [
        (Expr::And(Vec::new()), expected(|_| true)),
        (odd(), expected(|n| n % 2 == 1)),
        (
            Expr::IsNull(Box::new(Expr::Path(Path::value("secret")))),
            expected(|_| true),
        ),
        (secret_is_x(), Vec::new()),
    ];
    for (expr, want) in cases {
        let (s, b) = rig
            .scan((pushdown(), Some(expr.clone())), "size=1000&whole=1")
            .await;
        assert_eq!(s, StatusCode::OK, "{expr:?}: {b}");
        let got = ids(&b);
        assert!(
            got.iter().all(|id| shown.contains(id)),
            "{expr:?} widened: {got:?}"
        );
        assert_eq!(got, want, "{expr:?}");
        assert_eq!(b["count"], want.len(), "{expr:?}");
        assert_eq!(b["next"], Value::Null);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn residual_pages_concatenate() {
    let rig = rig().await.expect("rig");
    let (_, whole) = rig.scan((pushdown(), Some(odd())), "size=0&whole=1").await;
    let whole = ids(&whole);
    assert_eq!(whole, expected(|n| n % 2 == 1));
    for size in [1, 2, 3, 5, 7, 100] {
        for upstream in [1, 2, 3, 1000] {
            for budget in [1, 3, 4, 1000] {
                let (mut start, mut got, mut pages) = (Some(0), Vec::new(), 0);
                while let Some(at) = start {
                    let q = format!("start={at}&size={size}&budget={budget}&upstream={upstream}");
                    let (s, b) = rig.scan((pushdown(), Some(odd())), &q).await;
                    assert_eq!(s, StatusCode::OK, "{q}: {b}");
                    let page = ids(&b);
                    assert!(page.len() <= size, "{q}: {page:?}");
                    assert!(b["read"].as_u64() <= u64::try_from(budget).ok(), "{q}: {b}");
                    got.extend(page);
                    start = b["next"].as_u64().and_then(|n| usize::try_from(n).ok());
                    pages += 1;
                    assert!(pages <= 2 * CARS, "{q}: no progress");
                }
                assert_eq!(
                    got, whole,
                    "size {size}, upstream {upstream}, budget {budget}"
                );
            }
        }
    }
    // without a test bound, the broker's own page and budget
    let (mut start, mut got) = (Some(0), Vec::new());
    while let Some(at) = start {
        let (_, b) = rig
            .scan((pushdown(), Some(odd())), &format!("start={at}&size=2"))
            .await;
        got.extend(ids(&b));
        start = b["next"].as_u64().and_then(|n| usize::try_from(n).ok());
    }
    assert_eq!(got, whole);
}

#[tokio::test(flavor = "multi_thread")]
async fn residual_scan_is_bounded() {
    let rig = rig().await.expect("rig");
    // a page reads at most the budget and continues where it stopped
    let (s, b) = rig
        .scan((pushdown(), Some(odd())), "size=100&budget=5&upstream=2")
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["read"], 5, "{b}");
    assert_eq!(b["next"], 5, "{b}");
    // the reads the 15 shown Cars take, never the 23 stored
    let (_, b) = rig
        .scan((pushdown(), Some(odd())), "size=100&budget=1000&upstream=4")
        .await;
    assert_eq!(b["read"], 15, "{b}");
    // a whole answer over the budget is refused from the count, before a page is read
    let (s, b) = rig
        .scan(
            (pushdown(), Some(odd())),
            "size=0&whole=1&budget=14&upstream=1000",
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(
        b["type"],
        "https://uri.etsi.org/ngsi-ld/errors/TooManyResults"
    );
    assert!(
        b["detail"]
            .as_str()
            .is_some_and(|d| d.contains("selects 15 Entities") && d.contains("at most 14")),
        "{b}"
    );
    // a count the store does not give is no excuse: reading past the budget is refused too
    let (s, b) = rig
        .scan(
            (pushdown(), Some(odd())),
            "size=0&whole=1&budget=15&upstream=3",
        )
        .await;
    assert_eq!(s, StatusCode::OK, "exactly the budget fits: {b}");
    assert_eq!(b["read"], 15, "{b}");
    // an evaluation past the residual's step bound is refused, never answered in part
    let tags = || Box::new(Expr::Path(Path::value("tags")));
    let nested = Expr::Any(
        tags(),
        Some(Box::new(Expr::Any(
            tags(),
            Some(Box::new(Expr::Any(
                tags(),
                Some(Box::new(Expr::Lit(Lit::Bool(false)))),
            ))),
        ))),
    );
    let (s, b) = rig.scan((pushdown(), Some(nested)), "size=3").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(
        b["detail"]
            .as_str()
            .is_some_and(|d| d.contains("residual evaluator")),
        "{b}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_attribute_is_absent_in_residual() {
    let rig = rig().await.expect("rig");
    // the residual reads speed, the standard selected colour: speed is read, never served
    let only_colour = Pushdown {
        attrs: Some(vec!["colour".into()]),
        ..pushdown()
    };
    let (s, b) = rig.scan((only_colour, Some(odd())), "size=100").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(ids(&b), expected(|n| n % 2 == 1));
    for e in b["entities"].as_array().into_iter().flatten() {
        let mut names: Vec<&str> = e
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        names.sort_unstable();
        assert_eq!(names, ["colour", "id", "type"], "{e}");
    }
    // the view hides secret from the residual as from the answer
    let (_, b) = rig
        .scan((pushdown(), Some(secret_is_x())), "size=100")
        .await;
    assert_eq!(ids(&b), Vec::<String>::new(), "{b}");
    // a residual on createdAt reads the system members and serves them only when asked
    let created = Expr::Not(Box::new(Expr::IsNull(Box::new(Expr::Path(
        Path::CreatedAt,
    )))));
    let (_, b) = rig
        .scan((pushdown(), Some(created.clone())), "size=100")
        .await;
    assert_eq!(ids(&b), expected(|_| true));
    let text = b["entities"].to_string();
    assert!(
        !text.contains("createdAt") && !text.contains("modifiedAt"),
        "{text}"
    );
    assert!(!text.contains("secret"), "{text}");
    let asked = Pushdown {
        sys_attrs: true,
        ..pushdown()
    };
    let (_, b) = rig.scan((asked, Some(created)), "size=100").await;
    assert!(b["entities"][0]["createdAt"].is_string(), "{b}");
    assert!(b["entities"][0]["colour"]["createdAt"].is_string(), "{b}");
    assert!(b["entities"][0]["speed"][1]["createdAt"].is_string(), "{b}");
}

#[tokio::test(flavor = "multi_thread")]
async fn scan_without_residual_is_one_page_and_store_errors_pass_through() {
    let rig = rig().await.expect("rig");
    let (s, b) = rig.scan((pushdown(), None), "size=4").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(ids(&b), expected(|_| true)[..4]);
    assert_eq!(b["next"], 4);
    assert_eq!(b["count"], Value::Null);
    let (_, b) = rig.scan((pushdown(), None), "start=12&size=4").await;
    assert_eq!(ids(&b).len(), 3);
    assert_eq!(b["next"], Value::Null);
    // a page of none with a residual reads nothing
    let (_, b) = rig
        .scan((pushdown(), Some(odd())), "size=0&budget=5&upstream=5")
        .await;
    assert_eq!(
        (b["entities"].clone(), b["read"].clone()),
        (json!([]), json!(0))
    );
    // the store's own refusal of the pushdown reaches the surface as the store gave it
    let broken = Pushdown {
        query: "type=Car&q=speed%3D%3D".into(),
        ..pushdown()
    };
    for plan in [(broken.clone(), None), (broken.clone(), Some(odd()))] {
        let (s, b) = rig.scan(plan, "size=3").await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    }
    let (s, _) = rig.scan((broken, Some(odd())), "size=0&whole=1").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // orderBy and attrs names are percent-encoded, and the order the surface gave stays first
    let ordered = Pushdown {
        order: vec!["speed".into()],
        attrs: Some(vec![
            "speed".into(),
            "colour name".into(),
            "a-b.c_d~".into(),
        ]),
        context: Some(json!(
            "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld"
        )),
        ..pushdown()
    };
    let (s, b) = rig.scan((ordered, Some(odd())), "size=2").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(ids(&b), expected(|n| n % 2 == 1)[..2]);
    let by_id = Pushdown {
        order: vec!["id".into()],
        ..pushdown()
    };
    let (_, b) = rig.scan((by_id, Some(odd())), "size=2").await;
    assert_eq!(ids(&b), expected(|n| n % 2 == 1)[..2]);
    // the outer request's Accept never reaches the store: the scan reads a list of Entities
    *rig.plan.lock().unwrap_or_else(PoisonError::into_inner) = (pushdown(), Some(odd()));
    let req = Request::get("/x/scan/cars?size=2")
        .header("NGSILD-Tenant", TENANT)
        .header("Accept", "application/geo+json")
        .body(Body::empty())
        .unwrap_or_default();
    let (s, b) = send(&rig.app, req).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(ids(&b), expected(|n| n % 2 == 1)[..2]);
    // a missing view is the probe's own 404
    let req = Request::get("/x/scan/none?size=1")
        .header("NGSILD-Tenant", TENANT)
        .body(Body::empty())
        .unwrap_or_default();
    assert_eq!(send(&rig.app, req).await.0, StatusCode::NOT_FOUND);
}

/// T-3773: a store page larger than the broker buffers for one request is 413, never read whole
/// into memory.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_past_the_answer_bound_is_413() {
    let broker = TestBroker::with_max_answer_bytes(64).await.expect("broker");
    let rig = rig_on(broker).await.expect("rig");
    for plan in [(pushdown(), None), (pushdown(), Some(odd()))] {
        let (s, b) = rig.scan(plan, "size=4").await;
        assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{b}");
        assert_eq!(b["detail"], antares_ops::bounds::ANSWER_TOO_LARGE);
    }
}

/// The sorted member names of each served Entity.
fn members(page: &Value) -> Vec<Vec<String>> {
    page["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let mut names: Vec<String> = e
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            names.sort_unstable();
            names
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selection_projects_and_never_selects() {
    // T-3822: `attrs` also drops the Entities holding none of the names; `pick` only projects
    let rig = rig().await.expect("rig");
    let seen = Pushdown {
        attrs: Some(vec!["seen".into()]),
        ..pushdown()
    };
    let frame = vec!["id".to_owned(), "type".to_owned()];
    // an IRI is one member name to `pick`, percent-encoded on the way
    let iri = Pushdown {
        attrs: Some(vec!["https://example.org/x#seen".into()]),
        ..pushdown()
    };
    let (s, b) = rig.scan((iri, None), "size=1000&whole=1").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(ids(&b), expected(|_| true));
    let (s, b) = rig.scan((seen.clone(), None), "size=1000&whole=1").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        (ids(&b), b["count"].clone()),
        (expected(|_| true), json!(15))
    );
    assert!(members(&b).iter().all(|m| *m == frame), "{b}");
    // under a residual, which reads speed and serves it not
    let (_, b) = rig.scan((seen, Some(odd())), "size=1000&whole=1").await;
    assert_eq!(ids(&b), expected(|n| n % 2 == 1));
    assert!(members(&b).iter().all(|m| *m == frame), "{b}");
    // a name `pick` would read as another member is not pushed: the scan projects itself
    let odd_name = Pushdown {
        attrs: Some(vec!["colour".into(), "a-b.c_d~".into()]),
        ..pushdown()
    };
    let shown = vec!["colour".to_owned(), "id".to_owned(), "type".to_owned()];
    for residual in [None, Some(odd())] {
        let (s, b) = rig.scan((odd_name.clone(), residual), "size=1000").await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert!(!ids(&b).is_empty(), "{b}");
        assert!(members(&b).iter().all(|m| *m == shown), "{b}");
    }
}

#[test]
fn pickable_names_are_one_member() {
    use antares_ops::pickable;
    for name in [
        "speed",
        "a_b",
        "https://uri.etsi.org/ngsi-ld/v1.8#x",
        "urn:a.b",
        "@id",
    ] {
        assert!(pickable(name), "{name}");
    }
    for name in ["", "a.b", "colour name", "a~", "-x", "a,b", "a{b}", "a|b"] {
        assert!(!pickable(name), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_past_the_lookup_budget_is_reported_under_a_residual() {
    // T-3823: the scan reads the store's `NGSILD-Warning`, so a surface can refuse a partial
    // expansion. Hub 0 links Hub 1 more times than one request may look up (4.5.23.1).
    let broker = TestBroker::new().await.expect("broker");
    let admin = broker.router();
    let view = json!({"id": "hubs", "tenant": TENANT, "selector": ["Hub"],
        "rules": [{"types": ["Hub"], "attributes": ["to", "n"]}],
        "operations": ["query_entities"]});
    let (s, b) = send(&admin, with_body(Request::put("/q/views/hubs"), &view)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    for (n, width) in [(0, 1001), (1, 1), (2, 1)] {
        let hub = json!({"id": format!("urn:ngsi-ld:Hub:{n}"), "type": "Hub",
            "n": {"type": "Property", "value": n},
            "to": {"type": "ListRelationship", "objectList": vec!["urn:ngsi-ld:Hub:1"; width]}});
        let (s, b) = send(
            &admin,
            with_body(Request::post("/ngsi-ld/v1/entities"), &hub),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    let plan = Plan::default();
    let app = broker.serve(Probe(plan.clone())).expect("serve");
    let scan = |query: &str, n: Option<i64>| {
        let residual = n.map(|n| {
            Expr::Cmp(
                Cmp::Ge,
                Box::new(Expr::Path(Path::value("n"))),
                Box::new(Expr::Lit(Lit::Int(n))),
            )
        });
        *plan.lock().unwrap_or_else(PoisonError::into_inner) = (
            Pushdown {
                query: format!("type=Hub&join=inline&joinLevel=1{query}"),
                ..pushdown()
            },
            residual,
        );
        let req = Request::get("/x/scan/hubs?size=10")
            .header("NGSILD-Tenant", TENANT)
            .body(Body::empty())
            .unwrap_or_default();
        send(&app, req)
    };
    for residual in [None, Some(0)] {
        let (s, b) = scan("", residual).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(b["truncated"], true, "{residual:?}: {b}");
        // past the long link list, the walk fits
        let (_, b) = scan("&id=urn:ngsi-ld:Hub:1,urn:ngsi-ld:Hub:2", residual).await;
        assert_eq!(b["truncated"], false, "{b}");
        assert_eq!(b["entities"].as_array().map(Vec::len), Some(2), "{b}");
    }
    // the residual drops Hub 0, yet the page it was read in still says so
    let (_, b) = scan("", Some(1)).await;
    assert_eq!(
        (b["truncated"].clone(), ids(&b).len()),
        (json!(true), 2),
        "{b}"
    );
}

/// T-4926: a page larger than the broker's `max_limit` is read in store pages of it and served
/// whole: the count asked once, nothing twice; `next` just past the last Entity of a page
/// that is not whole, none on a whole (counted) one, even when it fills `size` exactly.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_past_max_limit_is_read_in_store_pages() {
    let broker = TestBroker::with_max_limit(4).await.expect("broker");
    let rig = rig_on(broker).await.expect("rig");
    let (s, b) = rig.scan((pushdown(), None), "size=10&whole=1").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        b["pageSize"], 4,
        "one store read, what a surface streams at a time"
    );
    assert_eq!(ids(&b), expected(|_| true)[..10]);
    assert_eq!(
        (b["count"].clone(), b["next"].clone()),
        (json!(15), Value::Null)
    );
    let (_, b) = rig.scan((pushdown(), None), "size=15&whole=1").await;
    assert_eq!(ids(&b), expected(|_| true));
    assert_eq!(
        (b["count"].clone(), b["next"].clone()),
        (json!(15), Value::Null)
    );
    let (_, b) = rig.scan((pushdown(), None), "start=10&size=100").await;
    assert_eq!(ids(&b), expected(|_| true)[10..]);
    assert_eq!(b["next"], Value::Null);
    let (_, b) = rig.scan((pushdown(), None), "size=8").await;
    assert_eq!(ids(&b), expected(|_| true)[..8]);
    assert_eq!(b["next"], 8);
}
