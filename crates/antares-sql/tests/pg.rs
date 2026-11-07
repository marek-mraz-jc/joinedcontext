// SPDX-License-Identifier: EUPL-1.2
//! Postgres foundation integration tests. Need a live
//! PostGIS; skip (pass vacuously, loudly) when ANTARES_TEST_DATABASE_URL is
//! unset so plain `cargo test` stays service-free.
//!
//! Local:  docker run -d --name pgdev -e POSTGRES_USER=antares \
//!           -e POSTGRES_PASSWORD=antares -e POSTGRES_DB=antares \
//!           -p 15432:5432 ghcr.io/baosystems/postgis:17-3.5
//!         ANTARES_TEST_DATABASE_URL=postgresql://antares:antares@localhost:15432/antares \
//!           cargo test -p antares-sql --test pg

// Every test here talks to the Postgres store, which only exists with the
// `postgres` feature; without it the file builds to nothing (T-3465).
#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use sqlx::Row;

macro_rules! require_db {
    () => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

/// 5.5.10: tenant existence — false for a never-seen Tenant, true once its
/// row exists. Regression: the Pg arm never bound $1, so EVERY call errored
/// ("bind message supplies 0 parameters") and the NonexistentTenant
/// middleware 500'd every non-default-tenant request on the Pg store.
#[tokio::test(flavor = "multi_thread")]
async fn tenant_exists_answers_for_the_actual_tenant() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let t = TenantId::new(&format!("pgte{run}")).expect("tenant");
    assert!(
        !store
            .tenant_exists(&t)
            .await
            .expect("tenant_exists must not error"),
        "never-seen tenant reads as existing"
    );
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    assert!(store
        .tenant_exists(&t)
        .await
        .expect("tenant_exists must not error"));
}

#[tokio::test]
async fn migrations_apply_and_indexes_exist() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");

    // The LIVE index set on entities — the exact indexes the compiled
    // statements route through (pg_explain.rs proves usability, this proves
    // existence).
    for idx in [
        "i_entities_location",
        "i_entities_types",
        "i_entities_loc_ambiguous",
        "i_entities_first_type_id",
        "i_entities_multi_type_id",
        "i_entities_types_multi",
    ] {
        let n: i64 = sqlx::query(
            "SELECT count(*) FROM pg_indexes WHERE tablename = 'entities' AND indexname = $1",
        )
        .bind(idx)
        .fetch_one(&pool)
        .await
        .expect("pg_indexes")
        .get(0);
        assert_eq!(n, 1, "index {idx} missing on entities");
    }
    // 0019 dropped the whole-document GIN: no jsonb operator is leakproof,
    // so it never served the broker's role under RLS (T-5193).
    let gin: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes WHERE tablename = 'entities' AND indexname = 'i_entities_jsonb'",
    )
    .fetch_one(&pool)
    .await
    .expect("pg_indexes");
    assert_eq!(gin, 0, "i_entities_jsonb is back on entities");

    // Every schema table exists.
    for table in [
        "tenants",
        "entities",
        "subscriptions",
        "csource_subscriptions",
        "csource_registrations",
        "csource_index",
        "jsonld_contexts",
        "outbox",
    ] {
        let ok: bool = sqlx::query("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("to_regclass")
            .get(0);
        assert!(ok, "missing table {table}");
    }

    // Idempotence: running the migrator again is a no-op, not an error.
    pg::MIGRATOR.run(&pool).await.expect("re-run migrations");
}

#[tokio::test]
async fn tenant_upsert_is_idempotent() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("race_tenant").expect("tenant");
    // Two concurrent first-writes both succeed.
    let (a, b) = tokio::join!(pg::ensure_tenant(&pool, &t), pg::ensure_tenant(&pool, &t));
    a.expect("first");
    b.expect("second");
}

/// T-5303: `entity_values` (0024) follows every write of an Entity: a
/// create keys its values, a read-modify-write and a replace re-key them, a
/// delete drops them, and a q= read finds the Entity by the new value only.
#[tokio::test]
async fn entity_values_follow_create_update_replace_and_delete() {
    use antares_sql::store::pg::entity::{EntityFilter, PgEntityStore};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pg_entity_values").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let store = PgEntityStore::new(pool.clone());
    let id = "urn:ngsi-ld:V:1";
    let _ = store.delete(&t, id).await.expect("clear");
    let doc = |speed: i64| serde_json::json!({"id": id, "type": "V", "speed": [{"type": "Property", "value": speed}]});
    assert!(store.create(&t, id, &doc(1)).await.expect("create"));
    let keyed = || async {
        let rows: Vec<(String, Option<f64>)> = sqlx::query_as(
            "SELECT attr, f FROM entity_values WHERE tenant_id = $1 AND id = $2 ORDER BY f NULLS FIRST",
        )
        .bind(t.as_str())
        .bind(id)
        .fetch_all(&pool)
        .await
        .expect("rows");
        rows
    };
    let finds = |q: &'static str| {
        let (store, t) = (&store, &t);
        async move {
            let ast = antares_ql::parse_q(q).expect("q");
            let expand = |term: &str| term.to_owned();
            let f = EntityFilter {
                q: Some(&ast),
                expand: &expand,
                ..Default::default()
            };
            store.query(t, &f).await.expect("query").rows.len()
        }
    };
    assert_eq!(keyed().await, [("speed".to_owned(), Some(1.0))]);
    store
        .mutate(&t, id, |d| {
            d["speed"][0]["value"] = serde_json::json!(2);
            Ok::<_, ()>(())
        })
        .await
        .expect("mutate")
        .expect("present")
        .expect("applied");
    assert_eq!(keyed().await, [("speed".to_owned(), Some(2.0))]);
    assert_eq!((finds("speed==1").await, finds("speed==2").await), (0, 1));
    store
        .batch_upsert_replace(&t, &[(id.to_owned(), doc(3))])
        .await
        .expect("replace");
    assert_eq!(keyed().await, [("speed".to_owned(), Some(3.0))]);
    assert_eq!((finds("speed==2").await, finds("speed>=3").await), (0, 1));
    assert!(store.delete(&t, id).await.expect("delete").is_some());
    assert!(keyed().await.is_empty(), "a deleted Entity leaves no row");
}

