// SPDX-License-Identifier: EUPL-1.2
//! A temporal query through a view's rules reads the history its window
//! needs (T-4246). A rule that names types, Attributes and windows decides
//! each instance without the Entity's state, so the query's range reaches
//! the store; a rule with a condition still rebuilds the state at each
//! instance from the whole history. On dev, a type-wide query that loaded and
//! replayed every Entity's whole history ran past the gateway's 30 s.
#![cfg(feature = "test-kit")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
    TimeBound, Window,
};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::{filter, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const CAR: &str = "urn:ngsi-ld:Vehicle:car";

/// Narrows every read to the rules it holds.
struct Ruled(Vec<Rule>);

impl PolicyEngine for Ruled {
    fn name(&self) -> &str {
        "ruled"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Filter(Filter {
            rules: self.0.clone(),
            ..Filter::default()
        })))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Speed and zone, one instance a second from 00:00:01 to 00:00:10.
fn evolution() -> Value {
    let at = |s: u32| format!("2026-01-01T00:00:{s:02}Z");
    json!({"id": CAR, "type": "Vehicle",
           "speed": (1..=10).map(|s| json!({"type": "Property", "value": s, "observedAt": at(s)})).collect::<Vec<_>>(),
           "zone": (1..=10).map(|s| json!({"type": "Property", "value": "A", "observedAt": at(s)})).collect::<Vec<_>>()})
}

/// The rule of a platform Policy: its types, its Attributes and the
/// Policy's temporal window, no condition.
fn plain() -> Rule {
    Rule {
        types: vec!["Vehicle".into()],
        attributes: vec!["speed".into()],
        temporal: vec![Window {
            from: Some(TimeBound::At(
                "2026-01-01T00:00:00Z".parse().expect("instant"),
            )),
            to: None,
        }],
        ..Rule::default()
    }
}

fn conditional() -> Rule {
    Rule {
        q: Some(Box::new(antares_ql::parse_q("zone==\"A\"").expect("q"))),
        ..plain()
    }
}

/// Records, for each temporal read, whether it asked the store for a range
/// and for a lastN cap.
struct Recording {
    inner: Arc<dyn TemporalDriver>,
    ranged: Mutex<Vec<(bool, bool)>>,
    /// instances the store answered each query with
    served: Mutex<Vec<usize>>,
    /// per query: whether the store paged, through which Attributes
    paged: Mutex<Vec<Option<Option<Vec<String>>>>>,
}

/// The Attribute instances of `docs`, every Attribute and dataset.
fn instances(docs: &[Value]) -> usize {
    docs.iter()
        .filter_map(Value::as_object)
        .flat_map(|o| o.iter())
        .filter(|(k, _)| !matches!(k.as_str(), "type" | "@type" | "scope"))
        .filter_map(|(_, v)| v.as_array())
        .map(Vec::len)
        .sum()
}

#[async_trait::async_trait]
impl TemporalDriver for Recording {
    async fn temporal_append(
        &self,
        t: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        self.inner.temporal_append(t, id, shell, additions).await
    }
    async fn query_temporal(
        &self,
        t: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        self.ranged
            .lock()
            .expect("lock")
            .push((f.range.is_some(), f.last_n.is_some()));
        self.paged.lock().expect("lock").push(
            f.page
                .as_ref()
                .map(|_| f.page_attrs.map(<[String]>::to_vec)),
        );
        let out = self.inner.query_temporal(t, f).await?;
        self.served.lock().expect("lock").push(instances(&out.rows));
        Ok(out)
    }
    async fn get_temporal(
        &self,
        t: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        self.ranged
            .lock()
            .expect("lock")
            .push((f.range.is_some(), f.last_n.is_some()));
        self.inner.get_temporal(t, id, f).await
    }
    async fn get(&self, t: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        self.inner.get(t, id).await
    }
    async fn create(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.inner.create(t, id, doc).await
    }
    async fn upsert(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.inner.upsert(t, id, doc).await
    }
    async fn delete(&self, t: &TenantId, id: &str) -> Result<bool, NgsiError> {
        self.inner.delete(t, id).await
    }
    async fn list(&self, t: &TenantId) -> Result<Vec<Value>, NgsiError> {
        self.inner.list(t).await
    }
    async fn mutate_boxed<'a>(
        &self,
        t: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.inner.mutate_boxed(t, id, f).await
    }
}

