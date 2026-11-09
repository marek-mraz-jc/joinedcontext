// SPDX-License-Identifier: EUPL-1.2
//! `GET /types` through a view on Postgres over 100 000 Entities (T-4683).
//! Probing a type a rule does not name as the two-type group
//! `[name, ruleType]` matched nothing and walked the Tenant's whole primary
//! key for its `ORDER BY id LIMIT`: 17 s for MCP `list_types` on dev, and the
//! assistant routed without the source. The guard is the plan, which cannot
//! flake: Postgres names no Stop or Lamp beside a rule's type, so neither is
//! probed (`types_attrs::probe_plan` asserts what that read leaves out). The
//! clock only catches the walk: 1 s at best of five, where the fixed path
//! takes 41-91 ms locally and about 200 ms on a shared runner (T-4736).
//! `ANTARES_TEST_TYPES_BUDGET_MS=200` on a quiet machine checks the 200 ms claim.
//! Skips loudly without `ANTARES_TEST_DATABASE_URL` (container recipe in
//! `antares-sql/tests/pg.rs`).
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
use std::time::{Duration, Instant};
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "types-list-pg"
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

/// Shows Cars and Buses, the Cars only in zone `a`.
fn view() -> Filter {
    let rule = |t: &str, q: Option<&str>| Rule {
        types: vec![iri(t)],
        q: q.map(|q| Box::new(antares_ql::parse_q(q).expect("q"))),
        attributes: vec![iri("zone")],
        ..Rule::default()
    };
    Filter {
        rules: vec![rule("Car", Some("zone==\"a\"")), rule("Bus", None)],
        ..Filter::default()
    }
}

async fn types(st: &AppState, tenant: &str) -> (StatusCode, Value, Duration) {
    types_at(st, tenant, "/ngsi-ld/v1/types").await
}

