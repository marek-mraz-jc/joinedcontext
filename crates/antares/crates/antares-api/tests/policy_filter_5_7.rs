// SPDX-License-Identifier: EUPL-1.2
//! What a narrowing policy decision does to a read (ADR-0020).
//!
//! The seam's third answer is `Filter`: the engine allows the operation over
//! less than it asked for. Three clauses decide what "less" means here.
//!
//! 5.7.2.4 gives the query its filters — "the filter conditions specified by
//! the query" — and the narrowing is one more condition conjoined into them,
//! so an Entity the subject may not see is simply not in the result set.
//! 5.7.1.4 answers a retrieve of one Entity, and an Entity outside the
//! narrowing has to answer the way an absent one does: "If the NGSI-LD
//! Entity does not exist, an error of type ResourceNotFound shall be
//! raised". Telling a caller apart from that would leak the row.
//!
//! 4.21 gives the projection language `pick` and `omit` are written in, and
//! 6.5.3.1 what they do: `omit` removes "the listed Entity members" and
//! `pick` reduces the Entity "down to only contain the listed Entity
//! members". A policy `pick` keeps `id` and `type` besides — 5.2.4 makes
//! them the Entity, and a document without them is not one.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// An engine that narrows the reads a narrowing can reach
/// (`policy::FILTERABLE`) and allows everything else, so a test can seed and
/// register through the same router it then reads through. A Filter on any
/// other clause is a refusal, which is the seam's own rule and not this
/// engine's.
struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "narrowing"
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
        NotifyDecision::Deliver
    }
}

fn slow() -> Filter {
    Filter {
        q: Some(Box::new(antares_ql::parse_q("speed<50").expect("q"))),
        ..Filter::default()
    }
}

/// The notification pipeline is wired, because the history a temporal read
/// answers from is written by it.
async fn state(f: Filter) -> AppState {
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Narrowing(f)));
    antares_api::wire(&mut st).await;
    st
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
            b.body(Body::from(payload)).expect("request")
        }
        None => b.body(Body::empty()).expect("request"),
    };
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let doc = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, doc)
}

/// Two Vehicles, one under the narrowing and one over it.
async fn seed(st: &AppState) {
    for (id, speed) in [
        ("urn:ngsi-ld:Vehicle:slow", 10),
        ("urn:ngsi-ld:Vehicle:fast", 90),
    ] {
        let (code, _) = send(
            st,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(json!({
                "id": id, "type": "Vehicle",
                "speed": {"type": "Property", "value": speed},
                "colour": {"type": "Property", "value": "red"},
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "seed {id}");
    }
}

/// The narrowing marker on a GET, which is a response header rather than a
/// payload member and so cannot be read through [`send`].
async fn restricted(st: &AppState, uri: &str) -> Option<String> {
    antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
        .headers()
        .get("Antares-Results-Restricted")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// 5.6.11 seeds the Temporal Evolution directly: the history a write to
/// `/entities` leaves behind is drained after the response, and these tests
/// are about the read, not about when the drain lands.
async fn seed_history(st: &AppState) {
    for id in ["urn:ngsi-ld:Vehicle:slow", "urn:ngsi-ld:Vehicle:fast"] {
        let (code, _) = send(
            st,
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            Some(json!({
                "id": id, "type": "Vehicle",
                "speed": [{"type": "Property", "value": 10,
                           "observedAt": "2026-01-01T00:00:00Z"}],
                "colour": [{"type": "Property", "value": "red",
                            "observedAt": "2026-01-01T00:00:00Z"}],
            })),
        )
        .await;
        assert!(
            code == StatusCode::CREATED || code == StatusCode::NO_CONTENT,
            "seed {id}: {code}"
        );
    }
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("array")
        .iter()
        .filter_map(|e| e["id"].as_str().map(str::to_owned))
        .collect()
}

/// 5.7.2.4: the narrowing is one more filter condition on the query the
/// store runs, so the Entity over it is absent — not hidden after the fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_q_narrows_the_entity_query() {
    let wide = state(Filter::default()).await;
    seed(&wide).await;
    let (code, all) = send(&wide, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(ids(&all).len(), 2, "the raw query returns both: {all}");

    let narrow = state(slow()).await;
    seed(&narrow).await;
    let (code, some) = send(&narrow, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        ids(&some),
        vec!["urn:ngsi-ld:Vehicle:slow".to_owned()],
        "{some}"
    );
}

/// The request's own `q` still applies: the two are conjoined, never
/// replaced, so a query for the fast Vehicle under a slow-only policy is
/// empty rather than answered with the slow one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_request_query_and_the_policy_query_are_conjoined() {
    let st = state(slow()).await;
    seed(&st).await;
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Vehicle&q=speed%3E50",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(ids(&list).is_empty(), "{list}");
}

/// 5.7.1.4: an Entity outside the narrowing answers the way an absent one
/// does, so a caller cannot tell a hidden Entity from one that never was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_entity_is_a_resource_not_found() {
    let st = state(slow()).await;
    seed(&st).await;
    let (code, _) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (code, pd) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:fast",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{pd}");
    assert_eq!(
        pd["type"],
        "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
    );
}

/// 5.7.1.4 and ADR-0023 rule 3 for Retrieve Attribute (10.4.4): an
/// Attribute of an Entity outside the narrowing is the Attribute of an
/// absent Entity, value and all, never served because the Attribute itself
/// is visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attribute_of_a_hidden_entity_is_a_resource_not_found() {
    let st = state(slow()).await;
    seed(&st).await;
    for suffix in ["", "/value"] {
        let shown = format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow/attrs/speed{suffix}");
        let (code, body) = send(&st, "GET", &shown, None).await;
        assert_eq!(code, StatusCode::OK, "{shown}: {body}");
        let hidden = format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:fast/attrs/speed{suffix}");
        let absent = format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:none/attrs/speed{suffix}");
        let (code, hid) = send(&st, "GET", &hidden, None).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "{hidden}: {hid}");
        let (_, abs) = send(&st, "GET", &absent, None).await;
        assert_eq!(
            hid.to_string().replace("fast", "X"),
            abs.to_string().replace("none", "X"),
            "{hidden} answers apart from an absent Entity"
        );
    }
}

/// 6.5.3.1 `omit`: "the listed Entity members are removed from the Entity",
/// on every document the operation serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_omit_removes_the_member_from_every_document() {
    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    for e in list.as_array().expect("array") {
        assert!(e.get("colour").is_none(), "omitted member served: {e}");
        assert!(
            e.get("speed").is_some(),
            "omit removed more than it named: {e}"
        );
    }
    let (_, one) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow",
        None,
    )
    .await;
    assert!(one.get("colour").is_none(), "{one}");
}

