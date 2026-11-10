// SPDX-License-Identifier: EUPL-1.2
//! `ViewOps::scan_from` on the Postgres arm (T-4907): a type paged by the store's keyset cursor
//! comes whole, once and in id order while rows are written and deleted between pages, the
//! Policy narrows every page, and a cursor of another view, Tenant or query, or an altered
//! one, is the store's 400; a cursor of another read size is not sent. Skips loudly without
//! `ANTARES_TEST_DATABASE_URL` (see antares-sql/tests/pg.rs for the recipe).

use antares_ops::policy::{Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine};
use antares_ops::policy::Subject;
use antares_ops::test_kit::{q_filter, TestBroker};
use antares_ops::{Caller, Ops, Page, Pushdown, Scanned, Surface};
use antares_sql::store::any::{AnyStore, PgBackend};
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

/// Shows the red Cars only.
struct RedOnly;

impl PolicyEngine for RedOnly {
    fn name(&self) -> &'static str {
        "red-only"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = match q_filter("colour==\"red\"") {
            Ok(f) if op.clause.starts_with("5.7.") => Decision::Filter(Arc::new(f)),
            Ok(_) => Decision::Allow,
            Err(e) => Decision::Deny(e),
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// `GET /x/keyset/{view}?start&size[&cursor][&q]`: one `scan_from` page of the view's Cars.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "keyset-probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/keyset"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}",
            get(
                move |caller: Caller,
                      UrlPath(id): UrlPath<String>,
                      Query(q): Query<HashMap<String, String>>| async move {
                    let Some(view) = ops.view(&id) else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    let at = |k: &str| q.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
                    let page = Page {
                        start: at("start"),
                        size: at("size"),
                        whole: false,
                    };
                    let pushdown = Pushdown {
                        query: match q.get("q") {
                            Some(more) => format!("type=Car&q={more}"),
                            None => "type=Car".into(),
                        },
                        ..Pushdown::default()
                    };
                    let cursor = q.get("cursor").map(String::as_str);
                    match view.scan_from(&caller, &pushdown, page, cursor).await {
                        Ok(Scanned::Page {
                            entities,
                            next,
                            cursor,
                            ..
                        }) => {
                            let ids: Vec<&str> =
                                entities.iter().filter_map(|e| e["id"].as_str()).collect();
                            Json(json!({"ids": ids, "next": next, "cursor": cursor}))
                                .into_response()
                        }
                        Ok(Scanned::Answer(answer)) => answer,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                },
            ),
        )
    }
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

fn with_body(req: axum::http::request::Builder, tenant: &str, body: &Value) -> Request<Body> {
    let text = body.to_string();
    req.header("Content-Type", "application/json")
        .header("NGSILD-Tenant", tenant)
        .header("Content-Length", text.len())
        .body(Body::from(text))
        .unwrap_or_default()
}

/// Car `n`, red unless `n` is a multiple of 3.
fn car(n: usize) -> Value {
    let colour = if n.is_multiple_of(3) { "blue" } else { "red" };
    json!({"id": id(n), "type": "Car", "colour": {"type": "Property", "value": colour}})
}

fn id(n: usize) -> String {
    format!("urn:ngsi-ld:Car:{n:03}")
}

async fn write(admin: &Router, tenant: &str, n: usize) {
    let (s, b) = send(admin, with_body(Request::post("/ngsi-ld/v1/entities"), tenant, &car(n))).await;
    assert_eq!(s, StatusCode::CREATED, "car {n}: {b}");
}

async fn delete(admin: &Router, tenant: &str, n: usize) {
    let req = Request::delete(format!("/ngsi-ld/v1/entities/{}", id(n)))
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .unwrap_or_default();
    let (s, b) = send(admin, req).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "delete {n}: {b}");
}

