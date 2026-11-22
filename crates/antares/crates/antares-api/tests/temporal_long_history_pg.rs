// SPDX-License-Identifier: EUPL-1.2
//! T-7526: histories of 10 and 100 years through a view, on Postgres. A public
//! view always carries rules, and an aggregated read through one used to
//! rebuild every instance of the window in the broker; monthly and yearly
//! periods did so for every caller. The store computes them now, under the
//! view's own scope: only the Attributes it shows, only inside its Policy's
//! window, never an expired instance.
//! Skips loudly without `ANTARES_TEST_DATABASE_URL`.
#![cfg(feature = "postgres")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
    TimeBound, Window,
};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::{filter, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
/// What one long aggregated read may take here: the aggregate's statement budget.
const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "temporal-long-history-pg"
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

/// `Decade` holds `level` hourly 2016 to 2025, `Century` daily 1926 to 2025, each
/// value the year it was observed in. Beside them a text `name` and a numeric
/// `hidden` no view shows, and one expired `level` in 2020.
async fn seeded() -> Option<(antares_sql::sqlx::PgPool, String)> {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = format!("tt{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let tid = TenantId::new(&tenant).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tid)
        .await
        .expect("tenant row");
    let mut tx = pool.begin().await.expect("tx");
    antares_sql::sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(&tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    // the seed is setup, not the read under test (T-5661)
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    for stmt in [
        "INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:' || t || ':1', ARRAY[$2 || t],
                jsonb_build_object('id', 'urn:ngsi-ld:' || t || ':1', 'type', jsonb_build_array($2 || t)), now(), now()
           FROM unnest(ARRAY['Decade', 'Century']) AS t",
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                     created_at, modified_at, data)
         SELECT $1, 'urn:ngsi-ld:' || t || ':1', $2 || a, 'urn:i:' || gen_random_uuid(), at, now(), now(),
                jsonb_build_object('type', 'Property', 'observedAt', to_char(at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
                                   'value', extract(year FROM at AT TIME ZONE 'UTC')::int)
           FROM (SELECT 'Decade' AS t, generate_series('2016-01-01T00:00:00Z'::timestamptz, '2025-12-31T23:00:00Z', '1 hour') AS at
                 UNION ALL
                 SELECT 'Century', generate_series('1926-01-01T00:00:00Z'::timestamptz, '2025-12-31T00:00:00Z', '24 hours')) s,
                unnest(ARRAY['level', 'hidden']) AS a",
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                     created_at, modified_at, data)
         SELECT $1, 'urn:ngsi-ld:Decade:1', $2 || a, 'urn:i:' || gen_random_uuid(), '2020-06-15T12:30:00Z', now(), now(), d
           FROM (VALUES ('name', '{\"type\":\"Property\",\"value\":\"Hron\",\"observedAt\":\"2020-06-15T12:30:00Z\"}'::jsonb),
                        ('level', '{\"type\":\"Property\",\"value\":1000000,\"observedAt\":\"2020-06-15T12:30:00Z\",\"expiresAt\":\"2020-07-01T00:00:00Z\"}'::jsonb)) v(a, d)",
        "INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:Edge:1', ARRAY[$2 || 'Edge', $2 || 'Meter'],
                jsonb_build_object('id', 'urn:ngsi-ld:Edge:1', 'type', jsonb_build_array($2 || 'Edge', $2 || 'Meter')), now(), now()",
        // month ends, millisecond edges, a second dataset, a deletion, an instance with
        // no observedAt and an expired one
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, dataset_id, observed_at,
                                     created_at, modified_at, deleted_at, data)
         SELECT $1, 'urn:ngsi-ld:Edge:1', $2 || 'level', 'urn:i:' || gen_random_uuid(),
                COALESCE(d ->> 'datasetId', ''), COALESCE(d ->> 'observedAt', d ->> 'deletedAt', '2024-03-20T00:00:00Z')::timestamptz, now(), now(),
                (d ->> 'deletedAt')::timestamptz, jsonb_build_object('type', 'Property') || d
           FROM (VALUES ('{\"value\":1.25,\"observedAt\":\"2023-12-31T23:59:59.999Z\"}'::jsonb),
                        ('{\"value\":-2,\"observedAt\":\"2024-01-31T00:00:00Z\"}'),
                        ('{\"value\":3,\"observedAt\":\"2024-02-28T23:59:59.999Z\"}'),
                        ('{\"value\":4.5,\"observedAt\":\"2024-02-29T00:00:00Z\"}'),
                        ('{\"value\":6,\"observedAt\":\"2024-03-30T23:59:59Z\"}'),
                        ('{\"value\":7,\"observedAt\":\"2024-03-31T00:00:00Z\"}'),
                        ('{\"value\":0.1,\"observedAt\":\"2024-04-30T00:00:00Z\"}'),
                        ('{\"value\":0.2,\"observedAt\":\"2024-04-30T00:00:00.001Z\"}'),
                        ('{\"value\":9,\"observedAt\":\"2025-02-28T12:00:00Z\"}'),
                        ('{\"value\":40,\"observedAt\":\"2024-02-10T00:00:00Z\",\"datasetId\":\"urn:ngsi-ld:Dataset:b\"}'),
                        ('{\"value\":50,\"observedAt\":\"2024-05-10T00:00:00Z\",\"datasetId\":\"urn:ngsi-ld:Dataset:b\"}'),
                        ('{\"value\":\"urn:ngsi-ld:null\",\"deletedAt\":\"2024-06-01T00:00:00Z\"}'),
                        ('{\"value\":77}'),
                        ('{\"value\":88,\"observedAt\":\"2024-03-15T00:00:00Z\",\"expiresAt\":\"2024-04-01T00:00:00Z\"}')) v(d)",
    ] {
        antares_sql::sqlx::query(stmt)
            .bind(&tenant)
            .bind(DEFAULT)
            .execute(&mut *tx)
            .await
            .expect(stmt);
    }
    tx.commit().await.expect("commit");
    // the tests of this file seed at once, and two ANALYZEs of one table wait on each other
    let mut tx = pool.begin().await.expect("tx");
    for stmt in [
        "SET LOCAL lock_timeout = 0",
        "SET LOCAL statement_timeout = 0",
        "ANALYZE attr_instances",
    ] {
        antares_sql::sqlx::query(stmt)
            .execute(&mut *tx)
            .await
            .expect(stmt);
    }
    tx.commit().await.expect("analyze");
    Some((pool, tenant))
}

async fn cleaned(pool: &antares_sql::sqlx::PgPool, tenant: &str) {
    for stmt in [
        "DELETE FROM attr_instances WHERE tenant_id = $1",
        "DELETE FROM temporal_entities WHERE tenant_id = $1",
    ] {
        antares_sql::sqlx::query(stmt)
            .bind(tenant)
            .execute(pool)
            .await
            .expect("clean");
    }
}

/// Records, for each temporal query, whether the store answered it aggregated,
/// and whether the read carried a deadline.
struct Store {
    inner: Arc<dyn TemporalDriver>,
    aggregated: Mutex<Vec<bool>>,
    bounded: Mutex<Vec<bool>>,
}

#[async_trait::async_trait]
impl TemporalDriver for Store {
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
        self.bounded
            .lock()
            .expect("lock")
            .push(f.deadline.is_some());
        let out = self.inner.query_temporal(t, f).await?;
        self.aggregated.lock().expect("lock").push(out.aggregated);
        Ok(out)
    }
    async fn get_temporal(
        &self,
        t: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
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

impl Store {
    /// What the store answered since the last call: aggregated or not.
    fn answered(&self) -> Vec<bool> {
        std::mem::take(&mut *self.aggregated.lock().expect("lock"))
    }

    /// Whether each read since the last call carried a deadline.
    fn bounded(&self) -> Vec<bool> {
        std::mem::take(&mut *self.bounded.lock().expect("lock"))
    }
}

/// A broker over the fixture whose view shows `attributes` of both types, inside `window`.
/// The window is the rules' alone, as the policy engine writes a Policy's.
async fn view(pool: &antares_sql::sqlx::PgPool, window: Option<Window>) -> (AppState, Arc<Store>) {
    let filter = Filter {
        rules: vec![Rule {
            types: vec![iri("Decade"), iri("Century"), iri("Edge")],
            attributes: vec![iri("level"), iri("name")],
            temporal: window.into_iter().collect(),
            ..Rule::default()
        }],
        ..Filter::default()
    };
    ruled(pool, filter).await
}

/// A broker over the fixture whose view holds `filter`.
async fn ruled(pool: &antares_sql::sqlx::PgPool, filter: Filter) -> (AppState, Arc<Store>) {
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(filter)));
    antares_api::wire(&mut st).await;
    let store = Arc::new(Store {
        inner: st.temporal.clone(),
        aggregated: Mutex::new(Vec::new()),
        bounded: Mutex::new(Vec::new()),
    });
    st.temporal = store.clone();
    (st, store)
}

