// SPDX-License-Identifier: EUPL-1.2
//! `GET /types` through a view of one type and one `idPattern` on Postgres
//! (T-5661). The probe used to read every Entity of the type into the broker
//! and judge the regex there: 9-10 s on dev, cut by the gateway at 10 s. It
//! now reads the exact id prefixes the pattern forces, one per id segment
//! the Tenant shows (`jc.example` and the decoy's `UPPER` here), as ranges
//! of the C-collated id index; `pg_explain` pins that plan. The guard counts the
//! Entity rows Postgres returns to the broker during the cold call, from
//! sqlx's own `sqlx::query` events (`rows_returned`): the fixed probe gets
//! only the decoy, the old one every KeyPerformanceIndicator up to the fold
//! ceiling. Wall time is printed, not asserted: the remaining SQL scan took
//! 0.16 s locally and 3.15 s on a CI runner (run 40257497736), beside the
//! 100k seeds of other tests. Own test binary because the subscriber is
//! process-wide. Skips loudly without `ANTARES_TEST_DATABASE_URL`.
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
        "types-id-pattern-pg"
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

async fn types(st: &AppState, tenant: &str) -> (StatusCode, Value, Duration) {
    let req = Request::builder()
        .uri("/ngsi-ld/v1/types")
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .expect("request");
    let started = Instant::now();
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        started.elapsed(),
    )
}

/// T-5661: a view of one type and one `idPattern`, as the Helsinki
/// statistics Endpoints on dev. 200 000 KeyPerformanceIndicators, none of
/// them in the view: the probe walked every one to the fold ceiling and
/// judged the regex in Rust, 9-10 s for `/types` on dev, the gateway's 503.
/// The literal the pattern forces now narrows the read in SQL. A decoy that
/// carries the literal but fails the regex, and a type outside the view,
/// stay unlisted; one matching Entity lists the type.
#[tokio::test(flavor = "multi_thread")]
async fn a_views_id_pattern_narrows_the_type_probe() {
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
    let tenant = format!("tp{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let tid = TenantId::new(&tenant).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tid)
        .await
        .expect("tenant row");
    // `n` KPIs per city in stat-services, plus the rows `extra` names
    let insert = |n: i32, extra: &[(&str, &str)]| {
        let (ids, types): (Vec<String>, Vec<String>) = extra
            .iter()
            .map(|(i, t)| ((*i).to_owned(), (*t).to_owned()))
            .unzip();
        let pool = pool.clone();
        let tenant = tenant.clone();
        async move {
            let mut tx = pool.begin().await.expect("tx");
            antares_sql::sqlx::query("SELECT set_config('antares.tenant', $1, true)")
                .bind(&tenant)
                .execute(&mut *tx)
                .await
                .expect("tenant setting");
            // the seed is setup, not the read under test: a slow CI runner
            // took over the pool's 30 s statement_timeout writing 200 000 rows
            antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
                .execute(&mut *tx)
                .await
                .expect("seed timeout");
            antares_sql::sqlx::query(
                "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
                 SELECT $1, i, jsonb_build_object('id', i, 'type', jsonb_build_array($2 || t),
                            $2 || 'kpiValue', jsonb_build_array(
                                jsonb_build_object('type', 'Property', 'value', 1))),
                        ARRAY[$2 || t], now(), now()
                   FROM (SELECT 'urn:ngsi-ld:KeyPerformanceIndicator:jc.example:' || c
                                    || ':stat-services-' || lpad(n::text, 6, '0'),
                                'KeyPerformanceIndicator'
                           FROM unnest(ARRAY['helsinki','praha']) AS c,
                                generate_series(1, $5) AS n
                         UNION ALL SELECT * FROM unnest($3::text[], $4::text[])) AS e(i, t)",
            )
            .bind(&tenant)
            .bind(DEFAULT)
            .bind(ids)
            .bind(types)
            .bind(n)
            .execute(&mut *tx)
            .await
            .expect("entities");
            tx.commit().await.expect("commit");
        }
    };
    insert(
        100_000,
        &[
            (
                "urn:ngsi-ld:KeyPerformanceIndicator:UPPER:helsinki:stat-environment-decoy",
                "KeyPerformanceIndicator",
            ),
            (
                "urn:ngsi-ld:Car:jc.example:helsinki:stat-environment-car",
                "Car",
            ),
        ],
    )
    .await;
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
                "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:helsinki:stat-environment-[A-Za-z0-9._-]+$"
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

    // cold: the first call after start is the one the gateway cut at 10 s
    rows.take();
    let (code, body, cold) = types(&st, &tenant).await;
    let read = rows.take();
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["typeList"], serde_json::json!([]), "{body}");
    assert!(body.get("NGSILD-Warning").is_none(), "{body}");
    eprintln!("/types through an idPattern view over 200 000 KPIs, cold: {cold:?}, {read} Entity rows read");
    // only the decoy carries the literal, once per probe statement; the
    // unnarrowed probe read every KPI up to the fold ceiling
    assert!(
        read <= 2,
        "/types read {read} Entity rows for an empty view"
    );

    insert(
        0,
        &[(
            "urn:ngsi-ld:KeyPerformanceIndicator:jc.example:helsinki:stat-environment-air",
            "KeyPerformanceIndicator",
        )],
    )
    .await;
    let (code, body, _) = types(&st, &tenant).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body["typeList"],
        serde_json::json!(["KeyPerformanceIndicator"]),
        "{body}"
    );

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