/// T-5772: an update rewrites only the `entity_values` rows that changed
/// (0029). A document move that keeps every value (a new `observedAt`)
/// leaves every row in place, a changed value replaces that Attribute's row
/// alone, and after each write the rows equal those of the stored document.
#[tokio::test]
async fn an_update_rewrites_only_the_entity_values_rows_that_changed() {
    use antares_sql::store::pg::entity::PgEntityStore;
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pg_entity_values_diff").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let store = PgEntityStore::new(pool.clone());
    let id = "urn:ngsi-ld:V:diff";
    let _ = store.delete(&t, id).await.expect("clear");
    let doc = serde_json::json!({"id": id, "type": "V",
        "speed": [{"type": "Property", "value": 1, "observedAt": "2026-10-29T00:00:00Z"}],
        "name": [{"type": "Property", "value": "a"}]});
    assert!(store.create(&t, id, &doc).await.expect("create"));
    // Each row with the transaction that wrote it, and whether the stored
    // rows are exactly entity_value_rows() of the stored document.
    let rows = || async {
        let rows: Vec<(String, Option<f64>, Option<String>, String)> = sqlx::query_as(
            "SELECT attr, f, txt, xmin::text FROM entity_values
              WHERE tenant_id = $1 AND id = $2 ORDER BY attr, f, txt",
        )
        .bind(t.as_str())
        .bind(id)
        .fetch_all(&pool)
        .await
        .expect("rows");
        let same: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (
               (SELECT attr, f, txt FROM entity_values WHERE tenant_id = $1 AND id = $2
                EXCEPT SELECT r.attr, r.f, r.txt FROM entities e, entity_value_rows(e.entity) r
                        WHERE e.tenant_id = $1 AND e.id = $2)
               UNION ALL
               (SELECT r.attr, r.f, r.txt FROM entities e, entity_value_rows(e.entity) r
                 WHERE e.tenant_id = $1 AND e.id = $2
                EXCEPT SELECT attr, f, txt FROM entity_values WHERE tenant_id = $1 AND id = $2))",
        )
        .bind(t.as_str())
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("compare");
        (rows, same)
    };
    let set = |path: &'static str, v: serde_json::Value| {
        let (store, t) = (&store, &t);
        async move {
            store
                .mutate(t, id, |d| {
                    d[path][0] = v.clone();
                    Ok::<_, ()>(())
                })
                .await
                .expect("mutate")
                .expect("present")
                .expect("applied");
        }
    };
    let (before, same) = rows().await;
    assert!(same, "created rows follow the document");
    assert_eq!(before.len(), 2);

    set(
        "speed",
        serde_json::json!({"type": "Property", "value": 1, "observedAt": "2026-10-29T01:00:00Z"}),
    )
    .await;
    let (moved, same) = rows().await;
    assert!(same, "rows follow the document after an observedAt move");
    assert_eq!(moved, before, "no row rewritten when no value changed");

    set("speed", serde_json::json!({"type": "Property", "value": 2})).await;
    let (changed, same) = rows().await;
    assert!(same, "rows follow the document after a value change");
    assert_eq!(changed.first(), before.first(), "the name row stays");
    assert_eq!(
        changed.get(1).map(|r| (r.0.as_str(), r.1)),
        Some(("speed", Some(2.0)))
    );
    assert_ne!(changed.get(1).map(|r| &r.3), before.get(1).map(|r| &r.3));

    // Past 32 scalars the Attribute is one NaN row, and back again.
    let many: Vec<i64> = (0..40).collect();
    set(
        "speed",
        serde_json::json!({"type": "Property", "value": many}),
    )
    .await;
    let (unknown, same) = rows().await;
    assert!(same, "rows follow the document into the unknown mark");
    assert_eq!(unknown.len(), 2);
    assert!(unknown.get(1).and_then(|r| r.1).is_some_and(f64::is_nan));
    set("speed", serde_json::json!({"type": "Property", "value": 5})).await;
    let (back, same) = rows().await;
    assert!(same, "rows follow the document out of the unknown mark");
    assert_eq!(back.get(1).map(|r| r.1), Some(Some(5.0)));
    assert!(store.delete(&t, id).await.expect("delete").is_some());
}

/// T-5303: a term more than 1 000 Entities match probes no id list and the
/// page reads as before; one that fewer match reads only those. Both count
/// exactly.
#[tokio::test]
async fn a_probe_past_its_cap_leaves_the_page_unnarrowed() {
    use antares_sql::store::pg::entity::{EntityFilter, Page, PgEntityStore};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pg_entity_values_cap").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("clear");
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:V:' || g,
                jsonb_build_object('id', 'urn:ngsi-ld:V:' || g, 'type', 'V',
                  'speed', jsonb_build_array(jsonb_build_object('type', 'Property', 'value', least(g, 2) - 1))),
                ARRAY['V'], now(), now()
           FROM generate_series(1, 1200) g",
    )
    .bind(t.as_str())
    .execute(&pool)
    .await
    .expect("seed");
    let store = PgEntityStore::new(pool.clone());
    let total = |q: &'static str| {
        let (store, t) = (&store, &t);
        async move {
            let ast = antares_ql::parse_q(q).expect("q");
            let expand = |term: &str| term.to_owned();
            let f = EntityFilter {
                q: Some(&ast),
                expand: &expand,
                page: Some(Page {
                    offset: 0,
                    limit: 5,
                    count: true,
                }),
                ..Default::default()
            };
            let out = store.query(t, &f).await.expect("query");
            (out.rows.len(), out.total)
        }
    };
    assert_eq!(total("speed==1").await, (5, Some(1199)), "past the cap");
    assert_eq!(total("speed==0").await, (1, Some(1)), "under the cap");
    assert_eq!(total("speed==7").await, (0, Some(0)), "no candidate");
}

/// The non-superuser role the RLS tests connect as (idempotent). Two tests of this binary
/// run it at once, and two concurrent GRANTs on one schema fail with "tuple concurrently
/// updated" (strict run 38884915143, T-4334): one transaction-scoped advisory lock queues them.
async fn app_role(admin: &sqlx::PgPool) {
    let mut tx = admin.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock(4334)")
        .execute(&mut *tx)
        .await
        .expect("advisory lock");
    for stmt in [
        "DO $$ BEGIN CREATE ROLE antares_app LOGIN PASSWORD 'app';
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$",
        "GRANT USAGE ON SCHEMA public TO antares_app",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO antares_app",
    ] {
        sqlx::query(stmt).execute(&mut *tx).await.expect(stmt);
    }
    tx.commit().await.expect("commit");
}

/// T-4334: the RLS tests' role setup holds when every test of the binary runs it at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_app_role_setup_runs_concurrently() {
    let url = require_db!();
    let admin = pg::connect(&url, 10).await.expect("connect");
    let runs: Vec<_> = (0..8)
        .map(|_| {
            let admin = admin.clone();
            tokio::spawn(async move { app_role(&admin).await })
        })
        .collect();
    for run in runs {
        run.await.expect("a concurrent role setup");
    }
}

