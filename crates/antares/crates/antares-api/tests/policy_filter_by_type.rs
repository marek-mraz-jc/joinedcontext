// SPDX-License-Identifier: EUPL-1.2
//! Attributes allowed per rule (ADR-0023 rule 11, gap G4, T-3528).
//!
//! A flat `pick` is one list for the whole answer: an Attribute allowed on
//! `Car` is served on `User` too, which is the platform's T-1862 leak ("a
//! projection's attributes are not per type, a query over two types
//! leaks"). `Filter::rules` lists `{types, condition, attributes}`: an
//! Entity is served when a rule holds for it and keeps the union of the
//! Attributes of the rules that hold, so a Car in zone A shows `speed` and
//! a Car elsewhere only `status`. Every document an answer carries is judged
//! by its own rules: the top level, a joined target, a temporal instance and
//! the Entities of a notification.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

/// Narrows every read the seam can narrow and every notification with the
/// same Filter; allows everything else, so a test seeds through the router.
struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "by-type"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
            Decision::Filter(std::sync::Arc::new(self.0.clone()))
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Filter(Box::new(self.0.clone()))
    }
}

/// `type → attributes` as an omit writes it: expanded IRIs.
fn by_type(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(t, attrs)| (iri(t), attrs.iter().map(|a| iri(a)).collect()))
        .collect()
}

/// One unconditional rule per type: the per-type map as rules.
fn rule(t: &str, attrs: &[&str]) -> Rule {
    Rule {
        types: vec![iri(t)],
        attributes: attrs.iter().map(|a| iri(a)).collect(),
        ..Rule::default()
    }
}

fn picks(entries: &[(&str, &[&str])]) -> Filter {
    Filter {
        rules: entries.iter().map(|(t, attrs)| rule(t, attrs)).collect(),
        ..Filter::default()
    }
}

async fn state(f: Filter) -> AppState {
    antares_jsonld::allow_private_egress(true);
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
            b.body(Body::from(payload)).unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let doc = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, doc)
}

async fn create(st: &AppState, doc: Value) {
    let (code, body) = send(st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
}

fn prop(v: Value) -> Value {
    json!({"type": "Property", "value": v})
}

/// A Car and a User that both carry `colour`, each with its own members.
async fn seed(st: &AppState) {
    create(
        st,
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
               "speed": prop(json!(80)), "colour": prop(json!("red")),
               "ownedBy": {"type": "Relationship", "object": "urn:ngsi-ld:User:1"}}),
    )
    .await;
    create(
        st,
        json!({"id": "urn:ngsi-ld:User:1", "type": "User",
               "name": prop(json!("Ana")), "email": prop(json!("ana@example.org")),
               "colour": prop(json!("blue"))}),
    )
    .await;
}

/// The member names of each Entity of a list, by id.
fn members(list: &Value) -> BTreeMap<String, Vec<String>> {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|e| {
            let mut keys: Vec<String> = e.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            (e["id"].as_str().unwrap().to_owned(), keys)
        })
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_types_one_view_each_sees_only_its_own_attributes() {
    let st = state(picks(&[("Car", &["speed"]), ("User", &["name"])])).await;
    seed(&st).await;
    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:1"],
        keys(&["id", "type", "speed"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name"]),
        "{list}"
    );

    // the retrieve and the batch query answer the same
    let (code, user) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:User:1", None).await;
    assert_eq!(code, StatusCode::OK, "{user}");
    assert_eq!(
        members(&json!([user]))["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name"])
    );
    let (code, list) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Car"}, {"type": "User"}]})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(members(&list), got);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_iri_visible_on_car_hidden_on_user() {
    let st = state(picks(&[("Car", &["colour"]), ("User", &["name"])])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:1"],
        keys(&["id", "type", "colour"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name"]),
        "{list}"
    );
}

/// Two vocabularies, one short name: the rule keys on the IRI, so the Car's
/// `name` from vocabulary A is shown and the `name` from vocabulary B on the
/// same Car is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_short_name_two_vocabularies_are_different_attributes() {
    let (a, b) = ("https://a.example.org/name", "https://b.example.org/name");
    let st = state(Filter {
        rules: vec![Rule {
            types: vec![iri("Car")],
            attributes: vec![a.to_owned()],
            ..Rule::default()
        }],
        ..Filter::default()
    })
    .await;
    create(
        &st,
        json!({"id": "urn:ngsi-ld:Car:2", "type": "Car",
               a: prop(json!("from a")), b: prop(json!("from b"))}),
    )
    .await;
    let (code, car) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:2", None).await;
    assert_eq!(code, StatusCode::OK, "{car}");
    assert_eq!(car[a]["value"], "from a", "{car}");
    assert!(
        car.get(b).is_none(),
        "the other vocabulary's name leaked: {car}"
    );
}

/// No rule holds for a User: it is out, as if absent, never "frame only".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_matching_no_rule_is_not_served() {
    let st = state(picks(&[("Car", &["speed"])])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:1"], "{list}");
    let (code, body) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:User:1", None).await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car,User&count=true&limit=0",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        count(&st, "/ngsi-ld/v1/entities?type=Car,User&count=true&limit=0").await,
        Some(1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_typed_entity_gets_union_of_selected_types_only() {
    let st = state(picks(&[("Car", &["speed"]), ("User", &["name"])])).await;
    create(
        &st,
        json!({"id": "urn:ngsi-ld:Thing:1", "type": ["Car", "User", "Robot"],
               "speed": prop(json!(5)), "name": prop(json!("R2")),
               "email": prop(json!("r2@example.org")), "serial": prop(json!("x"))}),
    )
    .await;
    let (code, doc) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Thing:1", None).await;
    assert_eq!(code, StatusCode::OK, "{doc}");
    assert_eq!(
        members(&json!([doc]))["urn:ngsi-ld:Thing:1"],
        keys(&["id", "type", "speed", "name"]),
        "{doc}"
    );
}

/// An omit is a promise that the Attribute is never served on that type:
/// a Car that is also a User loses what either type omits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omit_by_type_removes_what_any_of_the_types_omits() {
    let st = state(Filter {
        omit_by_type: by_type(&[("User", &["email"])]),
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    create(
        &st,
        json!({"id": "urn:ngsi-ld:Car:3", "type": ["Car", "User"],
               "speed": prop(json!(5)), "email": prop(json!("c@example.org"))}),
    )
    .await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name", "colour"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:Car:3"],
        keys(&["id", "type", "speed"]),
        "{list}"
    );
    assert!(
        got["urn:ngsi-ld:Car:1"].contains(&"speed".to_owned()),
        "{list}"
    );
}

/// 4.5.23 `join`: the target is judged by its own type, so the Car's
/// allowlist never reaches the User it links to, and the User's never
/// reaches back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn join_target_projected_by_its_own_rules() {
    let st = state(picks(&[
        ("Car", &["speed", "ownedBy", "email"]),
        ("User", &["name"]),
    ]))
    .await;
    seed(&st).await;
    for join in ["inline", "flat"] {
        let (code, body) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/entities?type=Car&join={join}&joinLevel=1"),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        let text = body.to_string();
        assert!(
            text.contains("Ana"),
            "{join}: the target was not embedded: {body}"
        );
        assert!(
            !text.contains("ana@example.org"),
            "{join}: the Car's allowlist served the User's email: {body}"
        );
        assert!(
            !text.contains("blue"),
            "{join}: the User's colour leaked: {body}"
        );
    }
}

/// 5.7.4: the history of each Entity is narrowed by its own type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temporal_answers_are_projected_per_type() {
    let st = state(picks(&[("Car", &["speed"]), ("User", &["name"])])).await;
    for (id, ty, attrs) in [
        ("urn:ngsi-ld:Car:t", "Car", vec!["speed", "colour"]),
        ("urn:ngsi-ld:User:t", "User", vec!["name", "colour"]),
    ] {
        let mut doc = json!({"id": id, "type": ty});
        for a in attrs {
            doc[a] = json!([{"type": "Property", "value": 1,
                             "observedAt": "2026-01-01T00:00:00Z"}]);
        }
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
        assert!(code.is_success(), "{body}");
    }
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Car,User&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:t"],
        keys(&["id", "type", "speed"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:User:t"],
        keys(&["id", "type", "name"]),
        "{list}"
    );
    let (code, user) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:User:t",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{user}");
    assert_eq!(
        members(&json!([user]))["urn:ngsi-ld:User:t"],
        keys(&["id", "type", "name"])
    );
}