/// One page of `view`: its ids, `next` and cursor.
async fn page(app: &Router, view: &str, query: &str) -> (StatusCode, Vec<String>, Value) {
    let req = Request::get(format!("/x/keyset/{view}?{query}"))
        .body(Body::empty())
        .unwrap_or_default();
    let (s, b) = send(app, req).await;
    let ids = b["ids"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (s, ids, b)
}

fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

#[expect(clippy::print_stderr, reason = "a skipped database test says so")]
#[tokio::test(flavor = "multi_thread")]
async fn a_type_pages_by_cursor_once_and_in_order_on_postgres() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let state = antares_api::AppState::with_store(
        "antares-test".into(),
        Arc::new(AnyStore::Pg(PgBackend::new(pool))),
        "postgres",
    );
    let broker = TestBroker::over(state).await.expect("broker");
    let admin = broker.router();
    // Tenants of this run alone: the database is shared with other tests.
    let run = uuid_like();
    let (city, other) = (format!("k{run}a"), format!("k{run}b"));
    for (view, tenant) in [("cars", &city), ("cars-other", &other)] {
        let body = json!({"id": format!("{view}-{run}"), "tenant": tenant, "selector": ["Car"],
            "rules": [{"types": ["Car"], "attributes": ["colour"]}],
            "operations": ["query_entities"]});
        let (s, b) = send(
            &admin,
            with_body(Request::put(format!("/q/views/{view}-{run}")), tenant, &body),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "the view loads: {b}");
    }
    for n in 0..40 {
        write(&admin, &city, n).await;
        write(&admin, &other, n).await;
    }
    let app = broker
        .with_policy(Arc::new(RedOnly))
        .serve(Probe)
        .expect("app");
    let (cars, cars_other) = (format!("cars-{run}"), format!("cars-other-{run}"));

    // Pages of 5: after page 2 the served Cars 1 and 2 are deleted and Car 1 (before the
    // cursor) written again, Car 41 (after it) written; the blue Car 45 is never shown.
    let (mut seen, mut start, mut cursor) = (Vec::new(), 0usize, None::<String>);
    let mut cursors = 0;
    for n in 0.. {
        let mut query = format!("start={start}&size=5");
        if let Some(c) = &cursor {
            query.push_str(&format!("&cursor={c}"));
        }
        let (s, ids, b) = page(&app, &cars, &query).await;
        assert_eq!(s, StatusCode::OK, "page {n}: {b}");
        seen.extend(ids);
        cursor = b["cursor"].as_str().map(String::from);
        cursors += usize::from(cursor.is_some());
        let Some(next) = b["next"].as_u64() else {
            break;
        };
        start = usize::try_from(next).unwrap_or(usize::MAX);
        if n == 1 {
            delete(&admin, &city, 1).await;
            delete(&admin, &city, 2).await;
            for m in [1, 41, 45] {
                write(&admin, &city, m).await;
            }
        }
        assert!(n < 20, "the pages end");
    }
    let red = |n: &usize| n % 3 != 0;
    let want: Vec<String> = (0..40).chain([41]).filter(red).map(id).collect();
    assert_eq!(seen, want, "every red Car once, in id order");
    assert!(cursors >= 4, "the pages were read by cursor ({cursors})");

    // A cursor of page 1 on another query, another view of another Tenant, altered, or with
    // another start is the store's 400; with another size it is not sent.
    let (_, first, b) = page(&app, &cars, "start=0&size=5").await;
    let c = b["cursor"].as_str().expect("page 1 names a cursor").to_owned();
    let flipped = {
        let mut t = c.clone();
        let last = if t.ends_with('0') { "1" } else { "0" };
        t.pop();
        t.push_str(last);
        t
    };
    for (what, view, query) in [
        ("another query", &cars, format!("start=5&size=5&cursor={c}&q=colour==%22red%22")),
        ("another Tenant", &cars_other, format!("start=5&size=5&cursor={c}")),
        ("altered", &cars, format!("start=5&size=5&cursor={flipped}")),
        ("another start", &cars, format!("start=6&size=5&cursor={c}")),
    ] {
        let (s, ids, b) = page(&app, view, &query).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{what}: {b}");
        assert!(ids.is_empty(), "{what}");
    }
    let (s, ids, b) = page(&app, &cars, &format!("start=5&size=4&cursor={c}")).await;
    assert_eq!(s, StatusCode::OK, "another size: {b}");
    let now: Vec<String> = (0..40).chain([41]).filter(|n| *n != 2).filter(red).map(id).collect();
    assert_eq!(ids, now.get(5..9).unwrap_or_default(), "by offset; page 1 {first:?}");
}