/// RLS denial: connect as a NON-superuser role (superusers bypass
/// RLS) and prove a tenant sees zero foreign rows even with tenant-less SQL.
#[tokio::test]
async fn rls_denies_cross_tenant_reads_and_writes() {
    let url = require_db!();
    let admin = pg::connect(&url, 5).await.expect("connect");

    // Non-superuser app role (idempotent), least privilege on the data tables.
    app_role(&admin).await;

    let app_url = url.replace("antares:antares@", "antares_app:app@");
    assert_ne!(
        app_url, url,
        "test URL must embed antares:antares credentials"
    );
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url)
        .await
        .expect("app-role connect");

    let ta = TenantId::new("rls_a").expect("ta");
    let tb = TenantId::new("rls_b").expect("tb");

    // Write one entity as tenant a.
    let mut tx = app.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &ta).await.expect("set_tenant");
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         VALUES ($1, 'urn:rls:1', '{}'::jsonb, '{T}', now(), now())
         ON CONFLICT (tenant_id, id) DO NOTHING",
    )
    .bind(ta.as_str())
    .execute(&mut *tx)
    .await
    .expect("insert as a");
    tx.commit().await.expect("commit");

    // Tenant b, deliberately tenant-less SELECT: RLS must return zero rows.
    let mut tx = app.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &tb).await.expect("set_tenant b");
    let n: i64 = sqlx::query("SELECT count(*) FROM entities WHERE id = 'urn:rls:1'")
        .fetch_one(&mut *tx)
        .await
        .expect("select as b")
        .get(0);
    assert_eq!(n, 0, "RLS must hide tenant a's row from tenant b");

    // WITH CHECK: tenant b cannot forge a row claiming tenant a.
    let forged = sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         VALUES ($1, 'urn:rls:forged', '{}'::jsonb, '{T}', now(), now())",
    )
    .bind(ta.as_str())
    .execute(&mut *tx)
    .await;
    assert!(
        forged.is_err(),
        "WITH CHECK must reject cross-tenant inserts"
    );
    drop(tx); // rollback

    // Tenant a still sees its row.
    let mut tx = app.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &ta).await.expect("set_tenant a");
    let n: i64 = sqlx::query("SELECT count(*) FROM entities WHERE id = 'urn:rls:1'")
        .fetch_one(&mut *tx)
        .await
        .expect("select as a")
        .get(0);
    assert_eq!(n, 1);

    // No tenant set at all (current_setting returns NULL): zero rows.
    let n: i64 = sqlx::query("SELECT count(*) FROM entities")
        .fetch_one(&app)
        .await
        .expect("tenant-less select")
        .get(0);
    assert_eq!(n, 0, "no tenant in scope must mean no rows");
}

/// ADR-0021 + 4.14: a stored @context is under the same Row-Level Security as
/// every other Tenant-bearing row. A Hosted document holds term mappings
/// authored through one Tenant's requests, and 5.5.7 makes those mappings
/// decide what that Tenant's payloads mean, so another Tenant must not read
/// it, rewrite it or delete it. A Cached copy is a public document the broker
/// downloaded (5.13.1): it belongs to no Tenant and every Tenant reaches it.
///
/// Proven as the non-superuser role the deployment runs under, with SQL that
/// names no tenant at all — the belt has to bite where the code forgot to.
#[tokio::test]
async fn rls_scopes_hosted_contexts_and_shares_cached_ones() {
    let url = require_db!();
    let admin = pg::connect(&url, 5).await.expect("connect");
    app_role(&admin).await;
    let app_url = url.replace("antares:antares@", "antares_app:app@");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url)
        .await
        .expect("app-role connect");

    let ta = TenantId::new("ctxrls_a").expect("ta");
    let tb = TenantId::new("ctxrls_b").expect("tb");
    let hosted = "ctxrls:hosted";
    let cached = "ctxrls:cached";
    for id in [hosted, cached] {
        sqlx::query("DELETE FROM jsonld_contexts WHERE id = $1")
            .bind(id)
            .execute(&admin)
            .await
            .expect("clean");
    }

    // Tenant a stores one Hosted @context and one Cached copy.
    let mut tx = app.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &ta).await.expect("set_tenant a");
    sqlx::query(
        "INSERT INTO jsonld_contexts (id, body, kind) VALUES
           ($1, jsonb_build_object('owner', $3::text, 'kind', 'Hosted'), 'Hosted'),
           ($2, jsonb_build_object('kind', 'Cached'), 'Cached')",
    )
    .bind(hosted)
    .bind(cached)
    .bind(ta.as_str())
    .execute(&mut *tx)
    .await
    .expect("insert as a");
    tx.commit().await.expect("commit");

    let seen = |t: Option<&TenantId>, id: &'static str| {
        let app = app.clone();
        let t = t.map(|t| t.as_str().to_owned());
        async move {
            let mut tx = app.begin().await.expect("tx");
            if let Some(t) = &t {
                let tid = TenantId::new(t).expect("tenant");
                pg::set_tenant(&mut tx, &tid).await.expect("set_tenant");
            }
            let n: i64 = sqlx::query("SELECT count(*) FROM jsonld_contexts WHERE id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .expect("count")
                .get(0);
            n
        }
    };
    assert_eq!(seen(Some(&ta), hosted).await, 1, "its owner reads it");
    assert_eq!(
        seen(Some(&tb), hosted).await,
        0,
        "another Tenant's Hosted @context must be invisible"
    );
    assert_eq!(
        seen(None, hosted).await,
        0,
        "no Tenant in scope reaches no Tenant's documents"
    );
    for t in [Some(&ta), Some(&tb), None] {
        assert_eq!(
            seen(t, cached).await,
            1,
            "a Cached copy is a public document and belongs to no Tenant"
        );
    }

    // Tenant b can neither rewrite nor delete what it cannot see, and cannot
    // forge a row into another Tenant either (WITH CHECK).
    let mut tx = app.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &tb).await.expect("set_tenant b");
    let touched = sqlx::query("UPDATE jsonld_contexts SET body = '{}'::jsonb WHERE id = $1")
        .bind(hosted)
        .execute(&mut *tx)
        .await
        .expect("update")
        .rows_affected();
    assert_eq!(touched, 0, "RLS must hide the row from an UPDATE");
    let removed = sqlx::query("DELETE FROM jsonld_contexts WHERE id = $1")
        .bind(hosted)
        .execute(&mut *tx)
        .await
        .expect("delete")
        .rows_affected();
    assert_eq!(removed, 0, "RLS must hide the row from a DELETE");
    let forged = sqlx::query(
        "INSERT INTO jsonld_contexts (id, body, kind)
         VALUES ('ctxrls:forged', jsonb_build_object('owner', $1::text), 'Hosted')",
    )
    .bind(ta.as_str())
    .execute(&mut *tx)
    .await;
    assert!(
        forged.is_err(),
        "WITH CHECK must reject an @context forged into another Tenant"
    );
    drop(tx); // rollback

    assert_eq!(
        seen(Some(&ta), hosted).await,
        1,
        "the owner's @context survived every foreign attempt"
    );
    for id in [hosted, cached] {
        sqlx::query("DELETE FROM jsonld_contexts WHERE id = $1")
            .bind(id)
            .execute(&admin)
            .await
            .expect("cleanup");
    }
}