/// A capture server yielding the body of each notification POST.
async fn capture() -> (String, tokio::sync::mpsc::Receiver<Value>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(8);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move |body: axum::body::Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx
                    .send(serde_json::from_slice(&body).unwrap_or(Value::Null))
                    .await;
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/notify"), rx)
}

/// 5.8.6 with `showChanges`: the notified User keeps its own allowlist,
/// `previousValue` included, and an Attribute only the Car may show never
/// travels on the User.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_previous_value_projected_per_type() {
    let st = state(picks(&[("Car", &["email"]), ("User", &["name"])])).await;
    seed(&st).await;
    let (uri, mut rx) = capture().await;
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some(
            json!({"type": "Subscription", "entities": [{"type": "User"}],
                    "notification": {"endpoint": {"uri": uri}, "showChanges": true}}),
        ),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:User:1/attrs",
        Some(json!({"name": prop(json!("Ana B")), "email": prop(json!("b@example.org"))})),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let mut seen = Vec::new();
    while let Ok(Some(n)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
    {
        let text = n.to_string();
        seen.push(n);
        if text.contains("Ana B") {
            break;
        }
    }
    let last = seen.last().cloned().unwrap_or(Value::Null);
    let text = last.to_string();
    assert!(
        text.contains("Ana B"),
        "no notification of the change: {seen:?}"
    );
    assert!(
        text.contains("previousValue"),
        "showChanges sent no previousValue: {last}"
    );
    assert!(
        !text.contains("example.org") && !text.contains("email"),
        "an Attribute allowed only on Car travelled on the User: {last}"
    );
}

