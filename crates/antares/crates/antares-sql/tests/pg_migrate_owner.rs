// SPDX-License-Identifier: EUPL-1.2
//! The migrations run as the database owner the broker connects as on dev:
//! no superuser, no BYPASSRLS, so FORCE RLS applies to every backfill
//! (T-6610). Every other pg test migrates as the superuser of
//! ANTARES_TEST_DATABASE_URL, where RLS never bites: 0032's backfill passed
//! them all and stopped dev's broker at boot ("new row violates row-level
//! security policy for table entity_values"). Its own database and role, so
//! no other test's rows or migrations meet it.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use sqlx::ConnectOptions;

const OWNER: &str = "antares_mig_owner";

#[tokio::test(flavor = "multi_thread")]
async fn the_migrations_backfill_scoped_entities_as_a_non_superuser_owner() {
    let Ok(shared) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let admin = pg::connect(&shared, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_owner_{run}");
    // Role DDL is cluster-global: the advisory lock of the pen-test's
    // app_pool queues every process's role setup.
    let mut tx = admin.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock(4334)")
        .execute(&mut *tx)
        .await
        .expect("advisory lock");
    sqlx::query(
        "DO $$ BEGIN CREATE ROLE antares_mig_owner LOGIN PASSWORD 'owner' NOSUPERUSER NOBYPASSRLS;
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$",
    )
    .execute(&mut *tx)
    .await
    .expect("owner role");
    tx.commit().await.expect("commit");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER {OWNER}"
    )))
    .execute(&admin)
    .await
    .expect("own database");

    let base: sqlx::postgres::PgConnectOptions = shared.parse().expect("a database URL");
    let as_admin = base.clone().database(&name);
    let as_owner = base.username(OWNER).password("owner").database(&name);
    // The extensions are no owner's to create (postgis is untrusted); dev's
    // operator creates them the same way, before the broker connects.
    let ext = sqlx::PgPool::connect_with(as_admin)
        .await
        .expect("admin on own db");
    for stmt in [
        "CREATE EXTENSION IF NOT EXISTS postgis",
        "CREATE EXTENSION IF NOT EXISTS btree_gin",
    ] {
        sqlx::query(stmt).execute(&ext).await.expect(stmt);
    }
    ext.close().await;

    // The schema dev held before 0032, then scoped Entities of two Tenants.
    let owner = sqlx::PgPool::connect_with(as_owner.clone())
        .await
        .expect("owner on own db");
    let bypass: bool =
        sqlx::query_scalar("SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = $1")
            .bind(OWNER)
            .fetch_one(&owner)
            .await
            .expect("role flags");
    assert!(
        !bypass,
        "the owner must wear RLS or this test proves nothing"
    );
    let before = sqlx::migrate::Migrator::with_migrations(
        pg::MIGRATOR
            .iter()
            .filter(|m| m.version < 32)
            .cloned()
            .collect(),
    );
    before.run(&owner).await.expect("migrations up to 0031");
    let tenants = [
        (
            TenantId::new("t6610-a").expect("tenant"),
            r#"["/A//B/", "C"]"#,
        ),
        (TenantId::new("t6610-b").expect("tenant"), r#"["/D"]"#),
    ];
    for (tenant, scopes) in &tenants {
        let mut tx = owner.begin().await.expect("tx");
        pg::set_tenant(&mut tx, tenant).await.expect("set_tenant");
        sqlx::query(
            "INSERT INTO entities (tenant_id, id, entity, types, scopes, created_at, modified_at)
             VALUES ($1, 'urn:ngsi-ld:T:1', jsonb_build_object('scope', $2::jsonb), '{T}',
                     ARRAY(SELECT jsonb_array_elements_text($2::jsonb)), now(), now())",
        )
        .bind(tenant.as_str())
        .bind(scopes)
        .execute(&mut *tx)
        .await
        .expect("seed a scoped Entity");
        tx.commit().await.expect("commit seed");
    }
    owner.close().await;

    // The boot dev's broker makes: connect as the owner and migrate to the end.
    let url = as_owner.to_url_lossy().to_string();
    let pool = pg::connect(&url, 2)
        .await
        .expect("0032 migrates as a non-superuser owner with scoped Entities");

    for (tenant, want) in [
        (&tenants[0].0, vec!["/A/B", "/C"]),
        (&tenants[1].0, vec!["/D"]),
    ] {
        let mut tx = pool.begin().await.expect("tx");
        pg::set_tenant(&mut tx, tenant).await.expect("set_tenant");
        let got: Vec<String> =
            sqlx::query_scalar("SELECT txt FROM entity_values WHERE attr = 'scope' ORDER BY txt")
                .fetch_all(&mut *tx)
                .await
                .expect("scope rows");
        assert_eq!(got, want, "{} keeps its own scope rows", tenant.as_str());
    }
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE {name} WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop own database");
}