/// Purging a tenant empties every tenant-bearing table for it in one
/// transaction and touches no row of another tenant.
#[tokio::test(flavor = "multi_thread")]
async fn purge_tenant_empties_every_tenant_table() {
    use antares_store::{CurrentStateDriver, Kind, TemporalDriver};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    store.set_outbox(true);
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let a = TenantId::new(&format!("pgpa{run}")).expect("tenant");
    // `b` extends `a`: a purge that ever matched its tenant by prefix
    // instead of by equality would take this one with it.
    let b = TenantId::new(&format!("pgpa{run}x")).expect("tenant");
    let doc = |id: &str| {
        serde_json::json!({
            "id": id, "type": "Test",
            "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
            "n": {"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}
        })
    };
    for t in [&a, &b] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
        for kind in [
            Kind::Entity,
            Kind::Subscription,
            Kind::Registration,
            Kind::CSourceSubscription,
            Kind::Snapshot,
            Kind::EntityMap,
            Kind::DistSub,
            Kind::DeadLetter,
            Kind::View,
        ] {
            let id = "urn:x:1";
            let d = if kind == Kind::Registration {
                serde_json::json!({"id": id, "type": "ContextSourceRegistration",
                    "information": [{"entities": [{"type": "Test"}]}],
                    "endpoint": "http://127.0.0.1:9"})
            } else {
                doc(id)
            };
            assert!(
                CurrentStateDriver::create(&store, t, kind, id, d)
                    .await
                    .expect("create"),
                "{kind:?}"
            );
        }
        // ADR-0021: a Hosted @context belongs to the Tenant that stored it,
        // so it is one more table the purge has to reach.
        CurrentStateDriver::context_put(
            &store,
            Some(t),
            &format!("ctx-{}", t.as_str()),
            serde_json::json!({"url": format!("http://b.example/ngsi-ld/v1/jsonldContexts/ctx-{}", t.as_str()),
                               "localId": format!("ctx-{}", t.as_str()), "kind": "Hosted",
                               "owner": t.as_str(), "createdAt": "2026-08-04T09:00:00Z",
                               "body": {"@context": {"n": "https://x/n"}}}),
        )
        .await.expect("hosted @context");
        TemporalDriver::create(&store, t, "urn:x:1", doc("urn:x:1"))
            .await
            .expect("temporal");
        store
            .temporal_append(
                t,
                "urn:x:1",
                &doc("urn:x:1"),
                &serde_json::json!({"n": [{"type": "Property", "value": 2,
                    "observedAt": "2026-08-04T09:01:00Z", "instanceId": "urn:i:1"}]}),
            )
            .await
            .expect("append");
        // Another value for the same instant: a correction, which keeps v1 in attr_versions.
        store
            .temporal_append(
                t,
                "urn:x:1",
                &doc("urn:x:1"),
                &serde_json::json!({"n": [{"type": "Property", "value": 3,
                    "observedAt": "2026-08-04T09:01:00Z", "instanceId": "urn:i:1"}]}),
            )
            .await
            .expect("correction");
        // The keyed history parent (0020) is not written before the cut-over:
        // seeded by hand, as the cut-over will fill it.
        for stmt in [
            "INSERT INTO iri_ids (tenant_id, iri) VALUES ($1, 'urn:x:1') ON CONFLICT DO NOTHING",
            "INSERT INTO attr_rows (tenant_id, entity_no, attr_no, instance_no, observed_at,
                                    created_at, modified_at, data)
             VALUES ($1, iri_no($1, 'urn:x:1'), 1, gen_random_uuid(), now(), now(), now(), '{}')",
        ] {
            sqlx::query(stmt)
                .bind(t.as_str())
                .execute(&pool)
                .await
                .expect(stmt);
        }
    }
    const TABLES: [&str; 20] = [
        "entities",
        "entity_values",
        "entity_types",
        "entity_attrs",
        "jsonld_contexts",
        "subscriptions",
        "csource_subscriptions",
        "csource_registrations",
        "csource_index",
        "outbox",
        "snapshots",
        "entity_map_docs",
        "dist_subs",
        "dead_letters",
        "views",
        "temporal_entities",
        "attr_instances",
        "attr_versions",
        "iri_ids",
        "attr_rows",
    ];
    let count = |table: &'static str, t: &TenantId| {
        let pool = pool.clone();
        let t = t.as_str().to_string();
        async move {
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE tenant_id = $1"
            )))
            .bind(t)
            .fetch_one(&pool)
            .await
            .expect("count");
            n
        }
    };
    // The list above is hand-written, and so is the one `purge_tenant`
    // deletes from. A new tenant-bearing table added to the schema without a
    // purge branch would keep that tenant's rows after its deletion, and no
    // assertion below would notice. Ask the catalogue instead: every table
    // with a `tenant_id` column is either purged here or `tenants` itself,
    // whose row purge_tenant removes and `tenant_exists` reports on.
    let bearing: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = 'public'
           JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id'
          WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition
          ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("catalogue");
    let mut want: Vec<String> = TABLES.iter().map(|t| (*t).to_owned()).collect();
    want.push("tenants".to_owned());
    want.sort();
    assert_eq!(
        bearing, want,
        "a tenant-bearing table is missing from the purge"
    );
    for table in TABLES {
        assert!(count(table, &a).await > 0, "seed missing in {table}");
    }
    let before_b: Vec<i64> = {
        let mut v = Vec::new();
        for table in TABLES {
            v.push(count(table, &b).await);
        }
        v
    };
    let ids = store.tenant_ids().await.expect("ids");
    assert!(ids.iter().any(|t| t == a.as_str()), "listed: {ids:?}");
    assert!(
        ids.iter().any(|t| t == antares_model::TenantId::DEFAULT),
        "5.5.10: the default tenant is always listed: {ids:?}"
    );
    let row = store
        .tenant_stats_one(&a)
        .await
        .expect("stats")
        .expect("tenant exists");
    assert_eq!(
        (row.entities, row.subscriptions, row.registrations),
        (1, 1, 1)
    );
    assert_eq!(row.dist_subs, 1);
    assert!(
        row.created_at.is_some(),
        "the tenants row carries its stamp"
    );
    assert!(
        store
            .tenant_stats_one(&TenantId::new("never-seen").expect("tenant"))
            .await
            .expect("stats")
            .is_none(),
        "an unknown tenant has no stats to report"
    );
    assert!(
        TemporalDriver::attr_instance_count(&store, &a)
            .await
            .expect("n")
            >= 1
    );

    assert!(CurrentStateDriver::purge_tenant(&store, &a)
        .await
        .expect("purge"));
    TemporalDriver::purge_tenant(&store, &a)
        .await
        .expect("purge history");
    for table in TABLES {
        assert_eq!(
            count(table, &a).await,
            0,
            "{table} still holds rows of the purged tenant"
        );
    }
    assert!(!store.tenant_exists(&a).await.expect("exists"));
    for (table, before) in TABLES.iter().zip(before_b) {
        assert_eq!(
            count(table, &b).await,
            before,
            "{table} of the other tenant changed"
        );
    }
    assert!(
        !CurrentStateDriver::purge_tenant(&store, &a)
            .await
            .expect("purge"),
        "nothing left"
    );
    // cleanup
    assert!(CurrentStateDriver::purge_tenant(&store, &b)
        .await
        .expect("purge b"));
    TemporalDriver::purge_tenant(&store, &b)
        .await
        .expect("purge b history");
}