/// 6.5.3.1 `pick`: the Entity is "reduced down to only contain the listed
/// Entity members" — and stays an Entity, so 5.2.4's `id` and `type` remain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_pick_leaves_the_entity_frame_and_nothing_it_did_not_name() {
    let st = state(Filter {
        pick: vec!["speed".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (_, one) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow",
        None,
    )
    .await;
    assert_eq!(one["id"], "urn:ngsi-ld:Vehicle:slow", "{one}");
    assert_eq!(one["type"], "Vehicle", "{one}");
    assert!(one.get("speed").is_some(), "{one}");
    assert!(
        one.get("colour").is_none(),
        "a picked document kept what it did not name: {one}"
    );
}

/// A request's own projection narrows further; the policy's never widens it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_request_projection_and_the_policy_projection_both_apply() {
    let st = state(Filter {
        pick: vec!["speed".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (_, one) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow?pick=colour",
        None,
    )
    .await;
    assert!(
        one.get("colour").is_none() && one.get("speed").is_none(),
        "picking outside the policy's pick widened it: {one}"
    );
}

/// A narrowed answer says so when the engine asks for it, and says nothing
/// when it does not: narrowing is silent by default (ADR-0020).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restricted_answer_carries_the_header_and_a_silent_one_does_not() {
    let st = state(Filter {
        restricted: true,
        ..slow()
    })
    .await;
    seed(&st).await;
    assert_eq!(
        restricted(&st, "/ngsi-ld/v1/entities?type=Vehicle").await,
        Some("true".to_owned())
    );

    let quiet = state(slow()).await;
    seed(&quiet).await;
    assert_eq!(
        restricted(&quiet, "/ngsi-ld/v1/entities?type=Vehicle").await,
        None,
        "narrowing is silent unless the engine asks for the header"
    );
}

/// A single-Entity read is narrowed too — 5.7.1 and 5.7.3 both project the
/// document they answer with — so both say so, or a client that reads one
/// Entity at a time never learns it is seeing less than the broker holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_restricted_header_reaches_every_read_that_was_narrowed() {
    let st = state(Filter {
        restricted: true,
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    seed_history(&st).await;
    for uri in [
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:slow/attrs/speed",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:slow",
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z",
        "/ngsi-ld/v1/entityMaps?type=Vehicle",
    ] {
        assert_eq!(
            restricted(&st, uri).await,
            Some("true".to_owned()),
            "{uri} answered a narrowed read without saying so"
        );
    }
}

/// 5.7.2.4 refuses a query with no discriminating filter, and a policy `q`
/// is not the client's filter: a request that was too wide stays too wide.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_too_wide_query_is_still_too_wide_under_a_narrowing_policy() {
    let st = state(slow()).await;
    seed(&st).await;
    let (code, pd) = send(&st, "GET", "/ngsi-ld/v1/entities", None).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{pd}");
}

/// The batch query (6.23) is the POST spelling of 5.7.2 and narrows with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_narrowing_reaches_the_batch_query() {
    let st = state(slow()).await;
    seed(&st).await;
    let (code, list) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Vehicle"}]})),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        ids(&list),
        vec!["urn:ngsi-ld:Vehicle:slow".to_owned()],
        "{list}"
    );
}

/// ADR-0023 rule 8, read for every narrowing: the history served is that
/// of the Entities the narrowing selects now. `fast` was at 10 on
/// 2026-01-01 and is at 90 now, so its history is hidden by `speed<50`
/// however slow it once was; its retrieve answers as an absent one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_history_is_served_only_for_entities_the_narrowing_selects_now() {
    let st = state(slow()).await;
    seed(&st).await;
    seed_history(&st).await;
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(
        ids(&list),
        vec!["urn:ngsi-ld:Vehicle:slow".to_owned()],
        "{list}"
    );
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:slow",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, hid) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:fast",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{hid}");
    let (_, abs) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:none",
        None,
    )
    .await;
    assert_eq!(
        hid.to_string().replace("fast", "X"),
        abs.to_string().replace("none", "X")
    );
}

/// 5.7.4 reads the same Entities over time, and the same narrowing decides
/// which of them the subject may see.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_narrowing_reaches_the_temporal_query() {
    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    seed_history(&st).await;
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let entities = list.as_array().expect("array");
    assert_eq!(entities.len(), 2, "the history of both Vehicles: {list}");
    for e in entities {
        assert!(
            e.get("speed").is_some(),
            "the narrowing removed more than it named: {e}"
        );
        assert!(
            e.get("colour").is_none(),
            "omitted member served by history: {e}"
        );
    }
}