async fn state(rule: Rule) -> (AppState, Arc<Recording>) {
    state_with(rule, &[]).await
}

/// [`state`] with `more` appended to the evolution before the rule applies.
async fn state_with(rule: Rule, more: &[Value]) -> (AppState, Arc<Recording>) {
    state_ruled(vec![rule], more).await
}

/// [`state_with`] under several rules.
async fn state_ruled(rules: Vec<Rule>, more: &[Value]) -> (AppState, Arc<Recording>) {
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let (status, body) = send(&st, "/ngsi-ld/v1/temporal/entities", Some(evolution())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for doc in more {
        let (status, body) = send(&st, "/ngsi-ld/v1/temporal/entities", Some(doc.clone())).await;
        assert!(status.is_success(), "{status} {body}");
    }
    let recording = Arc::new(Recording {
        inner: st.temporal.clone(),
        ranged: Mutex::new(Vec::new()),
        served: Mutex::new(Vec::new()),
        paged: Mutex::new(Vec::new()),
    });
    st.temporal = recording.clone();
    st.policy = Some(Arc::new(Ruled(rules)));
    (st, recording)
}

async fn send(st: &AppState, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    send_as(st, None, uri, body).await
}

async fn send_as(
    st: &AppState,
    tenant: Option<&str>,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().uri(uri);
    if let Some(t) = tenant {
        b = b.header("NGSILD-Tenant", t);
    }
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.method("POST")
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
                .expect("request")
        }
        None => b.method("GET").body(Body::empty()).expect("request"),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const AFTER: &str = "timerel=after&timeAt=2026-01-01T00:00:07Z";

/// The speeds an answer shows, in its order.
fn speeds(doc: &Value) -> Vec<i64> {
    doc.get("speed")
        .or_else(|| doc.get(format!("{DC}speed")))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| i["value"].as_i64())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_without_a_condition_reads_only_the_window_and_shows_the_same() {
    let (st, recording) = state(plain()).await;
    let (status, body) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(speeds(&body[0]), vec![7, 8, 9, 10], "{body}");
    assert!(
        body[0].get("zone").is_none(),
        "the rule does not show zone: {body}"
    );

    let (status, one) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(speeds(&one), vec![7, 8, 9, 10], "{one}");

    // the window opens before the query's start: the default page's cap
    // reaches the store too (T-5777)
    assert_eq!(
        *recording.ranged.lock().expect("lock"),
        vec![(true, true), (true, true)]
    );
}

/// A chart's plain window read, paged by offsetN: each page asks the store
/// for its own instances only, never the whole window again, and the pages
/// are the ones the uncapped read served (T-5777: a week of five-minute
/// weather took 2-3 s per page through a view).
#[tokio::test(flavor = "multi_thread")]
async fn offset_pages_of_a_window_read_reach_the_store_capped() {
    let (mut st, recording) = state(plain()).await;
    st.temporal_default_instances = 3;
    let mut pages = Vec::new();
    for offset in [0, 3] {
        let (status, body) = send(
            &st,
            &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}&offsetN={offset}"),
            None,
        )
        .await;
        pages.push((status, speeds(&body[0])));
        let (status, one) = send(
            &st,
            &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{AFTER}&offsetN={offset}"),
            None,
        )
        .await;
        pages.push((status, speeds(&one)));
    }
    let cut = StatusCode::PARTIAL_CONTENT;
    assert_eq!(
        pages,
        vec![
            (cut, vec![7, 8, 9]),
            (cut, vec![7, 8, 9]),
            (StatusCode::OK, vec![10]),
            (StatusCode::OK, vec![10]),
        ]
    );
    assert_eq!(
        *recording.ranged.lock().expect("lock"),
        vec![(true, true); 4]
    );
}