/// The startup probe reads what the server actually is, and the store serves
/// that copy afterwards. `/q/health` is polled, so a health body that named
/// the server by querying for it would put a database round trip on every
/// probe; the version is captured once, here.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_version_is_probed_once_and_served_from_the_store() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let probed = pg::version_info(&pool).await;
    assert_eq!(probed["engine"], "postgres", "{probed}");
    assert!(
        probed["server"].as_str().is_some_and(|v| !v.is_empty()),
        "the server version must be named: {probed}"
    );
    assert!(
        probed["postgis"].as_str().is_some(),
        "the broker needs PostGIS and must report which one: {probed}"
    );

    let plain = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    assert_eq!(
        plain.version_info(),
        serde_json::json!({}),
        "a store built without the probe reports nothing rather than guessing"
    );
    let probed_store = antares_sql::store::any::AnyStore::Pg(
        antares_sql::store::any::PgBackend::new(pool).with_version(probed.clone()),
    );
    assert_eq!(probed_store.version_info(), probed);
}

/// The Postgres arms answer the same driver contract as the in-process ones
/// (`antares_store::contract`). A backend's own tests assert what that
/// backend does; this asserts what the seam promises, which is what
/// `antares-api` was written against.
#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_store_keeps_the_driver_contract() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let a = TenantId::new(&format!("ctra{run}")).expect("tenant");
    let b = TenantId::new(&format!("ctrb{run}")).expect("tenant");
    for t in [&a, &b] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
    }
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let prefix = format!("pgctr{run}");
    antares_store::contract::run_current_state_contract(&store, &a, &b, &prefix).await;
    antares_store::contract::run_temporal_contract(&store, &a, &b, &prefix).await;
}

/// T-3914: the same isolation walk as the in-process stores, on fresh
/// Tenants (the purge empties `a`). `_` would be a LIKE wildcard; `x`
/// extends the other name. The default Tenant is `b`, never purged.
#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_store_keeps_tenants_apart() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let t = |s: String| TenantId::new(&s).expect("tenant");
    let pairs = [
        (t(format!("iso{run}_b")), t(format!("iso{run}xb"))),
        (t(format!("isp{run}xb")), t(format!("isp{run}_b"))),
        (t(format!("isq{run}")), t(format!("isq{run}x"))),
        (t(format!("isr{run}")), TenantId::default()),
    ];
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    for (n, (a, b)) in pairs.iter().enumerate() {
        for t in [a, b] {
            pg::ensure_tenant(&pool, t).await.expect("tenant row");
        }
        antares_store::contract::run_tenant_isolation_contract(
            &store,
            &store,
            a,
            b,
            &format!("pgiso{run}{n}"),
        )
        .await;
    }
}

/// The tenants the broker mints for itself are not customer accounts, and
/// `/q/tenants` reads this list. They still hold a row: on Postgres this table
/// IS the broker's tenant enumeration — `subscription_tenants` reads it,
/// because `subscriptions` sits outside the `antares.service` escape (a
/// subscription document carries the credentials its notification is sent
/// with), and the mirror seed and the notification sweep walk what it answers.
/// So the row is written and the READ is what leaves it out.
#[tokio::test(flavor = "multi_thread")]
async fn an_internal_tenant_holds_a_row_and_is_left_out_of_the_inventory() {
    use antares_store::{CurrentStateDriver, Kind};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let synth = TenantId::new_internal(&format!("snap-{run}")).expect("synthetic tenant");
    assert!(synth.is_internal());
    let id = "urn:ngsi-ld:Vehicle:frozen";
    store
        .create(
            &synth,
            Kind::Entity,
            id,
            serde_json::json!({"id": id, "type": ["urn:x:Vehicle"]}),
        )
        .await
        .expect("write into the synthetic tenant");

    let row: Option<i32> = sqlx::query_scalar("SELECT 1 FROM tenants WHERE tenant_id = $1")
        .bind(synth.as_str())
        .fetch_optional(&pool)
        .await
        .expect("inventory read");
    assert!(
        row.is_some(),
        "{} lost the row the tenant enumeration reads",
        synth.as_str()
    );
    assert!(
        store
            .subscription_tenants()
            .await
            .expect("enumeration")
            .iter()
            .any(|t| t == synth.as_str()),
        "the notification pipeline cannot reach a tenant it cannot enumerate"
    );

    let inventory = store.tenant_ids().await.expect("inventory");
    assert!(
        !inventory.iter().any(|t| t == synth.as_str()),
        "the broker's own tenant is listed as an account: {inventory:?}"
    );

    assert!(
        CurrentStateDriver::purge_tenant(&store, &synth)
            .await
            .expect("purge"),
        "the teardown purges the synthetic tenant"
    );
    assert!(
        store
            .list(&synth, Kind::Entity)
            .await
            .expect("list")
            .is_empty(),
        "the copy survived its teardown"
    );
}

