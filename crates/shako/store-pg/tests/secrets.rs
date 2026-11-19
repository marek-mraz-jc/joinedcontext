#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! Hook and mirror secrets survive a restart (S6-09, T-7280): a `PgSecretStore` on a new pool
//! reads what an earlier one wrote, a second Organization reads none of it, the table holds no
//! plaintext, and another installation's key opens nothing.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection; the test creates a fresh
//! database, migrates it as the admin and connects the store as a NOSUPERUSER NOBYPASSRLS login
//! role in `shako_app`. Unset: prints `SKIP` and returns.

use kaisatsu_crypto::Kek;
use shako_core::{InstanceId, OrgId, SecretStore};
use shako_store_pg::{PgSecretStore, PgStore};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

const APP_LOGIN: &str = "shako_secrets_login";
const HOOK_REF: &str = "repo:0190f5c4-5c1e-7000-8000-000000000001:webhook-1";
const HOOK_SECRET: &str = "wh-signing-secret-7f3a9c";

#[tokio::test]
async fn a_hook_secret_survives_a_restart_sealed_and_per_organization() {
    let Ok(admin_url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
        println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
        return;
    };
    let admin_opts = PgConnectOptions::from_str(&admin_url).expect("valid SHAKO_TEST_DATABASE_URL");
    let server = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(admin_opts.clone())
        .await
        .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");
    let db = format!("shako_secrets_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {db}")))
        .execute(&server)
        .await
        .expect("create test database");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(admin_opts.clone().database(&db))
        .await
        .expect("connect test database");
    PgStore::run_migrations(&admin).await.expect("migrate");
    let pw = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    sqlx::raw_sql(AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE {APP_LOGIN}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
         ALTER ROLE {APP_LOGIN} LOGIN INHERIT NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
         GRANT shako_app TO {APP_LOGIN};"
    )))
    .execute(&admin)
    .await
    .expect("create request login role");
    let app_opts = admin_opts.database(&db).username(APP_LOGIN).password(&pw);

    let instance = InstanceId::from_u128(1);
    let org = OrgId::from_u128(1);
    let other_org = OrgId::from_u128(2);
    let kek = Kek::generate_for_tests();

    let first = PgStore::connect_with(app_opts.clone(), 2)
        .await
        .expect("first start");
    let secrets = PgSecretStore::new(&first, instance, &kek).expect("the store");
    secrets
        .put_secret(org, HOOK_REF, HOOK_SECRET)
        .await
        .expect("the secret is written");
    first.pool().close().await;
    drop((secrets, first));

    // The process is rebuilt: a new pool, a new store, the same installation key.
    let second = PgStore::connect_with(app_opts, 2)
        .await
        .expect("second start");
    let secrets = PgSecretStore::new(&second, instance, &kek).expect("the store");
    assert_eq!(
        secrets.get_secret(org, HOOK_REF).await.expect("read"),
        Some(HOOK_SECRET.to_owned()),
        "the hook's secret survives the restart"
    );

    // Another Organization reads nothing under the same reference, and its forced RLS context
    // sees no row at all.
    assert_eq!(
        secrets.get_secret(other_org, HOOK_REF).await.expect("read"),
        None
    );
    let mut tx = second
        .begin_tenant_tx(instance, other_org)
        .await
        .expect("tx");
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM shako.secret")
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    assert_eq!(visible, 0, "another Organization sees no sealed row");
    drop(tx);

    // The table holds no plaintext (read as the admin, past RLS).
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT sealed FROM shako.secret")
        .fetch_all(&admin)
        .await
        .expect("read the table");
    assert_eq!(stored.len(), 1);
    assert!(
        !stored[0]
            .windows(HOOK_SECRET.len())
            .any(|w| w == HOOK_SECRET.as_bytes()),
        "the secret is stored in the clear"
    );

    // Another installation's key, or the row under another reference, opens nothing.
    let foreign = PgSecretStore::new(&second, instance, &Kek::generate_for_tests()).expect("store");
    assert!(foreign.get_secret(org, HOOK_REF).await.is_err());
    sqlx::query("UPDATE shako.secret SET secret_ref = 'repo:other:webhook-1'")
        .execute(&admin)
        .await
        .expect("move the row");
    assert!(
        secrets
            .get_secret(org, "repo:other:webhook-1")
            .await
            .is_err(),
        "a sealed value opens only under the reference it was written for"
    );

    second.pool().close().await;
    admin.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {db} WITH (FORCE)")))
        .execute(&server)
        .await
        .expect("drop test database");
}