/// The `NGSILD-Warning` of a GET, a header [`send`] does not return.
async fn warning(st: &AppState, uri: &str) -> Option<String> {
    antares_api::router(st.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .headers()
        .get("NGSILD-Warning")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn ids(list: &Value) -> Vec<String> {
    members(list).into_keys().collect()
}

/// `NGSILD-Results-Count` of a GET.
async fn count(st: &AppState, uri: &str) -> Option<u64> {
    antares_api::router(st.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// ADR-0023 rule 2 per Entity (T-1862): `colour` is visible on Car and
/// hidden on User, so to the client's `q`, `attrs` and `orderBy` a User has
/// no `colour` at all (7.2.3.3): `q=colour` never finds it, `q=!colour`
/// does, ordering puts it where an absent member goes, and no warning
/// tells the two cases apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn q_on_an_attribute_hidden_for_user_reads_it_as_absent() {
    let st = state(picks(&[("Car", &["speed", "colour"]), ("User", &["name"])])).await;
    seed(&st).await;
    let car = "urn:ngsi-ld:Car:1";
    let user = "urn:ngsi-ld:User:1";
    for (query, want) in [
        ("q=colour!=%22green%22", vec![car]),
        ("q=colour==%22blue%22", vec![]),
        ("q=colour", vec![car]),
        ("q=!colour", vec![user]),
        ("attrs=colour", vec![car]),
        ("pick=id,colour", vec![car, user]),
    ] {
        let uri = format!("/ngsi-ld/v1/entities?type=Car,User&{query}");
        let (code, list) = send(&st, "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{query}: {list}");
        assert_eq!(ids(&list), want, "{query}: {list}");
        assert_eq!(warning(&st, &uri).await, None, "{query}");
    }
    let (_, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car,User&q=colour&count=true&limit=0",
        None,
    )
    .await;
    assert_eq!(list, json!([]));
    assert_eq!(
        count(
            &st,
            "/ngsi-ld/v1/entities?type=Car,User&q=colour&count=true&limit=0"
        )
        .await,
        Some(1)
    );
    let (code, list) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/query",
        Some(json!({"type": "Query", "entities": [{"type": "Car"}, {"type": "User"}], "q": "colour"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), [car], "{list}");
    // a retrieve's `attrs` reads the shown Entity too: hidden is absent, 404
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:User:1?attrs=colour",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");
}

/// The engine may condition on what it hides: its own `q` is not the
/// client's read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engines_own_q_on_a_hidden_attribute_drops_nothing() {
    let st = state(Filter {
        q: Some(Box::new(antares_ql::parse_q("colour").unwrap())),
        ..picks(&[("Car", &["speed"]), ("User", &["name"])])
    })
    .await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:1"],
        keys(&["id", "type", "speed"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name"]),
        "{list}"
    );
}

/// 5.7.4: the temporal query applies the same rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_temporal_q_on_an_attribute_hidden_for_user_drops_user() {
    let st = state(picks(&[("Car", &["colour"]), ("User", &["name"])])).await;
    for (id, ty) in [("urn:ngsi-ld:Car:t", "Car"), ("urn:ngsi-ld:User:t", "User")] {
        let (code, body) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            Some(json!({"id": id, "type": ty,
                        "colour": [{"type": "Property", "value": "red", "observedAt": "2026-01-01T00:00:00Z"}],
                        "name": [{"type": "Property", "value": "n", "observedAt": "2026-01-01T00:00:00Z"}]})),
        )
        .await;
        assert!(code.is_success(), "{body}");
    }
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Car,User&q=colour==%22red%22&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:t"], "{list}");
}

/// The property, exhaustively over a small universe: rules `{Car, q=a1==1:
/// C}`, `{Car: D}` and `{User: U}` for every C, D, U over two Attributes,
/// and Entities typed Car, User, both or neither, with `a1` 1 or 2. No
/// answer holds an (Entity, Attribute) pair that no rule holding for the
/// Entity allows, and an Entity no rule holds for is not answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_answer_holds_an_entity_attribute_pair_no_matching_rule_allows() {
    let attrs = ["a2", "a3"];
    let pick = |mask: u8| -> Vec<&str> {
        attrs
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, a)| *a)
            .collect()
    };
    let universe: [(&[&str], i64); 6] = [
        (&["Car"], 1),
        (&["Car"], 2),
        (&["User"], 1),
        (&["Car", "User"], 1),
        (&["Car", "User"], 2),
        (&["Robot"], 1),
    ];
    for c in 0..4u8 {
        for d in 0..4u8 {
            for u in 0..4u8 {
                let (c_l, d_l, u_l) = (pick(c), pick(d), pick(u));
                let st = state(Filter {
                    rules: vec![
                        Rule {
                            q: Some(Box::new(antares_ql::parse_q("a1==1").unwrap())),
                            ..rule("Car", &c_l)
                        },
                        rule("Car", &d_l),
                        rule("User", &u_l),
                    ],
                    ..Filter::default()
                })
                .await;
                for (i, (types, a1)) in universe.iter().enumerate() {
                    let mut doc = json!({"id": format!("urn:ngsi-ld:T:{i}"), "type": types,
                                         "a1": prop(json!(a1))});
                    for a in attrs {
                        doc[a] = prop(json!(1));
                    }
                    create(&st, doc).await;
                }
                let (code, list) =
                    send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User,Robot", None).await;
                assert_eq!(code, StatusCode::OK, "{list}");
                let served = ids(&list);
                for (i, (types, _)) in universe.iter().enumerate() {
                    let id = format!("urn:ngsi-ld:T:{i}");
                    // the unconditional Car and User rules hold for every
                    // Entity of those types, whatever a1 is
                    let held = types.contains(&"Car") || types.contains(&"User");
                    assert_eq!(
                        served.contains(&id),
                        held,
                        "c={c_l:?} d={d_l:?} u={u_l:?}: {id}: {list}"
                    );
                }
                for e in list.as_array().unwrap() {
                    let i: usize = e["id"].as_str().unwrap()["urn:ngsi-ld:T:".len()..]
                        .parse()
                        .unwrap();
                    let (types, a1) = universe[i];
                    for k in e
                        .as_object()
                        .unwrap()
                        .keys()
                        .filter(|k| !["id", "type"].contains(&k.as_str()))
                    {
                        let k = k.as_str();
                        let allowed = (types.contains(&"Car")
                            && (d_l.contains(&k) || (a1 == 1 && c_l.contains(&k))))
                            || (types.contains(&"User") && u_l.contains(&k));
                        assert!(
                            allowed,
                            "c={c_l:?} d={d_l:?} u={u_l:?}: {k} served on {types:?} a1={a1}: {e}"
                        );
                    }
                }
            }
        }
    }
}

