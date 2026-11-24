#![forbid(unsafe_code)]
//! What the store answers when the database refuses a step (T-7624): each test gets a database
//! of its own, revokes there the one privilege a step needs (catalogue grants are per
//! database) and drops the database afterwards, so no other test sees the change.

mod common;

use common::*;
use dataroom_core::DataroomError;
use dataroom_store_pg::{set_tenant_config, verify_rls_on_pool, PgStore, REQUIRED_RLS_TABLES};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, PgPool};
use std::str::FromStr;

/// A fresh, unmigrated database and a pool acting as a role without SUPERUSER in it.
struct Scratch {
    server: PgPool,
    admin: PgPool,
    name: String,
}

impl Scratch {
    async fn create(test: &str) -> Option<Self> {
        let url = database_url(test)?;
        let options = PgConnectOptions::from_str(&url).expect("valid URL");
        let server = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .expect("connect server");
        let name = format!("dataroom_faults_{}", uuid::Uuid::now_v7().simple());
        for sql in [
            format!("CREATE DATABASE {name}"),
            format!("CREATE ROLE {name}_x NOLOGIN NOSUPERUSER NOBYPASSRLS"),
        ] {
            sqlx::raw_sql(AssertSqlSafe(sql))
                .execute(&server)
                .await
                .expect("scratch database");
        }
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.database(&name))
            .await
            .expect("connect scratch database");
        Some(Self {
            server,
            admin,
            name,
        })
    }

    async fn run(&self, sql: &str) {
        sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
            .execute(&self.admin)
            .await
            .expect("scratch statement");
    }

    /// A pool whose every connection acts as the scratch role.
    async fn limited(&self) -> PgPool {
        let role = format!("{}_x", self.name);
        PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let set = format!("SET ROLE {role}");
                Box::pin(async move {
                    sqlx::query(AssertSqlSafe(set))
                        .execute(&mut *conn)
                        .await
                        .map(|_| ())
                })
            })
            .connect_with(self.admin.connect_options().as_ref().clone())
            .await
            .expect("connect as the scratch role")
    }

    async fn drop_it(self) {
        self.admin.close().await;
        for sql in [
            format!("DROP DATABASE {} WITH (FORCE)", self.name),
            format!("DROP ROLE {}_x", self.name),
        ] {
            sqlx::raw_sql(AssertSqlSafe(sql))
                .execute(&self.server)
                .await
                .expect("drop scratch database");
        }
    }
}

fn storage(result: dataroom_core::Result<impl Sized>) -> String {
    match result {
        Err(DataroomError::Storage(m)) => m,
        Err(other) => panic!("expected a storage refusal, got {other:?}"),
        Ok(_) => panic!("expected a storage refusal, got an answer"),
    }
}

#[tokio::test]
async fn a_refused_step_is_a_storage_error_naming_it() {
    let Some(db) = Scratch::create("a_refused_step_is_a_storage_error_naming_it").await else {
        return;
    };
    let pool = db.limited().await;

    // Nothing migrated: the first required table is missing.
    let msg = storage(verify_rls_on_pool(&pool).await);
    assert!(
        msg.contains("dataroom.cat_catalogue does not exist"),
        "{msg}"
    );

    db.run("REVOKE SELECT ON pg_catalog.pg_class FROM PUBLIC")
        .await;
    let msg = storage(verify_rls_on_pool(&pool).await);
    assert!(
        msg.starts_with("failed to check RLS in schema dataroom"),
        "{msg}"
    );

    db.run("REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM PUBLIC")
        .await;
    let mut conn = pool.acquire().await.expect("connection");
    let (org, _) = tenant();
    let msg = storage(set_tenant_config(&mut conn, org, None).await);
    assert!(msg.starts_with("set jc.org"), "{msg}");
    drop(conn);

    db.run("REVOKE EXECUTE ON FUNCTION pg_catalog.pg_advisory_lock(bigint) FROM PUBLIC")
        .await;
    let msg = storage(PgStore::from_pool_unchecked(pool.clone()).migrate().await);
    assert!(msg.starts_with("advisory lock"), "{msg}");

    pool.close().await;
    db.drop_it().await;
}

#[tokio::test]
async fn a_schema_missing_one_required_table_is_refused_by_its_name() {
    let Some(db) =
        Scratch::create("a_schema_missing_one_required_table_is_refused_by_its_name").await
    else {
        return;
    };
    PgStore::from_pool_unchecked(db.admin.clone())
        .migrate()
        .await
        .expect("migrate");
    let last = REQUIRED_RLS_TABLES[REQUIRED_RLS_TABLES.len() - 1];
    db.run(&format!("DROP TABLE dataroom.{last} CASCADE")).await;
    let pool = db.limited().await;
    let msg = storage(verify_rls_on_pool(&pool).await);
    assert!(
        msg.contains(&format!("dataroom.{last} does not exist")),
        "{msg}"
    );
    pool.close().await;
    db.drop_it().await;
}

#[tokio::test]
async fn a_move_on_a_closed_pool_names_the_catalogue() {
    let Some(db) = Scratch::create("a_move_on_a_closed_pool_names_the_catalogue").await else {
        return;
    };
    let store = PgStore::from_pool_unchecked(db.admin.clone());
    db.admin.close().await;
    let (from, cat) = tenant();
    let (to, _) = tenant();
    let msg = storage(store.move_catalogue(from, to, cat).await);
    assert!(msg.starts_with(&format!("move catalogue {cat}")), "{msg}");
    db.drop_it().await;
}

#[tokio::test]
async fn a_record_the_reindex_cannot_read_fails_the_migration_and_keeps_rls_forced() {
    let Some(db) = Scratch::create(
        "a_record_the_reindex_cannot_read_fails_the_migration_and_keeps_rls_forced",
    )
    .await
    else {
        return;
    };
    let owner = PgStore::from_pool_unchecked(db.admin.clone());
    owner.migrate().await.expect("migrate");
    // A row an older build left unindexed whose record no longer reads as a dataset.
    db.run(
        "INSERT INTO dataroom.cat_dataset (org_id, catalogue_id, id, name, title, publisher_id,
                                           doc, search_v)
         VALUES (gen_random_uuid(), gen_random_uuid(), 'bad', 'bad', 'bad', 'city',
                 '\"not a dataset\"'::jsonb, 0)",
    )
    .await;
    let msg = storage(owner.migrate().await);
    assert!(
        msg.starts_with("migration failed")
            && msg.contains("search reindex: ")
            && msg.contains("Dataset"),
        "{msg}"
    );
    let (forced,): (bool,) = sqlx::query_as(
        "SELECT relforcerowsecurity FROM pg_class WHERE oid = 'dataroom.cat_dataset'::regclass",
    )
    .fetch_one(&db.admin)
    .await
    .expect("rls flag");
    assert!(forced, "the failed reindex rolls its NO FORCE back");
    db.drop_it().await;
}
