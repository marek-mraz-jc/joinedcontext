#![forbid(unsafe_code)]
//! PostgreSQL store isolation, boot RLS check, `shako.org` unset zero-row check,
//! and leakproof `(instance_id, owner, name)` index plan test (Contract S2).
//!
//! When `SHAKO_TEST_DATABASE_URL` is unset, returns early with a printed `SKIP` line;
//! when set and unreachable, FAILS as required by Contract S2.

use shako_core::Store;
use shako_core::testkit::run_isolation_suite;
use shako_store_pg::{
    PgStore, PgStoreConfig, REQUIRED_RLS_TABLES, TableRlsStatus, validate_rls_catalog_rows,
};
use sqlx::Row;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

#[test]
fn catalog_validation_refuses_unforced_rls_and_bypassrls_and_table_ownership() {
    let valid: Vec<TableRlsStatus> = REQUIRED_RLS_TABLES
        .iter()
        .map(|&name| TableRlsStatus {
            table_name: name.to_owned(),
            rls_enabled: true,
            rls_forced: true,
            policy_count: 1,
        })
        .collect();
    assert!(validate_rls_catalog_rows(false, false, 0, 0, &valid).is_ok());

    // BYPASSRLS -> refused
    assert!(validate_rls_catalog_rows(true, false, 0, 0, &valid).is_err());
    // SUPERUSER -> refused
    assert!(validate_rls_catalog_rows(false, true, 0, 0, &valid).is_err());
    // Owns a table -> refused
    assert!(validate_rls_catalog_rows(false, false, 1, 0, &valid).is_err());
    // External schema grant -> refused
    assert!(validate_rls_catalog_rows(false, false, 0, 1, &valid).is_err());

    // Extra table in schema `shako` without forced RLS -> refused
    let mut with_rogue = valid.clone();
    with_rogue.push(TableRlsStatus {
        table_name: "rogue_unforced".into(),
        rls_enabled: true,
        rls_forced: false,
        policy_count: 1,
    });
    assert!(validate_rls_catalog_rows(false, false, 0, 0, &with_rogue).is_err());
}

#[tokio::test]
async fn postgres_isolation_boot_rls_and_leakproof_index_plan() {
    let Ok(db_url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
        println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
        return;
    };
    // When SHAKO_TEST_DATABASE_URL is set and unreachable, this `.expect(...)` FAILS the test (S2).
    let opts = PgConnectOptions::from_str(&db_url).expect("valid SHAKO_TEST_DATABASE_URL");
    let admin_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");

    PgStore::run_migrations(&admin_pool)
        .await
        .expect("migrations apply cleanly on Postgres");

    // 1. Queries as `shako_app` with `shako.org` unset return zero rows
    let mut tx = admin_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE shako_app")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count_unset: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shako.repo")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count_unset, 0, "queries as shako_app with shako.org unset must return zero rows");

    // 2. Prove leakproof index plan for `(instance_id, owner, name)` as `shako_app`
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "SELECT set_config('shako.instance', '00000000-0000-7000-8000-000000000001', true),
                set_config('shako.org', '00000000-0000-7000-8000-000000000001', true)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let explain_rows = sqlx::query(
        "EXPLAIN SELECT id FROM shako.repo
         WHERE instance_id = '00000000-0000-7000-8000-000000000001'::uuid
           AND owner = 'helsinki'
           AND name = 'configuration'",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let plan_text: String = explain_rows
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan_text.contains("idx_shako_repo_instance_owner_name"),
        "expected Index Scan on idx_shako_repo_instance_owner_name, got:\n{plan_text}"
    );
    tx.commit().await.unwrap();

    // 3. Verify boot check catches a table without forced RLS
    let store = PgStore::from_pool_unchecked(admin_pool.clone());
    sqlx::query("CREATE TABLE IF NOT EXISTS shako.test_unforced (instance_id UUID, org_id UUID)")
        .execute(&admin_pool)
        .await
        .unwrap();
    let boot_res = store.verify_rls_enforced().await;
    sqlx::query("DROP TABLE IF EXISTS shako.test_unforced")
        .execute(&admin_pool)
        .await
        .unwrap();
    assert!(
        boot_res.is_err(),
        "boot check must refuse when a table lacks forced RLS"
    );

    let _ = PgStoreConfig {
        url_without_password: db_url,
        password: None,
        max_connections: 3,
    };
    run_isolation_suite(&store).await;
}