/// Discovery (6.25-6.27, FILTERABLE since T-3479) reads the per-type rule:
/// a type lists only its own Attributes, and an Attribute names only the
/// types it is visible on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_lists_each_types_own_attributes_only() {
    let st = state(picks(&[("Car", &["speed", "colour"]), ("User", &["name"])])).await;
    seed(&st).await;
    let get = |uri: &'static str| {
        let st = st.clone();
        async move { send(&st, "GET", uri, None).await }
    };
    let (code, body) = get("/ngsi-ld/v1/types?details=true").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let names: BTreeMap<String, Value> = body
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {body}"))
        .iter()
        .map(|t| {
            (
                t["typeName"].as_str().unwrap_or_default().to_owned(),
                t["attributeNames"].clone(),
            )
        })
        .collect();
    assert_eq!(names["Car"], json!(["colour", "speed"]), "{body}");
    assert_eq!(names["User"], json!(["name"]), "{body}");
    let (code, body) = get("/ngsi-ld/v1/types/User").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let text = body.to_string();
    assert!(
        !text.contains("colour") && !text.contains("email"),
        "{text}"
    );
    let (_, body) = get("/ngsi-ld/v1/types/Car").await;
    assert!(!body.to_string().contains("ownedBy"), "{body}");
    let (code, body) = get("/ngsi-ld/v1/attributes/colour").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["typeNames"], json!(["Car"]), "{body}");
    let (_, body) = get("/ngsi-ld/v1/attributes?details=true").await;
    let text = body.to_string();
    assert!(
        !text.contains("email") && !text.contains("ownedBy"),
        "{text}"
    );
    for hidden in [
        "/ngsi-ld/v1/attributes/email",
        "/ngsi-ld/v1/attributes/ownedBy",
    ] {
        assert_eq!(get(hidden).await.0, StatusCode::NOT_FOUND, "{hidden}");
    }
}

/// Zone A: the square (0,0)-(1,1); a Car at x 0.5 is inside, at x 5 not.
fn zone_a() -> Box<antares_ql::geo::GeoQuery> {
    let params = std::collections::HashMap::from([
        ("georel".to_owned(), "within".to_owned()),
        ("geometry".to_owned(), "Polygon".to_owned()),
        (
            "coordinates".to_owned(),
            "[[[0,0],[1,0],[1,1],[0,1],[0,0]]]".to_owned(),
        ),
    ]);
    Box::new(
        antares_ql::geo::GeoQuery::from_params(&params)
            .unwrap()
            .unwrap(),
    )
}

/// A rule written with short names, which the broker expands through the
/// core `@context` (`location` and `status` are core terms).
fn short_rule(t: &str, attrs: &[&str]) -> Rule {
    Rule {
        types: vec![t.to_owned()],
        attributes: attrs.iter().map(|a| (*a).to_owned()).collect(),
        ..Rule::default()
    }
}

/// The owner's example: `{Car in zone A: speed, location, status}`,
/// `{Car: status}`, `{User: name}`.
fn zones() -> Filter {
    Filter {
        rules: vec![
            Rule {
                geo_q: Some(zone_a()),
                ..short_rule("Car", &["speed", "location", "status"])
            },
            short_rule("Car", &["status"]),
            short_rule("User", &["name"]),
        ],
        ..Filter::default()
    }
}

fn point(x: f64) -> Value {
    json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 0.5]}})
}

/// Car `a` in zone A, Car `b` outside it, both fast; and a User.
async fn seed_zones(st: &AppState) {
    for (id, x, speed) in [
        ("urn:ngsi-ld:Car:a", 0.5, 120),
        ("urn:ngsi-ld:Car:b", 5.0, 130),
    ] {
        create(
            st,
            json!({"id": id, "type": "Car", "speed": prop(json!(speed)),
                   "status": prop(json!("ok")), "location": point(x)}),
        )
        .await;
    }
    create(
        st,
        json!({"id": "urn:ngsi-ld:User:1", "type": "User", "name": prop(json!("Ana")),
               "speed": prop(json!(5))}),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_type_two_zones_two_attribute_sets() {
    let st = state(zones()).await;
    seed_zones(&st).await;
    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:a"],
        keys(&["id", "type", "speed", "status", "location"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:Car:b"],
        keys(&["id", "type", "status"]),
        "{list}"
    );
    assert_eq!(
        got["urn:ngsi-ld:User:1"],
        keys(&["id", "type", "name"]),
        "{list}"
    );
    let (_, b) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:b", None).await;
    assert_eq!(
        members(&json!([b]))["urn:ngsi-ld:Car:b"],
        keys(&["id", "type", "status"])
    );
}

/// A Car that is also a User in zone A shows what all three rules allow.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_matching_two_rules_gets_the_union() {
    let st = state(zones()).await;
    create(
        &st,
        json!({"id": "urn:ngsi-ld:Thing:1", "type": ["Car", "User"], "speed": prop(json!(1)),
               "status": prop(json!("ok")), "name": prop(json!("n")),
               "serial": prop(json!("s")), "location": point(0.5)}),
    )
    .await;
    let (code, doc) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Thing:1", None).await;
    assert_eq!(code, StatusCode::OK, "{doc}");
    assert_eq!(
        members(&json!([doc]))["urn:ngsi-ld:Thing:1"],
        keys(&["id", "type", "speed", "status", "name", "location"]),
        "{doc}"
    );
}

/// `speed` exists for the client only on Cars in zone A: Car b's 130 is
/// never found, ordered by or counted, and `!speed` finds Car b.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn q_on_speed_never_matches_cars_outside_zone_a() {
    let st = state(zones()).await;
    seed_zones(&st).await;
    for (query, want) in [
        ("q=speed%3E100", vec!["urn:ngsi-ld:Car:a"]),
        ("q=speed%3E125", vec![]),
        ("q=speed", vec!["urn:ngsi-ld:Car:a"]),
        ("q=!speed", vec!["urn:ngsi-ld:Car:b"]),
        ("attrs=speed", vec!["urn:ngsi-ld:Car:a"]),
        (
            "georel=near%3BmaxDistance==1000000000&geometry=Point&coordinates=%5B5,0.5%5D",
            vec!["urn:ngsi-ld:Car:a"],
        ),
    ] {
        let uri = format!("/ngsi-ld/v1/entities?type=Car&{query}");
        let (code, list) = send(&st, "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{query}: {list}");
        assert_eq!(ids(&list), want, "{query}: {list}");
    }
    // ordering by speed: Car c outside zone A is the slowest stored, but to
    // the client it has no speed, so it sorts where an absent member does
    // (last ascending, first descending), never by its hidden 1
    create(
        &st,
        json!({"id": "urn:ngsi-ld:Car:c", "type": "Car", "speed": prop(json!(1)),
               "location": point(5.0)}),
    )
    .await;
    let order = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["id"].as_str().map(str::to_owned))
            .collect()
    };
    let (_, asc) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car&orderBy=speed",
        None,
    )
    .await;
    assert_eq!(order(&asc)[0], "urn:ngsi-ld:Car:a", "{asc}");
    let (_, desc) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car&orderBy=speed;desc",
        None,
    )
    .await;
    assert_eq!(
        order(&desc).last().map(String::as_str),
        Some("urn:ngsi-ld:Car:a"),
        "{desc}"
    );
}