/// A repeat write must not rewrite the Tenant row. `claim_tenant` runs in
/// every document transaction, so an upsert that takes the exclusive row lock
/// serialises every write in a Tenant behind every other and leaves one dead
/// tuple per write on a table with one row per Tenant. The lock the purge
/// waits on is still taken; only the row version is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeat_write_does_not_rewrite_the_tenant_row() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let t = TenantId::new(&format!("pgclaim{run}")).expect("tenant");

    let xmin = |pool: sqlx::PgPool, t: TenantId| async move {
        sqlx::query("SELECT xmin::text AS x FROM tenants WHERE tenant_id = $1")
            .bind(t.as_str())
            .fetch_one(&pool)
            .await
            .expect("xmin")
            .get::<String, _>("x")
    };
    let claim = |pool: sqlx::PgPool, t: TenantId| async move {
        let mut tx = pool.begin().await.expect("begin");
        pg::set_tenant(&mut tx, &t).await.expect("set_tenant");
        pg::claim_tenant(&mut tx, &t).await.expect("claim");
        tx.commit().await.expect("commit");
    };

    claim(pool.clone(), t.clone()).await;
    let first = xmin(pool.clone(), t.clone()).await;
    claim(pool.clone(), t.clone()).await;
    let second = xmin(pool.clone(), t.clone()).await;
    assert_eq!(
        first, second,
        "the second write rewrote the tenant row (xmin moved), so every write \
         in this tenant takes the exclusive row lock"
    );

    sqlx::query("DELETE FROM tenants WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// The invariant the row lock exists for: a purge must wait for a write that
/// is already in flight in the same Tenant, so it cannot step over it and
/// leave rows behind for a Tenant the inventory no longer names. The write
/// holds a shared lock, the purge asks for `FOR UPDATE`, and the two
/// conflict — proven here by giving the purge a deadline it must miss.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_waits_for_an_in_flight_write_in_the_same_tenant() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let t = TenantId::new(&format!("pgrace{run}")).expect("tenant");

    // the tenant exists before the race, so the purge has a row to lock
    let mut seed = pool.begin().await.expect("begin");
    pg::set_tenant(&mut seed, &t).await.expect("set_tenant");
    pg::claim_tenant(&mut seed, &t).await.expect("claim");
    seed.commit().await.expect("commit");

    // a write in flight: claimed, not yet committed
    let mut writing = pool.begin().await.expect("begin");
    pg::set_tenant(&mut writing, &t).await.expect("set_tenant");
    pg::claim_tenant(&mut writing, &t).await.expect("claim");

    let mut purge = pool.begin().await.expect("begin");
    sqlx::query("SET LOCAL lock_timeout = '750ms'")
        .execute(&mut *purge)
        .await
        .expect("lock_timeout");
    pg::set_tenant(&mut purge, &t).await.expect("set_tenant");
    let blocked =
        sqlx::query_scalar::<_, i32>("SELECT 1 FROM tenants WHERE tenant_id = $1 FOR UPDATE")
            .bind(t.as_str())
            .fetch_optional(&mut *purge)
            .await;
    assert!(
        blocked.is_err(),
        "the purge took the tenant row while a write held it, so it can step \
         over an in-flight write"
    );
    drop(purge);

    // once the write commits the purge gets the row
    writing.commit().await.expect("commit");
    let mut after = pool.begin().await.expect("begin");
    pg::set_tenant(&mut after, &t).await.expect("set_tenant");
    let got = sqlx::query_scalar::<_, i32>("SELECT 1 FROM tenants WHERE tenant_id = $1 FOR UPDATE")
        .bind(t.as_str())
        .fetch_optional(&mut *after)
        .await
        .expect("after the write commits the purge proceeds");
    assert_eq!(got, Some(1));
    after.commit().await.expect("commit");

    sqlx::query("DELETE FROM tenants WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// T-2890: a tenant's size is measured in its own rows, by the Entity's first
/// type, so the types sum to the whole; history is measured apart, and
/// another tenant's rows (RLS and the predicate both) never count.
#[tokio::test(flavor = "multi_thread")]
async fn tenant_bytes_are_the_tenants_own_rows_by_first_type() {
    use antares_store::{CurrentStateDriver, Kind, TemporalDriver};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let a = TenantId::new(&format!("pgsz{run}")).expect("tenant");
    let b = TenantId::new(&format!("pgsz{run}x")).expect("tenant");
    let doc = |id: &str, ty: serde_json::Value| {
        serde_json::json!({
            "id": id, "type": ty,
            "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
            "n": {"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}
        })
    };
    let empty = CurrentStateDriver::tenant_bytes(&store, &a)
        .await
        .expect("bytes");
    assert_eq!(
        empty,
        antares_store::TenantBytes::default(),
        "no rows, no bytes"
    );
    for t in [&a, &b] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
    }
    for (id, ty) in [
        ("urn:x:1", serde_json::json!("Room")),
        ("urn:x:2", serde_json::json!("Room")),
        ("urn:x:3", serde_json::json!(["Bridge", "Room"])),
    ] {
        assert!(
            CurrentStateDriver::create(&store, &a, Kind::Entity, id, doc(id, ty))
                .await
                .expect("create")
        );
    }
    let big = serde_json::json!({"id": "urn:x:9", "type": "Room", "pad": {"type": "Property", "value": "x".repeat(4000)}});
    CurrentStateDriver::create(&store, &b, Kind::Entity, "urn:x:9", big)
        .await
        .expect("create in b");
    let size = CurrentStateDriver::tenant_bytes(&store, &a)
        .await
        .expect("bytes");
    assert_eq!(
        size.by_type.keys().collect::<Vec<_>>(),
        ["Bridge", "Room"],
        "{size:?}"
    );
    assert!(
        size.by_type["Room"] > size.by_type["Bridge"],
        "two rows outweigh one: {size:?}"
    );
    assert_eq!(
        size.entities,
        size.by_type.values().sum::<u64>(),
        "{size:?}"
    );
    let other = CurrentStateDriver::tenant_bytes(&store, &b)
        .await
        .expect("bytes");
    assert!(
        other.entities > 0 && size.entities > 0,
        "{size:?} {other:?}"
    );
    assert_eq!(other.by_type.len(), 1, "{other:?}");

    assert_eq!(
        TemporalDriver::attr_instance_bytes(&store, &a)
            .await
            .expect("n"),
        0
    );
    TemporalDriver::create(
        &store,
        &a,
        "urn:x:1",
        doc("urn:x:1", serde_json::json!("Room")),
    )
    .await
    .expect("temporal");
    store
        .temporal_append(
            &a,
            "urn:x:1",
            &doc("urn:x:1", serde_json::json!("Room")),
            &serde_json::json!({"n": [{"type": "Property", "value": 2,
                "observedAt": "2026-08-04T09:01:00Z", "instanceId": "urn:i:1"}]}),
        )
        .await
        .expect("append");
    assert!(
        TemporalDriver::attr_instance_bytes(&store, &a)
            .await
            .expect("n")
            > 0
    );
    assert_eq!(
        TemporalDriver::attr_instance_bytes(&store, &b)
            .await
            .expect("n"),
        0
    );
    for t in [&a, &b] {
        CurrentStateDriver::purge_tenant(&store, t)
            .await
            .expect("purge");
        TemporalDriver::purge_tenant(&store, t)
            .await
            .expect("purge history");
    }
}

/// T-6159: the size estimate of a tenant whose exact scan fails counts the
/// tenant's own rows by first type and weighs each by the analyzed table's
/// average row, so the types sum to the whole and another tenant's rows
/// never count.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_size_estimate_counts_its_own_rows_by_first_type() {
    use antares_store::{CurrentStateDriver, Kind};
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let a = TenantId::new(&format!("pgest{run}")).expect("tenant");
    let b = TenantId::new(&format!("pgest{run}x")).expect("tenant");
    for t in [&a, &b] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
    }
    for (t, id, ty) in [
        (&a, "urn:x:1", serde_json::json!("Room")),
        (&a, "urn:x:2", serde_json::json!("Room")),
        (&a, "urn:x:3", serde_json::json!(["Bridge", "Room"])),
        (&b, "urn:x:4", serde_json::json!("Car")),
    ] {
        let doc = serde_json::json!({"id": id, "type": ty,
            "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z"});
        CurrentStateDriver::create(&store, t, Kind::Entity, id, doc)
            .await
            .expect("create");
    }
    sqlx::query("ANALYZE entities")
        .execute(&pool)
        .await
        .expect("analyze");
    let est = CurrentStateDriver::tenant_bytes_estimate(&store, &a, 3)
        .await
        .expect("estimate");
    assert_eq!(
        est.by_type.keys().collect::<Vec<_>>(),
        ["Bridge", "Room"],
        "{est:?}"
    );
    assert!(est.by_type["Room"] > est.by_type["Bridge"], "{est:?}");
    assert!(est.by_type["Bridge"] > 0, "{est:?}");
    assert_eq!(est.entities, est.by_type.values().sum::<u64>(), "{est:?}");
    for t in [&a, &b] {
        CurrentStateDriver::purge_tenant(&store, t)
            .await
            .expect("purge");
    }
}

