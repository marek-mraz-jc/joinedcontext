// SPDX-License-Identifier: EUPL-1.2
//! A temporal query through a view whose rule narrows its type by one
//! `idPattern`, on Postgres (T-6052). The Helsinki space holds the street
//! cleaning pilot's swarco sensors beside FMI's 11 stations, well over 10 000
//! AirQualityObserved; the public air-quality view shows the stations by
//! `…:helsinki:fmi-[a-z0-9-]+`. The store used to walk the whole type, judged
//! the pattern in Rust and refused the read 403 TooManyResults past its
//! 10 000-row ceiling, so every air-quality chart showed the refusal. The
//! store now selects by the pattern: a page and its count, and an unpaged
//! read with the client's own `idPattern`, answer 200 with the stations only.
//! Skips loudly without `ANTARES_TEST_DATABASE_URL`.
#![cfg(feature = "postgres")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use antares_model::TenantId;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const FMI: &str = "^urn:ngsi-ld:AirQualityObserved:[a-z0-9.-]+:helsinki:fmi-[a-z0-9-]+$";

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "temporal-id-pattern-pg"
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

async fn get(st: &AppState, tenant: &str, query: &str) -> (StatusCode, Option<String>, Value) {
    let req = Request::builder()
        .uri(format!("/ngsi-ld/v1/temporal/entities?{query}"))
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .expect("request");
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
    )
}

/// 10 001 swarco sensors and 5 stations, each with one pm10 reading; the
/// view shows the stations' pm10 only.
#[tokio::test(flavor = "multi_thread")]
async fn a_views_id_pattern_selects_a_temporal_query_in_the_store() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
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
        "CREATE TEMP TABLE seeded ON COMMIT DROP AS
         SELECT 'urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:' || CASE
                    WHEN n <= 5 THEN 'fmi-station-' ELSE 'swarco-' || n || '-' END || n AS id
           FROM generate_series(1, 10006) AS n",
        "INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
         SELECT $1, id, ARRAY[$2 || 'AirQualityObserved'],
                jsonb_build_object('id', id, 'type', jsonb_build_array($2 || 'AirQualityObserved')), now(), now()
           FROM seeded",
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                     created_at, modified_at, data)
         SELECT $1, id, $2 || 'pm10', 'urn:i:' || id, '2026-01-02T00:00:00Z', now(), now(),
                jsonb_build_object('type', 'Property', 'value', 12,
                                   'observedAt', '2026-01-02T00:00:00Z')
           FROM seeded",
    ] {
        antares_sql::sqlx::query(stmt)
            .bind(&tenant)
            .bind(DEFAULT)
            .execute(&mut *tx)
            .await
            .expect(stmt);
    }
    tx.commit().await.expect("commit");
    antares_sql::sqlx::query("ANALYZE temporal_entities")
        .execute(&pool)
        .await
        .expect("analyze");

    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let view = Filter {
        rules: vec![Rule {
            types: vec![iri("AirQualityObserved")],
            id_patterns: vec![FMI.into()],
            attributes: vec![iri("pm10")],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(view)));
    antares_api::wire(&mut st).await;

    let window = "type=AirQualityObserved&attrs=pm10&timerel=after&timeAt=2026-01-01T00:00:00Z";
    let pattern: String = FMI
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'0'..=b'9' | b'-' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    for (query, total, shown) in [
        (format!("{window}&limit=2&count=true"), Some("5"), 2),
        (format!("{window}&limit=10&idPattern={pattern}"), None, 5),
    ] {
        let (code, count, body) = get(&st, &tenant, &query).await;
        assert_eq!(code, StatusCode::OK, "{query}: {body}");
        let ids: Vec<&str> = body
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(ids.len(), shown, "{query}: {ids:?}");
        assert!(
            ids.iter().all(|i| i.contains(":helsinki:fmi-station-")),
            "{query}: {ids:?}"
        );
        if total.is_some() {
            assert_eq!(count.as_deref(), total, "{query}");
        }
    }

    for stmt in [
        "DELETE FROM attr_instances WHERE tenant_id = $1",
        "DELETE FROM temporal_entities WHERE tenant_id = $1",
    ] {
        antares_sql::sqlx::query(stmt)
            .bind(&tenant)
            .execute(&pool)
            .await
            .expect("clean");
    }
}