/// ADR-0020 asks an engine to write its rules against IRIs. 4.21 reads a
/// dot as the sub-attribute path separator, so a name run through that
/// grammar would be truncated at the first dot of the authority
/// (`https://uri.etsi.org/…` → the member `https://uri`) and would remove
/// nothing — silently, which is the worst way for a projection to fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_projection_named_as_an_iri_removes_the_member_it_names() {
    let st = state(Filter {
        omit: vec!["https://uri.etsi.org/ngsi-ld/default-context/colour".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    for e in list.as_array().expect("array") {
        assert!(e.get("speed").is_some(), "more was removed than named: {e}");
        assert!(
            e.get("colour").is_none(),
            "an IRI-named omit matched nothing: {e}"
        );
    }
}

/// 5.14.4.4 builds the EntityMap from the query's own candidate set, and
/// Table 5.2.39-1/-2 give that map `id`, `type`, `expiresAt`, `entityMap`
/// and `linkedMaps` — no Entity members, so `pick` and `omit` have nothing
/// to act on here. What the narrowing owes the map is its key set: a key is
/// an Entity id, and an id the subject may not see would leak the row's
/// existence to every page read through the map afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_entity_map_lists_only_the_narrowed_candidates() {
    let wide = state(Filter::default()).await;
    seed(&wide).await;
    let (code, map) = send(&wide, "GET", "/ngsi-ld/v1/entityMaps?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::CREATED, "{map}");
    let keys = |m: &Value| {
        let mut k: Vec<String> = m["entityMap"]
            .as_object()
            .expect("entityMap")
            .keys()
            .cloned()
            .collect();
        k.sort();
        k
    };
    assert_eq!(
        keys(&map),
        vec![
            "urn:ngsi-ld:Vehicle:fast".to_owned(),
            "urn:ngsi-ld:Vehicle:slow".to_owned()
        ],
        "the raw query maps both: {map}"
    );

    let st = state(slow()).await;
    seed(&st).await;
    let (code, map) = send(&st, "GET", "/ngsi-ld/v1/entityMaps?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::CREATED, "{map}");
    assert_eq!(
        keys(&map),
        vec!["urn:ngsi-ld:Vehicle:slow".to_owned()],
        "{map}"
    );
}

/// Entities under three Scopes, for the narrowing below.
async fn seed_scoped(st: &AppState) {
    for (id, scope) in [
        ("urn:ngsi-ld:Vehicle:traffic", "/BB/Traffic"),
        ("urn:ngsi-ld:Vehicle:parking", "/BB/Parking"),
        ("urn:ngsi-ld:Vehicle:other", "/Other"),
    ] {
        let (code, _) = send(
            st,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(json!({
                "id": id, "type": "Vehicle", "scope": scope,
                "speed": {"type": "Property", "value": 10},
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "seed {id}");
    }
}

/// The ids a narrowed query answers with, sorted.
async fn sorted_ids(st: &AppState, uri: &str) -> Vec<String> {
    let (code, body) = send(st, "GET", uri, None).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let mut v = ids(&body);
    v.sort();
    v
}

/// A `scopeQ` narrowing joins a request that brought its own: 4.19's `and`
/// is over independent per-pattern predicates, so it distributes over the
/// `,`/`|` disjunction and the intersection is itself a Scope Query. The
/// caller sees what both select and never more than the engine allowed —
/// a gateway that narrows a subject to a scope subtree must not have to
/// refuse every consumer that filters by Scope of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_narrowing_intersects_the_requests_own() {
    let st = state(Filter {
        scope_q: Some("/BB/#".into()),
        ..Filter::default()
    })
    .await;
    seed_scoped(&st).await;

    // no scopeQ of its own: the narrowing is simply the query's scope
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Vehicle").await,
        vec![
            "urn:ngsi-ld:Vehicle:parking".to_owned(),
            "urn:ngsi-ld:Vehicle:traffic".to_owned()
        ],
        "the engine's subtree alone"
    );
    // its own, inside the engine's: the intersection is the caller's
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Vehicle&scopeQ=/BB/Traffic").await,
        vec!["urn:ngsi-ld:Vehicle:traffic".to_owned()]
    );
    // its own, reaching outside: the part the engine forbids is dropped,
    // and nothing the engine forbids is served
    assert_eq!(
        sorted_ids(
            &st,
            "/ngsi-ld/v1/entities?type=Vehicle&scopeQ=/BB/Traffic,/Other"
        )
        .await,
        vec!["urn:ngsi-ld:Vehicle:traffic".to_owned()],
        "an entity outside the engine's subtree is never served"
    );
    // disjoint: an empty answer, not a wider one
    assert!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Vehicle&scopeQ=/Other")
            .await
            .is_empty()
    );
}

/// An intersection too large to write as a Scope Query leaves the seam the
/// answer it had before: refuse, because serving either side alone is wider
/// than the engine decided.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inexpressible_scope_intersection_is_still_a_refusal() {
    let many = (0..200)
        .map(|n| format!("/BB/S{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let st = state(Filter {
        scope_q: Some(many.clone()),
        ..Filter::default()
    })
    .await;
    seed_scoped(&st).await;
    let (code, pd) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/entities?type=Vehicle&scopeQ={many}"),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{pd}");
    assert_eq!(pd["type"], antares_api::policy::ACCESS_DENIED_TYPE);
    assert_eq!(pd["detail"], antares_api::policy::SCOPE_NOT_NARROWABLE);
}

