#![allow(dead_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! Shared fixtures for the store tests: a migrated Postgres database reached through a request
//! role without SUPERUSER or BYPASSRLS (the role production runs as, C1), and record builders.

use dataroom_store_pg::PgStore;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::OnceCell;

/// The request role the tests connect as (`SET ROLE` on every pooled connection).
pub const APP_ROLE: &str = "dataroom_app_test";

static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// The test database URL, or `None` (printing `SKIP`) when it is unset. With
/// `DATAROOM_REQUIRE_PG=1` a missing URL fails the test instead of skipping it.
pub fn database_url(test: &str) -> Option<String> {
    match std::env::var("DATAROOM_TEST_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(url) => Some(url),
        None => {
            assert!(
                std::env::var("DATAROOM_REQUIRE_PG").ok().as_deref() != Some("1"),
                "{test}: DATAROOM_REQUIRE_PG=1 but DATAROOM_TEST_DATABASE_URL is unset"
            );
            println!("SKIP {test}: DATAROOM_TEST_DATABASE_URL is unset");
            None
        }
    }
}

/// Pools for one test: `admin` is the migration owner (superuser), `app` the request role.
pub struct Pg {
    pub url: String,
    pub admin: PgPool,
    pub app: PgPool,
}

impl Pg {
    /// A store on the request role, checked by the boot check like `serve` does.
    pub async fn app_store(&self) -> PgStore {
        PgStore::connect_and_verify(self.app.clone())
            .await
            .expect("the request role passes the boot check")
    }
}

/// Migrates once per test binary, creates the request role, and opens both pools.
pub async fn pg(test: &str) -> Option<Pg> {
    let url = database_url(test)?;
    MIGRATED
        .get_or_init(|| async {
            let admin = PgPool::connect(&url).await.expect("connect admin");
            sqlx::query("SELECT pg_advisory_lock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup lock");
            PgStore::from_pool_unchecked(admin.clone())
                .migrate()
                .await
                .expect("migrate");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$
                 BEGIN
                     IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{APP_ROLE}') THEN
                         CREATE ROLE {APP_ROLE} NOLOGIN NOSUPERUSER NOBYPASSRLS;
                     END IF;
                 END $$;
                 GRANT USAGE ON SCHEMA dataroom TO {APP_ROLE};
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {APP_ROLE};"
            )))
            .execute(&admin)
            .await
            .expect("request role");
            sqlx::query("SELECT pg_advisory_unlock(hashtext('dataroom-test-setup'))")
                .execute(&admin)
                .await
                .expect("setup unlock");
            admin.close().await;
        })
        .await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect admin");
    let app = app_pool(&url, 2).await;
    Some(Pg { url, admin, app })
}

/// A pool whose every connection acts as [`APP_ROLE`].
pub async fn app_pool(url: &str, max: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(format!("SET ROLE {APP_ROLE}")))
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(url)
        .await
        .expect("connect as the request role")
}

mod fixtures;
pub use fixtures::*;

pub mod contract;