/// Count and every page describe the rule union, never the stored set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn count_and_paging_consistent_with_rule_union() {
    let st = state(Filter {
        rules: vec![Rule {
            geo_q: Some(zone_a()),
            ..rule("Car", &["speed"])
        }],
        ..Filter::default()
    })
    .await;
    for i in 0..7 {
        let x = if i % 2 == 0 { 0.5 } else { 5.0 };
        create(
            &st,
            json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                   "speed": prop(json!(i)), "location": point(x)}),
        )
        .await;
    }
    let base = "/ngsi-ld/v1/entities?type=Car&count=true";
    assert_eq!(count(&st, &format!("{base}&limit=0")).await, Some(4));
    let mut seen = Vec::new();
    for offset in [0, 2, 4] {
        let uri = format!("{base}&limit=2&offset={offset}");
        assert_eq!(count(&st, &uri).await, Some(4), "{uri}");
        seen.extend(ids(&send(&st, "GET", &uri, None).await.1));
    }
    seen.sort();
    assert_eq!(
        seen,
        [
            "urn:ngsi-ld:Car:0",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:4",
            "urn:ngsi-ld:Car:6"
        ]
    );
}

/// T-4516: a count under conditional rules reads the store a page at a
/// time, never as one unpaged read: that one hit the store's 10 000-row
/// ceiling and answered TooManyResults for a dataset of a large type, while
/// the window and the total stay those of the rule union.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_count_under_conditional_rules_reads_the_store_in_pages() {
    let mut st = state(Filter {
        rules: vec![Rule {
            geo_q: Some(zone_a()),
            ..rule("Car", &["speed"])
        }],
        ..Filter::default()
    })
    .await;
    for i in 0..7 {
        let x = if i % 2 == 0 { 0.5 } else { 5.0 };
        create(
            &st,
            json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                   "speed": prop(json!(i)), "location": point(x)}),
        )
        .await;
    }
    let spy = Arc::new(crate::common::Double::passthrough(st.store.clone()));
    st.store = spy.clone();
    let base = "/ngsi-ld/v1/entities?type=Car&count=true&q=speed%3E1";
    assert_eq!(count(&st, &format!("{base}&limit=0")).await, Some(3));
    let uri = format!("{base}&limit=2&offset=1");
    assert_eq!(count(&st, &uri).await, Some(3), "{uri}");
    assert_eq!(
        ids(&send(&st, "GET", &uri, None).await.1),
        ["urn:ngsi-ld:Car:4", "urn:ngsi-ld:Car:6"]
    );
    assert_eq!(spy.unpaged_queries(), 0, "the count read the store unpaged");
}

/// The next-page `Link` of a GET, announced only when one exists.
async fn link(st: &AppState, uri: &str) -> Option<String> {
    antares_api::router(st.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .headers()
        .get_all("Link")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.contains("rel=\"next\""))
        .map(str::to_owned)
}

/// T-4562: a page without count under conditional rules keeps only the
/// client's window while it scans, never every match before it, and still
/// answers the right ids and a next Link only when more match.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_under_conditional_rules_keeps_only_its_window() {
    let mut st = state(Filter {
        rules: vec![Rule {
            geo_q: Some(zone_a()),
            ..rule("Car", &["speed"])
        }],
        ..Filter::default()
    })
    .await;
    for i in 0..9 {
        let x = if i % 2 == 0 { 0.5 } else { 5.0 };
        create(
            &st,
            json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                   "speed": prop(json!(i)), "location": point(x)}),
        )
        .await;
    }
    let spy = Arc::new(crate::common::Double::passthrough(st.store.clone()));
    st.store = spy.clone();
    let base = "/ngsi-ld/v1/entities?type=Car&q=speed%3E1&limit=2";
    let first = format!("{base}&offset=1");
    assert_eq!(
        ids(&send(&st, "GET", &first, None).await.1),
        ["urn:ngsi-ld:Car:4", "urn:ngsi-ld:Car:6"]
    );
    assert!(link(&st, &first).await.is_some(), "no next Link");
    let last = format!("{base}&offset=3");
    assert_eq!(
        ids(&send(&st, "GET", &last, None).await.1),
        ["urn:ngsi-ld:Car:8"]
    );
    assert_eq!(link(&st, &last).await, None);
    let past = format!("{base}&offset=9");
    assert!(ids(&send(&st, "GET", &past, None).await.1).is_empty());
    assert_eq!(spy.unpaged_queries(), 0, "the page read the store unpaged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn car_leaving_zone_a_loses_speed_on_next_read() {
    let st = state(zones()).await;
    seed_zones(&st).await;
    let uri = "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:a";
    let (_, before) = send(&st, "GET", uri, None).await;
    assert!(before.get("speed").is_some(), "{before}");
    let (code, body) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:a/attrs",
        Some(json!({"location": point(5.0)})),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let (_, after) = send(&st, "GET", uri, None).await;
    assert_eq!(
        members(&json!([after]))["urn:ngsi-ld:Car:a"],
        keys(&["id", "type", "status"])
    );
}