/// Oldest first, a rule window that opens after the query's start drops the
/// oldest instances the store would keep: the cap stays in the API and the
/// answer is the oldest the rule shows.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_opening_after_the_start_keeps_the_cap_out_of_the_store() {
    let late = Rule {
        temporal: vec![Window {
            from: Some(TimeBound::At(
                "2026-01-01T00:00:09Z".parse().expect("instant"),
            )),
            to: None,
        }],
        ..plain()
    };
    let (mut st, recording) = state(late).await;
    st.temporal_default_instances = 1;
    let (status, body) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{body}");
    assert_eq!(speeds(&body[0]), vec![9], "{body}");
    let (status, one) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{one}");
    assert_eq!(speeds(&one), vec![9], "{one}");
    assert_eq!(
        *recording.ranged.lock().expect("lock"),
        vec![(true, false), (true, false)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_with_a_condition_still_reads_the_whole_history() {
    let (st, recording) = state(conditional()).await;
    let (status, body) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(speeds(&body[0]), vec![7, 8, 9, 10], "{body}");

    let (status, one) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{AFTER}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(speeds(&one), vec![7, 8, 9, 10], "{one}");

    assert_eq!(
        *recording.ranged.lock().expect("lock"),
        vec![(false, false), (false, false)]
    );
}

/// The newest `n` speeds through the query and the by-id read, and what the
/// store was asked for.
async fn last_two(rule: Rule) -> (Vec<i64>, Vec<i64>, Vec<(bool, bool)>) {
    let (st, recording) = state(rule).await;
    let (status, body) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}&lastN=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, one) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{AFTER}&lastN=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    let asked = recording.ranged.lock().expect("lock").clone();
    (speeds(&body[0]), speeds(&one), asked)
}

/// Rules whose windows stay open towards now keep the newest instances of
/// what they show, so the store's lastN cap keeps what they would: the cap
/// goes down with the range.
#[tokio::test(flavor = "multi_thread")]
async fn last_n_reaches_the_store_under_rules_that_keep_the_newest() {
    let (query, one, asked) = last_two(plain()).await;
    assert_eq!(query, vec![10, 9]);
    assert_eq!(one, vec![10, 9]);
    assert_eq!(asked, vec![(true, true), (true, true)]);
}

