#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! Throwaway PostgreSQL databases for the `kaisatsu-store-pg` tests.
//!
//! `KAISATSU_TEST_DATABASE_URL` names a superuser connection to a server the tests may use. Each
//! test gets its own database `kaisatsu_t_<uuid>` owned by `kaisatsu_migrate`, migrated by that
//! role, and a store connected as `kaisatsu_app` (no SUPERUSER, no BYPASSRLS). The role passwords
//! are random per test process and never written anywhere.
#![allow(dead_code)]

use kaisatsu_crypto::Kek;
use kaisatsu_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::str::FromStr;
use tokio::sync::OnceCell;

static ROLES: OnceCell<(String, String)> = OnceCell::const_new();

pub struct TestDb {
    pub name: String,
    admin: PgPool,
    base: PgConnectOptions,
    app_password: String,
    migrate_password: String,
    pub migrate_pool: PgPool,
}

fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

async fn ensure_roles(admin: &PgPool) -> (String, String) {
    ROLES
        .get_or_init(|| async {
            let app = random_secret();
            let mig = random_secret();
            for (role, pw) in [("kaisatsu_app", &app), ("kaisatsu_migrate", &mig)] {
                sqlx::raw_sql(AssertSqlSafe(format!(
                    "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
                     ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
                )))
                .execute(admin)
                .await
                .expect("create test roles");
            }
            (app, mig)
        })
        .await
        .clone()
}

/// Creates a migrated throwaway database, or prints `SKIP` and returns `None` without a server.
pub async fn test_db(test: &str) -> Option<TestDb> {
    let Ok(url) = std::env::var("KAISATSU_TEST_DATABASE_URL") else {
        println!("SKIP {test}: KAISATSU_TEST_DATABASE_URL is unset; live PostgreSQL test not run");
        return None;
    };
    let base = PgConnectOptions::from_str(&url).expect("KAISATSU_TEST_DATABASE_URL parses");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(base.clone())
        .await
        .expect("connect test server as superuser");
    let (app_password, migrate_password) = ensure_roles(&admin).await;
    let name = format!("kaisatsu_t_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER kaisatsu_migrate"
    )))
    .execute(&admin)
    .await
    .expect("create test database");
    let migrate_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            base.clone()
                .database(&name)
                .username("kaisatsu_migrate")
                .password(&migrate_password),
        )
        .await
        .expect("connect as kaisatsu_migrate");
    PgStore::run_migrations(&migrate_pool)
        .await
        .expect("migrations as kaisatsu_migrate");
    Some(TestDb {
        name,
        admin,
        base,
        app_password,
        migrate_password,
        migrate_pool,
    })
}

impl TestDb {
    /// Connection options of the request role on this database.
    pub fn app_options(&self) -> PgConnectOptions {
        self.base
            .clone()
            .database(&self.name)
            .username("kaisatsu_app")
            .password(&self.app_password)
    }

    pub fn app_password(&self) -> &str {
        &self.app_password
    }

    /// Connection options of the migration role on this database.
    pub fn migrate_options(&self) -> PgConnectOptions {
        self.base
            .clone()
            .database(&self.name)
            .username("kaisatsu_migrate")
            .password(&self.migrate_password)
    }

    /// Connection options of the superuser on this database.
    pub fn superuser_options(&self) -> PgConnectOptions {
        self.base.clone().database(&self.name)
    }

    /// Connection options with another user and password on this database.
    pub fn options_as(&self, user: &str, password: &str) -> PgConnectOptions {
        self.base
            .clone()
            .database(&self.name)
            .username(user)
            .password(password)
    }

    pub async fn superuser_pool(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(self.superuser_options())
            .await
            .expect("connect as superuser")
    }

    pub async fn app_pool(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(self.app_options())
            .await
            .expect("connect as kaisatsu_app")
    }

    /// A store as the request role (boot check included).
    pub async fn store(&self, kek: Kek) -> PgStore {
        PgStore::connect_with(self.app_options(), 4, kek)
            .await
            .expect("PgStore connects as kaisatsu_app")
    }

    /// Drops the database. A failing test leaves its `kaisatsu_t_*` database behind.
    pub async fn drop_db(self) {
        self.migrate_pool.close().await;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop test database");
        self.admin.close().await;
    }
}
