// SPDX-License-Identifier: EUPL-1.2
//! Gap G6 (gateway-to-broker map K13): an engine narrows a temporal read
//! (5.7.3, 5.7.4) to windows of `observedAt`. The caller's 4.11 window is
//! clamped to them before the store runs, a relative window slides with
//! now, and an instance outside every window is never served; a window the
//! grant does not reach answers nothing rather than a refusal. Carried over
//! from the gateway's `tests/temporal_tests.rs`.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject, TimeBound,
    Window,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Vehicle:g6";

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

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn windows(w: Vec<Window>) -> Filter {
    Filter {
        temporal: w.into(),
        ..Filter::default()
    }
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => b.body(Body::empty()),
    }
    .unwrap();
    let res = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A broker narrowing by `filter`, holding one Vehicle observed at `times`.
async fn seeded(filter: Filter, times: &[DateTime<Utc>]) -> AppState {
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Narrowing(filter)));
    antares_api::wire(&mut st).await;
    let speed: Vec<Value> = times
        .iter()
        .enumerate()
        .map(|(i, t)| json!({"type": "Property", "value": i, "observedAt": stamp(*t)}))
        .collect();
    let doc = json!({"id": ID, "type": "Vehicle", "speed": speed});
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
    assert!(status.is_success(), "{status} {body}");
    st
}

/// The `observedAt` of every instance a temporal read served, sorted.
fn observed(doc: &Value) -> Vec<String> {
    let mut seen: Vec<String> = doc["speed"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["observedAt"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    seen.sort();
    seen
}

async fn retrieve(st: &AppState, query: &str) -> Vec<String> {
    let (status, body) = send(
        st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}?{query}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    observed(&body)
}

async fn query(st: &AppState, window: &str) -> Vec<String> {
    let (status, body) = send(
        st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{window}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .unwrap()
        .first()
        .map(observed)
        .unwrap_or_default()
}

/// A year asked under a grant from 01:30 on: the answer starts at 01:30,
/// on the retrieve and on the query.
#[tokio::test]
async fn the_callers_window_is_clamped_to_the_grant() {
    let times = [
        at("2026-01-01T01:00:00Z"),
        at("2026-01-01T02:00:00Z"),
        at("2026-01-01T03:00:00Z"),
    ];
    let grant = windows(vec![Window {
        from: Some(TimeBound::At(at("2026-01-01T01:30:00Z"))),
        to: None,
    }]);
    let st = seeded(grant, &times).await;
    let kept = vec![
        "2026-01-01T02:00:00Z".to_owned(),
        "2026-01-01T03:00:00Z".to_owned(),
    ];

    assert_eq!(retrieve(&st, "").await, kept, "no window of its own");
    assert_eq!(
        retrieve(&st, "timerel=after&timeAt=2025-01-01T00:00:00Z").await,
        kept
    );
    assert_eq!(
        query(&st, "timerel=after&timeAt=2025-01-01T00:00:00Z").await,
        kept
    );
    assert_eq!(
        query(&st, "timerel=before&timeAt=2026-01-01T03:00:00Z").await,
        ["2026-01-01T02:00:00Z"],
        "the caller's own end holds inside the grant"
    );
}

/// "The last 90 minutes" is resolved when the request is decided.
#[tokio::test]
async fn a_relative_grant_window_slides_with_now() {
    let now = Utc::now();
    let times = [now - Duration::hours(2), now - Duration::minutes(30)];
    let grant = windows(vec![Window {
        from: Some(TimeBound::Ago(Duration::minutes(90))),
        to: None,
    }]);
    let st = seeded(grant, &times).await;

    assert_eq!(retrieve(&st, "").await, [stamp(times[1])]);
    assert_eq!(
        query(&st, "timerel=after&timeAt=2000-01-01T00:00:00Z").await,
        [stamp(times[1])]
    );
}

/// Every window holds: an instance outside any one is never served, and a
/// caller's window the grant does not reach answers nothing (200), on the
/// retrieve and on the query, as does another timeproperty.
#[tokio::test]
async fn an_instance_outside_every_window_is_never_served() {
    let times = [
        at("2026-01-01T01:00:00Z"),
        at("2026-01-01T02:00:00Z"),
        at("2026-01-01T03:00:00Z"),
    ];
    let grant = windows(vec![
        Window {
            from: Some(TimeBound::At(at("2026-01-01T01:30:00Z"))),
            to: None,
        },
        Window {
            from: None,
            to: Some(TimeBound::At(at("2026-01-01T02:30:00Z"))),
        },
    ]);
    let st = seeded(grant, &times).await;

    assert_eq!(retrieve(&st, "").await, ["2026-01-01T02:00:00Z"]);
    assert_eq!(
        query(&st, "timerel=after&timeAt=2000-01-01T00:00:00Z").await,
        ["2026-01-01T02:00:00Z"]
    );

    let before = "timerel=before&timeAt=2026-01-01T01:15:00Z";
    assert!(retrieve(&st, before).await.is_empty());
    assert!(query(&st, before).await.is_empty());
    let other = "timerel=after&timeAt=2000-01-01T00:00:00Z&timeproperty=modifiedAt";
    assert!(
        query(&st, other).await.is_empty(),
        "the windows are on observedAt"
    );
}

/// The forward of a distributed temporal query asks the Context Source for
/// the clamped window, never the caller's (4.3.6: one query, narrowed once).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forwarded_temporal_query_carries_the_clamped_window() {
    use std::io::{Read, Write};
    antares_jsonld::allow_private_egress(true);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            sink.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
        }
    });
    let grant = windows(vec![Window {
        from: Some(TimeBound::At(at("2026-01-01T01:30:00Z"))),
        to: None,
    }]);
    let st = seeded(grant, &[at("2026-01-01T02:00:00Z")]).await;
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:g6",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "operations": ["queryTemporal"],
        "endpoint": format!("http://127.0.0.1:{port}"),
    });
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    query(&st, "timerel=after&timeAt=2020-01-01T00:00:00Z").await;

    let sent = seen.lock().unwrap().clone();
    let line = sent
        .first()
        .and_then(|r| r.lines().next())
        .unwrap_or_default()
        .to_owned();
    assert!(
        line.starts_with("GET /ngsi-ld/v1/temporal/entities?"),
        "{sent:?}"
    );
    assert!(
        line.contains("timeAt=2026-01-01T01%3A30%3A00.000Z"),
        "{line}"
    );
    assert!(!line.contains("2020-01-01"), "{line}");
}

