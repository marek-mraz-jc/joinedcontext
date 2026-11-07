// SPDX-License-Identifier: EUPL-1.2
//! `GET /entities` through a view whose rule narrows by one `idPattern`, on
//! Postgres (T-5787). Every Bratislava KPI dataset Endpoint on prod is such a
//! view: 128 678 KeyPerformanceIndicators in the Tenant, about 70 per
//! dataset. The read walked every KPI in doubling pages and judged the regex
//! in Rust, so even `limit=1` took the gateway's 10 s and `count=true` hit
//! TooManyResults. It now hands the store the exact id prefixes the pattern
//! forces, ranges of the C-collated id index (0027, `pg_explain` pins that
//! plan). The guard counts the Entity rows Postgres returns to the broker,
//! from sqlx's `sqlx::query` events; wall time is printed, not asserted (CI
//! runners are slow beside other 100k seeds, T-5661). Own test binary
//! because the subscriber is process-wide. Skips loudly without
//! `ANTARES_TEST_DATABASE_URL`.
#![cfg(feature = "postgres")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use antares_model::TenantId;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

/// The rows each Entity read returned, summed over the statements sqlx sent
/// since the last `take`.
#[derive(Clone, Default)]
struct EntityRows(Arc<Mutex<u64>>);

impl EntityRows {
    fn take(&self) -> u64 {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[derive(Default)]
struct Query {
    sql: String,
    rows: u64,
}

impl tracing::field::Visit for Query {
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        if matches!(f.name(), "summary" | "db.statement") {
            self.sql.push_str(v);
        }
    }
    fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
        if f.name() == "rows_returned" {
            self.rows = v;
        }
    }
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.record_str(f, &format!("{v:?}"));
    }
}

impl<S: tracing::Subscriber> Layer<S> for EntityRows {
    fn on_event(&self, e: &tracing::Event<'_>, _: Context<'_, S>) {
        if e.metadata().target() == "sqlx::query" {
            let mut q = Query::default();
            e.record(&mut q);
            if q.sql.contains("SELECT entity AS entity FROM") {
                let mut n = self.0.lock().unwrap_or_else(|e| e.into_inner());
                *n = n.saturating_add(q.rows);
            }
        }
    }
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "query-id-pattern-pg"
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

async fn get(
    st: &AppState,
    tenant: &str,
    query: &str,
) -> (StatusCode, Option<String>, Value, Duration) {
    let req = Request::builder()
        .uri(format!("/ngsi-ld/v1/entities?{query}"))
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .expect("request");
    let started = Instant::now();
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let count = res
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        count,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        started.elapsed(),
    )
}

/// T-5787: 130 000 KPIs in one Tenant, 70 of them in the dataset the view's
/// `idPattern` names, plus a decoy that carries the dataset's prefix but
/// fails the pattern. A page of one and the count read only the prefix's
/// rows; the decoy is neither shown nor counted.
#[tokio::test(flavor = "multi_thread")]
async fn a_views_id_pattern_narrows_the_entity_query() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let rows = EntityRows::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(
            rows.clone().with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target("sqlx::query", tracing::Level::TRACE),
            ),
        ),
    )
    .expect("the only subscriber of this binary");
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = format!("tq{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
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
    // the seed is setup, not the read under test (T-5661: a CI runner took
    // over the pool's 30 s statement_timeout writing 200 000 rows)
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    // the prod ids: one domain segment, the dataset slug, then the cell
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, i, jsonb_build_object('id', i, 'type', jsonb_build_array($2 || 'KeyPerformanceIndicator'),
                    $2 || 'kpiValue', jsonb_build_array(
                        jsonb_build_object('type', 'Property', 'value', 1))),
                ARRAY[$2 || 'KeyPerformanceIndicator'], now(), now()
           FROM (SELECT 'urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:bratislava:'
                            || CASE WHEN n <= 70 THEN 'saldo-do.' ELSE 'kpi-' || (n % 450) || '.' END
                            || lpad(n::text, 6, '0')
                   FROM generate_series(1, 130000) AS n
                 UNION ALL SELECT 'urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:bratislava:saldo-do.DECOY') AS e(i)",
    )
    .bind(&tenant)
    .bind(DEFAULT)
    .execute(&mut *tx)
    .await
    .expect("entities");
    tx.commit().await.expect("commit");
    antares_sql::sqlx::query("ANALYZE entities")
        .execute(&pool)
        .await
        .expect("analyze");

    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let view = Filter {
        rules: vec![Rule {
            types: vec![iri("KeyPerformanceIndicator")],
            id_patterns: vec![
                "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:bratislava:saldo-do[.][a-z0-9.-]+$"
                    .into(),
            ],
            attributes: vec![iri("kpiValue")],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(view)));
    antares_api::wire(&mut st).await;

    for query in [
        "type=KeyPerformanceIndicator&limit=1",
        "type=KeyPerformanceIndicator&count=true&limit=1",
        "type=KeyPerformanceIndicator&count=true&limit=1000",
    ] {
        rows.take();
        let (code, count, body, took) = get(&st, &tenant, query).await;
        let read = rows.take();
        eprintln!("{query}: {code} in {took:?}, {read} Entity rows read");
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        let ids: Vec<&str> = body
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default();
        assert!(!ids.is_empty(), "{query}: {body}");
        assert!(
            ids.iter()
                .all(|i| i.contains(":saldo-do.") && !i.ends_with("DECOY")),
            "{query}: {ids:?}"
        );
        if query.contains("count=true") {
            assert_eq!(count.as_deref(), Some("70"), "{query}");
        }
        if query.ends_with("limit=1000") {
            assert_eq!(ids.len(), 70, "{query}");
        }
        // the dataset's 70 and the decoy, once per statement of the walk;
        // the unnarrowed read walked the whole type
        assert!(
            read <= 300,
            "{query} read {read} Entity rows for 70 matches"
        );
    }

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