/// T-2890: a size scan is bounded: held behind a lock (as behind a scan of a
/// history table tens of GB long) it gives up within its statement timeout
/// instead of holding the connection, and the caller answers without a size;
/// the estimate behind the same lock still sizes the tenant (T-6204).
/// The lock is taken in a database of the test's own, so the tests sharing
/// the main one never wait on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_size_scan_gives_up_within_its_bound() {
    use antares_store::{CurrentStateDriver, TemporalDriver};
    let url = require_db!();
    let admin = pg::connect(&url, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_szbound_{run}");
    let create = format!("CREATE DATABASE {name}");
    sqlx::query(sqlx::AssertSqlSafe(create))
        .execute(&admin)
        .await
        .expect("own database");
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    let pool = pg::connect(&format!("{base}/{name}"), 3)
        .await
        .expect("connect+migrate the own database");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let t = TenantId::new("pgszbound").expect("tenant");
    // a row and ANALYZE, so the catalog holds an average row to estimate by
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let doc = serde_json::json!({"id": "urn:x:1", "type": "Room",
        "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z"});
    CurrentStateDriver::create(&store, &t, antares_store::Kind::Entity, "urn:x:1", doc)
        .await
        .expect("create");
    sqlx::query("ANALYZE entities")
        .execute(&pool)
        .await
        .expect("analyze");
    for (table, lock) in [
        ("entities", "LOCK TABLE entities IN ACCESS EXCLUSIVE MODE"),
        (
            "attr_instances",
            "LOCK TABLE attr_instances IN ACCESS EXCLUSIVE MODE",
        ),
    ] {
        let mut hold = pool.begin().await.expect("begin");
        sqlx::query(lock).execute(&mut *hold).await.expect("lock");
        let started = std::time::Instant::now();
        let failed = if table == "entities" {
            CurrentStateDriver::tenant_bytes(&store, &t).await.is_err()
        } else {
            TemporalDriver::attr_instance_bytes(&store, &t)
                .await
                .is_err()
        };
        let took = started.elapsed();
        // T-6204: the estimate's split by type gives up behind the same
        // lock, and the size still comes from the catalog and the count
        let estimate = if table == "entities" {
            Some(CurrentStateDriver::tenant_bytes_estimate(&store, &t, 1000).await)
        } else {
            None
        };
        hold.rollback().await.expect("release");
        assert!(failed, "{table}: a scan behind the lock must give up");
        if let Some(estimate) = estimate {
            let b = estimate.expect("an estimate behind the lock");
            assert!(b.entities > 0, "{b:?}");
            assert!(b.by_type.is_empty(), "{b:?}");
        }
        assert!(
            took < std::time::Duration::from_secs(10),
            "{table}: {took:?}"
        );
    }
    pool.close().await;
    let drop = format!("DROP DATABASE {name}");
    sqlx::query(sqlx::AssertSqlSafe(drop))
        .execute(&admin)
        .await
        .expect("drop the own database");
}

