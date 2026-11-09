// SPDX-License-Identifier: EUPL-1.2
//! `GET /entities?count=true` through a view whose rule narrows by one
//! `idPattern` that matches most of its type, on Postgres (T-5826). The
//! Praha stromy, zelen and ulicni-useky Endpoints on dev are such views:
//! 163 806 Trees, every one matching `…:praha-mesto:green-…`, and the prefix
//! the pattern forces (T-5787) is the whole type. The count walked every
//! Tree in OFFSET pages, judged the regex in Rust, and passed the gateway's
//! 10 s (or hit TooManyResults past 100 000). The store now decides the
//! pattern: the page in SQL, the count over the ids in the broker
//! (T-6066). The guard counts the Entity rows Postgres
//! returns to the broker, from sqlx's `sqlx::query` events; wall time is
//! printed, not asserted (T-5661). Own test binary because the subscriber
//! is process-wide. Skips loudly without `ANTARES_TEST_DATABASE_URL`.
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
/// since the last `take`, and the counts Postgres decided with a regex.
#[derive(Clone, Default)]
struct EntityRows(Arc<Mutex<u64>>, Arc<Mutex<u64>>);

impl EntityRows {
    fn take(&self) -> u64 {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn take_regex_counts(&self) -> u64 {
        std::mem::take(&mut *self.1.lock().unwrap_or_else(|e| e.into_inner()))
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
            if q.sql.contains("count(*)") && q.sql.contains("regexp_like") {
                let mut n = self.1.lock().unwrap_or_else(|e| e.into_inner());
                *n = n.saturating_add(1);
            }
        }
    }
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "query-id-pattern-count-pg"
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

/// T-5826: 164 000 Trees the view's pattern selects, three that share its
/// prefix and fail it, and Gardens of the same ids beside them. A page of one
/// and its count read the page, not the type; the decoys are neither shown
/// nor counted.
#[tokio::test(flavor = "multi_thread")]
async fn a_views_id_pattern_over_a_whole_type_is_counted_in_the_store() {
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
    let tenant = format!("tc{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
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
    // the dev ids: one domain segment, the layer, then a 36-character cell
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, i, jsonb_build_object('id', i, 'type', jsonb_build_array($2 || t),
                    $2 || 'name', jsonb_build_array(
                        jsonb_build_object('type', 'Property', 'value', 'lipa'))),
                ARRAY[$2 || t], now(), now()
           FROM (SELECT 'urn:ngsi-ld:' || t || ':hel.fi:praha-mesto:'
                            || CASE WHEN n <= 3 THEN 'other-' ELSE 'green-' END
                            || 'trees-' || lpad(n::text, 36, '0'), t
                   FROM generate_series(1, 164003) AS n,
                        (VALUES ('Tree'), ('Garden')) AS k(t)
                  WHERE t = 'Tree' OR n % 4 = 0) AS e(i, t)",
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
            types: vec![iri("Tree")],
            id_patterns: vec![
                "^urn:ngsi-ld:Tree:[a-z0-9.-]+:praha-mesto:green-[a-z-]+-[0-9a-f-]{36}$".into(),
            ],
            attributes: vec![iri("name")],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(view)));
    antares_api::wire(&mut st).await;

    for query in [
        "type=Tree&count=true&limit=1",
        "type=Tree&count=true&limit=0",
        "type=Tree&limit=2&offset=5",
    ] {
        rows.take();
        rows.take_regex_counts();
        let (code, count, body, took) = get(&st, &tenant, query).await;
        let read = rows.take();
        eprintln!("{query}: {code} in {took:?}, {read} Entity rows read");
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        let ids: Vec<&str> = body
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            ids.iter()
                .all(|i| i.starts_with("urn:ngsi-ld:Tree:") && i.contains(":green-trees-")),
            "{query}: {ids:?}"
        );
        if query.contains("count=true") {
            assert_eq!(count.as_deref(), Some("164000"), "{query}");
            // T-6066: Postgres's regex took 2.9 s a core over dev's 241 752
            // Gardens; the broker matches the ids instead
            assert_eq!(rows.take_regex_counts(), 0, "{query}: a regex count in SQL");
        }
        if query.contains("offset=5") {
            assert_eq!(
                ids,
                [
                    "urn:ngsi-ld:Tree:hel.fi:praha-mesto:green-trees-000000000000000000000000000000000009",
                    "urn:ngsi-ld:Tree:hel.fi:praha-mesto:green-trees-000000000000000000000000000000000010",
                ],
                "{query}"
            );
        }
        // the page alone: the walk read every Tree
        assert!(read <= 10, "{query} read {read} Entity rows for a page");
    }

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
