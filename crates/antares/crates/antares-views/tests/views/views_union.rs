// SPDX-License-Identifier: EUPL-1.2
//! One Query Entities across many views (T-4494, `docs/analysis/view-union.md`):
//! each part is what a direct read through its view answers, tagged with the
//! view in `Antares-Entity-Views`; a view the caller may not read contributes
//! nothing; the order and the cursor are stable; writes are refused.

use crate::common::{fixture, fixture_with, put_view, send, Answer, READS};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::http::{Method, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const UNION: &str = "/x/views/_union/ngsi-ld/v1/entities";
const LD: (&str, &str) = ("Accept", "application/ld+json");

async fn union(app: &Router, query: &str) -> Answer {
    send(app, Method::GET, &format!("{UNION}?{query}"), &[LD], None).await
}

fn tags(a: &Answer) -> Vec<String> {
    a.header("antares-entity-views")
        .unwrap_or_default()
        .split(',')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `(view, Entity)` of every member of a union page.
fn pairs(a: &Answer) -> Vec<(String, Value)> {
    tags(a)
        .into_iter()
        .zip(a.json.as_array().cloned().unwrap_or_default())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_type_in_three_views_answers_one_merged_tagged_page() -> Result<(), String> {
    let f = fixture().await?;
    let a = union(&f.app, "views=cars,history,fleet&type=Car&count=true").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.ids(),
        [
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:2"
        ]
    );
    assert_eq!(
        tags(&a),
        ["cars", "history", "fleet", "cars", "history", "fleet"]
    );
    assert_eq!(a.header("NGSILD-Results-Count").as_deref(), Some("6"));
    assert_eq!(a.link("next"), None);
    // each part is exactly what the view answers on its own
    for (view, doc) in pairs(&a) {
        let id = doc.get("id").and_then(Value::as_str).unwrap_or_default();
        let direct = send(
            &f.app,
            Method::GET,
            &format!("/x/views/{view}/ngsi-ld/v1/entities?type=Car&id={id}"),
            &[LD],
            None,
        )
        .await;
        assert_eq!(direct.json, json!([doc]), "{view} {id}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_attribute_stays_hidden_per_view() -> Result<(), String> {
    let f = fixture().await?;
    let a = union(&f.app, "views=cars,history&type=Car").await;
    for (view, doc) in pairs(&a) {
        assert!(doc.get("secret").is_none(), "{view}: {doc}");
        assert_eq!(
            doc.get("owner").is_some(),
            view == "cars" && doc.get("id") == Some(&json!("urn:ngsi-ld:Car:1")),
            "{view}: {doc}"
        );
    }
    let q = union(&f.app, "views=cars,history&type=Car&q=secret==1").await;
    assert_eq!(q.ids(), Vec::<String>::new(), "{}", q.text);
    Ok(())
}

/// Refuses every read through the view `history`.
struct NotHistory;

impl PolicyEngine for NotHistory {
    fn name(&self) -> &'static str {
        "not-history"
    }

    fn decide<'a>(&'a self, subject: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if subject.view() == Some("history") {
            Decision::Deny("not this view".into())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_the_caller_may_not_read_contributes_nothing() -> Result<(), String> {
    let f = fixture_with(TestBroker::new().await?.with_policy(Arc::new(NotHistory))).await?;
    let a = union(&f.app, "views=history,nope,cars&type=Car&count=true").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(tags(&a), ["cars", "cars"]);
    assert_eq!(a.header("NGSILD-Results-Count").as_deref(), Some("2"));
    assert!(
        !a.text.contains("history") && !a.text.contains("not this view"),
        "{}",
        a.text
    );
    let none = union(&f.app, "views=history,nope&type=Car&count=true").await;
    assert_eq!(
        (none.status, none.json.clone()),
        (StatusCode::OK, json!([])),
        "{}",
        none.text
    );
    assert_eq!(none.header("NGSILD-Results-Count").as_deref(), Some("0"));
    assert_eq!(tags(&none), Vec::<String>::new());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_order_and_the_cursor_are_stable_across_views() -> Result<(), String> {
    let f = fixture().await?;
    let whole = pairs(&union(&f.app, "views=fleet,cars,history&type=Car,Person").await);
    assert_eq!(whole.len(), 8, "{whole:?}");
    let mut paged = Vec::new();
    let mut next = Some(format!(
        "{UNION}?views=fleet,cars,history&type=Car,Person&limit=2"
    ));
    let mut rounds = 0;
    while let Some(uri) = next {
        let a = send(&f.app, Method::GET, &uri, &[LD], None).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.text);
        assert!(a.ids().len() <= 2, "{uri}");
        paged.extend(pairs(&a));
        next = a.link("next");
        rounds += 1;
        assert!(rounds < 10, "the cursor does not advance");
    }
    assert_eq!(paged, whole);
    let again = pairs(&union(&f.app, "views=fleet,cars,history&type=Car,Person").await);
    assert_eq!(again, whole, "the same read answers the same order");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_and_what_a_union_cannot_honour_are_refused() -> Result<(), String> {
    let f = fixture().await?;
    let post = send(
        &f.app,
        Method::POST,
        &format!("{UNION}?views=fleet"),
        &[],
        Some(&json!({"id": "urn:ngsi-ld:Car:8", "type": "Car"})),
    )
    .await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED, "{}", post.text);
    assert_eq!(post.header("allow").as_deref(), Some("GET, HEAD"));
    for query in [
        "views=cars&type=Car&orderBy=speed",
        "type=Car",
        "views=cars&type=Car&offset=1",
        "views=cars,fleet&cursor=1",
    ] {
        let a = union(&f.app, query).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{query}: {}", a.text);
    }
    let tenant = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=cars&type=Car"),
        &[("NGSILD-Tenant", "other")],
        None,
    )
    .await;
    assert_eq!(tenant.status, StatusCode::BAD_REQUEST, "{}", tenant.text);
    let bad_q = union(&f.app, "views=cars,fleet&type=Car&q=speed%3D%3D").await;
    assert_eq!(
        bad_q.status,
        StatusCode::BAD_REQUEST,
        "a query wrong for every view: {}",
        bad_q.text
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_json_names_the_one_context_in_a_link() -> Result<(), String> {
    let f = fixture().await?;
    let a = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=cars,fleet&type=Car"),
        &[],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.header("content-type").as_deref(),
        Some("application/json")
    );
    assert!(
        a.json
            .as_array()
            .is_some_and(|l| l.len() == 4 && l.iter().all(|d| d.get("@context").is_none())),
        "{}",
        a.text
    );
    assert!(a
        .headers
        .get_all("link")
        .iter()
        .any(|l| l.to_str().is_ok_and(|l| l.contains("json-ld#context"))));
    let geo = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=cars&type=Car"),
        &[("Accept", "application/geo+json")],
        None,
    )
    .await;
    assert_eq!(geo.status, StatusCode::NOT_ACCEPTABLE, "{}", geo.text);
    Ok(())
}

/// 200 views of one page each answer within the request budget: one read
/// of every Car through 200 views.
#[tokio::test(flavor = "multi_thread")]
async fn two_hundred_views_answer_one_page_in_time() -> Result<(), String> {
    let f = fixture().await?;
    let mut ids = Vec::new();
    for i in 0..200 {
        let id = format!("car{i}");
        put_view(
            &f.app,
            &json!({
                "id": id, "tenant": "city", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                "operations": READS
            }),
        )
        .await?;
        ids.push(id);
    }
    let started = Instant::now();
    let a = union(
        &f.app,
        &format!("views={}&type=Car&limit=1000&count=true", ids.join(",")),
    )
    .await;
    let took = started.elapsed();
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.ids().len(), 600, "three Cars through each of 200 views");
    assert_eq!(a.header("NGSILD-Results-Count").as_deref(), Some("600"));
    assert_eq!(a.link("next"), None);
    assert!(took < Duration::from_secs(10), "200 views took {took:?}");
    Ok(())
}

/// What the operation layer refuses for every view (a remote `@context`
/// it cannot fetch) is the union's answer; an entity map is never built
/// through it; a page of nothing is still a page.
#[tokio::test(flavor = "multi_thread")]
async fn what_every_view_refuses_alike_is_the_answer() -> Result<(), String> {
    let f = fixture().await?;
    let context = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=cars&type=Car"),
        &[(
            "Link",
            "<http://127.0.0.1:1/ctx.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\"",
        )],
        None,
    )
    .await;
    assert_eq!(
        context.status,
        StatusCode::GATEWAY_TIMEOUT,
        "{}",
        context.text
    );
    let map = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=cars&type=Car"),
        &[("NGSILD-EntityMap", "urn:ngsi-ld:EntityMap:x")],
        None,
    )
    .await;
    assert_eq!(map.status, StatusCode::BAD_REQUEST, "{}", map.text);
    let nothing = send(
        &f.app,
        Method::GET,
        &format!("{UNION}?views=nope&type=Car"),
        &[],
        None,
    )
    .await;
    assert_eq!(
        (nothing.status, nothing.json.clone()),
        (StatusCode::OK, json!([]))
    );
    assert!(
        nothing.headers.get("link").is_none(),
        "no Entity names no context"
    );
    let none = union(&f.app, "views=cars&type=Car&limit=0").await;
    assert_eq!(
        (none.status, none.json.clone()),
        (StatusCode::OK, json!([]))
    );
    assert_eq!(none.link("next"), None);
    Ok(())
}

/// The next link keeps `count=true`, and every page counts the whole union.
#[tokio::test(flavor = "multi_thread")]
async fn every_page_counts_the_whole_union() -> Result<(), String> {
    let f = fixture().await?;
    let first = union(&f.app, "views=cars,fleet&type=Car&limit=3&count=true").await;
    assert_eq!(first.ids().len(), 3, "{}", first.text);
    let next = first.link("next").ok_or("no next link")?;
    assert!(next.contains("count=true"), "{next}");
    let second = send(&f.app, Method::GET, &next, &[LD], None).await;
    assert_eq!(second.ids(), ["urn:ngsi-ld:Car:2"], "{}", second.text);
    let counted = union(&f.app, "views=cars,fleet&type=Car&limit=0&count=true").await;
    assert_eq!(counted.json, json!([]), "{}", counted.text);
    assert_eq!(counted.link("next"), None);
    for page in [&first, &second, &counted] {
        assert_eq!(page.header("NGSILD-Results-Count").as_deref(), Some("4"));
    }
    let head = send(
        &f.app,
        Method::HEAD,
        &format!("{UNION}?views=cars&type=Car&count=true"),
        &[],
        None,
    )
    .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.header("NGSILD-Results-Count").as_deref(), Some("2"));
    Ok(())
}