/// T-4669: a history append holds `FOR KEY SHARE` on its Entity until it
/// commits. A write of that Entity goes through (it changes no key), and only
/// a DELETE waits for the append, which is the one thing the lock is for.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_passes_an_in_flight_history_append_and_a_delete_waits() {
    use antares_store::Kind;
    use std::time::Duration;
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let t = TenantId::new(&format!("pgks{run}")).expect("tenant");
    let id = "urn:ngsi-ld:BikeHireDockingStation:001";
    let doc = |n: i64| {
        serde_json::json!({
            "id": id, "type": "BikeHireDockingStation",
            "createdAt": "2026-10-19T09:00:00Z", "modifiedAt": "2026-10-19T09:00:00Z",
            "n": {"type": "Property", "value": n}
        })
    };
    store
        .create(&t, Kind::Entity, id, doc(1))
        .await
        .expect("create");

    // the append in flight, as `append_in` takes it
    let mut append = pool.begin().await.expect("begin");
    pg::set_tenant(&mut append, &t).await.expect("set_tenant");
    sqlx::query("SELECT 1 FROM entities WHERE tenant_id = $1 AND id = $2 FOR KEY SHARE")
        .bind(t.as_str())
        .bind(id)
        .fetch_optional(&mut *append)
        .await
        .expect("key share");

    let upserted = tokio::time::timeout(
        Duration::from_secs(3),
        store.batch_upsert(&t, vec![(id.to_owned(), doc(2))]),
    )
    .await;
    assert!(
        matches!(upserted, Ok(Ok(_))),
        "a batch upsert waited on the history append: {upserted:?}"
    );
    let mutated = tokio::time::timeout(
        Duration::from_secs(3),
        store.mutate::<(), ()>(&t, Kind::Entity, id, |e| {
            e["n"]["value"] = serde_json::json!(3);
            Ok(())
        }),
    )
    .await;
    assert!(
        matches!(mutated, Ok(Ok(Some(Ok(()))))),
        "a patch waited on the history append: {mutated:?}"
    );

    let mut delete = pool.begin().await.expect("begin");
    sqlx::query("SET LOCAL lock_timeout = '750ms'")
        .execute(&mut *delete)
        .await
        .expect("lock_timeout");
    pg::set_tenant(&mut delete, &t).await.expect("set_tenant");
    let blocked = sqlx::query("DELETE FROM entities WHERE tenant_id = $1 AND id = $2")
        .bind(t.as_str())
        .bind(id)
        .execute(&mut *delete)
        .await;
    assert!(
        blocked.is_err(),
        "a delete stepped over an in-flight history append"
    );
    drop(delete);
    append.commit().await.expect("commit");

    assert!(store.delete(&t, Kind::Entity, id).await.expect("delete"));
    sqlx::query("DELETE FROM tenants WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// T-5304: the history count of `GET /q/tenants/{tenant}` holds to its
/// bound. A table the count must read is held by another session, so the
/// count waits; with a bound under the session's 5 s lock_timeout it gives
/// up as a statement timeout, and without the hold it answers what the
/// unbounded count answers.
/// T-6085: one tenant's history count and size in one bounded pass, each
/// tenant's own rows only, and the statistics' guess a read takes when that
/// pass runs out of time lands near the exact numbers.
#[tokio::test(flavor = "multi_thread")]
async fn history_usage_is_per_tenant_and_its_estimate_is_near_it() {
    use antares_store::TemporalDriver;
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let a = TenantId::new(&format!("pgusea{run}")).expect("tenant");
    let b = TenantId::new(&format!("pguseb{run}")).expect("tenant");
    let shell = serde_json::json!({
        "id": "urn:x:1", "type": "Room",
        "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
        "n": {"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}
    });
    for (t, n) in [(&a, 300_u32), (&b, 2)] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
        // history is kept only for an Entity the current state holds
        antares_store::CurrentStateDriver::create(
            &store,
            t,
            antares_store::Kind::Entity,
            "urn:x:1",
            shell.clone(),
        )
        .await
        .expect("entity");
        TemporalDriver::create(&store, t, "urn:x:1", shell.clone())
            .await
            .expect("temporal");
        let instances: Vec<_> = (0..n)
            .map(|i| {
                serde_json::json!({"type": "Property", "value": i,
                    "observedAt": format!("2026-08-04T09:{:02}:{:02}Z", i / 60, i % 60),
                    "instanceId": format!("urn:i:{i}")})
            })
            .collect();
        store
            .temporal_append(t, "urn:x:1", &shell, &serde_json::json!({"n": instances}))
            .await
            .expect("append");
    }
    let bound = std::time::Duration::from_secs(5);
    let ua = TemporalDriver::attr_instance_usage(&store, &a, bound)
        .await
        .expect("usage a");
    let ub = TemporalDriver::attr_instance_usage(&store, &b, bound)
        .await
        .expect("usage b");
    assert_eq!(ua.0, 300, "{ua:?}");
    assert_eq!(ub.0, 2, "{ub:?}");
    assert_eq!(
        ua,
        (
            TemporalDriver::attr_instance_count(&store, &a)
                .await
                .expect("n"),
            TemporalDriver::attr_instance_bytes(&store, &a)
                .await
                .expect("b"),
        )
    );
    assert!(ub.1 > 0 && ub.1 < ua.1, "{ua:?} {ub:?}");

    sqlx::query("ANALYZE attr_instances, attr_rows")
        .execute(&pool)
        .await
        .expect("analyze");
    let ea = TemporalDriver::attr_instance_estimate(&store, &a)
        .await
        .expect("estimate a");
    let eb = TemporalDriver::attr_instance_estimate(&store, &b)
        .await
        .expect("estimate b");
    assert!(ea.0 >= 150 && ea.0 <= 600, "{ea:?} vs {ua:?}");
    assert!(ea.1 > 0, "{ea:?}");
    assert!(eb.0 < ea.0, "{eb:?} {ea:?}");

    for t in [&a, &b] {
        antares_store::CurrentStateDriver::purge_tenant(&store, t)
            .await
            .expect("purge");
        TemporalDriver::purge_tenant(&store, t)
            .await
            .expect("purge");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn history_count_within_gives_up_at_its_bound() {
    use antares_store::TemporalDriver;
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect+migrate");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let t = TenantId::new(&format!("pgcount{run}")).expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let bound = std::time::Duration::from_millis(300);
    assert_eq!(
        TemporalDriver::attr_instance_count_within(&store, &t, bound)
            .await
            .expect("bounded count"),
        TemporalDriver::attr_instance_count(&store, &t)
            .await
            .expect("count"),
    );

    let mut hold = pool.begin().await.expect("begin");
    sqlx::query("LOCK TABLE attr_rows IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *hold)
        .await
        .expect("hold attr_rows");
    let started = std::time::Instant::now();
    let cut = TemporalDriver::attr_instance_count_within(&store, &t, bound).await;
    let waited = started.elapsed();
    hold.rollback().await.expect("release");
    assert_eq!(
        cut.map_err(|e| e.to_string()),
        Err(
            antares_model::NgsiError::InternalError("database statement timeout".into())
                .to_string()
        )
    );
    assert!(waited < std::time::Duration::from_secs(3), "{waited:?}");
}