/// 5.7.2.4 rechecks the filters on the merged result — "the filters shall be
/// rechecked before returning results" — and the narrowing is one of them.
/// A Context Source that answers with more than the forwarded query asked
/// for (its own bug, or its own reading of the filter) must not widen what
/// the subject sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_ignores_the_forwarded_narrowing_still_does_not_widen_it() {
    use std::io::{Read, Write};

    let body = json!([{
        "id": "urn:ngsi-ld:Vehicle:remote",
        "type": "Vehicle",
        "speed": {"type": "Property", "value": 90},
        "colour": {"type": "Property", "value": "red"},
    }])
    .to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // what the peer was actually asked, so the forward is read rather than
    // assumed from the answer
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            if let Ok(mut v) = recorder.lock() {
                v.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
            let _ = s.write_all(reply.as_bytes());
        }
    });

    // both brokers register the same peer: one narrowing, one not, so the
    // assertion below cannot pass because the peer was never reached
    let wide = state(Filter::default()).await;
    let st = state(slow()).await;
    for st in [&wide, &st] {
        let (code, _) = send(
            st,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(json!({
                "id": "urn:ngsi-ld:ContextSourceRegistration:policy-peer",
                "type": "ContextSourceRegistration",
                "mode": "inclusive",
                "operations": ["queryEntity"],
                "information": [{"entities": [{"type": "Vehicle"}]}],
                "endpoint": format!("http://127.0.0.1:{port}"),
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
    }

    // draining, so each read below sees only the forwards its own query made
    let asked = || -> Vec<String> { seen.lock().expect("lock").drain(..).collect() };

    let (code, list) = send(&wide, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert!(
        ids(&list).contains(&"urn:ngsi-ld:Vehicle:remote".to_owned()),
        "the peer was never reached, so the assertions below prove nothing: {list}"
    );
    let unnarrowed = asked();
    assert!(
        !unnarrowed.is_empty() && unnarrowed.iter().all(|r| !r.contains("speed")),
        "an unnarrowed query carried a condition nobody asked for: {unnarrowed:?}"
    );

    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    // 4.3.6.1: the narrowing travels on the forward, so the peer is asked
    // the narrowed question rather than trusted to answer the wide one well
    let narrowed = asked();
    assert!(
        !narrowed.is_empty() && narrowed.iter().all(|r| r.contains("speed")),
        "the narrowing did not reach the peer: {narrowed:?}"
    );
    // and a peer that answers outside it anyway is still filtered
    assert!(
        !ids(&list).contains(&"urn:ngsi-ld:Vehicle:remote".to_owned()),
        "a peer's answer outside the narrowing was served: {list}"
    );
}

/// A policy name is the deployment's, not the caller's. 4.21 names a member,
/// and which member a name IS depends on the `@context` it is read in —
/// `Context::expand_key` consults the term map before it decides a name is
/// already an IRI, so a request that binds the rule's term (or the rule's
/// own IRI, as a term) in its own inline `@context` would otherwise move the
/// rule onto a member the document does not have and remove nothing. 5.5.7
/// makes the Fully Qualified Name the identity of an Attribute; the seam
/// reads its rules in the core `@context`, where a caller cannot reach.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_context_cannot_move_a_policy_name_off_its_target() {
    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    let (status, _) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({
            "id": "urn:ngsi-ld:Vehicle:decoy",
            "type": "Vehicle",
            "colour": {"type": "Property", "value": "red"},
            "speed": {"type": "Property", "value": 10}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // the caller binds the very term the rule names to somewhere else; an
    // inline @context travels with ld+json (6.3.5), so the request is built
    // here rather than through the json helper above
    let payload = json!({
        "@context": {"colour": "http://decoy.invalid/colour"},
        "type": "Query",
        "entities": [{"type": "Vehicle"}]
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entityOperations/query")
        .header("Content-Type", "application/ld+json")
        .header("Content-Length", payload.len())
        .body(Body::from(payload))
        .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = body.to_string();
    assert!(
        !text.contains("red"),
        "the policy still removes the member it names: {text}"
    );
    assert!(
        text.contains("urn:ngsi-ld:Vehicle:decoy"),
        "the Entity itself is still answered: {text}"
    );
}

/// 4.5.23 Linked Entity Retrieval answers Entities the request never named:
/// `join=flat` appends every Entity reached over a Relationship. A 4.21
/// projection is written per level — a bare `omit` name leaves that member
/// alone on the joined document, which is what the request asked for — but a
/// narrowing is not a representation choice. A member the subject may not
/// see is not less hidden one hop away, so the policy projection travels
/// down the walk while the request's own does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_join_does_not_carry_a_narrowed_member_out_of_the_answer() {
    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    for (id, kind, colour, rel) in [
        (
            "urn:ngsi-ld:Vehicle:joined",
            "Vehicle",
            "red",
            Some("urn:ngsi-ld:Person:owner"),
        ),
        ("urn:ngsi-ld:Person:owner", "Person", "blue", None),
    ] {
        let mut doc = json!({
            "id": id,
            "type": kind,
            "colour": {"type": "Property", "value": colour}
        });
        if let Some(target) = rel {
            doc["owner"] = json!({"type": "Relationship", "object": target});
        }
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }

    let (status, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Vehicle&join=flat&joinLevel=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = body.to_string();
    assert!(
        text.contains("urn:ngsi-ld:Person:owner"),
        "the linked Entity is still answered: {text}"
    );
    assert!(
        !text.contains("red") && !text.contains("blue"),
        "the narrowed member is gone from BOTH documents: {text}"
    );
}

/// A Vehicle linking one visible and one hidden Person, and the visible
/// Person linking a hidden Organization one level further.
async fn seed_linked(st: &AppState, hidden: Value, shown: Value) {
    let docs = [
        json!({"id": "urn:ngsi-ld:Vehicle:v", "type": "Vehicle",
               "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:hidden"},
               "driver": {"type": "Relationship", "object": "urn:ngsi-ld:Person:shown"}}),
        json!({"id": "urn:ngsi-ld:Person:hidden", "type": "Person", "name": {"type": "Property", "value": "Hid"}}),
        json!({"id": "urn:ngsi-ld:Person:shown", "type": "Person", "name": {"type": "Property", "value": "Sho"},
               "employer": {"type": "Relationship", "object": "urn:ngsi-ld:Org:hidden"}}),
        json!({"id": "urn:ngsi-ld:Org:hidden", "type": "Org", "name": {"type": "Property", "value": "Acme"}}),
    ];
    for mut doc in docs {
        let marks = if doc["id"].as_str().is_some_and(|i| i.ends_with(":hidden")) {
            &hidden
        } else {
            &shown
        };
        for (k, v) in marks.as_object().into_iter().flatten() {
            doc[k] = v.clone();
        }
        let (code, body) = send(st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
}

/// 5.7.1.4/5.7.2.4 and 4.5.23: a joined Entity is read the way the
/// Entity itself is, so one outside the narrowing is absent from the join
/// exactly as it is absent from a query; the Relationship stays.
async fn assert_join_hides(st: &AppState) {
    for path in [
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:v",
        "/ngsi-ld/v1/entities?type=Vehicle",
    ] {
        for join in ["inline", "flat"] {
            for level in [1, 2] {
                let sep = if path.contains('?') { '&' } else { '?' };
                let uri = format!("{path}{sep}join={join}&joinLevel={level}");
                let (status, body) = send(st, "GET", &uri, None).await;
                assert_eq!(status, StatusCode::OK, "{uri}: {body}");
                let text = body.to_string();
                assert!(
                    text.contains("Sho"),
                    "{uri}: allowed target embedded: {text}"
                );
                assert!(
                    text.contains("urn:ngsi-ld:Person:hidden"),
                    "{uri}: the Relationship itself stays: {text}"
                );
                assert!(!text.contains("Hid"), "{uri}: hidden target leaked: {text}");
                assert!(
                    !text.contains("Acme"),
                    "{uri}: hidden level-2 target leaked: {text}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_does_not_embed_a_target_the_q_narrowing_hides() {
    let st = state(Filter {
        q: Some(Box::new(antares_ql::parse_q("public==true").expect("q"))),
        ..Filter::default()
    })
    .await;
    seed_linked(
        &st,
        json!({"public": {"type": "Property", "value": false}}),
        json!({"public": {"type": "Property", "value": true}}),
    )
    .await;
    assert_join_hides(&st).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_does_not_embed_a_target_the_scope_narrowing_hides() {
    let st = state(Filter {
        scope_q: Some("/open/#".into()),
        ..Filter::default()
    })
    .await;
    seed_linked(
        &st,
        json!({"scope": "/closed"}),
        json!({"scope": "/open/a"}),
    )
    .await;
    assert_join_hides(&st).await;
}

/// Two Assets owned by two People: one public and one not, both earning.
async fn seed_owned(st: &AppState) {
    for doc in [
        json!({"id": "urn:ngsi-ld:Person:open", "type": "Person",
               "public": {"type": "Property", "value": true},
               "name": {"type": "Property", "value": "x"},
               "salary": {"type": "Property", "value": 90000}}),
        json!({"id": "urn:ngsi-ld:Person:shut", "type": "Person",
               "public": {"type": "Property", "value": false},
               "name": {"type": "Property", "value": "x"},
               "salary": {"type": "Property", "value": 90000}}),
        json!({"id": "urn:ngsi-ld:Asset:a", "type": "Asset",
               "public": {"type": "Property", "value": true},
               "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:open"}}),
        json!({"id": "urn:ngsi-ld:Asset:b", "type": "Asset",
               "public": {"type": "Property", "value": true},
               "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:shut"}}),
        json!({"id": "urn:ngsi-ld:Asset:dangling", "type": "Asset",
               "public": {"type": "Property", "value": true},
               "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:gone"}}),
    ] {
        let (code, body) = send(st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
}

fn asset_query(q: &str) -> String {
    format!(
        "/ngsi-ld/v1/entities?type=Asset&join=inline&joinLevel=1&q={}",
        q.replace('{', "%7B")
            .replace('}', "%7D")
            .replace('"', "%22")
            .replace('>', "%3E")
    )
}

/// 7.2.3.2 lets a Query Term follow a Relationship; a hop into an Entity
/// the narrowing hides reads the way a hop into a missing one does, so the
/// term cannot test a value the subject may not see (T-3474).
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_q_term_does_not_read_an_entity_the_narrowing_hides() {
    let st = state(Filter {
        q: Some(Box::new(antares_ql::parse_q("public==true").expect("q"))),
        ..Filter::default()
    })
    .await;
    seed_owned(&st).await;
    assert_eq!(
        sorted_ids(&st, &asset_query("owner{salary}>0")).await,
        ["urn:ngsi-ld:Asset:a"],
        "the hidden owner's salary selected its Asset"
    );
    // hidden reads as absent: the dangling and the hidden hop answer alike
    assert_eq!(
        sorted_ids(&st, &asset_query("!owner{name}")).await,
        ["urn:ngsi-ld:Asset:b", "urn:ngsi-ld:Asset:dangling"],
    );
}

/// ADR-0023 rule 2: an Attribute the narrowing omits does not exist while
/// q is evaluated, one hop away as much as on the Entity itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_q_term_does_not_read_an_attribute_the_narrowing_omits() {
    let st = state(Filter {
        omit: vec!["https://uri.etsi.org/ngsi-ld/default-context/salary".into()],
        ..Filter::default()
    })
    .await;
    seed_owned(&st).await;
    assert!(
        sorted_ids(&st, &asset_query("owner{salary}>0"))
            .await
            .is_empty(),
        "an omitted Attribute was tested through a Relationship"
    );
    assert_eq!(
        sorted_ids(&st, &asset_query("owner{name}==%22x%22")).await,
        ["urn:ngsi-ld:Asset:a", "urn:ngsi-ld:Asset:b"],
        "a visible linked Attribute still selects"
    );
}

/// ADR-0023 rule 5: discovery (5.7.5 to 5.7.10) answers from the Entities
/// the narrowing selects, with `omit` read as type and Attribute names. A
/// hidden type is absent from every list and `/types/{t}` is 404; a hidden
/// Attribute is absent from the lists, from attributeDetails, and
/// `/attributes/{a}` is 404 (T-3479).
#[tokio::test(flavor = "multi_thread")]
async fn discovery_answers_from_what_the_narrowing_selects() {
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
    let st = state(Filter {
        q: Some(Box::new(antares_ql::parse_q("public==true").expect("q"))),
        omit: vec![format!("{DC}Secret"), format!("{DC}salary")],
        restricted: true,
        ..Filter::default()
    })
    .await;
    for doc in [
        json!({"id": "urn:ngsi-ld:Vehicle:a", "type": "Vehicle",
               "public": {"type": "Property", "value": true},
               "speed": {"type": "Property", "value": 1},
               "salary": {"type": "Property", "value": 2}}),
        json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret",
               "public": {"type": "Property", "value": true},
               "code": {"type": "Property", "value": 3}}),
        json!({"id": "urn:ngsi-ld:Car:c", "type": "Car",
               "public": {"type": "Property", "value": false},
               "colour": {"type": "Property", "value": "red"}}),
    ] {
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let get = |uri: &'static str| {
        let st = st.clone();
        async move { send(&st, "GET", uri, None).await }
    };
    let (code, body) = get("/ngsi-ld/v1/types").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["typeList"], json!(["Vehicle"]), "{body}");
    let (_, body) = get("/ngsi-ld/v1/types?details=true").await;
    assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(
        body[0]["attributeNames"],
        json!(["public", "speed"]),
        "{body}"
    );
    let (code, body) = get("/ngsi-ld/v1/types/Vehicle").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("salary"), "{body}");
    for hidden in ["/ngsi-ld/v1/types/Secret", "/ngsi-ld/v1/types/Car"] {
        assert_eq!(get(hidden).await.0, StatusCode::NOT_FOUND, "{hidden}");
    }
    let (_, body) = get("/ngsi-ld/v1/attributes").await;
    assert_eq!(body["attributeList"], json!(["public", "speed"]), "{body}");
    let (_, body) = get("/ngsi-ld/v1/attributes?details=true").await;
    let text = body.to_string();
    assert!(
        !text.contains("salary") && !text.contains("Secret") && !text.contains("Car"),
        "{text}"
    );
    let (code, body) = get("/ngsi-ld/v1/attributes/speed").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["typeNames"], json!(["Vehicle"]), "{body}");
    for hidden in [
        "/ngsi-ld/v1/attributes/salary",
        "/ngsi-ld/v1/attributes/colour",
        "/ngsi-ld/v1/attributes/code",
    ] {
        assert_eq!(get(hidden).await.0, StatusCode::NOT_FOUND, "{hidden}");
    }
    assert_eq!(
        restricted(&st, "/ngsi-ld/v1/types").await.as_deref(),
        Some("true"),
        "a narrowed list says so"
    );
}

/// Four Vehicles on the equator, one per degree of longitude 0..3.
async fn seed_placed(st: &AppState) {
    for lon in 0..4 {
        let id = format!("urn:ngsi-ld:Vehicle:at{lon}");
        let (code, _) = send(
            st,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(json!({
                "id": id, "type": "Vehicle",
                "location": {"type": "GeoProperty",
                             "value": {"type": "Point", "coordinates": [lon, 0]}},
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "seed {id}");
    }
}

/// A `within` box from longitude `west` to `east`, 1 degree either side of
/// the equator, as the three 4.10 query parameters.
fn within_box(west: f64, east: f64) -> [(&'static str, String); 3] {
    [
        ("georel", "within".to_owned()),
        ("geometry", "Polygon".to_owned()),
        (
            "coordinates",
            json!([[
                [west, -1.0],
                [east, -1.0],
                [east, 1.0],
                [west, 1.0],
                [west, -1.0]
            ]])
            .to_string(),
        ),
    ]
}

fn box_query(west: f64, east: f64) -> String {
    within_box(west, east)
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencoding(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// ADR-0023 rule 1: a narrowing by area joins the request's own geoquery.
/// 7.2.5 lets a request carry one, so the store query holds both and every
/// one of them must hold: the caller's area cannot move the policy's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_geo_q_narrows_and_a_request_geo_q_still_applies() {
    let area = |west, east| {
        let params = within_box(west, east)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        antares_ql::geo::GeoQuery::from_params(&params)
            .expect("valid geoquery")
            .expect("a geoquery")
    };
    let at = |lons: &[u8]| -> Vec<String> {
        lons.iter()
            .map(|l| format!("urn:ngsi-ld:Vehicle:at{l}"))
            .collect()
    };

    // the request's area alone
    let wide = state(Filter::default()).await;
    seed_placed(&wide).await;
    let request = box_query(0.5, 2.5);
    assert_eq!(
        sorted_ids(
            &wide,
            &format!("/ngsi-ld/v1/entities?type=Vehicle&{request}")
        )
        .await,
        at(&[1, 2])
    );

    // the policy's area alone: longitudes 0 and 1
    let st = state(Filter {
        geo_q: Some(Box::new(area(-0.5, 1.5))),
        ..Filter::default()
    })
    .await;
    seed_placed(&st).await;
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Vehicle").await,
        at(&[0, 1])
    );

    // both: only longitude 1, and the count says so
    let uri = format!("/ngsi-ld/v1/entities?type=Vehicle&count=true&{request}");
    assert_eq!(sorted_ids(&st, &uri).await, at(&[1]));
    let count = antares_api::router(st.clone())
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    assert_eq!(count.as_deref(), Some("1"));

    // an area of the caller's own outside the policy's widens nothing
    assert!(sorted_ids(
        &st,
        &format!("/ngsi-ld/v1/entities?type=Vehicle&{}", box_query(1.5, 3.5))
    )
    .await
    .is_empty());

    // the batch query and the retrieve are narrowed alike
    let (code, list) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Vehicle"}],
                    "geoQ": {"geometry": "Polygon", "georel": "within",
                             "coordinates": [[[0.5, -1.0], [2.5, -1.0], [2.5, 1.0], [0.5, 1.0], [0.5, -1.0]]]}})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), at(&[1]), "{list}");
    let (code, _) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:at1",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (code, pd) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:at2",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{pd}");
}

/// 5.7.4 reads the same Entities over time under the same area.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_geo_q_narrows_the_temporal_query() {
    let params = within_box(-0.5, 1.5)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let st = state(Filter {
        geo_q: antares_ql::geo::GeoQuery::from_params(&params)
            .expect("valid geoquery")
            .map(Box::new),
        ..Filter::default()
    })
    .await;
    for lon in 0..4 {
        let id = format!("urn:ngsi-ld:Vehicle:at{lon}");
        let (code, _) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            Some(json!({
                "id": id, "type": "Vehicle",
                "location": [{"type": "GeoProperty",
                              "value": {"type": "Point", "coordinates": [lon, 0]},
                              "observedAt": "2026-01-01T00:00:00Z"}],
            })),
        )
        .await;
        assert!(
            code == StatusCode::CREATED || code == StatusCode::NO_CONTENT,
            "seed {id}: {code}"
        );
    }
    let base =
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z";
    assert_eq!(
        sorted_ids(&st, base).await,
        ["urn:ngsi-ld:Vehicle:at0", "urn:ngsi-ld:Vehicle:at1"]
    );
    // the retrieve of one held only temporally reads the same area
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:at1",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:at3",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        sorted_ids(&st, &format!("{base}&{}", box_query(0.5, 3.5))).await,
        ["urn:ngsi-ld:Vehicle:at1"]
    );
}

const SALARY: &str = "https://uri.etsi.org/ngsi-ld/default-context/salary";

/// ADR-0023 rule 2 on the Entity itself (T-3482): an Attribute the
/// narrowing omits does not exist while q is evaluated, so a comparison on
/// it selects nothing and its absence selects every Entity. The caller
/// cannot learn the hidden value one comparison at a time.
#[tokio::test(flavor = "multi_thread")]
async fn a_q_term_does_not_read_an_attribute_the_narrowing_omits() {
    let st = state(Filter {
        omit: vec![SALARY.into()],
        ..Filter::default()
    })
    .await;
    seed_owned(&st).await;
    let persons = ["urn:ngsi-ld:Person:open", "urn:ngsi-ld:Person:shut"];
    for q in [
        "salary%3E0",
        "salary==90000",
        "salary",
        "salary%3E0|salary%3C0",
    ] {
        assert!(
            sorted_ids(&st, &format!("/ngsi-ld/v1/entities?type=Person&q={q}"))
                .await
                .is_empty(),
            "q={q} read an omitted Attribute"
        );
    }
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Person&q=!salary").await,
        persons,
        "an omitted Attribute is absent from every Entity"
    );
    assert_eq!(
        sorted_ids(
            &st,
            "/ngsi-ld/v1/entities?type=Person&q=salary%3E0|name==%22x%22"
        )
        .await,
        persons,
        "a visible term beside a hidden one still selects"
    );
    assert!(
        sorted_ids(
            &st,
            "/ngsi-ld/v1/entities?type=Person&q=name==%22x%22;salary.amount%3E0"
        )
        .await
        .is_empty(),
        "a sub-attribute path of an omitted Attribute is as hidden as the Attribute"
    );
}

/// The same under a pick that leaves the Attribute out.
#[tokio::test(flavor = "multi_thread")]
async fn a_q_term_does_not_read_an_attribute_the_narrowing_does_not_pick() {
    let st = state(Filter {
        pick: vec!["name".into(), "public".into()],
        ..Filter::default()
    })
    .await;
    seed_owned(&st).await;
    assert!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Person&q=salary%3E0")
            .await
            .is_empty(),
        "an Attribute the pick leaves out was compared"
    );
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Person&q=public==true").await,
        ["urn:ngsi-ld:Person:open"],
        "a picked Attribute still selects"
    );
}

