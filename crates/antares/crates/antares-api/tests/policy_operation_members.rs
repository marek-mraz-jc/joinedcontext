// SPDX-License-Identifier: EUPL-1.2
//! What a write hands the policy engine (ADR-0020, T-3476).
//!
//! "`Operation` is the expanded request: clause, ids, types, attrs, `q`,
//! `scopeQ`, geo, body". A member the gate leaves empty is a decision the
//! engine cannot make: a rule on a type or an Attribute is then silently
//! not applied. Each write below names its members in expanded form. Where
//! the type lives only in the stored Entity (a delete, an update by id),
//! the gate reads it, so an engine judging by type judges the Entity the
//! write lands on.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// One recorded decision: the members an engine was shown.
#[derive(Debug, Clone)]
struct Seen {
    clause: String,
    types: Vec<String>,
    attrs: Vec<String>,
    body: Option<Value>,
    q: bool,
    scope_q: Option<String>,
    geo: bool,
}

/// Records every operation; refuses any but a create that names the type
/// `Secret`, so a test can seed one through the same router.
struct Recording(Arc<Mutex<Vec<Seen>>>);

impl PolicyEngine for Recording {
    fn name(&self) -> &str {
        "recording"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if let Ok(mut v) = self.0.lock() {
            v.push(Seen {
                clause: op.clause.to_owned(),
                types: op.types.to_vec(),
                attrs: op.attrs.to_vec(),
                body: op.body.cloned(),
                q: op.q.is_some(),
                scope_q: op.scope_q.map(str::to_owned),
                geo: op.geo.is_some(),
            });
        }
        let secret = format!("{DC}Secret");
        Box::pin(std::future::ready(
            if op.clause != "5.6.1" && op.types.contains(&secret) {
                Decision::Deny("a Secret".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn state() -> (AppState, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Recording(Arc::clone(&seen))));
    antares_api::wire(&mut st).await;
    (st, seen)
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> StatusCode {
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
    antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response")
        .status()
}

/// The Vehicle every write below lands on, and the Secret the engine guards.
async fn seed(st: &AppState) {
    for doc in [
        json!({"id": "urn:ngsi-ld:Vehicle:v", "type": "Vehicle",
               "speed": {"type": "Property", "value": 1},
               "colour": {"type": "Property", "value": "red"}}),
        json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret",
               "code": {"type": "Property", "value": 7}}),
    ] {
        let code = send(st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED);
    }
}

/// The last decision the engine took.
fn last(seen: &Arc<Mutex<Vec<Seen>>>) -> Seen {
    seen.lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the engine was asked")
}

fn iri(name: &str) -> String {
    format!("{DC}{name}")
}

#[tokio::test(flavor = "multi_thread")]
async fn each_write_names_its_types_attributes_and_body() {
    let (st, seen) = state().await;
    seed(&st).await;
    let v = "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:v";
    let speed = json!({"speed": {"type": "Property", "value": 2}});
    // (method, path, body, clause, attrs the request names, body expected)
    let cases = [
        (
            "PATCH",
            format!("{v}/attrs"),
            Some(speed.clone()),
            "5.6.2",
            vec![iri("speed")],
            true,
        ),
        (
            "POST",
            format!("{v}/attrs"),
            Some(json!({"tyre": {"type": "Property", "value": 1}})),
            "5.6.3",
            vec![iri("tyre")],
            true,
        ),
        (
            "PATCH",
            format!("{v}/attrs/speed"),
            Some(json!({"type": "Property", "value": 3})),
            "5.6.4",
            vec![iri("speed")],
            true,
        ),
        (
            "PUT",
            format!("{v}/attrs/speed"),
            Some(json!({"type": "Property", "value": 4})),
            "5.6.19",
            vec![iri("speed")],
            true,
        ),
        (
            "PATCH",
            v.to_owned(),
            Some(speed.clone()),
            "5.6.17",
            vec![iri("speed")],
            true,
        ),
        (
            "DELETE",
            format!("{v}/attrs/tyre"),
            None,
            "5.6.5",
            vec![iri("tyre")],
            false,
        ),
        (
            "PUT",
            v.to_owned(),
            Some(json!({"id": "urn:ngsi-ld:Vehicle:v", "type": "Vehicle",
            "speed": {"type": "Property", "value": 5}})),
            "5.6.18",
            vec![iri("speed")],
            true,
        ),
        ("DELETE", v.to_owned(), None, "5.6.6", vec![], false),
    ];
    for (method, path, body, clause, attrs, has_body) in cases {
        let code = send(&st, method, &path, body).await;
        assert!(code.is_success(), "{method} {path}: {code}");
        let s = last(&seen);
        assert_eq!(s.clause, clause, "{method} {path}");
        assert_eq!(s.types, vec![iri("Vehicle")], "{method} {path}: types");
        assert_eq!(s.attrs, attrs, "{method} {path}: attrs");
        assert_eq!(s.body.is_some(), has_body, "{method} {path}: body");
        if let Some(b) = &s.body {
            assert!(
                b.to_string().contains(&iri("speed"))
                    || b.to_string().contains(&iri("tyre"))
                    || b.get("value").is_some(),
                "{method} {path}: the body is the expanded request: {b}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_create_shows_the_engine_each_member_body() {
    let (st, seen) = state().await;
    let code = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/create",
        Some(json!([
            {"id": "urn:ngsi-ld:Vehicle:b1", "type": "Vehicle", "speed": {"type": "Property", "value": 1}},
            {"id": "urn:ngsi-ld:Vehicle:b2", "type": "Vehicle", "colour": {"type": "Property", "value": "x"}},
        ])),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let s = last(&seen);
    assert_eq!(s.clause, "5.6.7");
    assert_eq!(s.types, vec![iri("Vehicle")]);
    assert_eq!(s.attrs, vec![iri("colour")]);
    let body = s.body.expect("the member body reaches the engine");
    assert_eq!(
        body.get("id"),
        Some(&json!("urn:ngsi-ld:Vehicle:b2")),
        "{body}"
    );
    assert!(body.get(iri("colour")).is_some(), "expanded: {body}");
}

/// The answer and its body.
async fn call(st: &AppState, uri: &str, body: Value) -> (StatusCode, Value) {
    let payload = body.to_string();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", payload.len())
        .body(Body::from(payload))
        .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn exists(st: &AppState, id: &str) -> bool {
    send(st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await == StatusCode::OK
}

/// T-3480: a batch is decided member by member. The engine refuses the
/// Secret; the Vehicle beside it is applied and the Secret is reported in
/// E with the AccessDenied problem (207). A batch of Secrets alone is the
/// 403 a single refusal gives.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_member_is_reported_and_the_rest_applied() {
    let (st, _) = state().await;
    seed(&st).await;
    let ops = "/ngsi-ld/v1/entityOperations";
    let vehicle = |n: u32| {
        json!({"id": format!("urn:ngsi-ld:Vehicle:m{n}"), "type": "Vehicle",
                                  "speed": {"type": "Property", "value": n}})
    };
    let secret = json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret",
                        "code": {"type": "Property", "value": 9}});
    // the update and merge members name Vehicle: the Secret type is stored
    let to_secret = json!({"id": "urn:ngsi-ld:Secret:s", "type": "Vehicle", "code": {"type": "Property", "value": 9}});
    let to_vehicle = json!({"id": "urn:ngsi-ld:Vehicle:m1", "type": "Vehicle", "speed": {"type": "Property", "value": 5}});
    let cases = [
        (
            "create",
            json!([
                vehicle(1),
                json!({"id": "urn:ngsi-ld:Secret:new", "type": "Secret"})
            ]),
            "urn:ngsi-ld:Secret:new",
        ),
        (
            "upsert",
            json!([vehicle(2), secret.clone()]),
            "urn:ngsi-ld:Secret:s",
        ),
        (
            "update",
            json!([to_vehicle.clone(), to_secret.clone()]),
            "urn:ngsi-ld:Secret:s",
        ),
        (
            "merge",
            json!([to_vehicle, to_secret]),
            "urn:ngsi-ld:Secret:s",
        ),
        (
            "delete",
            json!(["urn:ngsi-ld:Vehicle:m2", "urn:ngsi-ld:Secret:s"]),
            "urn:ngsi-ld:Secret:s",
        ),
    ];
    for (op, body, refused) in cases {
        let (code, answer) = call(&st, &format!("{ops}/{op}"), body).await;
        assert_eq!(code, StatusCode::MULTI_STATUS, "{op}: {answer}");
        let errors = answer["errors"].as_array().expect("E");
        assert_eq!(errors.len(), 1, "{op}: {answer}");
        assert_eq!(errors[0]["entityId"], json!(refused), "{op}");
        assert_eq!(errors[0]["error"]["status"], json!(403), "{op}");
        assert_eq!(
            errors[0]["error"]["type"],
            json!(antares_api::policy::ACCESS_DENIED_TYPE),
            "{op}"
        );
        assert_eq!(errors[0]["error"]["detail"], json!("a Secret"), "{op}");
        assert_eq!(
            answer["success"].as_array().map(Vec::len),
            Some(1),
            "{op}: {answer}"
        );
    }
    assert!(
        exists(&st, "urn:ngsi-ld:Vehicle:m1").await,
        "create applied"
    );
    assert!(
        !exists(&st, "urn:ngsi-ld:Secret:new").await,
        "refused create not applied"
    );
    assert!(
        !exists(&st, "urn:ngsi-ld:Vehicle:m2").await,
        "delete applied"
    );
    assert!(
        exists(&st, "urn:ngsi-ld:Secret:s").await,
        "refused delete not applied"
    );
    let (code, _) = call(
        &st,
        &format!("{ops}/delete"),
        json!(["urn:ngsi-ld:Secret:s"]),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN, "a batch refused whole");
}

/// An engine answering a batch with the wrong number of decisions denies
/// the whole batch: the seam fails closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_length_answer_denies_the_batch() {
    struct Short;
    impl PolicyEngine for Short {
        fn name(&self) -> &str {
            "short"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
            Box::pin(std::future::ready(Decision::Allow))
        }
        fn decide_each<'a>(
            &'a self,
            _s: &'a Subject,
            _ops: &'a [Operation<'a>],
        ) -> antares_api::policy::DecisionsFuture<'a> {
            Box::pin(std::future::ready(vec![Decision::Allow]))
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
            NotifyDecision::Deliver
        }
    }
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Short));
    antares_api::wire(&mut st).await;
    let (code, _) = call(
        &st,
        "/ngsi-ld/v1/entityOperations/create",
        json!([
            {"id": "urn:ngsi-ld:Vehicle:w1", "type": "Vehicle"},
            {"id": "urn:ngsi-ld:Vehicle:w2", "type": "Vehicle"},
        ]),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert!(!exists(&st, "urn:ngsi-ld:Vehicle:w1").await);
}