/// 5.8.6 with `showChanges`: after the Car left zone A, the notification
/// of a change carries `status` only, no `speed` and no `previousValue` of
/// it; a Robot no rule holds for is never notified.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_after_zone_exit_has_no_hidden_previous_value() {
    let st = state(zones()).await;
    seed_zones(&st).await;
    let (uri, mut rx) = capture().await;
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some(
            json!({"type": "Subscription", "entities": [{"type": "Car"}],
                    "notification": {"endpoint": {"uri": uri}, "showChanges": true}}),
        ),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:a/attrs",
        Some(json!({"location": point(5.0), "speed": prop(json!(140)),
                    "status": prop(json!("left"))})),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let mut seen = Vec::new();
    while let Ok(Some(n)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
    {
        let text = n.to_string();
        seen.push(n);
        if text.contains("left") {
            break;
        }
    }
    let last = seen.last().cloned().unwrap_or(Value::Null);
    assert!(
        last.to_string().contains("left"),
        "no notification of the change: {seen:?}"
    );
    // the subscription's initial notification of Car a and b may travel in
    // the same batch as the change (T-3690): every entry is held to the
    // rules, not only the last one per id
    let data = last["data"].as_array().cloned().unwrap_or_default();
    for e in &data {
        assert_eq!(e["type"], "Car", "a Robot was notified: {last}");
        assert_eq!(
            members(&json!([e])).into_values().next(),
            Some(keys(&["id", "type", "status"])),
            "a hidden Attribute or its previousValue travelled: {last}"
        );
    }
    let change: Vec<_> = data
        .iter()
        .filter(|e| e["status"]["value"] == "left")
        .collect();
    assert_eq!(change.len(), 1, "{last}");
    assert_eq!(change[0]["id"], "urn:ngsi-ld:Car:a", "{last}");
    assert_eq!(change[0]["status"]["previousValue"], "ok", "{last}");
}

/// The history of `speed` is served only for the instants Car a was in
/// zone A: before it entered, and after it left, the instances are hidden.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temporal_speed_history_only_while_in_zone_a() {
    let st = state(zones()).await;
    let at = |h: u32| format!("2026-01-01T{h:02}:00:00Z");
    let loc = |x: f64, h: u32| {
        json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [x, 0.5]},
               "observedAt": at(h)})
    };
    let speed = |v: i64, h: u32| json!({"type": "Property", "value": v, "observedAt": at(h)});
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/temporal/entities",
        Some(json!({"id": "urn:ngsi-ld:Car:t", "type": "Car",
                    // outside until 02:00, inside 02:00..04:00, outside after
                    "location": [loc(5.0, 1), loc(0.5, 2), loc(5.0, 4)],
                    "speed": [speed(10, 0), speed(11, 1), speed(12, 2), speed(13, 3), speed(14, 5)],
                    "status": [{"type": "Property", "value": "ok", "observedAt": at(0)}]})),
    )
    .await;
    assert!(code.is_success(), "{body}");
    let values = |doc: &Value| -> Vec<i64> {
        doc["speed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["value"].as_i64())
            .collect()
    };
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let mut got = values(&list[0]);
    got.sort();
    assert_eq!(got, [12, 13], "{list}");
    let (code, one) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Car:t?lastN=1",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{one}");
    assert_eq!(values(&one), [13], "lastN counts only the shown: {one}");
    // the client's q reads the shown history too
    let (_, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Car&q=speed==14&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(list, json!([]), "a hidden instance matched: {list}");
}

/// A rule with `every_attribute` shows the whole Entity, live and in
/// history, and a sibling rule's list never narrows it; a rule with an
/// empty list and no flag still shows only `id` and `type` (ADR-0023
/// rule 11 amendment, T-3637).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rule_over_every_attribute_shows_the_whole_entity() {
    let whole = Rule {
        every_attribute: true,
        ..rule("Car", &[])
    };
    let st = state(Filter {
        rules: vec![whole, rule("Car", &["speed"]), rule("User", &[])],
        ..Filter::default()
    })
    .await;
    seed(&st).await;
    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car,User", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let got = members(&list);
    assert_eq!(
        got["urn:ngsi-ld:Car:1"],
        keys(&["id", "type", "speed", "colour", "ownedBy"]),
        "{list}"
    );
    assert_eq!(got["urn:ngsi-ld:User:1"], keys(&["id", "type"]), "{list}");

    let mut doc = json!({"id": "urn:ngsi-ld:Car:t", "type": "Car"});
    for a in ["speed", "colour"] {
        doc[a] = json!([{"type": "Property", "value": 1,
                         "observedAt": "2026-01-01T00:00:00Z"}]);
    }
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
    assert!(code.is_success(), "{body}");
    let (code, car) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Car:t",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{car}");
    assert_eq!(
        members(&json!([car]))["urn:ngsi-ld:Car:t"],
        keys(&["id", "type", "speed", "colour"])
    );
}

