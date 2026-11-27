//! Databases for the lib tests that break one: each a database of its own, so a fault planted in
//! one never reaches a parallel test on the shared database (T-7858).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{AssertSqlSafe, PgPool};

/// A pool whose every acquire fails at once: the database is down.
pub(crate) fn dead_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

/// A migrated database of its own; [`Scratch::remove`] drops it.
pub(crate) struct Scratch {
    admin: PgPool,
    name: String,
    pub(crate) pool: PgPool,
}

/// A fresh migrated database, or none when the run names no database
/// (`JC_PORTAL_TEST_DATABASE_URL` unset).
pub(crate) async fn scratch() -> Option<Scratch> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())?;
    let admin = PgPool::connect(&url).await.expect("connect");
    let name = format!(
        "jc_fault_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {name}")))
        .execute(&admin)
        .await
        .expect("drop");
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("create");
    let mut own = url::Url::parse(&url).expect("url");
    own.set_path(&name);
    let pool = crate::db::connect(own.as_str()).await.expect("migrate");
    Some(Scratch { admin, name, pool })
}

impl Scratch {
    /// Makes every `op` (INSERT, UPDATE, DELETE or a list joined by OR) on `table` fail with a
    /// database error from now on.
    pub(crate) async fn refuse(&self, table: &str, op: &str) {
        let function = format!("jc_refuse_{table}");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'refused by the test'; END $$"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger function");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE TRIGGER {function} BEFORE {op} ON {table} \
             FOR EACH STATEMENT EXECUTE FUNCTION {function}()"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger");
    }

    /// Makes every statement naming `table`, reads included, fail from now on.
    pub(crate) async fn drop_table(&self, table: &str) {
        sqlx::query(AssertSqlSafe(format!("DROP TABLE {table} CASCADE")))
            .execute(&self.pool)
            .await
            .expect("drop table");
    }

    pub(crate) async fn remove(self) {
        self.pool.close().await;
        sqlx::query(AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop");
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_refused_table_fails_its_writes_and_keeps_its_reads() {
        let Some(scratch) = super::scratch().await else {
            return;
        };
        scratch.refuse("organizations", "INSERT").await;
        let write = sqlx::query(
            "INSERT INTO organizations (name, host, state, created_by) \
             VALUES ('o', 'o.example', 'active', 'test')",
        )
        .execute(&scratch.pool)
        .await;
        assert!(write.is_err());
        let read: Result<i64, _> = sqlx::query_scalar("SELECT count(*) FROM organizations")
            .fetch_one(&scratch.pool)
            .await;
        assert!(read.is_ok());
        scratch.drop_table("organizations").await;
        let gone: Result<i64, _> = sqlx::query_scalar("SELECT count(*) FROM organizations")
            .fetch_one(&scratch.pool)
            .await;
        assert!(gone.is_err());
        scratch.remove().await;
    }
}
