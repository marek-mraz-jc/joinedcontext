// SPDX-License-Identifier: EUPL-1.2
//! `/q/erasures/{tenant}` (T-4665, platform ADR-N-054 2.6): an erasure leaves
//! no value of the data subject in the current state, the history (every
//! version) or the dead letters, keeps a content-free tombstone, and a pod
//! that starts over restored data erases it again, but not what was written
//! after the erasure. Runs on the memory store, and on Postgres when
//! `ANTARES_TEST_DATABASE_URL` is set (recipe in antares-sql/tests/pg.rs).

use antares_api::AppState;
use antares_model::TenantId;
use antares_ops::test_kit::TestBroker;
use antares_sql::sqlx;
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_sql::store::Kind;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TRIP: &str = "https://example.org/BikeTrip";
const RIDER: &str = "https://example.org/rider";
const STATION: &str = "https://example.org/station";
const A: &str = "urn:ngsi-ld:BikeTrip:a";
const B: &str = "urn:ngsi-ld:BikeTrip:b";
const T1: &str = "2026-01-01T07:00:00Z";
const T2: &str = "2026-01-01T08:00:00Z";

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let body = body.map(|b| b.to_string());
    if let Some(b) = &body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default()))
                .unwrap_or_default(),
        )
        .await;
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

fn prop(value: &str, at: &str) -> Value {
    json!({"type": "Property", "value": value, "observedAt": at})
}

#[expect(
    clippy::expect_used,
    reason = "a test's broker; a failure ends the test"
)]
async fn broker(store: &Arc<AnyStore>, kind: &str) -> (TestBroker, Router) {
    let state = AppState::with_drivers("antares-test".into(), store.clone(), store.clone(), kind);
    let b = TestBroker::over(state).await.expect("broker");
    let app = b.serve_mounted(Vec::new()).expect("app");
    (b, app)
}

