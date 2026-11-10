// SPDX-License-Identifier: EUPL-1.2
//! Reads that walk a tenant do it a page at a time.
//!
//! 5.16.1.4 tells a Snapshot fill to retrieve "all pages" of its queries and
//! 5.14.4.4 tells an EntityMap to record the ids a query matched. Neither
//! says how much of the tenant may be in memory while that happens, and both
//! sit in front of the 100 000 000-Entity target: a fill that asks the store
//! for the whole match set at once, or a candidate scan that does, is bounded
//! by the tenant rather than by the broker.
//!
//! What is asserted here is the request the store receives. A backend that
//! cannot cut a page answers whole and is no worse off than before; one that
//! can is never asked to do more.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use crate::common;

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::Double;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn send(
    st: &AppState,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let b = Request::builder().method(method).uri(path);
    let req = match body {
        Some(body) => b
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body)),
        None => b.body(Body::empty()),
    }
    .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, body)
}

async fn create_vehicle(st: &AppState, id: &str) {
    let body = json!({"id": id, "type": "Vehicle",
        "speed": {"type": "Property", "value": 80}})
    .to_string();
    let (status, _, b) = send(st, "POST", "/ngsi-ld/v1/entities", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{b}");
}

/// Three Entities and a state whose store counts what it is asked for. The
/// counter starts after the seeding writes, so only the read under test is
/// in it.
async fn seeded() -> (AppState, Arc<Double>) {
    let mut st = AppState::new("antares-bounds".into());
    antares_api::wire(&mut st).await;
    for id in ["urn:ngsi-ld:Vehicle:b1", "urn:ngsi-ld:Vehicle:b2"] {
        create_vehicle(&st, id).await;
    }
    let spy = Arc::new(Double::passthrough(st.store.clone()));
    st.store = spy.clone();
    (st, spy)
}

/// 5.16.1.4: the fill retrieves every page of its query, one page at a time.
/// Buffering the match set to count it against the snapshot's budget is the
/// same read the budget exists to refuse.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_16_1_the_fill_reads_its_query_a_page_at_a_time() {
    let (st, spy) = seeded().await;
    let snap = json!({"type": "Snapshot",
        "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]})
    .to_string();
    let (status, headers, body) = send(&st, "POST", "/ngsi-ld/v1/snapshots", Some(snap)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let loc = headers
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .expect("Location header")
        .to_owned();

    let mut ready = Value::Null;
    for _ in 0..100 * antares_api::state::slow_factor() {
        let (status, _, body) = send(&st, "GET", &loc, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["snapshotStatus"] != "preparing" {
            ready = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(ready["snapshotStatus"], "success", "{ready}");
    assert_eq!(
        spy.unpaged_queries(),
        0,
        "the fill asked the store for a whole match set"
    );
}

/// 5.14.4.4: the candidate scan behind an EntityMap is bounded the same way.
/// `idPattern` is applied after the store answers (5.2.33), which is a reason
/// to keep the caller's regex — not a reason to read the tenant.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_14_4_the_candidate_scan_is_read_a_page_at_a_time() {
    let (st, spy) = seeded().await;
    let (status, _, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entityMaps?type=Vehicle&idPattern=%5Eurn:ngsi-ld:Vehicle:",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["entityMap"].as_object().map(|m| m.len()),
        Some(2),
        "{body}"
    );
    assert_eq!(
        spy.unpaged_queries(),
        0,
        "the candidate scan asked the store for the whole tenant"
    );
}

/// T-3773: the temporal candidate set behind an EntityMap, and the recheck
/// a later page runs against it, are read up to the broker's buffer bound;
/// past it the request is TooManyResults (5.5.6), not a body held whole.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_5_6_temporal_entity_map_reads_stop_at_the_byte_bound() {
    let (mut st, _) = seeded().await;
    let path = "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after\
        &timeAt=2000-01-01T00:00:00Z&timeproperty=createdAt";
    let (status, headers, body) = send(&st, "GET", &format!("{path}&entityMap=true"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let map = headers
        .get("NGSILD-EntityMap")
        .and_then(|v| v.to_str().ok())
        .expect("NGSILD-EntityMap header")
        .to_owned();
    st.max_answer_bytes = 16;
    let (status, _, body) = send(&st, "GET", &format!("{path}&entityMap=true"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the build: {body}");
    assert_eq!(body["title"], "TooManyResults", "{body}");
    assert_eq!(
        body["detail"],
        antares_api::bounds::ANSWER_TOO_LARGE,
        "{body}"
    );
    let req = Request::builder()
        .uri(path)
        .header("NGSILD-EntityMap", map)
        .body(Body::empty())
        .unwrap();
    let res = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "the recheck");
}

/// The walks above against a store that really pages: the memory arm answers
/// every query with the whole match set, so it can only prove what was asked
/// for, never what a chunked walk does with the answer. Skips without
/// ANTARES_TEST_DATABASE_URL (the container recipe is in antares-sql/tests/pg.rs).
#[cfg(feature = "postgres")]
mod paging_store {
    use super::*;
    use antares_model::TenantId;
    use antares_store::Kind;

    /// A Postgres-backed state with an empty tenant and a page size small
    /// enough that a handful of Entities spans several chunks. The tenant is
    /// the caller's own: these tests run in parallel against one database, and
    /// a shared tenant would have each of them seeding and cleaning the other's
    /// rows.
    async fn pg_state(chunk: usize, tenant_name: &str) -> Option<(AppState, TenantId)> {
        let url = match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return None;
            }
        };
        let pool = antares_sql::store::pg::connect(&url, 5)
            .await
            .expect("connect");
        let tenant = TenantId::new(tenant_name).expect("tenant");
        antares_sql::store::pg::ensure_tenant(&pool, &tenant)
            .await
            .expect("tenant row");
        let mut st = AppState::with_store(
            "antares-bounds".into(),
            Arc::new(antares_sql::store::any::AnyStore::Pg(
                antares_sql::store::any::PgBackend::new(pool),
            )),
            "postgres",
        );
        antares_api::wire(&mut st).await;
        clear(&st, &tenant).await;
        st.max_limit = chunk;
        Some((st, tenant))
    }

    /// Empties the tenant a page at a time: `list` refuses a tenant past the
    /// store's ceiling, which a seed below leaves behind if its test dies.
    async fn clear(st: &AppState, tenant: &TenantId) {
        loop {
            let page = st
                .store
                .list_page(tenant, Kind::Entity, None, 1000)
                .await
                .expect("list");
            let ids: Vec<String> = page
                .iter()
                .filter_map(|d| d["id"].as_str().map(str::to_owned))
                .collect();
            if ids.is_empty() {
                return;
            }
            st.store.batch_delete(tenant, &ids).await.expect("clean");
        }
    }

    async fn seed(st: &AppState, tenant: &TenantId, n: usize) {
        for i in 1..=n {
            st.store
                .create(
                    tenant,
                    Kind::Entity,
                    &format!("urn:ngsi-ld:Vehicle:w{i:02}"),
                    json!({"id": format!("urn:ngsi-ld:Vehicle:w{i:02}"),
                           "type": ["https://uri.etsi.org/ngsi-ld/default-context/Vehicle"]}),
                )
                .await
                .expect("seed");
        }
    }

    async fn send_t(
        st: &AppState,
        tenant: &TenantId,
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let b = Request::builder()
            .method(method)
            .uri(path)
            .header("NGSILD-Tenant", tenant.as_str());
        let req = match body {
            Some(body) => b
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body)),
            None => b.body(Body::empty()),
        }
        .expect("request");
        let res = antares_api::router(st.clone())
            .oneshot(req)
            .await
            .expect("response");
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, headers, body)
    }

    /// 5.16.1.4: "all pages are to be retrieved completely". A walk that
    /// stops at the first page copies a fraction of the match set into the
    /// snapshot and reports the query a success over it.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_16_1_the_fill_copies_every_page() {
        let Some((mut st, tenant)) = pg_state(3, "boundedfill").await else {
            return;
        };
        seed(&st, &tenant, 9).await;
        let snap = json!({"type": "Snapshot",
            "snapshotQueries": [{"type": "Query", "entities": [{"type": "Vehicle"}]}]})
        .to_string();
        let (status, headers, body) =
            send_t(&st, &tenant, "POST", "/ngsi-ld/v1/snapshots", Some(snap)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let loc = headers
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .expect("Location header")
            .to_owned();
        let mut ready = Value::Null;
        for _ in 0..200 * antares_api::state::slow_factor() {
            let (status, _, body) = send_t(&st, &tenant, "GET", &loc, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["snapshotStatus"] != "preparing" {
                ready = body;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(ready["snapshotStatus"], "success", "{ready}");
        let sid = ready["id"].as_str().expect("id").to_owned();

        // read the frozen copy in one page, not three
        st.max_limit = 1000;
        let req = Request::builder()
            .method("GET")
            .uri("/ngsi-ld/v1/entities?type=Vehicle&limit=1000")
            .header("NGSILD-Tenant", tenant.as_str())
            .header("NGSILD-Snapshot", &sid)
            .body(Body::empty())
            .expect("request");
        let res = antares_api::router(st.clone())
            .oneshot(req)
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let list: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(
            list.as_array().map(Vec::len),
            Some(9),
            "the fill stopped at a page boundary: {list}"
        );

        for doc in st.store.list(&tenant, Kind::Entity).await.expect("list") {
            if let Some(id) = doc["id"].as_str() {
                st.store
                    .delete(&tenant, Kind::Entity, id)
                    .await
                    .expect("clean");
            }
        }
    }

    /// T-5088: a geoquery the store narrows but cannot decide reads the
    /// candidates its page needs, in id order, where one flat read took up
    /// to the store's undecided ceiling for a page of 20; the page is still
    /// exact.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_geoquery_page_reads_the_candidates_of_its_page() {
        let Some((mut st, tenant)) = pg_state(1000, "boundedgeo").await else {
            return;
        };
        for i in 1..=12 {
            let body = json!({"id": format!("urn:ngsi-ld:Vehicle:g{i:02}"), "type": "Vehicle",
                "location": {"type": "GeoProperty",
                    "value": {"type": "Point", "coordinates": [f64::from(i) * 0.001, 0.0]}}})
            .to_string();
            let (status, _, b) =
                send_t(&st, &tenant, "POST", "/ngsi-ld/v1/entities", Some(body)).await;
            assert_eq!(status, StatusCode::CREATED, "{b}");
        }
        let spy = Arc::new(Double::passthrough(st.store.clone()));
        st.store = spy.clone();
        let (status, _, body) = send_t(
            &st,
            &tenant,
            "GET",
            "/ngsi-ld/v1/entities?type=Vehicle&georel=near%3BmaxDistance%3D%3D100000\
             &geometry=Point&coordinates=%5B0,0%5D&limit=2&offset=4",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids: Vec<&str> = body
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(ids, ["urn:ngsi-ld:Vehicle:g05", "urn:ngsi-ld:Vehicle:g06"]);
        // the window and one match past it: 7 of the 12 candidates
        assert_eq!(spy.rows_read(), 7, "the page read every candidate");
        for doc in st.store.list(&tenant, Kind::Entity).await.expect("list") {
            if let Some(id) = doc["id"].as_str() {
                st.store
                    .delete(&tenant, Kind::Entity, id)
                    .await
                    .expect("clean");
            }
        }
    }

    /// More points of one type inside the box than the store's undecided
    /// ceiling (`MAX_UNDECIDED_ROWS`), ids `p00001..`, and three outside it.
    const IN_BOX: usize = 10_050;
    const BOX: &str = "georel=within&geometry=Polygon\
        &coordinates=%5B%5B%5B0,0%5D,%5B1,0%5D,%5B1,1%5D,%5B0,1%5D,%5B0,0%5D%5D%5D";

    fn poi(id: &str, t: &str, x: f64) -> (String, Value) {
        (
            id.to_owned(),
            json!({"id": id, "type": [format!("https://uri.etsi.org/ngsi-ld/default-context/{t}")],
                   "https://uri.etsi.org/ngsi-ld/location": [{"type": "GeoProperty",
                       "value": {"type": "Point", "coordinates": [x, 0.5]}}]}),
        )
    }

    async fn seed_box(st: &AppState, tenant: &TenantId) {
        let mut items: Vec<(String, Value)> = (1..=IN_BOX)
            .map(|i| {
                poi(
                    &format!("urn:ngsi-ld:PointOfInterest:p{i:05}"),
                    "PointOfInterest",
                    0.1 + i as f64 * 0.00005,
                )
            })
            .collect();
        items.extend((1..=3).map(|i| {
            poi(
                &format!("urn:ngsi-ld:PointOfInterest:q{i}"),
                "PointOfInterest",
                5.0,
            )
        }));
        for chunk in items.chunks(2000) {
            st.store
                .batch_create(tenant, chunk.to_vec())
                .await
                .expect("seed");
        }
    }

    fn count_of(headers: &axum::http::HeaderMap) -> Option<u64> {
        headers
            .get("NGSILD-Results-Count")
            .and_then(|v| v.to_str().ok()?.parse().ok())
    }

    fn ids_of(body: &Value) -> Vec<&str> {
        body.as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default()
    }

    /// A counted map viewport (jc praha-mapa at 1440 px) whose box holds more
    /// than the store's undecided ceiling walks the candidates in pages and
    /// answers its page and the exact count, where one flat read refused it
    /// with TooManyResults. A read the store cannot page at all still does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_counted_geoquery_past_the_ceiling_answers_its_page() {
        let Some((st, tenant)) = pg_state(1000, "boundedgeocount").await else {
            return;
        };
        seed_box(&st, &tenant).await;
        let (status, headers, body) = send_t(
            &st,
            &tenant,
            "GET",
            &format!("/ngsi-ld/v1/entities?type=PointOfInterest&count=true&limit=200&{BOX}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids = ids_of(&body);
        assert_eq!(ids.len(), 200);
        assert_eq!(ids.first(), Some(&"urn:ngsi-ld:PointOfInterest:p00001"));
        assert_eq!(ids.last(), Some(&"urn:ngsi-ld:PointOfInterest:p00200"));
        assert_eq!(count_of(&headers), Some(IN_BOX as u64));
        // a later page of the same read is the same walk's window
        let (status, headers, body) = send_t(
            &st,
            &tenant,
            "GET",
            &format!(
                "/ngsi-ld/v1/entities?type=PointOfInterest&count=true&limit=200&offset=10000&{BOX}"
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ids_of(&body).len(), 50, "the last 50 of the box");
        assert_eq!(count_of(&headers), Some(IN_BOX as u64));
        // a geoproperty the store has no column for leaves the store blind to
        // the geoquery: one flat read of every PointOfInterest, refused (5.5.6)
        let (status, _, body) = send_t(
            &st,
            &tenant,
            "GET",
            &format!(
                "/ngsi-ld/v1/entities?type=PointOfInterest&count=true&limit=200&{BOX}\
                 &geoproperty=otherLocation"
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            body["type"]
                .as_str()
                .is_some_and(|t| t.ends_with("TooManyResults")),
            "{body}"
        );
        clear(&st, &tenant).await;
    }

    /// A view: PointOfInterest only, with its location, whatever the request
    /// selects.
    struct OnlyPoi;

    impl antares_api::policy::PolicyEngine for OnlyPoi {
        fn name(&self) -> &str {
            "only-poi"
        }

        fn decide<'a>(
            &'a self,
            _s: &'a antares_api::policy::Subject,
            op: &'a antares_api::policy::Operation<'a>,
        ) -> antares_api::policy::DecisionFuture<'a> {
            use antares_api::policy::{Decision, Filter, Rule};
            let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
                Decision::Filter(Arc::new(Filter {
                    rules: vec![Rule {
                        types: vec![
                            "https://uri.etsi.org/ngsi-ld/default-context/PointOfInterest".into(),
                        ],
                        // as a view lists them: the geoquery reads a shown member
                        attributes: vec!["https://uri.etsi.org/ngsi-ld/location".into()],
                        ..Rule::default()
                    }],
                    ..Filter::default()
                }))
            } else {
                Decision::Allow
            };
            Box::pin(std::future::ready(answer))
        }

        fn pre_notify(
            &self,
            _s: &antares_api::policy::Subject,
            _sub: &Value,
            _n: &mut Value,
        ) -> antares_api::policy::NotifyDecision {
            antares_api::policy::NotifyDecision::Deliver
        }
    }

    /// The same counted viewport under a view that serves one type: the walk
    /// counts and serves only what the view admits, and a type the view
    /// leaves out stays out of the page and the count, inside the box or not.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_counted_geoquery_past_the_ceiling_keeps_the_narrowing() {
        let Some((st, tenant)) = pg_state(1000, "boundedgeoview").await else {
            return;
        };
        seed_box(&st, &tenant).await;
        let hidden: Vec<(String, Value)> = (1..=5)
            .map(|i| poi(&format!("urn:ngsi-ld:Secret:s{i}"), "Secret", 0.2))
            .collect();
        st.store.batch_create(&tenant, hidden).await.expect("seed");
        let st = st.with_policy(Arc::new(OnlyPoi));
        let (status, headers, body) = send_t(
            &st,
            &tenant,
            "GET",
            &format!("/ngsi-ld/v1/entities?type=PointOfInterest,Secret&count=true&limit=200&{BOX}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids = ids_of(&body);
        assert_eq!(ids.len(), 200);
        assert!(ids.iter().all(|i| !i.contains(":Secret:")), "{ids:?}");
        assert_eq!(count_of(&headers), Some(IN_BOX as u64));
        let (status, headers, body) = send_t(
            &st,
            &tenant,
            "GET",
            &format!("/ngsi-ld/v1/entities?type=Secret&count=true&limit=200&{BOX}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ids_of(&body), Vec::<&str>::new());
        assert_eq!(count_of(&headers), Some(0));
        clear(&st, &tenant).await;
    }

    /// 5.14.4.4: the candidate ids are the query's matches, and an idPattern
    /// is applied after the store answers — so a chunk can come back with
    /// nothing in it while every match is still waiting behind it.
    #[tokio::test(flavor = "multi_thread")]
    async fn clause_5_14_4_the_candidate_walk_reaches_the_last_chunk() {
        let Some((st, tenant)) = pg_state(3, "boundedmap").await else {
            return;
        };
        seed(&st, &tenant, 9).await;
        let (status, _, body) = send_t(
            &st,
            &tenant,
            "GET",
            "/ngsi-ld/v1/entityMaps?type=Vehicle&idPattern=w0%5B89%5D$",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(
            body["entityMap"].as_object().map(|m| m.len()),
            Some(2),
            "the walk gave up before the chunk holding the matches: {body}"
        );
        for doc in st.store.list(&tenant, Kind::Entity).await.expect("list") {
            if let Some(id) = doc["id"].as_str() {
                st.store
                    .delete(&tenant, Kind::Entity, id)
                    .await
                    .expect("clean");
            }
        }
    }
}

/// 5.8.6: a periodic Subscription whose stored `q` no longer parses (one
/// stored percent-encoded before 5.2.12's read-as-written, T-3564) matches
/// nothing, as `conditions_match` decides, so the sweep does not read the
/// Entities of its types to throw them all away.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_8_6_a_periodic_subscription_with_an_unparseable_q_reads_nothing() {
    let (st, spy) = seeded().await;
    let tenant = antares_model::TenantId::new("default").expect("tenant");
    let id = "urn:ngsi-ld:Subscription:stale-q";
    let created = chrono::Utc::now() - chrono::Duration::seconds(10);
    let sub = json!({"id": id, "type": "Subscription", "timeInterval": 1,
        "createdAt": created.to_rfc3339(),
        "entities": [{"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"}],
        "q": "speed%3E10%29",
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"}}});
    st.store
        .create(&tenant, antares_store::Kind::Subscription, id, sub.clone())
        .await
        .expect("subscription row");
    // the sweep reads the indexed mirror the write path keeps
    if let Some(m) = &st.sub_mirror {
        m.apply(tenant.as_str(), id, Some(sub));
    }

    antares_api::notify::interval_tick(&st).await;

    assert_eq!(
        spy.unpaged_queries(),
        0,
        "the sweep read the Entities for a q that matches nothing"
    );
}

/// F-ANT3-009: under bus=nats the claim stamps `lastNotification`; a stored
/// `q` that no longer parses sends nothing, so it claims nothing either.
#[tokio::test(flavor = "multi_thread")]
async fn an_unparseable_q_claims_no_periodic_firing() {
    let mut st = AppState::new("test".into());
    st.nats = true;
    let tenant = antares_model::TenantId::new("default").expect("tenant");
    let id = "urn:ngsi-ld:Subscription:stale-q-claim";
    let created = chrono::Utc::now() - chrono::Duration::seconds(10);
    let sub = json!({"id": id, "type": "Subscription", "timeInterval": 1,
        "createdAt": created.to_rfc3339(),
        "entities": [{"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"}],
        "q": "((",
        "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"}}});
    st.store
        .create(&tenant, antares_store::Kind::Subscription, id, sub.clone())
        .await
        .expect("subscription row");
    if let Some(m) = &st.sub_mirror {
        m.apply(tenant.as_str(), id, Some(sub));
    }

    antares_api::notify::interval_tick(&st).await;

    let stored = st
        .store
        .get(&tenant, antares_store::Kind::Subscription, id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(stored.pointer("/notification/lastNotification"), None);
}