/// The rule on a type holds on every write: the Secret's type comes from
/// the store, since none of these requests names it.
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_on_the_stored_type_refuses_every_write_to_it() {
    let (st, _) = state().await;
    seed(&st).await;
    let s = "/ngsi-ld/v1/entities/urn:ngsi-ld:Secret:s";
    let code7 = json!({"code": {"type": "Property", "value": 8}});
    for (method, path, body) in [
        ("PATCH", format!("{s}/attrs"), Some(code7.clone())),
        ("POST", format!("{s}/attrs"), Some(code7.clone())),
        (
            "PATCH",
            format!("{s}/attrs/code"),
            Some(json!({"type": "Property", "value": 9})),
        ),
        (
            "PUT",
            format!("{s}/attrs/code"),
            Some(json!({"type": "Property", "value": 9})),
        ),
        ("PATCH", s.to_owned(), Some(code7.clone())),
        ("DELETE", format!("{s}/attrs/code"), None),
        (
            "PUT",
            s.to_owned(),
            Some(json!({"id": "urn:ngsi-ld:Secret:s", "type": "Vehicle"})),
        ),
        ("DELETE", s.to_owned(), None),
    ] {
        assert_eq!(
            send(&st, method, &path, body).await,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
}

const GEO: &str = "georel=near%3BmaxDistance%3D%3D10&geometry=Point&coordinates=%5B1,2%5D";

fn geo_q() -> Value {
    json!({"georel": "near;maxDistance==10", "geometry": "Point", "coordinates": [1, 2]})
}

/// One read and the members its gate must carry: a space-separated subset
/// of `types attrs q scopeQ geo`, the same members, expanded, that the
/// store query runs on (T-3477).
struct Read {
    method: &'static str,
    path: String,
    body: Option<Value>,
    clause: &'static str,
    named: &'static str,
}

fn read(
    method: &'static str,
    path: String,
    body: Option<Value>,
    clause: &'static str,
    named: &'static str,
) -> Read {
    Read {
        method,
        path,
        body,
        clause,
        named,
    }
}

fn reads() -> Vec<Read> {
    let v = "/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:v";
    let tq = "timerel=after&timeAt=2020-01-01T00:00:00Z";
    vec![
        read(
            "GET",
            format!("{v}?type=Vehicle&attrs=speed"),
            None,
            "5.7.1",
            "types attrs",
        ),
        read("GET", format!("{v}/attrs/speed"), None, "5.7.1", "attrs"),
        read(
            "GET",
            format!("/ngsi-ld/v1/entities?type=Vehicle&attrs=speed&q=speed%3E0&{GEO}"),
            None,
            "5.7.2",
            "types attrs q geo",
        ),
        read(
            "POST",
            "/ngsi-ld/v1/entityOperations/query".into(),
            Some(json!({"type": "Query",
            "entities": [{"type": "Vehicle"}], "attrs": ["speed"], "q": "speed>0",
            "scopeQ": "/a", "geoQ": geo_q()})),
            "5.7.2",
            "types attrs q scopeQ geo",
        ),
        read(
            "GET",
            format!(
                "/ngsi-ld/v1/temporal/entities?type=Vehicle&attrs=speed&q=speed%3E0&{tq}&{GEO}"
            ),
            None,
            "5.7.4",
            "types attrs q geo",
        ),
        read(
            "POST",
            "/ngsi-ld/v1/temporal/entityOperations/query".into(),
            Some(json!({"type": "Query",
            "entities": [{"type": "Vehicle"}], "attrs": ["speed"], "q": "speed>0", "scopeQ": "/a",
            "geoQ": geo_q(),
            "temporalQ": {"timerel": "after", "timeAt": "2020-01-01T00:00:00Z"}})),
            "5.7.4",
            "types attrs q scopeQ geo",
        ),
        read(
            "POST",
            "/ngsi-ld/v1/subscriptions".into(),
            Some(json!({"type": "Subscription",
            "entities": [{"type": "Vehicle"}], "watchedAttributes": ["speed"], "q": "speed>0",
            "scopeQ": "/a", "geoQ": geo_q(),
            "notification": {"endpoint": {"uri": "https://example.org/notify"}}})),
            "5.8.1",
            "types attrs q scopeQ geo",
        ),
        read(
            "GET",
            "/ngsi-ld/v1/types/Vehicle".into(),
            None,
            "5.7.7",
            "types",
        ),
        read(
            "GET",
            "/ngsi-ld/v1/attributes/speed".into(),
            None,
            "5.7.10",
            "attrs",
        ),
        read(
            "GET",
            format!("/ngsi-ld/v1/entityMaps?type=Vehicle&attrs=speed&q=speed%3E0&{GEO}"),
            None,
            "5.14.4",
            "types attrs q geo",
        ),
        read(
            "POST",
            "/ngsi-ld/v1/entityMaps".into(),
            Some(json!({"type": "Query",
            "entities": [{"type": "Vehicle"}], "attrs": ["speed"], "q": "speed>0",
            "scopeQ": "/a", "geoQ": geo_q()})),
            "5.14.4",
            "types attrs q scopeQ geo",
        ),
        read(
            "GET",
            format!(
                "/ngsi-ld/v1/temporal/entityMaps?type=Vehicle&attrs=speed&q=speed%3E0&{tq}&{GEO}"
            ),
            None,
            "5.14.5",
            "types attrs q geo",
        ),
        read(
            "POST",
            "/ngsi-ld/v1/csourceRegistrations".into(),
            Some(json!({"type": "ContextSourceRegistration",
            "information": [{"entities": [{"type": "Vehicle"}], "propertyNames": ["speed"]}],
            "endpoint": "https://example.org/cs"})),
            "5.9.2",
            "types attrs",
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn each_read_names_its_types_attributes_query_and_geo() {
    let (st, seen) = state().await;
    seed(&st).await;
    for Read {
        method,
        path,
        body,
        clause,
        named,
    } in reads()
    {
        let has = |m: &str| named.split(' ').any(|n| n == m);
        let code = send(&st, method, &path, body).await;
        assert!(code.is_success(), "{method} {path}: {code}");
        let s = seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.clause == clause)
            .cloned()
            .expect("the engine was asked");
        let want_types: Vec<String> = if has("types") {
            vec![iri("Vehicle")]
        } else {
            vec![]
        };
        let want_attrs: Vec<String> = if has("attrs") {
            vec![iri("speed")]
        } else {
            vec![]
        };
        assert_eq!(s.types, want_types, "{method} {path}: types");
        assert_eq!(s.attrs, want_attrs, "{method} {path}: attrs");
        assert_eq!(s.q, has("q"), "{method} {path}: q");
        assert_eq!(
            s.scope_q.is_some(),
            has("scopeQ"),
            "{method} {path}: scopeQ"
        );
        assert_eq!(s.geo, has("geo"), "{method} {path}: geo");
    }
}

/// A rule on a type holds on every read that names the type.
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_on_a_named_type_refuses_the_reads_that_name_it() {
    let (st, _) = state().await;
    seed(&st).await;
    for Read {
        method,
        path,
        body,
        named,
        ..
    } in reads()
    {
        if !named.contains("types") {
            continue;
        }
        let path = path.replace("Vehicle", "Secret");
        let body = body
            .map(|b| serde_json::from_str(&b.to_string().replace("Vehicle", "Secret")).unwrap());
        assert_eq!(
            send(&st, method, &path, body).await,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
}

/// F-ANT2-023: a purge (5.6.21) shows the engine its whole selection, so a
/// rule on a type or an Attribute sees what the purge names.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_names_its_types_attributes_and_query() {
    let (st, seen) = state().await;
    seed(&st).await;
    let code = send(
        &st,
        "DELETE",
        "/ngsi-ld/v1/entities?type=Vehicle&attrs=speed&q=speed%3E0&id=urn:ngsi-ld:Vehicle:v",
        None,
    )
    .await;
    assert!(code.is_success(), "purge: {code}");
    let s = seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.clause == "5.6.21")
        .cloned()
        .expect("the purge was decided");
    assert_eq!(s.types, vec![iri("Vehicle")]);
    assert_eq!(s.attrs, vec![iri("speed")]);
    assert!(s.q, "the purge's q");
}