/// The batch query (5.7.2 over POST) and the temporal query read the same
/// rewritten q.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_q_term_selects_nothing_in_the_batch_and_temporal_queries() {
    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (code, list) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Vehicle"}], "q": "colour==\"red\""})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert!(
        ids(&list).is_empty(),
        "the batch query compared an omitted Attribute: {list}"
    );

    let st = state(Filter {
        omit: vec!["colour".into()],
        ..Filter::default()
    })
    .await;
    seed_history(&st).await;
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Vehicle&q=colour==%22red%22&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert!(
        ids(&list).is_empty(),
        "the temporal query compared an omitted Attribute: {list}"
    );
}

/// AllowAll and a narrowing that hides nothing the q names answer as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_q_term_on_a_visible_attribute_answers_as_before() {
    let st = state(Filter {
        omit: vec!["name".into()],
        ..Filter::default()
    })
    .await;
    seed_owned(&st).await;
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Person&q=salary%3E0").await,
        ["urn:ngsi-ld:Person:open", "urn:ngsi-ld:Person:shut"],
    );
    let st = state(Filter::default()).await;
    seed_owned(&st).await;
    assert_eq!(
        sorted_ids(&st, "/ngsi-ld/v1/entities?type=Person&q=salary%3E0").await,
        ["urn:ngsi-ld:Person:open", "urn:ngsi-ld:Person:shut"],
    );
}

