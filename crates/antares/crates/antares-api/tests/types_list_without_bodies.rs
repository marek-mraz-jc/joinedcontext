// SPDX-License-Identifier: EUPL-1.2
//! `GET /types` names the types from the store's type list (T-4610).
//!
//! MCP `list_types` on a 25 000-Entity view folded every Entity of the view
//! for a handful of names: 20 s, the pool exhausted, the assistant's question
//! unanswered. The names come from the type list now, each type kept when the
//! caller sees one Entity of it, so what `/types` reads does not grow with the
//! Tenant, and the answer is the one the fold gives.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use crate::common;
use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const TYPES: [&str; 5] = ["Car", "Bus", "Stop", "Tree", "Lamp"];

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

/// Narrows every read the seam can narrow with one Filter.
struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "types-list"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
            Decision::Filter(self.0.clone())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Filter(Box::new(self.0.clone()))
    }
}

/// A view that shows Cars and Buses, and the Cars only in zone `a`.
fn view() -> Filter {
    let rule = |t: &str, q: Option<&str>| Rule {
        types: vec![iri(t)],
        q: q.map(|q| Box::new(antares_ql::parse_q(q).unwrap())),
        attributes: vec![iri("zone")],
        ..Rule::default()
    };
    Filter {
        rules: vec![rule("Car", Some("zone==\"a\"")), rule("Bus", None)],
        ..Filter::default()
    }
}

/// `per_type` Entities of each of the five types, every one in zone `b`, plus
/// one Car in zone `a` that is a Tree too. The store reads through a double
/// that counts the documents it hands out.
async fn state(filter: Option<Filter>, per_type: usize) -> (AppState, Arc<common::Double>) {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("me".into());
    if let Some(f) = filter {
        st = st.with_policy(Arc::new(Narrowing(f)));
    }
    antares_api::wire(&mut st).await;
    let tenant = TenantId::new(TenantId::DEFAULT).unwrap();
    let zone = |z: &str| json!([{"type": "Property", "value": z}]);
    for t in TYPES {
        for n in 0..per_type {
            let id = format!("urn:ngsi-ld:{t}:{n:06}");
            let doc = json!({"id": id, "type": [iri(t)], iri("zone"): zone("b")});
            assert!(st
                .store
                .create(&tenant, Kind::Entity, &id, doc)
                .await
                .unwrap());
        }
    }
    let id = "urn:ngsi-ld:Car:zone-a";
    let doc = json!({"id": id, "type": [iri("Tree"), iri("Car")], iri("zone"): zone("a")});
    assert!(st
        .store
        .create(&tenant, Kind::Entity, id, doc)
        .await
        .unwrap());
    let double = Arc::new(common::Double::flaky_list(st.store.clone(), 0));
    st.store = double.clone();
    (st, double)
}

async fn get(st: &AppState, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn type_list(body: &Value) -> Vec<String> {
    body["typeList"]
        .as_array()
        .unwrap_or_else(|| panic!("no typeList: {body}"))
        .iter()
        .map(|t| t.as_str().unwrap().to_owned())
        .collect()
}

/// Through the view: Bus, and Car and Tree from the one Car the view shows.
/// Stop and Lamp are read not at all, and the Entities read are the same for
/// 100 and for 10 000 per type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_views_type_list_reads_the_same_few_entities_whatever_the_tenant_holds() {
    let mut read = Vec::new();
    for per_type in [100, 10_000] {
        let (st, double) = state(Some(view()), per_type).await;
        let (code, body) = get(&st, "/ngsi-ld/v1/types").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(type_list(&body), ["Bus", "Car", "Tree"], "{body}");
        read.push(double.rows_read());
    }
    assert_eq!(
        read[0], read[1],
        "what /types reads grew with the Tenant: {read:?}"
    );
    assert!(read[1] <= 100, "a probe per type, not a fold: {read:?}");
}

/// Without a narrowing the list is the store's, and no Entity is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unnarrowed_type_list_reads_no_entity() {
    let (st, double) = state(None, 1_000).await;
    let (code, body) = get(&st, "/ngsi-ld/v1/types").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(type_list(&body), ["Bus", "Car", "Lamp", "Stop", "Tree"]);
    assert_eq!(double.rows_read(), 0);
}

/// The names are the fold's: `details=true` still folds the Entities, and
/// lists the same types under the same view.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_type_list_names_what_the_detailed_fold_names() {
    let (st, _) = state(Some(view()), 50).await;
    let (_, names) = get(&st, "/ngsi-ld/v1/types").await;
    let (code, details) = get(&st, "/ngsi-ld/v1/types?details=true").await;
    assert_eq!(code, StatusCode::OK, "{details}");
    let folded: Vec<String> = details
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {details}"))
        .iter()
        .map(|t| t["typeName"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(type_list(&names), folded);
}

/// A view whose condition no Entity meets lists nothing, rather than every
/// type the rules name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_no_entity_of_which_is_shown_is_not_listed() {
    let filter = Filter {
        rules: vec![Rule {
            types: vec![iri("Lamp")],
            q: Some(Box::new(antares_ql::parse_q("zone==\"z\"").unwrap())),
            attributes: vec![iri("zone")],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let (st, _) = state(Some(filter), 30).await;
    let (code, body) = get(&st, "/ngsi-ld/v1/types").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert!(type_list(&body).is_empty(), "{body}");
}
