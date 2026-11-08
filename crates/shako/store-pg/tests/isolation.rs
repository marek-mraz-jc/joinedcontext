#![forbid(unsafe_code)]
//! PostgreSQL store isolation, boot RLS check, `shako.org` unset zero-row check,
//! and leakproof `(instance_id, owner, name)` index plan test (Contract S2).
//!
//! When `SHAKO_TEST_DATABASE_URL` is unset, returns early with a printed `SKIP` line;
//! when set and unreachable, FAILS as required by Contract S2.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection. The admin connection runs the
//! migrations and owns the tables, as `shako_migrate` does in production; the request-path store
//! connects as the LOGIN role `shako_app_login` (member of `shako_app`, NOSUPERUSER, NOBYPASSRLS,
//! owns no table), so forced RLS is what the store really runs under. Its password is random per
//! test process and never written anywhere.

use shako_core::Store;
use shako_core::testkit::run_isolation_suite;
use shako_store_pg::{
    PgStore, PgStoreConfig, REQUIRED_RLS_TABLES, TableRlsStatus, validate_rls_catalog_rows,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, Row};
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
    assert!(validate_rls_catalog_rows("shako_app_login", false, false, 0, 0, &valid).is_ok());

    // BYPASSRLS -> refused
    assert!(validate_rls_catalog_rows("shako_app_login", true, false, 0, 0, &valid).is_err());
    // SUPERUSER -> refused
    assert!(validate_rls_catalog_rows("shako_app_login", false, true, 0, 0, &valid).is_err());
    // Owns a table -> refused
    assert!(validate_rls_catalog_rows("shako_app_login", false, false, 1, 0, &valid).is_err());
    // External schema grant -> refused
    assert!(validate_rls_catalog_rows("shako_app_login", false, false, 0, 1, &valid).is_err());

    // Extra table in schema `shako` without forced RLS -> refused
    let mut with_rogue = valid.clone();
    with_rogue.push(TableRlsStatus {
        table_name: "rogue_unforced".into(),
        rls_enabled: true,
        rls_forced: false,
        policy_count: 1,
    });
    assert!(validate_rls_catalog_rows("shako_app_login", false, false, 0, 0, &with_rogue).is_err());
}

#[test]
fn catalog_validation_names_the_refused_role_and_attribute() {
    let err = validate_rls_catalog_rows("postgres", true, true, 0, 0, &[])
        .expect_err("superuser refused")
        .to_string();
    assert!(err.contains("postgres"), "names the role: {err}");
    assert!(
        err.contains("SUPERUSER") && err.contains("BYPASSRLS"),
        "{err}"
    );
    assert!(
        !err.contains("shako_app has"),
        "must not blame shako_app: {err}"
    );

    let err = validate_rls_catalog_rows("svc_x", true, false, 0, 0, &[])
        .expect_err("bypassrls refused")
        .to_string();
    assert!(err.contains("svc_x") && err.contains("BYPASSRLS"), "{err}");
    assert!(
        err.contains("has BYPASSRLS ("),
        "only BYPASSRLS named: {err}"
    );

    let err = validate_rls_catalog_rows("svc_y", false, false, 2, 0, &[])
        .expect_err("table owner refused")
        .to_string();
    assert!(err.contains("svc_y") && err.contains("owns 2"), "{err}");
}

/// LOGIN role of the request path in these tests: member of `shako_app`, no RLS bypass.
const APP_LOGIN: &str = "shako_app_login";

fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Creates (idempotently) the request LOGIN role with a fresh random password; returns the password.
async fn ensure_app_login(admin: &PgPool) -> String {
    let pw = random_secret();
    sqlx::raw_sql(AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE {APP_LOGIN}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
         ALTER ROLE {APP_LOGIN} LOGIN INHERIT NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
         GRANT shako_app TO {APP_LOGIN};"
    )))
    .execute(admin)
    .await
    .expect("create request login role");
    pw
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
        .connect_with(opts.clone())
        .await
        .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");

    // Migrations run on the admin connection (the `shako_migrate` role in production).
    PgStore::run_migrations(&admin_pool)
        .await
        .expect("migrations apply cleanly on Postgres");
    let app_password = ensure_app_login(&admin_pool).await;
    let app_opts = opts.clone().username(APP_LOGIN).password(&app_password);

    // 0. Red: the store on the admin (superuser) connection is refused, and the error names it.
    let admin_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&admin_pool)
        .await
        .unwrap();
    let refused = PgStore::connect(&PgStoreConfig {
        url_without_password: db_url.clone(),
        password: None,
        max_connections: 3,
    })
    .await;
    let Err(err) = refused else {
        panic!("a store connected as superuser {admin_role} must be refused at boot");
    };
    let msg = err.to_string();
    assert!(
        msg.contains(&admin_role) && msg.contains("SUPERUSER"),
        "refusal must name role {admin_role} and SUPERUSER, got: {msg}"
    );

    // The request-path store, as production runs it (boot check included).
    let store = PgStore::connect_with(app_opts.clone(), 4)
        .await
        .expect("PgStore connects as the NOSUPERUSER NOBYPASSRLS request role");
    let (who, sup, bypass): (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        (who.as_str(), sup, bypass),
        (APP_LOGIN, false, false),
        "request path must run as the non-bypassing login role"
    );

    // 1. Queries as the request role with `shako.org` unset return zero rows
    let mut tx = store.pool().begin().await.unwrap();
    let count_unset: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shako.repo")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        count_unset, 0,
        "queries as shako_app with shako.org unset must return zero rows"
    );

    // 2. Prove leakproof index plan for `(instance_id, owner, name)` as the request role
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
    println!("request-role plan:\n{plan_text}");
    assert!(
        plan_text.contains("idx_shako_repo_instance_owner_name"),
        "expected Index Scan on idx_shako_repo_instance_owner_name, got:\n{plan_text}"
    );
    tx.commit().await.unwrap();

    // 3. Verify boot check (as the request role) catches a table without forced RLS
    sqlx::query("CREATE TABLE IF NOT EXISTS shako.test_unforced (instance_id UUID, org_id UUID)")
        .execute(&admin_pool)
        .await
        .unwrap();
    let boot_res = store.verify_rls_enforced().await;
    sqlx::query("DROP TABLE IF EXISTS shako.test_unforced")
        .execute(&admin_pool)
        .await
        .unwrap();
    let Err(boot_err) = boot_res else {
        panic!("boot check must refuse when a table lacks forced RLS");
    };
    assert!(
        boot_err.to_string().contains("test_unforced"),
        "boot check must name the unforced table, got: {boot_err}"
    );

    // The superuser store bypasses RLS and is refused by the same boot check.
    let admin_store = PgStore::from_pool_unchecked(admin_pool.clone());
    let admin_err = admin_store
        .verify_rls_enforced()
        .await
        .expect_err("boot check refuses the superuser connection");
    assert!(admin_err.to_string().contains(&admin_role), "{admin_err}");

    run_isolation_suite(&store).await;
}