/// A window that closes before now hides the newest instances: a store cap
/// would keep exactly those and the rule would then drop them, so the cap
/// stays in the API and the answer is the newest the rule shows.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_closed_before_now_keeps_last_n_out_of_the_store() {
    let closed = Rule {
        temporal: vec![Window {
            from: None,
            to: Some(TimeBound::At(
                "2026-01-01T00:00:09Z".parse().expect("instant"),
            )),
        }],
        ..plain()
    };
    let (query, one, asked) = last_two(closed).await;
    assert_eq!(query, vec![8, 7]);
    assert_eq!(one, vec![8, 7]);
    assert_eq!(asked, vec![(true, false), (true, false)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_condition_keeps_last_n_out_of_the_store() {
    let (query, one, asked) = last_two(conditional()).await;
    assert_eq!(query, vec![10, 9]);
    assert_eq!(one, vec![10, 9]);
    assert_eq!(asked, vec![(false, false), (false, false)]);
}

/// Rule windows judge observedAt. Ordered by createdAt, the newest
/// instances can be a backfill whose observedAt the window drops, so a
/// store cap would answer with nothing: the cap stays in the API.
#[tokio::test(flavor = "multi_thread")]
async fn last_n_by_another_timeproperty_stays_out_of_the_store() {
    let instances: Vec<Value> = [100, 101]
        .iter()
        .map(|v| json!({"type": "Property", "value": v, "observedAt": "2025-12-31T00:00:00Z"}))
        .collect();
    let backfill = json!({"id": CAR, "type": "Vehicle", "speed": instances});
    let (st, recording) = state_with(plain(), &[backfill]).await;
    let q = "timeproperty=createdAt&timerel=after&timeAt=2000-01-01T00:00:00Z&lastN=2";
    let (status, body) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{q}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let query = speeds(&body[0]);
    let (status, one) = send(
        &st,
        &format!("/ngsi-ld/v1/temporal/entities/{CAR}?{q}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(query.len(), 2, "{body}");
    assert_eq!(speeds(&one).len(), 2, "{one}");
    assert!(
        query.iter().chain(&speeds(&one)).all(|v| *v <= 10),
        "the window drops the backfill"
    );
    let asked = recording.ranged.lock().expect("lock").clone();
    assert_eq!(asked, vec![(true, false), (true, false)]);
}

/// The chart's read on PostgreSQL: a type's window of 600 speed and zone
/// instances, oldest first in default pages, through a view's plain rule.
/// The store answers each page with its own instances and one more per
/// Attribute; without the cap it answered the whole window, 1 200
/// instances, for every page (T-5777). Skips without a database.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_page_reads_only_its_instances_from_postgres() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store = Arc::new(antares_sql::store::any::AnyStore::Pg(
        antares_sql::store::any::PgBackend::new(pool),
    ));
    let mut st = AppState::with_store("me".into(), store, "postgres");
    antares_api::wire(&mut st).await;
    let tenant = format!("rw{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let at = |i: u32| format!("2026-01-01T{:02}:{:02}:00Z", i / 60, i % 60);
    let speed: Vec<Value> = (0..600)
        .map(|i| json!({"type": "Property", "value": i, "observedAt": at(i)}))
        .collect();
    let zone: Vec<Value> = (0..600)
        .map(|i| json!({"type": "Property", "value": "A", "observedAt": at(i)}))
        .collect();
    let doc = json!({"id": CAR, "type": "Vehicle", "speed": speed, "zone": zone});
    let (status, body) = send_as(
        &st,
        Some(&tenant),
        "/ngsi-ld/v1/temporal/entities",
        Some(doc),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let recording = Arc::new(Recording {
        inner: st.temporal.clone(),
        ranged: Mutex::new(Vec::new()),
        served: Mutex::new(Vec::new()),
        paged: Mutex::new(Vec::new()),
    });
    st.temporal = recording.clone();
    st.policy = Some(Arc::new(Ruled(vec![plain()])));
    let mut firsts = Vec::new();
    for offset in [0, 100, 500] {
        let uri = format!(
            "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=2026-01-01T00:00:00Z&offsetN={offset}"
        );
        let (status, body) = send_as(&st, Some(&tenant), &uri, None).await;
        let shown = speeds(&body[0]);
        assert_eq!(shown.len(), 100, "{body}");
        firsts.push((status, shown.first().copied()));
    }
    let cut = StatusCode::PARTIAL_CONTENT;
    assert_eq!(
        firsts,
        vec![
            (cut, Some(0)),
            (cut, Some(100)),
            (StatusCode::OK, Some(500))
        ]
    );
    assert_eq!(
        *recording.served.lock().expect("lock"),
        vec![2 * 101, 2 * 201, 2 * 600],
        "each page reads its own instances and one more per Attribute"
    );
}

/// T-5616: a view's type-only rule pages the Entities in the store, through
/// the Attributes it shows (the request's own `attrs` among them), so a
/// type-wide query reads one page of Entities, not all 577 bike stations.
/// Every rule shape whose verdict on an Entity the store cannot see keeps the
/// page in the API; an idPattern the store writes exactly is one it sees
/// (T-6052).
#[tokio::test(flavor = "multi_thread")]
async fn a_type_only_rule_pages_in_the_store_through_its_attributes() {
    let speed = vec![format!("{DC}speed")];
    let opens_late = Rule {
        temporal: vec![Window {
            from: Some(TimeBound::At(
                "2026-01-01T00:00:09Z".parse().expect("instant"),
            )),
            to: None,
        }],
        ..plain()
    };
    let by_pattern = Rule {
        id_patterns: vec!["urn:ngsi-ld:Vehicle:.*".into()],
        ..plain()
    };
    // T-6052: a pattern the store writes exactly pages there too, and one
    // that misses the Entity shows nothing
    let by_exact = |p: &str| Rule {
        id_patterns: vec![p.into()],
        ..plain()
    };
    for (rule, query, paged, shown) in [
        (
            plain(),
            "type=Vehicle",
            Some(Some(speed.clone())),
            vec![7, 8, 9, 10],
        ),
        (
            plain(),
            "type=Vehicle&attrs=speed",
            Some(Some(speed.clone())),
            vec![7, 8, 9, 10],
        ),
        (
            plain(),
            "type=Vehicle&attrs=zone",
            Some(Some(Vec::new())),
            Vec::new(),
        ),
        (plain(), "type=Vehicle&q=speed%3E8", None, vec![9, 10]),
        (opens_late, "type=Vehicle", None, vec![9, 10]),
        (conditional(), "type=Vehicle", None, vec![7, 8, 9, 10]),
        (by_pattern, "type=Vehicle", None, vec![7, 8, 9, 10]),
        (
            by_exact("^urn:ngsi-ld:Vehicle:[a-z]+$"),
            "type=Vehicle",
            Some(Some(speed.clone())),
            vec![7, 8, 9, 10],
        ),
        (
            by_exact("urn:ngsi-ld:Vehicle:bus-[a-z]+"),
            "type=Vehicle",
            Some(Some(speed.clone())),
            Vec::new(),
        ),
        (plain(), "type=Vehicle,Bus", None, vec![7, 8, 9, 10]),
    ] {
        let (st, recording) = state(rule.clone()).await;
        let (status, body) = send(
            &st,
            &format!("/ngsi-ld/v1/temporal/entities?{query}&{AFTER}&limit=5"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{query} {rule:?}: {body}");
        let got = body.as_array().map_or(Vec::new(), |a| {
            a.iter().flat_map(speeds).collect::<Vec<_>>()
        });
        assert_eq!(got, shown, "{query} {rule:?}: {body}");
        assert_eq!(
            *recording.paged.lock().expect("lock"),
            vec![paged],
            "{query} {rule:?}"
        );
    }
}

/// T-5835: a view splits a type's Attribute list longer than a rule holds over
/// several rules of the same types (KeyPerformanceIndicator: 501 Attributes,
/// two rules), and an Entity shows their union. The store still pages through
/// that union, so a history read of the type reads one page of Entities, not
/// every Entity and its history (prod: 503 after 10 s). Rules of other types
/// that show another set keep the page in the API.
#[tokio::test(flavor = "multi_thread")]
async fn rules_splitting_one_list_page_in_the_store_through_their_union() {
    let zone = Rule {
        attributes: vec!["zone".into()],
        ..plain()
    };
    let zone_of_more = Rule {
        types: vec!["Vehicle".into(), "Bus".into()],
        ..zone.clone()
    };
    let both = vec![format!("{DC}speed"), format!("{DC}zone")];
    for (rules, paged) in [
        (vec![plain(), zone], Some(Some(both))),
        (vec![plain(), zone_of_more], None),
    ] {
        let (st, recording) = state_ruled(rules.clone(), &[]).await;
        let (status, body) = send(
            &st,
            &format!("/ngsi-ld/v1/temporal/entities?type=Vehicle&{AFTER}&limit=5"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{rules:?}: {body}");
        assert_eq!(speeds(&body[0]), vec![7, 8, 9, 10], "{rules:?}: {body}");
        let zones = body[0]
            .get("zone")
            .or_else(|| body[0].get(format!("{DC}zone")))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(zones, 4, "{rules:?}: {body}");
        assert_eq!(
            *recording.paged.lock().expect("lock"),
            vec![paged],
            "{rules:?}"
        );
    }
}
