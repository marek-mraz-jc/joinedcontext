// SPDX-License-Identifier: EUPL-1.2
//! `GET /entities?count=true&q=...` through a type-only view at the size of
//! dev's Helsinki Trees (67 742), on Postgres (T-6277). The count with a `q`
//! passed the gateway's 10 s on dev: one store statement whose Bitmap Heap
//! Scan read the type's scattered heap blocks one at a time. The guard here
//! is the statement budget: the count is decided in the store (no Entity
//! comes back to the broker), the scattered seed is counted exactly, and the
//! answer stays under the 5 s the task allows. A local PostGIS holds the
//! seed in its cache, so the dev I/O itself is proven by EXPLAIN in the task
//! and by `pg_pool_release::a_broker_session_reads_heap_blocks_ahead`. Own
//! test binary because the subscriber is process-wide. Skips loudly without
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
const TREES: i64 = 67_742;
const BUDGET: Duration = Duration::from_secs(5);

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

/// The most Entity rows one store read returned to the broker since `take`.
/// The largest statement, not the sum: sqlx logs a statement when its
/// logger drops, which can come after the answer, so the previous count's
/// own row may land in the next count's window (sweep 41279198278).
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
                *n = (*n).max(q.rows);
            }
        }
    }
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "query-q-count-pg"
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

async fn count(st: &AppState, tenant: &str, query: &str) -> (StatusCode, Option<String>, Duration) {
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
    let total = res
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let _ = axum::body::to_bytes(res.into_body(), usize::MAX).await;
    (status, total, started.elapsed())
}

/// T-6277: 67 742 Trees of seven trunk classes, inserted in random order
/// among four times as many rows of another type, as dev's daily rewrites
/// scatter them; the view shows Tree by type and attribute list, as the
/// Portal renders the Helsinki tree Endpoint. Each class count and the whole
/// count is one store statement, exact, under the budget.
#[tokio::test(flavor = "multi_thread")]
async fn a_q_count_over_a_scattered_type_is_one_store_statement() {
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
    // the seed is setup, not the read under test (T-5661)
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    // tree n has class n % 7; every fifth row of the heap is a Tree
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, i, jsonb_build_object('id', i, 'type', jsonb_build_array($2 || t),
                    $2 || 'trunkDiameterClass', jsonb_build_array(
                        jsonb_build_object('type', 'Property', 'value', 'cm' || (n % 7))),
                    $2 || 'species', jsonb_build_array(
                        jsonb_build_object('type', 'Property', 'value', 'Tilia x vulgaris'))),
                ARRAY[$2 || t], now(), now()
           FROM (SELECT 'urn:ngsi-ld:' || t || ':hel.fi:helsinki:tree-' || n, t, n
                   FROM generate_series(1, $3::bigint) AS n,
                        (VALUES ('Tree'), ('Stop'), ('Stop2'), ('Stop3'), ('Stop4')) AS k(t)
                  ORDER BY md5(t || n)) AS e(i, t, n)",
    )
    .bind(&tenant)
    .bind(DEFAULT)
    .bind(TREES)
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
            types: vec![iri("Tree")],
            attributes: vec![iri("trunkDiameterClass"), iri("species")],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(view)));
    antares_api::wire(&mut st).await;

    // warm-up, untimed: the first read after the seed pays for its cold
    // cache (5.9 s under preflight's parallel load, 1.1 s alone), which is
    // the seed, not the count path; the type count is timed again below
    let (code, total, _) = count(&st, &tenant, "type=Tree&count=true&limit=0").await;
    assert_eq!((code, total), (StatusCode::OK, Some(TREES.to_string())));
    let mut sum = 0i64;
    for class in 0..7i64 {
        let query = format!("type=Tree&count=true&limit=0&q=trunkDiameterClass==%22cm{class}%22");
        rows.take();
        let (code, total, took) = count(&st, &tenant, &query).await;
        let read = rows.take();
        eprintln!("{query}: {code} {total:?} in {took:?}, at most {read} Entity rows in one read");
        assert_eq!(code, StatusCode::OK, "{query}");
        let want = (1..=TREES).filter(|n| n % 7 == class).count().to_string();
        assert_eq!(total.as_deref(), Some(want.as_str()), "{query}");
        // the count statement's own row; a walk returns the class (~9 700)
        assert!(
            read <= 1,
            "{query}: a store read returned {read} Entities to the broker"
        );
        assert!(took < BUDGET, "{query} took {took:?}");
        sum = sum.saturating_add(want.parse::<i64>().unwrap_or_default());
    }
    assert_eq!(sum, TREES, "the classes split every Tree");
    let (code, total, took) = count(&st, &tenant, "type=Tree&count=true&limit=0").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(total, Some(TREES.to_string()));
    assert!(took < BUDGET, "the type count took {took:?}");

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
