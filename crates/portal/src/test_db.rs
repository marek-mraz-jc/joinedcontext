//! Databases for the lib tests that break one: each a database of its own, so a fault planted in
//! one never reaches a parallel test on the shared database (T-7858).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::{AssertSqlSafe, PgPool};

/// A pool whose every acquire fails at once: the database is down.
pub(crate) fn dead_pool() -> PgPool {
    jc_pool::options::<sqlx::Postgres>()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

/// A mirror holding `project` of an Organization, so a key lookup for it reaches the database
/// (and fails on a [`dead_pool`]).
pub(crate) fn placed_mirror(project: &str) -> std::sync::Arc<crate::store::Mirror> {
    let mirror = std::sync::Arc::new(crate::store::Mirror::new());
    mirror.upsert(
        serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Project",
            "metadata": { "name": project, "namespace": crate::permissions::ORG_NAMESPACE },
            "spec": { "organizationRef": "somewhere" },
        }))
        .expect("a project"),
    );
    mirror
}

/// A migrated database of its own; [`Scratch::remove`] drops it.
pub(crate) struct Scratch {
    admin: PgPool,
    name: String,
    pub(crate) pool: PgPool,
}

/// A fresh migrated database beside the one `JC_PORTAL_TEST_DATABASE_URL` names; ci.yml and
/// coverage.yml always set it, so a run without it fails instead of passing untested.
pub(crate) async fn scratch() -> Scratch {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("JC_PORTAL_TEST_DATABASE_URL names the test database");
    let admin = jc_pool::options::<sqlx::Postgres>()
        .connect(&url)
        .await
        .expect("connect");
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
    Scratch { admin, name, pool }
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

    /// Makes every commit of a transaction that wrote a row of `table` fail: a deferred
    /// constraint trigger raises at COMMIT, after each statement succeeded.
    pub(crate) async fn refuse_commit(&self, table: &str) {
        let function = format!("jc_refuse_commit_{table}");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'commit refused by the test'; END $$"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger function");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE CONSTRAINT TRIGGER {function} AFTER INSERT OR UPDATE OR DELETE ON {table} \
             DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {function}()"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger");
    }

    /// Makes every UPDATE that sets `column` of `table` fail from now on, while the other writes
    /// of that table go through: one store call of several on one row fails.
    pub(crate) async fn refuse_column(&self, table: &str, column: &str) {
        let function = format!("jc_refuse_{table}_{column}");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'refused by the test'; END $$"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger function");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE TRIGGER {function} BEFORE UPDATE OF {column} ON {table} \
             FOR EACH STATEMENT EXECUTE FUNCTION {function}()"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger");
    }

    /// A mirror placing `project` in an active Organization of this database, so its rows have a
    /// tenancy key here.
    pub(crate) async fn placed(&self, project: &str) -> std::sync::Arc<crate::store::Mirror> {
        sqlx::query(
            "INSERT INTO organizations (name, host, state, created_by) \
             VALUES ('somewhere', 'somewhere.example', 'active', 'test') ON CONFLICT DO NOTHING",
        )
        .execute(&self.pool)
        .await
        .expect("an Organization of the registry");
        placed_mirror(project)
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

impl Scratch {
    /// Makes every `op` (INSERT, UPDATE or DELETE) on `table` silently affect no row: a row
    /// trigger returning NULL, so the statement succeeds and reports zero rows.
    pub(crate) async fn skip_rows(&self, table: &str, op: &str) {
        let function = format!("jc_skip_{table}");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RETURN NULL; END $$"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger function");
        sqlx::query(AssertSqlSafe(format!(
            "CREATE TRIGGER {function} BEFORE {op} ON {table} \
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        )))
        .execute(&self.pool)
        .await
        .expect("trigger");
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_refused_table_fails_its_writes_and_keeps_its_reads() {
        let scratch = super::scratch().await;
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