async fn types_at(st: &AppState, tenant: &str, uri: &str) -> (StatusCode, Value, Duration) {
    let req = Request::builder()
        .uri(uri)
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

/// 20 000 Entities of each of five types in zone `b`, one Car in zone `a`
/// that is a Tree too: through the view, Bus, Car and Tree, with no probe of
/// Stop or Lamp, inside the budget.
#[tokio::test(flavor = "multi_thread")]
async fn a_views_type_list_over_100k_entities_probes_no_empty_group() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = format!("tl{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let tid = TenantId::new(&tenant).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tid)
        .await
        .expect("tenant row");
    // RLS reads the tenant from the session; the bulk insert sets it too.
    let mut tx = pool.begin().await.expect("tx");
    antares_sql::sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(&tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    // the seed is setup, not the read under test: CI runners took over the
    // pool's 30 s statement_timeout writing 100 000 rows (57014)
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:' || t || ':' || lpad(n::text, 6, '0'),
                jsonb_build_object('id', 'urn:ngsi-ld:' || t || ':' || lpad(n::text, 6, '0'),
                                   'type', jsonb_build_array($2 || t),
                                   $2 || 'zone', jsonb_build_array(
                                       jsonb_build_object('type', 'Property', 'value', 'b')),
                                   $2 || 'speed', jsonb_build_array(
                                       jsonb_build_object('type', 'Property', 'value', n))),
                ARRAY[$2 || t], now(), now()
           FROM unnest(ARRAY['Car','Bus','Stop','Tree','Lamp']) AS t,
                generate_series(1, 20000) AS n
         UNION ALL
         SELECT $1, 'urn:ngsi-ld:Car:zone-a',
                jsonb_build_object('id', 'urn:ngsi-ld:Car:zone-a',
                                   'type', jsonb_build_array($2 || 'Tree', $2 || 'Car'),
                                   $2 || 'zone', jsonb_build_array(
                                       jsonb_build_object('type', 'Property', 'value', 'a'))),
                ARRAY[$2 || 'Tree', $2 || 'Car'], now(), now()",
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
    // the plan's input: what the view's rule types carry beside them
    for (rule, other) in [("Car", Some("Tree")), ("Bus", None)] {
        let beside = store
            .entity_types_beside(&tid, &[iri(rule)])
            .await
            .expect("beside");
        if let Some(other) = other {
            assert!(beside.contains(&iri(other)), "{rule}: {beside:?}");
        }
        for empty in ["Stop", "Lamp"] {
            assert!(!beside.contains(&iri(empty)), "{rule}: {beside:?}");
        }
    }
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres")
        .with_policy(Arc::new(Narrowing(view())));
    antares_api::wire(&mut st).await;

    // the first call warms the connections; the best of the next five is
    // measured, so a shared CI runner's one slow moment is not the claim
    let (code, body, _) = types(&st, &tenant).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let (code, body, took) = types(&st, &tenant).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(
            body["typeList"],
            serde_json::json!(["Bus", "Car", "Tree"]),
            "{body}"
        );
        best = best.min(took);
    }
    let budget = std::env::var("ANTARES_TEST_TYPES_BUDGET_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(Duration::from_secs(1), Duration::from_millis);
    eprintln!("/types over 100 000 Entities: {best:?} at best of 5");
    assert!(
        best < budget,
        "/types took {best:?} at best of 5, budget {budget:?}"
    );

    // T-4934: the attribute catalogue answers through the same view; the
    // `speed` every Entity carries is hidden by both rules, so it is neither
    // listed nor probed (a probe would walk 20 000 Cars for nothing).
    for (uri, want) in [
        ("/ngsi-ld/v1/attributes", serde_json::json!(["zone"])),
        (
            "/ngsi-ld/v1/types?details=true",
            serde_json::json!(["Bus", "Car", "Tree"]),
        ),
    ] {
        let mut best = Duration::MAX;
        for _ in 0..5 {
            let (code, body, took) = types_at(&st, &tenant, uri).await;
            assert_eq!(code, StatusCode::OK, "{body}");
            let got = match body.as_array() {
                Some(types) => types.iter().map(|t| t["typeName"].clone()).collect(),
                None => body["attributeList"].clone(),
            };
            assert_eq!(got, want, "{uri}: {body}");
            for t in body.as_array().into_iter().flatten() {
                assert_eq!(t["attributeNames"], serde_json::json!(["zone"]), "{body}");
            }
            best = best.min(took);
        }
        eprintln!("{uri} over 100 000 Entities: {best:?} at best of 5");
        assert!(
            best < budget,
            "{uri} took {best:?} at best of 5, budget {budget:?}"
        );
    }

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}

/// `GET /types?details=true` and `/attributes` over 100 000 Entities read
/// the attribute catalogue (T-4934): a page of each type, no fold. The fold
/// they replaced read each row once by keyset (T-4683, under 2 s locally;
/// 13-19 s for `/attributes` on dev); OFFSET paging before it took 57-59 s
/// on one prod tenant. Same 1 s bound at best of five as the type list.
#[tokio::test(flavor = "multi_thread")]
async fn type_details_over_100k_entities_read_each_row_once() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = format!("td{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
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
    // the seed is setup, not the read under test: CI runners took over the
    // pool's 30 s statement_timeout writing 100 000 rows (57014)
    antares_sql::sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await
        .expect("seed timeout");
    antares_sql::sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:' || t || ':' || lpad(n::text, 6, '0'),
                jsonb_build_object('id', 'urn:ngsi-ld:' || t || ':' || lpad(n::text, 6, '0'),
                                   'type', jsonb_build_array($2 || t),
                                   $2 || 'zone', jsonb_build_array(
                                       jsonb_build_object('type', 'Property', 'value', 'b'))),
                ARRAY[$2 || t], now(), now()
           FROM unnest(ARRAY['Car','Bus','Stop','Tree','Lamp']) AS t,
                generate_series(1, 20000) AS n",
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
    let mut st = AppState::with_store("me".into(), Arc::new(store), "postgres");
    antares_api::wire(&mut st).await;

    let mut best = Duration::MAX;
    let mut body = Value::Null;
    for _ in 0..5 {
        let (code, got, took) = types_at(&st, &tenant, "/ngsi-ld/v1/types?details=true").await;
        assert_eq!(code, StatusCode::OK, "{got}");
        (body, best) = (got, best.min(took));
    }
    let names: Vec<&str> = body
        .as_array()
        .expect("an EntityType array")
        .iter()
        .filter_map(|t| t["typeName"].as_str())
        .collect();
    assert_eq!(names, ["Bus", "Car", "Lamp", "Stop", "Tree"], "{body}");
    assert!(
        body.as_array()
            .into_iter()
            .flatten()
            .all(|t| t["attributeNames"] == serde_json::json!(["zone"])),
        "{body}"
    );
    eprintln!("details over 100 000 Entities: {best:?} at best of 5");
    assert!(best < Duration::from_secs(1), "details took {best:?}");
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let (code, body, took) =
            types_at(&st, &tenant, "/ngsi-ld/v1/attributes?details=true").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().map(Vec::len), Some(1), "{body}");
        assert_eq!(
            body[0]["typeNames"],
            serde_json::json!(["Bus", "Car", "Lamp", "Stop", "Tree"]),
            "{body}"
        );
        best = best.min(took);
    }
    eprintln!("/attributes?details=true over 100 000 Entities: {best:?} at best of 5");
    assert!(best < Duration::from_secs(1), "/attributes took {best:?}");

    antares_sql::sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean");
}