/// Puts views `car0`..`car{n-1}`, each the Cars with their speed; their ids.
async fn speed_views(app: &Router, n: usize) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = format!("car{i}");
        put_view(
            app,
            &json!({
                "id": id, "tenant": "city", "selector": ["Car"],
                "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                "operations": READS
            }),
        )
        .await?;
        ids.push(id);
    }
    Ok(ids)
}

/// F-ANT1-001: every view read of one union request takes from one byte
/// budget, the broker's `max_answer_bytes`; pages that pass it together are
/// a 413 naming the bound, while a few views still answer.
#[tokio::test(flavor = "multi_thread")]
async fn view_pages_past_the_answer_budget_together_are_413() -> Result<(), String> {
    let plain = fixture().await?;
    speed_views(&plain.app, 1).await?;
    let one = union(&plain.app, "views=car0&type=Car&limit=1000").await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let page = one.text.len();
    let f = fixture_with(TestBroker::with_max_answer_bytes(page.saturating_mul(3)).await?).await?;
    let ids = speed_views(&f.app, 8).await?;
    let few = union(&f.app, "views=car0,car1&type=Car&limit=1000").await;
    assert_eq!(few.status, StatusCode::OK, "{}", few.text);
    let many = union(
        &f.app,
        &format!("views={}&type=Car&limit=1000", ids.join(",")),
    )
    .await;
    assert_eq!(many.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", many.text);
    assert!(many.text.contains("max_answer_bytes"), "{}", many.text);
    Ok(())
}
