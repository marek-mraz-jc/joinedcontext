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

    // A whole-space Endpoint binds the space's every Policy: a type-wide
    // public read beside the dataset's pattern rule of the same type admits
    // every Tree, so the store counts and pages them as at the root, the
    // decoys included.
    let wide = Filter {
        rules: vec![
            Rule {
                types: vec![iri("Tree"), iri("Garden")],
                attributes: vec![iri("name")],
                ..Rule::default()
            },
            Rule {
                types: vec![iri("Tree")],
                id_patterns: vec![
                    "^urn:ngsi-ld:Tree:[a-z0-9.-]+:praha-mesto:green-[a-z-]+-[0-9a-f-]{36}$".into(),
                ],
                attributes: vec![iri("name")],
                ..Rule::default()
            },
        ],
        ..Filter::default()
    };
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(wide)));
    antares_api::wire(&mut st).await;
    for query in [
        "type=Tree&count=true&limit=0",
        "type=Tree&count=true&limit=1",
    ] {
        rows.take();
        let (code, count, body, took) = get(&st, &tenant, query).await;
        let read = rows.take();
        eprintln!("whole space {query}: {code} in {took:?}, {read} Entity rows read");
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        assert_eq!(count.as_deref(), Some("164003"), "{query}");
        assert!(read <= 10, "{query} read {read} Entity rows for a page");
    }

    // Each dataset Policy of a whole space narrows the type by its own
    // pattern: the rules' patterns united decide in the store.
    let datasets = Filter {
        rules: ["green", "other"]
            .iter()
            .map(|layer| Rule {
                types: vec![iri("Tree")],
                id_patterns: vec![format!(
                    "^urn:ngsi-ld:Tree:[a-z0-9.-]+:praha-mesto:{layer}-[a-z-]+-[0-9a-f-]{{36}}$"
                )],
                attributes: vec![iri("name")],
                ..Rule::default()
            })
            .collect(),
        ..Filter::default()
    };
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(datasets)));
    antares_api::wire(&mut st).await;
    rows.take();
    let (code, count, body, took) = get(&st, &tenant, "type=Tree&count=true&limit=0").await;
    let read = rows.take();
    eprintln!("datasets type=Tree&count=true&limit=0: {code} in {took:?}, {read} Entity rows read");
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(count.as_deref(), Some("164003"));
    assert!(read <= 10, "read {read} Entity rows for a count");

    // T-6971: bratislava's dataset views each narrow its 128 678 KPIs by
    // one pattern, zs-kapacita to 17 of them. Under the view's own Policy the
    // count is the slice and a page of five reads its six rows, sparse or
    // dense; jc-dev's 128 678 and 10 s came from a view the engine decided
    // by every Policy of the space.
    let mut tx = pool.begin().await.expect("tx");
    antares_sql::sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(&tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, i, jsonb_build_object('id', i, 'type', jsonb_build_array($2 || 'KeyPerformanceIndicator'),
                    $2 || 'name', jsonb_build_array(
                        jsonb_build_object('type', 'Property', 'value', 'miesta'))),
                ARRAY[$2 || 'KeyPerformanceIndicator'], now(), now()
           FROM (SELECT 'urn:ngsi-ld:KeyPerformanceIndicator:'
                            || CASE WHEN n % 7 = 0 THEN 'bbsk.sk' ELSE 'hel.fi' END
                            || ':bratislava:'
                            || CASE WHEN n <= 17 THEN 'zs-kapacita'
                                    WHEN n <= 5117 THEN 'demografia'
                                    ELSE 'zs-ziaci-' || (n % 431)::text END
                            || '.miesta.' || n::text || '.2021'
                   FROM generate_series(1, 128678) AS n) AS e(i)",
    )
    .bind(&tenant)
    .bind(DEFAULT)
    .execute(&mut *tx)
    .await
    .expect("kpis");
    tx.commit().await.expect("commit");
    antares_sql::sqlx::query("ANALYZE entities")
        .execute(&pool)
        .await
        .expect("analyze");
    for (dataset, n) in [("zs-kapacita", "17"), ("demografia", "5100"), ("none", "0")] {
        let view = Filter {
            rules: vec![Rule {
                types: vec![iri("KeyPerformanceIndicator")],
                id_patterns: vec![format!(
                    "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:bratislava:{dataset}[.][a-z0-9.-]+$"
                )],
                attributes: vec![iri("name")],
                ..Rule::default()
            }],
            ..Filter::default()
        };
        let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
            pool.clone(),
        ));
        let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
            .with_policy(Arc::new(Narrowing(view)));
        antares_api::wire(&mut st).await;
        for query in [
            "type=KeyPerformanceIndicator&limit=0&count=true",
            "type=KeyPerformanceIndicator&limit=5",
            "type=KeyPerformanceIndicator&limit=5&count=true",
        ] {
            rows.take();
            let (code, count, body, took) = get(&st, &tenant, query).await;
            let read = rows.take();
            eprintln!("{dataset} {query}: {code} {count:?} in {took:?}, {read} Entity rows read");
            assert_eq!(code, StatusCode::OK, "{dataset} {query}: {body}");
            if query.contains("count=true") {
                assert_eq!(count.as_deref(), Some(n), "{dataset} {query}");
            }
            let ids: Vec<&str> = body
                .as_array()
                .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
                .unwrap_or_default();
            assert!(
                ids.iter()
                    .all(|i| i.contains(&format!(":bratislava:{dataset}."))),
                "{dataset} {query}: {ids:?}"
            );
            let page = if query.contains("limit=0") { 0 } else { 5 };
            assert_eq!(
                ids.len(),
                page.min(n.parse().unwrap_or(0)),
                "{dataset} {query}"
            );
            assert!(
                read <= 10,
                "{dataset} {query} read {read} Entity rows for a page"
            );
        }
    }

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