async fn read(
    st: &AppState,
    tenant: &str,
    query: &str,
) -> (StatusCode, Value, std::time::Duration) {
    let (code, _, body, took) = answered(st, tenant, query).await;
    (code, body, took)
}

async fn answered(
    st: &AppState,
    tenant: &str,
    query: &str,
) -> (StatusCode, Vec<String>, Value, std::time::Duration) {
    let req = Request::builder()
        .uri(format!(
            "/ngsi-ld/v1/temporal/entities?{query}&format=aggregatedValues&limit=1000"
        ))
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .expect("request");
    let began = std::time::Instant::now();
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let warnings = res
        .headers()
        .get_all("NGSILD-Warning")
        .iter()
        .map(|v| v.to_str().unwrap_or_default().to_owned())
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let took = began.elapsed();
    (
        status,
        warnings,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        took,
    )
}

/// `[value, start, end]` of `method` in the one Entity of the answer.
fn buckets(body: &Value, method: &str) -> Vec<(f64, String, String)> {
    body[0]["level"][method]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    let text = |i: usize| r[i].as_str().unwrap_or_default().to_owned();
                    (r[0].as_f64().unwrap_or(f64::NAN), text(1), text(2))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn ten_and_a_hundred_years_aggregate_in_the_store_within_the_budget() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let (st, store) = view(&pool, None).await;
    for (kind, from, period, count, instances) in [
        ("Decade", 2016, "P1D", 3653, 87_672.0),
        ("Decade", 2016, "P1M", 120, 87_672.0),
        ("Decade", 2016, "P1Y", 10, 87_672.0),
        ("Century", 1926, "P1M", 1200, 36_525.0),
        ("Century", 1926, "P1Y", 100, 36_525.0),
    ] {
        let query = format!(
            "type={kind}&attrs=level&aggrMethods=avg,max,totalCount&aggrPeriodDuration={period}\
             &timerel=between&timeAt={from}-01-01T00:00:00Z&endTimeAt=2026-01-01T00:00:00Z"
        );
        let (code, body, took) = read(&st, &tenant, &query).await;
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        assert_eq!(store.answered(), [true], "{query}: the store aggregates");
        eprintln!("{kind} {period}: {took:?}");
        let (avg, max) = (buckets(&body, "avg"), buckets(&body, "max"));
        assert_eq!(avg.len(), count, "{query}");
        assert_eq!(avg[0].1, format!("{from}-01-01T00:00:00Z"), "{query}");
        assert_eq!(avg[count - 1].2, "2026-01-01T00:00:00Z", "{query}");
        // every value is its year: no expired one, no other Attribute's
        for (value, start, _) in avg.iter().chain(&max) {
            assert_eq!(format!("{value:.0}"), start[..4], "{query}");
        }
        let total: f64 = buckets(&body, "totalCount").iter().map(|b| b.0).sum();
        assert_eq!(total, instances, "{query}");
        assert!(body[0].get("hidden").is_none(), "{query}: {body}");
        assert!(took < BUDGET, "{query} took {took:?}");
    }
    cleaned(&pool, &tenant).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_instance_before_the_policys_start_is_aggregated() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let start = "2025-12-02T06:00:00Z";
    let grant = Window {
        from: Some(TimeBound::At(start.parse().expect("instant"))),
        to: None,
    };
    let (st, store) = view(&pool, Some(grant)).await;
    for (period, count, first) in [
        ("P1D", 30, "2025-12-02T00:00:00Z"),
        ("P1M", 1, "2025-12-01T00:00:00Z"),
        ("P1Y", 1, "2025-01-01T00:00:00Z"),
    ] {
        let query = format!(
            "type=Decade&attrs=level&aggrMethods=min,totalCount&aggrPeriodDuration={period}\
             &timerel=between&timeAt=1926-01-01T00:00:00Z&endTimeAt=2026-01-01T00:00:00Z"
        );
        let (code, body, _) = read(&st, &tenant, &query).await;
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        let total = buckets(&body, "totalCount");
        assert_eq!(total.len(), count, "{query}");
        assert_eq!(
            total[0].1, first,
            "{query}: the periods count from the asked timeAt"
        );
        assert_eq!(store.answered(), [true], "{query}: the store aggregates");
        // 06:00 on 2 December to midnight of the year's end
        assert_eq!(total.iter().map(|b| b.0).sum::<f64>(), 714.0, "{query}");
        assert!(
            buckets(&body, "min").iter().all(|b| b.0 == 2025.0),
            "{query}"
        );
    }
    let hidden = "type=Decade&attrs=hidden&aggrMethods=avg&aggrPeriodDuration=P1Y\
                  &timerel=between&timeAt=1926-01-01T00:00:00Z&endTimeAt=2026-01-01T00:00:00Z";
    let (code, body, _) = read(&st, &tenant, hidden).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body, serde_json::json!([]), "an Attribute the view hides");
    cleaned(&pool, &tenant).await;
}

