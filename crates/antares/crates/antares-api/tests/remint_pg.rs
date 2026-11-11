// SPDX-License-Identifier: EUPL-1.2
//! The remint of Entity ids to the owner prefix (T-6770, migration 0033), on
//! Postgres: `urn:ngsi-ld:KPI:bbsk.sk:...` becomes `urn:ngsi-ld:KPI:sk_bbsk:...`
//! with its history in both parents, its superseded versions and every
//! Relationship naming it; a GET of the old id answers the Entity under the
//! new one; the map rolls it back; the per-table digest proves nothing but
//! the ids moved. Skips without `ANTARES_TEST_DATABASE_URL`.
#![cfg(feature = "postgres")]

use antares_api::AppState;
use antares_sql::sqlx::{self, PgPool, Row};
use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use tower::ServiceExt;

const OLD_A: &str = "urn:ngsi-ld:KPI:bbsk.sk:otvorene-data:a";
const NEW_A: &str = "urn:ngsi-ld:KPI:sk_bbsk:otvorene-data:a";
const OLD_B: &str = "urn:ngsi-ld:KPI:bbsk.sk:otvorene-data:b";
const NEW_B: &str = "urn:ngsi-ld:KPI:sk_bbsk:otvorene-data:b";
/// Another owner's Entity that names A: only its Relationship moves.
const OTHER: &str = "urn:ngsi-ld:Road:sk_bratislava:r1";

async fn send(st: &AppState, req: Request<Body>) -> (u16, Value) {
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post(st: &AppState, tenant: &str, uri: &str, body: &Value) -> u16 {
    let body = body.to_string();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("NGSILD-Tenant", tenant)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    send(st, req).await.0
}

async fn get(st: &AppState, tenant: &str, uri: &str) -> (u16, Value) {
    let req = Request::builder()
        .uri(uri)
        .header("NGSILD-Tenant", tenant)
        .header("Accept", "application/json")
        .body(Body::empty())
        .expect("request");
    send(st, req).await
}

fn kpi(id: &str, next: &str) -> Value {
    json!({"id": id, "type": "KPI",
           "kpiValue": {"type": "Property", "value": 7},
           "next": {"type": "Relationship", "object": next}})
}

fn temporal(id: &str, next: &str) -> Value {
    let at = |h: u32| format!("2026-01-01T{h:02}:00:00Z");
    json!({"id": id, "type": "KPI",
           "kpiValue": (0..6).map(|h| json!({"type": "Property", "value": h, "observedAt": at(h)}))
                .collect::<Vec<_>>(),
           "next": [{"type": "Relationship", "object": next, "observedAt": at(0)}]})
}

/// Moves half of `id`'s history into the keyed parent, as the cut-over
/// places it (tests/pg_history_keyed.rs in antares-sql).
async fn move_to_keyed(pool: &PgPool, tenant: &str, id: &str) -> u64 {
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant");
    sqlx::query(
        "INSERT INTO iri_ids (tenant_id, iri)
         SELECT DISTINCT $1, unnest(ARRAY[entity_id, attr_id]) FROM attr_instances
          WHERE tenant_id = $1 AND entity_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(id)
    .execute(&mut *tx)
    .await
    .expect("dictionary");
    let moved = sqlx::query(
        "WITH gone AS (DELETE FROM attr_instances
                        WHERE tenant_id = $1 AND entity_id = $2
                          AND observed_at >= '2026-01-01T03:00:00Z' RETURNING *)
         INSERT INTO attr_rows (tenant_id, entity_no, attr_no, instance_no, dataset_id,
                                observed_at, created_at, modified_at, deleted_at, data,
                                geo_value, version, provenance)
         SELECT g.tenant_id, iri_no(g.tenant_id, g.entity_id), iri_no(g.tenant_id, g.attr_id),
                md5(g.instance_id)::uuid, g.dataset_id, g.observed_at, g.created_at,
                g.modified_at, g.deleted_at,
                g.data || jsonb_build_object('instanceId', g.instance_id),
                g.geo_value, g.version, g.provenance
           FROM gone g",
    )
    .bind(tenant)
    .bind(id)
    .execute(&mut *tx)
    .await
    .expect("move")
    .rows_affected();
    // a superseded version of the Relationship instance
    sqlx::query(
        "INSERT INTO attr_versions (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                    version, data)
         SELECT tenant_id, entity_id, attr_id, instance_id, observed_at, 1, data
           FROM attr_instances WHERE tenant_id = $1 AND entity_id = $2 AND data::text LIKE '%object%'",
    )
    .bind(tenant)
    .bind(id)
    .execute(&mut *tx)
    .await
    .expect("version");
    tx.commit().await.expect("commit");
    moved
}

async fn digest(pool: &PgPool, tenant: &str, from: &str, to: &str) -> Vec<(String, i64, String)> {
    sqlx::query("SELECT tbl, row_count, digest::text FROM antares_remint_digest($1, $2, $3)")
        .bind(tenant)
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await
        .expect("digest")
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect()
}

async fn call(pool: &PgPool, f: &str, tenant: &str) -> Result<Value, String> {
    let sql = match f {
        "remint" => "SELECT antares_remint($1, 'bbsk.sk', 'sk_bbsk')",
        _ => "SELECT antares_remint_rollback($1, 'bbsk.sk', 'sk_bbsk')",
    };
    sqlx::query_scalar(sql)
        .bind(tenant)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())
}