/// The ids of a GET page and whether its `Link` names a next page.
async fn page(st: &AppState, uri: &str) -> (Vec<String>, bool) {
    let resp = antares_api::router(st.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    let next = resp
        .headers()
        .get_all("Link")
        .iter()
        .any(|l| l.to_str().unwrap_or_default().contains("rel=\"next\""));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let list: Value = serde_json::from_slice(&bytes).unwrap();
    (
        list.as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_owned())
            .collect(),
        next,
    )
}

/// Every page of `uri` (`limit` 2), in order: each page's ids and whether
/// it links a next one.
async fn pages(st: &AppState, uri: &str) -> Vec<(Vec<String>, bool)> {
    let mut out = Vec::new();
    for offset in (0..).step_by(2) {
        let got = page(st, &format!("{uri}&limit=2&offset={offset}")).await;
        let last = !got.1;
        out.push(got);
        if last {
            return out;
        }
    }
    out
}

/// Five Cars (speed 0..4), two Users and two Bikes no rule names.
async fn fleet(st: &AppState) {
    for n in 0..5 {
        create(
            st,
            json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
                   "speed": prop(json!(n)), "colour": prop(json!("red"))}),
        )
        .await;
    }
    for n in 0..2 {
        create(
            st,
            json!({"id": format!("urn:ngsi-ld:User:{n}"), "type": "User",
                   "name": prop(json!("Ana")), "email": prop(json!("a@example.org"))}),
        )
        .await;
        create(
            st,
            json!({"id": format!("urn:ngsi-ld:Bike:{n}"), "type": "Bike",
                   "speed": prop(json!(n))}),
        )
        .await;
    }
}

fn id_list(kind: &str, ns: &[u8]) -> Vec<String> {
    ns.iter()
        .map(|n| format!("urn:ngsi-ld:{kind}:{n}"))
        .collect()
}

/// T-3667: rules that select by type alone page in the store, so a page
/// is the page of what the rules admit, `Link` says whether one follows,
/// the count is theirs and a hidden Attribute stays hidden.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_under_type_rules_is_a_page_of_what_they_admit() {
    let st = state(picks(&[("Car", &["speed"]), ("User", &["name"])])).await;
    fleet(&st).await;
    let uri = "/ngsi-ld/v1/entities?type=Car,User,Bike";
    let got = pages(&st, uri).await;
    let seen: Vec<String> = got.iter().flat_map(|p| p.0.clone()).collect();
    let mut all = id_list("Car", &[0, 1, 2, 3, 4]);
    all.extend(id_list("User", &[0, 1]));
    assert_eq!(
        {
            let mut s = seen.clone();
            s.sort();
            s
        },
        all
    );
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(got[..3].iter().all(|p| p.0.len() == 2 && p.1), "{got:?}");
    assert_eq!(got[3], (seen[6..].to_vec(), false));
    assert_eq!(
        count(&st, &format!("{uri}&count=true&limit=0")).await,
        Some(7)
    );
    assert_eq!(
        count(&st, "/ngsi-ld/v1/entities?type=Bike&count=true&limit=0").await,
        Some(0)
    );
    assert_eq!(
        page(&st, "/ngsi-ld/v1/entities?type=Bike&limit=1").await,
        (vec![], false)
    );
    let (_, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car&limit=1&offset=3",
        None,
    )
    .await;
    assert_eq!(
        members(&list).into_values().collect::<Vec<_>>(),
        [keys(&["id", "speed", "type"])]
    );
}

/// A conditional rule is judged on the Entity, a page at a time: the page
/// still holds only what the rule admits, and the last one links nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_under_a_conditional_rule_is_a_page_of_what_holds() {
    let st = state(Filter {
        rules: vec![Rule {
            q: Some(Box::new(antares_ql::parse_q("speed>=2").unwrap())),
            ..rule("Car", &["speed"])
        }],
        ..Filter::default()
    })
    .await;
    fleet(&st).await;
    let uri = "/ngsi-ld/v1/entities?type=Car,User,Bike";
    let got = pages(&st, uri).await;
    let mut seen: Vec<String> = got.iter().flat_map(|p| p.0.clone()).collect();
    seen.sort();
    assert_eq!(seen, id_list("Car", &[2, 3, 4]));
    assert_eq!(
        got.iter().map(|p| (p.0.len(), p.1)).collect::<Vec<_>>(),
        [(2, true), (1, false)]
    );
    assert_eq!(
        count(&st, &format!("{uri}&count=true&limit=0")).await,
        Some(3)
    );
    let (one, next) = page(&st, &format!("{uri}&limit=1&offset=2")).await;
    assert_eq!((one.len(), next), (1, false));
}

/// A rule naming ids is not judged by type: the page is cut after the ids
/// are checked, so it never fills with Entities the rule refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_under_a_rule_naming_ids_holds_only_those() {
    let st = state(Filter {
        rules: vec![Rule {
            ids: id_list("Car", &[1, 4]),
            ..rule("Car", &["speed"])
        }],
        ..Filter::default()
    })
    .await;
    fleet(&st).await;
    let got = pages(&st, "/ngsi-ld/v1/entities?type=Car").await;
    assert_eq!(got, [(id_list("Car", &[1, 4]), false)]);
    let (one, next) = page(&st, "/ngsi-ld/v1/entities?type=Car&limit=1").await;
    assert_eq!((one, next), (id_list("Car", &[1]), true));
}