/// Equal but for the last digits of a float.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(1.0),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The instances an answer counts.
fn counted(body: &Value) -> Value {
    let total: f64 = buckets(body, "totalCount").iter().map(|b| b.0).sum();
    serde_json::json!(total)
}

/// The same read of an Entity whose one Attribute is `level`, rebuilt in the broker:
/// `omit` keeps the answer and takes the store's aggregation away.
fn unpushed(query: &str) -> String {
    format!("{}&omit=zzz", query.replace("&attrs=level", ""))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_store_answers_what_the_rules_leave_of_each_instance() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let at = |t: &str| TimeBound::At(t.parse().expect("instant"));
    let ago = TimeBound::before_now(jiff::SignedDuration::from_hours(24 * 900)).expect("duration");
    let windows = [
        None,
        Some((Some(at("2024-01-31T00:00:00Z")), None)),
        Some((
            Some(at("2024-02-10T06:00:00Z")),
            Some(at("2024-04-30T00:00:00Z")),
        )),
        Some((None, Some(at("2024-03-31T00:00:00Z")))),
        // the grant opens after the range asked has ended
        Some((Some(at("2029-01-01T00:00:00Z")), None)),
        Some((Some(ago), None)),
    ];
    let asked = [
        "timerel=between&timeAt=2023-12-31T00:00:00Z&endTimeAt=2025-06-01T00:00:00Z",
        "timerel=between&timeAt=2024-01-31T00:00:00Z&endTimeAt=2024-04-30T00:00:00Z",
        "timerel=after&timeAt=2020-02-29T00:00:00Z",
        "timerel=before&timeAt=2025-03-31T00:00:00Z",
        "timerel=between&timeAt=2024-01-31T00:00:00.500Z&endTimeAt=2024-05-01T00:00:00Z",
    ];
    for window in windows {
        let (st, store) = view(&pool, window.map(|(from, to)| Window { from, to })).await;
        for range in asked {
            for period in ["PT6H", "P1D", "P1M", "P3M", "P1Y", "PT0S"] {
                let query = format!(
                    "type=Edge&attrs=level&aggrMethods=avg,min,max,sum,totalCount,distinctCount\
                     &aggrPeriodDuration={period}&{range}"
                );
                let (code, warned, body, _) = answered(&st, &tenant, &query).await;
                let pushed = store.answered() == [true];
                let (as_rebuilt, marked, rebuilt, _) =
                    answered(&st, &tenant, &unpushed(&query)).await;
                assert_eq!(
                    store.answered(),
                    [false],
                    "{query}: the reference is rebuilt"
                );
                assert_eq!(code, as_rebuilt, "{window:?} {query}");
                assert_eq!(warned, marked, "{window:?} {query}");
                // counting back from timeAt by seconds, the broker's `seconds_bucket`
                // drops the milliseconds towards timeAt (chyby.md, T-7526): there
                // the two agree on what is counted
                let (store_says, broker_says) =
                    if range.contains("=before") && matches!(period, "PT6H" | "P1D") {
                        (counted(&body), counted(&rebuilt))
                    } else {
                        (body, rebuilt)
                    };
                assert!(
                    same(&store_says, &broker_says),
                    "{window:?} {query}: {store_says} != {broker_says}"
                );
                let whole = period == "PT0S";
                assert_eq!(pushed, !(whole && window.is_some()), "{window:?} {query}");
            }
        }
    }
    cleaned(&pool, &tenant).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_held_by_a_type_not_asked_for_is_still_carried() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let (st, store) = view(&pool, None).await;
    let on_change = antares_api::state::OnChange {
        heartbeat: std::time::Duration::from_secs(40 * 86_400),
        deadband: None,
        min_distance: None,
    };
    st.history_off.write().expect("lock").recording.insert(
        (tenant.clone(), iri("Meter")),
        antares_api::state::Recording {
            default: Some(antares_api::state::RecordMode::OnChange(on_change)),
            ..Default::default()
        },
    );
    let query = "type=Edge&attrs=level&aggrMethods=avg&aggrPeriodDuration=P1M\
                 &timerel=between&timeAt=2024-01-01T00:00:00Z&endTimeAt=2024-07-01T00:00:00Z";
    let (code, body, _) = read(&st, &tenant, query).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        store.answered(),
        [true, false],
        "the page is read again, rebuilt"
    );
    let (_, rebuilt, _) = read(&st, &tenant, &unpushed(query)).await;
    assert!(same(&body, &rebuilt), "{body} != {rebuilt}");
    cleaned(&pool, &tenant).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_aggregation_over_its_budget_says_to_ask_for_less() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let (st, _) = view(&pool, None).await;
    let types = [iri("Decade")];
    let attrs = [iri("level")];
    let methods = ["avg".to_owned()];
    let asked = filter::TemporalFilter {
        types: Some(&types),
        aggregate: Some(filter::Aggregate {
            methods: &methods,
            period: filter::Period::Months(1),
            anchor: Some("2016-01-01T00:00:00Z"),
            attrs: Some(&attrs),
        }),
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_millis(1)),
        ..Default::default()
    };
    let tid = TenantId::new(&tenant).expect("tenant");
    match st.temporal.query_temporal(&tid, &asked).await {
        Err(NgsiError::TooManyResults(said)) => assert!(said.contains("shorter window"), "{said}"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("87,672 instances aggregated within a millisecond"),
    }
    // the instances the broker rebuilds itself are read under the same deadline
    let raw = filter::TemporalFilter {
        types: Some(&types),
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    match st.temporal.query_temporal(&tid, &raw).await {
        Err(NgsiError::TooManyResults(said)) => assert!(said.contains("shorter window"), "{said}"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("a read past its deadline answered"),
    }
    // the budget was that transaction's alone
    let left: String = antares_sql::sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&pool)
        .await
        .expect("show");
    assert_ne!(left, "1ms");
    cleaned(&pool, &tenant).await;
}

/// One rule per Attribute list, types grouped, each list cut to `MAX_NAMES`: the
/// rules the Portal's reconciler writes for a view (views.rs `rules`).
fn reconciled(shown: &[(&str, &[&str], Option<Window>)]) -> Filter {
    let mut rules: Vec<Rule> = Vec::new();
    for (kind, attributes, window) in shown {
        let attributes: Vec<String> = attributes.iter().map(|a| iri(a)).collect();
        let temporal: Vec<Window> = (*window).into_iter().collect();
        match rules
            .iter_mut()
            .find(|r| r.attributes == attributes && r.temporal == temporal)
        {
            Some(rule) => rule.types.push(iri(kind)),
            None => rules.push(Rule {
                types: vec![iri(kind)],
                attributes,
                temporal,
                ..Rule::default()
            }),
        }
    }
    Filter {
        rules,
        ..Filter::default()
    }
}

/// T-7526 review: a view of several types, as the Portal writes it, aggregates in the
/// store when the rules of the types asked decide alone, also for an Entity of two
/// types (`Edge` is a `Meter` too); where they do not, the broker rebuilds the
/// instances under the same deadline. Each answer is the one the broker rebuilds.
#[tokio::test(flavor = "multi_thread")]
async fn views_of_several_types_aggregate_in_the_store_or_stay_bounded() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let from = |t: &str| {
        Some(Window {
            from: Some(TimeBound::At(t.parse().expect("instant"))),
            to: None,
        })
    };
    let level: &[&str] = &["level"];
    let both: &[&str] = &["level", "hidden"];
    let cases: Vec<(&str, Filter, &str, bool)> = vec![
        (
            "two types, same list",
            reconciled(&[("Decade", level, None), ("Century", level, None)]),
            "Decade",
            true,
        ),
        (
            "two types, each its own list",
            reconciled(&[("Decade", level, None), ("Century", both, None)]),
            "Decade",
            true,
        ),
        (
            "two types, same list, one with its own window",
            reconciled(&[
                ("Decade", level, None),
                ("Century", level, from("1950-01-01T00:00:00Z")),
            ]),
            "Decade",
            true,
        ),
        (
            "an Entity of two types, the other with its own list",
            reconciled(&[("Edge", level, None), ("Meter", both, None)]),
            "Edge",
            true,
        ),
        (
            "an Entity of two types, the other with its own window",
            reconciled(&[
                ("Edge", level, None),
                ("Meter", level, from("2024-03-01T00:00:00Z")),
            ]),
            "Edge",
            true,
        ),
        (
            "an Entity of two types, asked by the type with a window",
            reconciled(&[
                ("Edge", level, None),
                ("Meter", level, from("2024-03-01T00:00:00Z")),
            ]),
            "Meter",
            false,
        ),
        (
            "a rule with a condition",
            Filter {
                rules: vec![Rule {
                    types: vec![iri("Edge")],
                    attributes: vec![iri("level")],
                    q: Some(Box::new(antares_ql::parse_q("level>0").expect("q"))),
                    ..Rule::default()
                }],
                ..Filter::default()
            },
            "Edge",
            false,
        ),
    ];
    for (case, filter, kind, pushed) in cases {
        let (st, store) = ruled(&pool, filter).await;
        // ten years for the time, a year and a half against the rebuilt answer
        for (range, compared) in [
            (
                "timerel=between&timeAt=2016-01-01T00:00:00Z&endTimeAt=2026-01-01T00:00:00Z",
                false,
            ),
            (
                "timerel=between&timeAt=2023-12-01T00:00:00Z&endTimeAt=2025-06-01T00:00:00Z",
                true,
            ),
        ] {
            let query = format!(
                "type={kind}&attrs=level&aggrMethods=avg,max,totalCount&aggrPeriodDuration=P1M&{range}"
            );
            let (code, warned, body, took) = answered(&st, &tenant, &query).await;
            assert_eq!(code, StatusCode::OK, "{case} {query}: {body}");
            let reads = store.answered();
            assert_eq!(reads.first(), Some(&pushed), "{case} {query}: {reads:?}");
            assert!(
                store.bounded().iter().all(|b| *b),
                "{case} {query}: every read carries the deadline"
            );
            assert!(took < BUDGET, "{case} {query} took {took:?}");
            if !compared {
                continue;
            }
            let (as_rebuilt, marked, rebuilt, _) = answered(&st, &tenant, &unpushed(&query)).await;
            assert_eq!(
                store.answered(),
                [false],
                "{case}: the reference is rebuilt"
            );
            assert_eq!(
                store.bounded(),
                [true],
                "{case}: the reference is bounded too"
            );
            assert_eq!((code, warned), (as_rebuilt, marked), "{case} {query}");
            // the reference reads every Attribute the rules show; `attrs` asked `level`
            let ids = |b: &Value| {
                b.as_array()
                    .map(|a| a.iter().map(|e| e["id"].clone()).collect::<Vec<_>>())
            };
            assert_eq!(ids(&body), ids(&rebuilt), "{case} {query}");
            for (asked, rebuilt) in body
                .as_array()
                .into_iter()
                .flatten()
                .zip(rebuilt.as_array().into_iter().flatten())
            {
                assert!(
                    same(&asked["level"], &rebuilt["level"]),
                    "{case} {query}: {asked} != {rebuilt}"
                );
                assert!(asked.get("hidden").is_none(), "{case} {query}: {asked}");
            }
        }
    }
    cleaned(&pool, &tenant).await;
}

/// A text Attribute beside the numeric one: the store reads the numbers in SQL,
/// meets the text, and the broker rebuilds the page on the same deadline.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_attribute_is_rebuilt_on_the_same_deadline() {
    let Some((pool, tenant)) = seeded().await else {
        return;
    };
    let (st, store) = view(&pool, None).await;
    let query = "type=Decade&attrs=level,name&aggrMethods=totalCount&aggrPeriodDuration=P1Y\
                 &timerel=between&timeAt=2019-01-01T00:00:00Z&endTimeAt=2022-01-01T00:00:00Z";
    let (code, body, took) = read(&st, &tenant, query).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(store.answered(), [false], "rebuilt for the text");
    assert_eq!(store.bounded(), [true]);
    assert!(took < BUDGET, "took {took:?}");
    let names: f64 = body[0]["name"]["totalCount"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r[0].as_f64()).sum())
        .unwrap_or_default();
    assert_eq!(names, 1.0, "{body}");
    assert_eq!(counted(&body), serde_json::json!(26_304.0), "{body}");
    cleaned(&pool, &tenant).await;
}