/// A rule's windows narrow the instances of its own Attributes: `speed`
/// shows from 01:30 on, `colour` under a rule without windows shows whole.
#[tokio::test]
async fn a_rules_windows_narrow_its_own_attributes() {
    let iri = |t: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{t}");
    let rules = Filter {
        rules: vec![
            antares_api::policy::Rule {
                types: vec![iri("Vehicle")],
                attributes: vec![iri("speed")],
                temporal: vec![Window {
                    from: Some(TimeBound::At(at("2026-01-01T01:30:00Z"))),
                    to: None,
                }],
                ..antares_api::policy::Rule::default()
            },
            antares_api::policy::Rule {
                types: vec![iri("Vehicle")],
                attributes: vec![iri("colour")],
                ..antares_api::policy::Rule::default()
            },
        ],
        ..Filter::default()
    };
    let st = seeded(
        rules,
        &[at("2026-01-01T01:00:00Z"), at("2026-01-01T02:00:00Z")],
    )
    .await;
    let colour = json!({"colour": [
        {"type": "Property", "value": "red", "observedAt": "2026-01-01T01:00:00Z"},
        {"type": "Property", "value": "blue", "observedAt": "2026-01-01T02:00:00Z"},
    ]});
    let (status, body) = send(
        &st,
        "POST",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}/attrs"),
        Some(colour),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");

    let (status, body) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(observed(&body), ["2026-01-01T02:00:00Z"], "{body}");
    assert_eq!(body["colour"].as_array().map(Vec::len), Some(2), "{body}");
}