fn enc(q: &str) -> String {
    q.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The status and the problem title a caller `q` meets on the entity, the
/// temporal and the batch query.
async fn verdicts(st: &AppState, q: &str) -> Vec<(StatusCode, Value)> {
    let mut out = Vec::new();
    for uri in [
        format!("/ngsi-ld/v1/entities?type=Vehicle&q={}", enc(q)),
        format!(
            "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z&q={}",
            enc(q)
        ),
    ] {
        let (code, doc) = send(st, "GET", &uri, None).await;
        out.push((code, doc.get("title").cloned().unwrap_or(Value::Null)));
    }
    let (code, doc) = send(
        st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Vehicle"}], "q": q})),
    )
    .await;
    out.push((code, doc.get("title").cloned().unwrap_or(Value::Null)));
    out
}

/// `MAX_Q_NODES` (512) counts the caller's `q` alone through a narrowing
/// engine (threat V23): with the engine's own `q` joined and a term on the
/// Attribute it omits rewritten as `(colour;!colour)`, a `q` meets the
/// verdict it meets with no engine at all, at the cap and one past it, so
/// the cap is an oracle neither on the engine's predicate nor on "hidden".
#[tokio::test]
async fn the_q_node_cap_counts_the_callers_q_alone() {
    const MAX_Q_NODES: usize = 512;
    let open = AppState::new("me".into());
    let narrowed = state(Filter {
        omit: vec!["colour".into()],
        ..slow()
    })
    .await;
    seed(&open).await;
    seed(&narrowed).await;
    for (term, terms) in [
        ("a==1", MAX_Q_NODES - 1),
        ("a==1", MAX_Q_NODES),
        ("colour==\"red\"", 170),
        ("colour==\"red\"", 270),
    ] {
        let q = vec![term; terms].join("|");
        let want = verdicts(&open, &q).await;
        let ok = terms < MAX_Q_NODES;
        assert!(
            want.iter().all(|(code, _)| code.is_success() == ok),
            "{terms} x {term} with no engine: {want:?}"
        );
        assert_eq!(verdicts(&narrowed, &q).await, want, "{terms} x {term}");
    }
}

/// The share of the narrowed caps an engine's `q` may take is judged on
/// the engine's `q` alone: past it every caller meets the same refusal,
/// the one with no `q` as much as the one with the longest.
#[tokio::test]
async fn an_engine_q_past_its_share_is_refused_whatever_the_caller_asks() {
    let leaf = antares_ql::parse_q("speed<50").expect("q");
    let st = state(Filter {
        q: Some(Box::new(antares_ql::QNode::Or(vec![leaf; 4096]))),
        ..Filter::default()
    })
    .await;
    let none = send(&st, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert!(!none.0.is_success(), "{none:?}");
    for q in ["speed==1".to_owned(), vec!["a==1"; 511].join("|")] {
        let (code, doc) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/entities?type=Vehicle&q={}", enc(&q)),
            None,
        )
        .await;
        assert_eq!(
            (code, doc.get("title")),
            (none.0, none.1.get("title")),
            "{doc} vs {}",
            none.1
        );
    }
}

/// The `q` parameter of a request line, percent-decoded.
fn forwarded_q(request: &str) -> Option<String> {
    let target = request.split_whitespace().nth(1)?;
    let query = target.split_once('?')?.1;
    let raw = query.split('&').find_map(|kv| kv.strip_prefix("q="))?;
    let mut out = Vec::new();
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'%' => {
                let hex: String = bytes.by_ref().take(2).map(char::from).collect();
                out.push(u8::from_str_radix(&hex, 16).ok()?);
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
    }
    String::from_utf8(out).ok()
}

/// A peer with Antares's own `q` caps (threat V23 across federation,
/// T-3621): it answers 400 TooComplexQuery to a forwarded `q` its strict
/// parser refuses, else one Entity both the caller's `q` and the engine's
/// select. Returns its port and the `q`s it was asked.
fn strict_peer() -> (u16, Arc<std::sync::Mutex<Vec<Option<String>>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen: Arc<std::sync::Mutex<Vec<Option<String>>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 8192];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&request);
            let temporal = request.contains("/temporal/entities");
            let map = request.contains("/entityMaps");
            let q = forwarded_q(&request);
            let refused = q
                .as_deref()
                .is_some_and(|q| antares_ql::parse_q(q).is_err());
            if let Ok(mut v) = recorder.lock() {
                v.push(q);
            }
            let (status, body) = if refused {
                (
                    "400 Bad Request",
                    json!({
                        "type": "https://uri.etsi.org/ngsi-ld/errors/TooComplexQuery",
                        "title": "q expression exceeds 512 nodes",
                    }),
                )
            } else if map {
                (
                    "200 OK",
                    json!({
                        "id": "urn:ngsi-ld:entitymap:strict-peer",
                        "type": "EntityMap",
                        "expiresAt": "2999-01-01T00:00:00Z",
                        "entityMap": {
                            "urn:ngsi-ld:Vehicle:remote": ["@none"],
                            "urn:ngsi-ld:Vehicle:fast": ["@none"],
                        },
                        "linkedMaps": {},
                    }),
                )
            } else if temporal {
                let at = "2026-01-01T00:00:00Z";
                (
                    "200 OK",
                    json!([{
                        "id": "urn:ngsi-ld:Vehicle:remote",
                        "type": "Vehicle",
                        "a": [{"type": "Property", "value": 1, "observedAt": at}],
                        "speed": [{"type": "Property", "value": 30, "observedAt": at}],
                    }]),
                )
            } else {
                let mut found = vec![json!({
                    "id": "urn:ngsi-ld:Vehicle:remote",
                    "type": "Vehicle",
                    "a": {"type": "Property", "value": 1},
                    "speed": {"type": "Property", "value": 30},
                })];
                // the Entity only an EntityMap names: read back by id
                if request.contains("Vehicle%3Afast") || request.contains("Vehicle:fast") {
                    found.push(json!({
                        "id": "urn:ngsi-ld:Vehicle:fast",
                        "type": "Vehicle",
                        "a": {"type": "Property", "value": 1},
                        "speed": {"type": "Property", "value": 90},
                    }));
                }
                ("200 OK", json!(found))
            };
            let body = body.to_string();
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(reply.as_bytes());
        }
    });
    (port, seen)
}