/// A client `q` on an Attribute the rules hide reads it as absent, so the
/// store never pages on it: `q=colour` finds no Car though each holds one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_q_under_type_rules_reads_the_entity_as_shown() {
    let st = state(picks(&[("Car", &["speed"])])).await;
    fleet(&st).await;
    assert_eq!(
        page(&st, "/ngsi-ld/v1/entities?type=Car&q=colour&limit=1").await,
        (vec![], false)
    );
    let (one, next) = page(&st, "/ngsi-ld/v1/entities?type=Car&q=speed%3E=3&limit=1").await;
    assert_eq!((one.len(), next), (1, true));
}

/// T-4289: under rules that admit by type alone, a client `q` on Attributes
/// every rule shows reaches the store with a page, `count=true` included, as
/// OGC API Features and OData send it; Postgres then counts in SQL instead
/// of reading every Entity the policy admits into its TooManyResults
/// ceiling. A `q` on an Attribute a rule hides stays on the shown Entity:
/// the store is asked for the policy's Entities whole, and the hidden value
/// still matches nothing.
#[tokio::test]
async fn a_q_on_shown_attributes_is_paged_in_the_store_and_a_hidden_one_is_not() {
    let mut st = state(picks(&[("Car", &["speed", "colour"]), ("User", &["name"])])).await;
    seed(&st).await;
    let spy = Arc::new(crate::common::Double::passthrough(st.store.clone()));
    st.store = spy.clone();
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Car&q=colour==%22red%22&count=true&limit=1",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        members(&body).keys().collect::<Vec<_>>(),
        ["urn:ngsi-ld:Car:1"]
    );
    assert_eq!(
        spy.unpaged_queries(),
        0,
        "the shown q was not handed to the store with a page"
    );
    // `email` is a User's, hidden by its rule: the store reads the admitted
    // Entities without the q, in pages since T-4516, and the shown User has
    // no `email` to match: a pushed q would have found Ana.
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=User&q=email==%22ana@example.org%22&count=true",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body, json!([]));
    assert_eq!(
        spy.unpaged_queries(),
        0,
        "the count under a hidden q read the store unpaged"
    );
}

/// T-5062: under rules that admit by type alone, a client geoquery on a
/// `location` every rule shows reaches the store with a page, so Postgres
/// answers it from its spatial index (an OGC `bbox` or an NGSI-LD `georel`
/// through a view read every Entity of the type before). A rule that hides
/// `location` keeps the geoquery on the shown Entity, which has no
/// `location` to match.
#[tokio::test]
async fn a_geoquery_on_a_shown_location_is_paged_in_the_store_and_a_hidden_one_is_not() {
    let near = "georel=near%3BmaxDistance%3D%3D20000&geometry=Point&coordinates=%5B0.5,0.5%5D";
    let location = "https://uri.etsi.org/ngsi-ld/location".to_owned();
    for (attrs, pushed, found) in [
        (
            vec![iri("speed"), location],
            true,
            vec!["urn:ngsi-ld:Car:a"],
        ),
        (vec![iri("speed")], false, vec![]),
    ] {
        let rule = Rule {
            types: vec![iri("Car")],
            attributes: attrs.clone(),
            ..Rule::default()
        };
        let mut st = state(Filter {
            rules: vec![rule],
            ..Filter::default()
        })
        .await;
        seed_zones(&st).await;
        let spy = Arc::new(crate::common::Double::passthrough(st.store.clone()));
        st.store = spy.clone();
        for uri in [
            format!("/ngsi-ld/v1/entities?type=Car&{near}&limit=5"),
            format!("/ngsi-ld/v1/entities?type=Car&{near}&q=speed%3E100&count=true&limit=5"),
        ] {
            let (code, body) = send(&st, "GET", &uri, None).await;
            assert_eq!(code, StatusCode::OK, "{body}");
            assert_eq!(
                members(&body).keys().collect::<Vec<_>>(),
                found,
                "{attrs:?} {uri}"
            );
        }
        assert_eq!(spy.geo_paged() == 2, pushed, "{attrs:?}: geoquery pushdown");
        assert_eq!(spy.unpaged_queries(), 0, "{attrs:?}: a read was unpaged");
    }
}

/// T-5826: a rule that narrows by an `idPattern` AND a `q` stays judged per
/// row: the pattern alone is never handed to the store as the whole rule, so
/// a Tree the pattern selects and the `q` refuses is neither shown nor
/// counted. The same pattern without the `q` counts both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pattern_with_a_q_is_still_judged_per_entity() {
    let narrowed = |q: Option<&str>| Filter {
        rules: vec![Rule {
            id_patterns: vec!["^urn:ngsi-ld:Tree:green-[0-9]+$".into()],
            q: q.map(|q| Box::new(antares_ql::parse_q(q).unwrap())),
            ..rule("Tree", &["height"])
        }],
        ..Filter::default()
    };
    for (q, count, ids) in [
        (Some("height>5"), "1", vec!["urn:ngsi-ld:Tree:green-1"]),
        (
            None,
            "2",
            vec!["urn:ngsi-ld:Tree:green-1", "urn:ngsi-ld:Tree:green-2"],
        ),
    ] {
        let st = state(narrowed(q)).await;
        for (id, height) in [("green-1", 10), ("green-2", 1), ("other-3", 10)] {
            create(
                &st,
                json!({"id": format!("urn:ngsi-ld:Tree:{id}"), "type": "Tree",
                       "height": prop(json!(height))}),
            )
            .await;
        }
        let req = Request::builder()
            .uri("/ngsi-ld/v1/entities?type=Tree&count=true&limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{q:?}");
        let total = resp.headers()["NGSILD-Results-Count"]
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(total, count, "{q:?}");
        assert_eq!(members(&list).into_keys().collect::<Vec<_>>(), ids, "{q:?}");
    }
}
