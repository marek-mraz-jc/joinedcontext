// SPDX-License-Identifier: EUPL-1.2
//! `GET /x/views/{view}/facets` (T-6757): a value's count equals the Query Entities count of
//! the same selection through the view, an Entity the Policy hides counts nowhere, an `attr`
//! that is not listed, unknown or hidden by the engine answers one 400, and only an anonymous
//! answer may be cached by a shared cache.

use crate::common::{get, prop, put_view, send};
use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::{q_filter, TestBroker};
use axum::http::{Method, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;

const FACETS: &str = "/x/views/events/facets";
const ROOT: &str = "/x/views/events/ngsi-ld/v1";

/// Narrows Query Entities with its filter.
struct Narrowing(Arc<Filter>);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &'static str {
        "narrowing"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        // the seed's writes go through whole; the reads are narrowed
        Box::pin(std::future::ready(if op.clause == "5.7.2" {
            Decision::Filter(self.0.clone())
        } else {
            Decision::Allow
        }))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn event(id: &str, town: &str, categories: &[&str]) -> Value {
    json!({"id": format!("urn:ngsi-ld:Event:{id}"), "type": "Event",
           "name": prop(&json!(format!("Podujatie {id}"))),
           "category": prop(&json!(categories)),
           "secret": prop(&json!("s")),
           "address": prop(&json!({"addressLocality": town}))})
}

async fn fixture(engine: Option<Filter>) -> Result<(TestBroker, Router), String> {
    let mut broker = TestBroker::new().await?;
    if let Some(f) = engine {
        broker = broker.with_policy(Arc::new(Narrowing(Arc::new(f))));
    }
    let app = broker.serve(antares_views::Views)?;
    for e in [
        event("zv1", "Zvolen", &["music", "film"]),
        event("zv2", "Zvolen", &["music"]),
        event("sl1", "Sliač", &["theatre"]),
        event("bb1", "Banská Bystrica", &["music", "music"]),
    ] {
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            &[("NGSILD-Tenant", "city")],
            Some(&e),
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{}: {}", a.status, a.text));
        }
    }
    put_view(
        &app,
        &json!({"id": "events", "tenant": "city", "selector": ["Event"],
            "operations": ["query_entities"],
            "rules": [{"types": ["Event"], "attributes": ["name", "category", "address"]}],
            "representations": {"facets": ["category", "address.addressLocality"]}}),
    )
    .await?;
    Ok((broker, app))
}

fn counts(answer: &Value) -> Vec<(String, u64)> {
    answer
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|v| {
            (
                v.get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                v.get("count").and_then(Value::as_u64).unwrap_or_default(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, u64)]) -> Vec<(String, u64)> {
    list.iter().map(|(v, n)| ((*v).to_owned(), *n)).collect()
}

/// The `NGSILD-Results-Count` of Query Entities through the view with `q`.
async fn query_count(app: &Router, q: &str) -> u64 {
    let q = antares_ops::percent::encode(q);
    let a = get(
        app,
        &format!("{ROOT}/entities?type=Event&q={q}&count=true&limit=0"),
    )
    .await;
    a.header("ngsild-results-count")
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

#[tokio::test]
async fn each_count_equals_the_query_entities_count_of_that_value() -> Result<(), String> {
    let (_broker, app) = fixture(None).await?;
    let a = get(&app, &format!("{FACETS}?attr=address.addressLocality")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json.get("attr"), Some(&json!("address.addressLocality")));
    assert_eq!(a.json.get("truncated"), Some(&json!(false)));
    let towns = counts(&a.json);
    assert_eq!(
        towns,
        pairs(&[("Zvolen", 2), ("Banská Bystrica", 1), ("Sliač", 1)])
    );
    for (town, n) in &towns {
        let q = format!("address[addressLocality]==\"{town}\"");
        assert_eq!(query_count(&app, &q).await, *n, "{town}");
    }
    // a list counts each Entity once per value; limit keeps the top values
    let a = get(&app, &format!("{FACETS}?attr=category&limit=1")).await;
    assert_eq!(counts(&a.json), pairs(&[("music", 3)]));
    assert_eq!(a.json.get("truncated"), Some(&json!(true)));
    assert_eq!(query_count(&app, "category==\"music\"").await, 3);
    // the caller's q narrows what is counted
    let q = antares_ops::percent::encode("address[addressLocality]==\"Zvolen\"");
    let a = get(&app, &format!("{FACETS}?attr=category&q={q}")).await;
    assert_eq!(counts(&a.json), pairs(&[("music", 2), ("film", 1)]));
    Ok(())
}

#[tokio::test]
async fn a_row_the_policy_hides_changes_the_counts() -> Result<(), String> {
    let hidden = q_filter("address[addressLocality]!=\"Zvolen\"")?;
    let (_broker, app) = fixture(Some(hidden)).await?;
    let a = get(&app, &format!("{FACETS}?attr=category")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    // film is held by a Zvolen Entity alone: it is gone, not counted 0
    assert_eq!(counts(&a.json), pairs(&[("music", 1), ("theatre", 1)]));
    let a = get(&app, &format!("{FACETS}?attr=address.addressLocality")).await;
    assert_eq!(
        counts(&a.json),
        pairs(&[("Banská Bystrica", 1), ("Sliač", 1)])
    );
    assert_eq!(query_count(&app, "category==\"music\"").await, 1);
    Ok(())
}

#[tokio::test]
async fn an_attr_not_listed_unknown_or_hidden_is_one_400() -> Result<(), String> {
    let (_broker, app) = fixture(None).await?;
    let mut bodies = Vec::new();
    for attr in ["name", "secret", "nothing", "category.x", "address", "a;b"] {
        let a = get(&app, &format!("{FACETS}?attr={attr}")).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{attr}: {}", a.text);
        bodies.push(a.text);
    }
    let hiding = Filter {
        omit: vec!["https://uri.etsi.org/ngsi-ld/default-context/category".to_owned()],
        ..Filter::default()
    };
    let (_hidden_broker, hidden) = fixture(Some(hiding)).await?;
    let a = get(&hidden, &format!("{FACETS}?attr=category")).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    bodies.push(a.text);
    bodies.dedup();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    // the other facet still answers for that caller
    let a = get(&hidden, &format!("{FACETS}?attr=address.addressLocality")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    Ok(())
}

#[tokio::test]
async fn only_an_anonymous_answer_is_shared_and_a_write_is_refused() -> Result<(), String> {
    let (_broker, app) = fixture(None).await?;
    let uri = format!("{FACETS}?attr=category");
    let anonymous = get(&app, &uri).await;
    assert_eq!(
        anonymous.header("cache-control").as_deref(),
        Some("public, max-age=300")
    );
    for (name, value) in [("Authorization", "Bearer x"), ("Cookie", "s=1")] {
        let a = send(&app, Method::GET, &uri, &[(name, value)], None).await;
        assert_eq!(
            a.header("cache-control").as_deref(),
            Some("private, no-store"),
            "{name}"
        );
    }
    let a = send(&app, Method::POST, &uri, &[], Some(&json!({}))).await;
    assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED);
    let a = get(&app, "/x/views/nothing/facets?attr=category").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    let a = get(&app, &format!("{FACETS}?attr=category&type=Car")).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    Ok(())
}