/// T-3621, threat V23 across federation: a caller `q` under the cap,
/// conjoined with the engine's, would be refused by a peer that counts the
/// forwarded `q` with its own strict caps. The caller would then see a
/// warning and a missing Entity it sees with no engine, which tells it the
/// engine's predicate is there and how large. The answer through the
/// engine is the answer without it: the peer's Entity, no warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_narrowed_q_past_a_peers_cap_answers_as_the_callers_q_alone() {
    let (port, seen) = strict_peer();
    let open = state(Filter::default()).await;
    let narrowed = state(slow()).await;
    for st in [&open, &narrowed] {
        let (code, doc) = send(
            st,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(json!({
                "id": "urn:ngsi-ld:ContextSourceRegistration:strict-peer",
                "type": "ContextSourceRegistration",
                "mode": "inclusive",
                "operations": ["queryEntity", "queryTemporal"],
                "information": [{"entities": [{"type": "Vehicle"}]}],
                "endpoint": format!("http://127.0.0.1:{port}"),
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{doc}");
    }
    let q = vec!["a==1"; antares_ql::MAX_Q_NODES - 1].join("|");
    for uri in [
        format!("/ngsi-ld/v1/entities?type=Vehicle&q={}", enc(&q)),
        format!(
            "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=1970-01-01T00:00:00Z&q={}",
            enc(&q)
        ),
    ] {
        same_answer_through_the_engine(&open, &narrowed, &uri).await;
    }
    // the forwards: the caller's own q with no engine; through the engine
    // nothing the peer refuses
    let asked: Vec<Option<String>> = seen.lock().unwrap().drain(..).collect();
    assert_eq!(asked.len(), 4, "every read reached the peer: {asked:?}");
    assert!(
        asked
            .iter()
            .all(|q| q.as_deref().is_none_or(|q| antares_ql::parse_q(q).is_ok())),
        "a forward the peer refuses"
    );
}

async fn same_answer_through_the_engine(open: &AppState, narrowed: &AppState, uri: &str) {
    let path = uri.split('?').next().unwrap_or(uri);
    let read = |st: &AppState| {
        let st = st.clone();
        let uri = uri.to_owned();
        async move {
            let res = antares_api::router(st)
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let warning = res.headers().get("ngsild-warning").cloned();
            let body = res.into_body().collect().await.unwrap().to_bytes();
            let list: Value = serde_json::from_slice(&body).unwrap();
            (ids(&list), warning)
        }
    };
    let want = read(open).await;
    assert_eq!(
        want,
        (vec!["urn:ngsi-ld:Vehicle:remote".to_owned()], None),
        "{path}: with no engine the peer serves the caller's q"
    );
    assert_eq!(
        read(narrowed).await,
        want,
        "{path}: the engine's q changed the answer"
    );
}

/// T-3683, threat V23 on 5.14.4: an EntityMap forward carries the `q` the
/// Query Entities forward does. A narrowed `q` past the peer's cap would
/// leave the peer out of the map, which no engine would do. Through the
/// engine the peer is asked without it and its map is narrowed here: the
/// Entity the narrowing selects and the peer's linked map, as with no
/// engine; the one it hides (speed 90) is not in the map; no warning; and
/// no forward is one the peer refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entity_map_under_a_narrowed_q_past_a_peers_cap_is_the_callers_map() {
    let (port, seen) = strict_peer();
    let open = state(Filter::default()).await;
    let narrowed = state(slow()).await;
    let reg = "urn:ngsi-ld:ContextSourceRegistration:strict-peer";
    for st in [&open, &narrowed] {
        let (code, doc) = send(
            st,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(json!({
                "id": reg,
                "type": "ContextSourceRegistration",
                "mode": "inclusive",
                "operations": ["queryEntity", "createEntityMapQueryEntity"],
                "information": [{"entities": [{"type": "Vehicle"}]}],
                "endpoint": format!("http://127.0.0.1:{port}"),
            })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{doc}");
    }
    let q = vec!["a==1"; antares_ql::MAX_Q_NODES - 1].join("|");
    let uri = format!("/ngsi-ld/v1/entityMaps?type=Vehicle&q={}", enc(&q));
    let map = |st: &AppState| {
        let st = st.clone();
        let uri = uri.clone();
        async move {
            let res = antares_api::router(st)
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = res.status();
            let warning = res.headers().get("ngsild-warning").cloned();
            let body = res.into_body().collect().await.unwrap().to_bytes();
            let doc: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            (
                status,
                doc["entityMap"].clone(),
                doc["linkedMaps"].clone(),
                warning,
            )
        }
    };
    let want = map(&open).await;
    assert_eq!(
        (want.1.clone(), want.2.clone()),
        (
            json!({"urn:ngsi-ld:Vehicle:remote": [reg], "urn:ngsi-ld:Vehicle:fast": [reg]}),
            json!({reg: "urn:ngsi-ld:entitymap:strict-peer"})
        ),
        "with no engine the peer maps the caller's q: {want:?}"
    );
    let got = map(&narrowed).await;
    assert_eq!(
        got,
        (
            want.0,
            json!({"urn:ngsi-ld:Vehicle:remote": [reg]}),
            want.2.clone(),
            None
        ),
        "the map through the engine is the caller's map, narrowed"
    );
    let asked: Vec<Option<String>> = seen.lock().unwrap().drain(..).collect();
    assert!(
        asked
            .iter()
            .all(|q| q.as_deref().is_none_or(|q| antares_ql::parse_q(q).is_ok())),
        "a forward the peer refuses: {} forwards",
        asked.len()
    );
}