#[expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "one test body, run on each store"
)]
/// The scenario on one store; `rows` counts the rider's stored rows, current
/// and superseded, where the store can say (Postgres).
async fn erasure_over<F, Fut>(store: Arc<AnyStore>, kind: &str, tenant: &str, rows: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<i64>>,
{
    let (_b, app) = broker(&store, kind).await;
    let t = Some(tenant);
    let entities = "/ngsi-ld/v1/entities";
    for (id, body) in [
        (
            A,
            json!({RIDER: prop("r-17", T1), STATION: prop("s-9", T1)}),
        ),
        (B, json!({RIDER: prop("r-17", T1)})),
    ] {
        let mut doc = body;
        doc["id"] = json!(id);
        doc["type"] = json!(TRIP);
        let (s, b) = send(&app, Method::POST, entities, t, Some(doc)).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    // a correction (a version on Postgres) and a later value of the rider
    for at in [T1, T2] {
        let (s, b) = send(
            &app,
            Method::PATCH,
            &format!("{entities}/{A}/attrs"),
            t,
            Some(json!({RIDER: prop("r-17b", at)})),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    }
    let tid = TenantId::new(tenant).expect("tenant");
    // dl-c notified another trip that only points at A: it stays
    for (letter, about) in [("dl-a", A), ("dl-c", "urn:ngsi-ld:BikeTrip:c")] {
        let data = json!([{"id": about, RIDER: "r-17",
                           "https://example.org/follows": {"type": "Relationship", "object": A}}]);
        store
            .upsert(
                &tid,
                Kind::DeadLetter,
                letter,
                json!({"id": letter, "accept": "application/json",
                       "payload": {"type": "Notification", "data": data}}),
            )
            .await
            .expect("dead letter");
    }

    // Refused: no reason, a value as subject.
    let erasures = format!("/q/erasures/{tenant}");
    let refused = json!({"entities": [A], "actor": "steward", "reason": ""});
    let (s, _) = send(&app, Method::POST, &erasures, None, Some(refused)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let by_attr = json!({"entities": [A], "attrs": [RIDER], "subject": "5e884898da280471",
                         "actor": "steward-1", "reason": "Art. 17 request 2026-10-18"});
    let (s, first) = send(&app, Method::POST, &erasures, None, Some(by_attr)).await;
    assert_eq!(s, StatusCode::CREATED, "{first}");
    assert_eq!(first["counts"]["entities"], 1, "{first}");
    assert_eq!(first["counts"]["deadLetters"], 1, "{first}");
    assert!(first["counts"]["history"].as_u64() >= Some(2), "{first}");
    let whole = json!({"entities": [B], "actor": "steward-1", "reason": "Art. 17"});
    let (s, second) = send(&app, Method::POST, &erasures, None, Some(whole)).await;
    assert_eq!(s, StatusCode::CREATED, "{second}");

    // Nothing of the rider is left anywhere; A keeps its station.
    let (s, a) = send(&app, Method::GET, &format!("{entities}/{A}"), t, None).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert!(a.get(RIDER).is_none() && a.get(STATION).is_some(), "{a}");
    let temporal = format!("/ngsi-ld/v1/temporal/entities/{A}");
    let (s, history) = send(&app, Method::GET, &temporal, t, None).await;
    assert_eq!(s, StatusCode::OK, "{history}");
    assert!(!history.to_string().contains("r-17"), "{history}");
    assert!(history.get(STATION).is_some(), "{history}");
    for uri in [
        format!("{entities}/{B}"),
        format!("/ngsi-ld/v1/temporal/entities/{B}"),
    ] {
        assert_eq!(
            send(&app, Method::GET, &uri, t, None).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    let letter = |id: &'static str| {
        let (store, tid) = (store.clone(), tid.clone());
        async move { store.get(&tid, Kind::DeadLetter, id).await.expect("read") }
    };
    assert!(letter("dl-a").await.is_none());
    assert!(letter("dl-c").await.is_some());
    assert_eq!(
        rows().await.unwrap_or(0),
        0,
        "a version of the rider is left"
    );

    // One tombstone per erasure: who, why, when, counts, never a value.
    let (s, log) = send(&app, Method::GET, &erasures, None, None).await;
    assert_eq!(s, StatusCode::OK);
    let log = log.as_array().expect("records").clone();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log
        .iter()
        .all(|r| r["operation"] == "erase" && r["actor"] == "steward-1"));
    assert!(!Value::Array(log.clone()).to_string().contains("r-17"));

    // A restore brings the old rows back; after the erasure the rider was
    // written again, which stays.
    let later = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(1))
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (s, b) = send(
        &app,
        Method::PATCH,
        &format!("{entities}/{A}/attrs"),
        t,
        Some(json!({RIDER: prop("r-new", &later)})),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let restored = json!({RIDER: [prop("r-17", T1)]});
    let shell = json!({"id": A, "type": [TRIP]});
    store
        .temporal_append(&tid, A, &shell, &restored)
        .await
        .expect("restored history");
    store
        .upsert(
            &tid,
            Kind::Entity,
            B,
            json!({"id": B, "type": [TRIP], "createdAt": T1, "modifiedAt": T1,
                   RIDER: prop("r-17", T1)}),
        )
        .await
        .expect("restored entity");
    let (_again, app) = broker(&store, kind).await;
    let (s, history) = send(&app, Method::GET, &temporal, t, None).await;
    assert_eq!(s, StatusCode::OK, "{history}");
    let text = history.to_string();
    assert!(
        !text.contains("r-17") && text.contains("r-new"),
        "{history}"
    );
    let (_, a) = send(&app, Method::GET, &format!("{entities}/{A}"), t, None).await;
    assert_eq!(a[RIDER]["value"], "r-new", "{a}");
    assert_eq!(
        send(&app, Method::GET, &format!("{entities}/{B}"), t, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

fn run_id() -> String {
    antares_api::erasure::record_id()[..12].to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_erasure_leaves_no_value_and_a_restart_erases_restored_data_on_memory() {
    let store = Arc::new(AnyStore::Mem(antares_sql::store::Store::default()));
    erasure_over(store, "memory", &format!("er{}", run_id()), || async {
        None
    })
    .await;
}

#[expect(clippy::print_stderr, reason = "a skipped database test says so")]
#[tokio::test(flavor = "multi_thread")]
async fn an_erasure_leaves_no_value_and_a_restart_erases_restored_data_on_postgres() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store = Arc::new(AnyStore::Pg(PgBackend::new(pool.clone())));
    let tenant = format!("er{}", run_id());
    let rows = || {
        let (pool, tenant) = (pool.clone(), tenant.clone());
        async move {
            let mut n = 0i64;
            for sql in [
                "SELECT count(*) FROM attr_instances WHERE tenant_id = $1 AND attr_id = $2",
                "SELECT count(*) FROM attr_versions WHERE tenant_id = $1 AND attr_id = $2",
            ] {
                let mut tx = pool.begin().await.expect("tx");
                sqlx::query("SELECT set_config('antares.tenant', $1, true)")
                    .bind(&tenant)
                    .execute(&mut *tx)
                    .await
                    .expect("tenant");
                let c: i64 = sqlx::query_scalar(sql)
                    .bind(&tenant)
                    .bind(RIDER)
                    .fetch_one(&mut *tx)
                    .await
                    .expect("count");
                n = n.saturating_add(c);
            }
            Some(n)
        }
    };
    erasure_over(store, "postgres", &tenant, rows).await;
}

/// A tenant that is no tenant, a body past the limit or not a request is
/// refused before anything is stored; a pod that starts over a record it
/// cannot read skips it, and over a sound store erases nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_request_is_refused_and_a_broken_record_is_skipped() {
    let store = Arc::new(AnyStore::Mem(antares_sql::store::Store::default()));
    let (_b, app) = broker(&store, "memory").await;
    let tenant = format!("er{}", run_id());
    let erasures = format!("/q/erasures/{tenant}");
    let ok = json!({"entities": [A], "actor": "steward", "reason": "Art. 17"});
    for (uri, body) in [
        ("/q/erasures/a%20b", Some(ok.clone())),
        ("/q/erasures/a%20b", None),
        (
            erasures.as_str(),
            Some(json!({"pad": "x".repeat(300 * 1024)})),
        ),
        (erasures.as_str(), Some(json!("not a request"))),
    ] {
        let method = if body.is_some() {
            Method::POST
        } else {
            Method::GET
        };
        let (s, b) = send(&app, method, uri, None, body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{uri}: {b}");
    }
    let (s, log) = send(&app, Method::GET, &erasures, None, None).await;
    assert_eq!((s, log), (StatusCode::OK, json!([])));

    let (s, b) = send(&app, Method::POST, &erasures, None, Some(ok)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    store
        .upsert(
            &TenantId::new_internal(antares_api::views::VIEW_TENANT).expect("view tenant"),
            Kind::View,
            &format!("erasure:{tenant}:broken"),
            json!({"id": format!("erasure:{tenant}:broken"), "operation": "erase"}),
        )
        .await
        .expect("broken record");
    let (_again, app) = broker(&store, "memory").await;
    let (s, log) = send(&app, Method::GET, &erasures, None, None).await;
    assert_eq!(s, StatusCode::OK, "{log}");
    assert_eq!(log.as_array().map(Vec::len), Some(2), "{log}");
}

/// A re-applied erasure whose own time does not parse erases nothing: read
/// as no cut, it would take what was written after the erasure.
#[tokio::test(flavor = "multi_thread")]
async fn an_erasure_time_that_does_not_parse_erases_nothing() {
    let store = Arc::new(AnyStore::Mem(antares_sql::store::Store::default()));
    let (_b, app) = broker(&store, "memory").await;
    let tenant = format!("er{}", run_id());
    let doc = json!({"id": A, "type": TRIP, RIDER: prop("r-17", T1)});
    let (s, b) = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some(&tenant),
        Some(doc),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let st = AppState::with_drivers(
        "antares-test".into(),
        store.clone(),
        store.clone(),
        "memory",
    );
    let tid = TenantId::new(&tenant).expect("tenant");
    let ids = [A.to_owned()];
    let out = antares_api::erasure::erase(&st, &tid, &ids, None, Some("yesterday")).await;
    assert!(out.is_err(), "{out:?}");
    let kept = store.get(&tid, Kind::Entity, A).await.expect("read");
    assert!(kept.is_some());
}