async fn count(pool: &PgPool, sql: &str, tenant: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(tenant)
        .fetch_one(pool)
        .await
        .expect(sql)
}

async fn temporal_of(st: &AppState, tenant: &str, id: &str) -> String {
    let (status, body) = get(
        st,
        tenant,
        &format!("/ngsi-ld/v1/temporal/entities/{id}?timerel=after&timeAt=2025-12-31T00:00:00Z"),
    )
    .await;
    assert_eq!(status, 200, "temporal {id}: {body}");
    assert!(
        body["kpiValue"].as_array().map_or(0, Vec::len) >= 6,
        "{body}"
    );
    body.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn remint_renames_history_and_relationships_resolves_old_ids_and_rolls_back() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let st = AppState::with_store("antares1".into(), std::sync::Arc::new(store), "postgres");
    let tenant = format!("remint{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let t = tenant.as_str();

    for (id, next) in [(OLD_A, OLD_B), (OLD_B, OLD_A), (OTHER, OLD_A)] {
        assert_eq!(
            post(&st, t, "/ngsi-ld/v1/entities", &kpi(id, next)).await,
            201,
            "{id}"
        );
    }
    let status = post(
        &st,
        t,
        "/ngsi-ld/v1/temporal/entities",
        &temporal(OLD_A, OLD_B),
    )
    .await;
    assert!(matches!(status, 201 | 204), "temporal create: {status}");
    assert!(
        move_to_keyed(&pool, t, OLD_A).await >= 3,
        "history in attr_rows"
    );
    let history = temporal_of(&st, t, OLD_A).await;
    let identity_before = digest(&pool, t, "bbsk.sk", "bbsk.sk").await;
    let expected = digest(&pool, t, "bbsk.sk", "sk_bbsk").await;

    let report = call(&pool, "remint", t).await.expect("remint");
    assert_eq!(report["ids"], 2, "{report}");
    assert_eq!(report["entities"], 3, "two renamed, one naming A: {report}");
    assert_eq!(report["iri_ids"], 1, "{report}");
    assert_eq!(report["selectors_still_naming_from"], 0, "{report}");

    // the rehearsal's proof: equal counts and hashes, no old-prefix id left
    assert_eq!(digest(&pool, t, "sk_bbsk", "sk_bbsk").await, expected);
    for sql in [
        "SELECT count(*) FROM entities WHERE tenant_id = $1 AND split_part(id, ':', 4) = 'bbsk.sk'",
        "SELECT count(*) FROM temporal_entities WHERE tenant_id = $1 AND split_part(id, ':', 4) = 'bbsk.sk'",
        "SELECT count(*) FROM attr_instances WHERE tenant_id = $1 AND data::text LIKE '%bbsk.sk%'",
        "SELECT count(*) FROM attr_versions WHERE tenant_id = $1 AND entity_id LIKE '%bbsk.sk%'",
        "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND data::text LIKE '%bbsk.sk%'",
        "SELECT count(*) FROM entity_values WHERE tenant_id = $1 AND (id LIKE '%bbsk.sk%' OR txt LIKE '%bbsk.sk%')",
    ] {
        assert_eq!(count(&pool, sql, t).await, 0, "{sql}");
    }

    // current state under the new ids, Relationships following
    let (status, a) = get(&st, t, &format!("/ngsi-ld/v1/entities/{NEW_A}")).await;
    assert_eq!(status, 200, "{a}");
    assert_eq!(a["next"]["object"], NEW_B);
    let (_, other) = get(&st, t, &format!("/ngsi-ld/v1/entities/{OTHER}")).await;
    assert_eq!(other["next"]["object"], NEW_A);
    let keys = "SELECT count(*) FROM entity_values WHERE tenant_id = $1 AND id LIKE '%sk_bbsk%'";
    assert!(
        count(&pool, keys, t).await > 0,
        "the renamed Entities' q= keys"
    );

    // the old id resolves, read-only, to the Entity under its new id
    let (status, old) = get(&st, t, &format!("/ngsi-ld/v1/entities/{OLD_A}")).await;
    assert_eq!(status, 200, "{old}");
    assert_eq!(old["id"], NEW_A);
    let (status, _) = get(
        &st,
        t,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:KPI:bbsk.sk:otvorene-data:never",
    )
    .await;
    assert_eq!(status, 404, "an id the remint never saw stays absent");

    // history: every instance, both parents, under the new id
    assert_eq!(
        temporal_of(&st, t, NEW_A).await,
        history.replace("bbsk.sk", "sk_bbsk")
    );

    // a second run finds nothing
    assert_eq!(call(&pool, "remint", t).await.expect("again")["ids"], 0);

    // rollback from the map: back to the very rows of before
    let back = call(&pool, "rollback", t).await.expect("rollback");
    assert_eq!(back["ids"], 2, "{back}");
    assert_eq!(
        digest(&pool, t, "bbsk.sk", "bbsk.sk").await,
        identity_before
    );
    let (status, a) = get(&st, t, &format!("/ngsi-ld/v1/entities/{OLD_A}")).await;
    assert_eq!((status, a["next"]["object"].clone()), (200, json!(OLD_B)));
    let (status, _) = get(&st, t, &format!("/ngsi-ld/v1/entities/{NEW_A}")).await;
    assert_eq!(status, 404, "the rollback leaves no map row behind");
}

#[tokio::test(flavor = "multi_thread")]
async fn remint_refuses_a_clash_a_stray_rollback_and_the_broker_role() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let st = AppState::with_store("antares1".into(), std::sync::Arc::new(store), "postgres");
    let tenant = format!(
        "remintx{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let t = tenant.as_str();
    assert_eq!(
        post(&st, t, "/ngsi-ld/v1/entities", &kpi(OLD_A, OLD_B)).await,
        201
    );
    assert_eq!(
        post(&st, t, "/ngsi-ld/v1/entities", &kpi(NEW_A, OLD_B)).await,
        201
    );
    let before = digest(&pool, t, "bbsk.sk", "bbsk.sk").await;

    let clash = call(&pool, "remint", t).await.expect_err("a clash");
    assert!(clash.contains(NEW_A), "{clash}");
    assert_eq!(
        digest(&pool, t, "bbsk.sk", "bbsk.sk").await,
        before,
        "nothing renamed"
    );

    // NEW_A was written, not reminted: the rollback would invent an id
    let stray = call(&pool, "rollback", t).await.expect_err("a stray");
    assert!(stray.contains("pg_dump"), "{stray}");

    // a role under row-level security may not run it, not even with EXECUTE
    // granted the way the functions' owner holds it, and not by calling
    // remint_apply directly
    for sql in [
        "SELECT antares_remint($1, 'bbsk.sk', 'sk_bbsk')",
        "SELECT remint_apply($1, 'bbsk.sk', 'sk_bbsk', true)",
    ] {
        let refused = as_role_without_bypass(&pool, sql, t).await;
        assert!(
            refused.contains("bypasses row-level security"),
            "{sql}: {refused}"
        );
    }
    assert_eq!(
        digest(&pool, t, "bbsk.sk", "bbsk.sk").await,
        before,
        "nothing renamed"
    );

    let bad = sqlx::query_scalar::<_, Value>("SELECT antares_remint($1, 'bbsk.sk', 'x''; --')")
        .bind(t)
        .fetch_one(&pool)
        .await
        .expect_err("a segment outside the grammar");
    assert!(bad.to_string().contains("owner segments"), "{bad}");
}

/// Runs `sql` as a fresh role without superuser or BYPASSRLS that holds
/// EXECUTE on the remint functions; the role and grants roll back.
async fn as_role_without_bypass(pool: &PgPool, sql: &str, tenant: &str) -> String {
    let role = format!(
        "remintrole{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let mut tx = pool.begin().await.expect("tx");
    for stmt in [
        format!("CREATE ROLE {role} NOSUPERUSER NOBYPASSRLS"),
        format!(
            "GRANT EXECUTE ON FUNCTION remint_guard(text, text, text), \
             remint_apply(text, text, text, boolean), antares_remint(text, text, text) TO {role}"
        ),
        format!("SET LOCAL ROLE {role}"),
    ] {
        sqlx::query(&stmt).execute(&mut *tx).await.expect(&stmt);
    }
    let err = sqlx::query(sql)
        .bind(tenant)
        .execute(&mut *tx)
        .await
        .expect_err("refused");
    err.to_string()
}
