//! ekiben's stream definitions in Postgres (`[ekiben.database]`). Standalone ekiben keeps them
//! in a config directory; inside `jc` the API is the only writer, so the definitions go to
//! schema `ekiben`, one row per (organization, stream id), the config as posted (before `${VAR}`
//! interpolation, so no filled-in secret is stored).
//!
//! The same rules as the other modules' stores: forced RLS on every table of the schema, a
//! request role that is neither SUPERUSER nor BYPASSRLS and owns no table there (refused at boot
//! otherwise), every statement in a transaction scoped by `set_config('ekiben.org', ..)`, and
//! the migration run as the owning role under sqlx's advisory lock.

use ekiben_core::OrgId;
use ekiben_server::{StreamManager, StreamPersistence};
use futures::future::BoxFuture;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

const MIGRATION_0001_SQL: &str = "
CREATE SCHEMA IF NOT EXISTS ekiben;
-- The organizations holding streams: names only, readable by the request role so a start can
-- find every organization to load.
CREATE TABLE IF NOT EXISTS ekiben.org (
    org TEXT PRIMARY KEY COLLATE \"C\"
);
CREATE TABLE IF NOT EXISTS ekiben.stream (
    org TEXT NOT NULL COLLATE \"C\",
    id TEXT NOT NULL COLLATE \"C\",
    posted JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org, id)
);
ALTER TABLE ekiben.org ENABLE ROW LEVEL SECURITY;
ALTER TABLE ekiben.org FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS org_list ON ekiben.org;
CREATE POLICY org_list ON ekiben.org FOR SELECT USING (true);
DROP POLICY IF EXISTS org_add ON ekiben.org;
CREATE POLICY org_add ON ekiben.org FOR INSERT
    WITH CHECK (org = current_setting('ekiben.org', true));
ALTER TABLE ekiben.stream ENABLE ROW LEVEL SECURITY;
ALTER TABLE ekiben.stream FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON ekiben.stream;
CREATE POLICY tenant_isolation ON ekiben.stream
    USING (org = current_setting('ekiben.org', true))
    WITH CHECK (org = current_setting('ekiben.org', true));
";

/// Tables the boot check requires (every table of the schema must have forced RLS too).
const REQUIRED_TABLES: [&str; 2] = ["org", "stream"];

/// Runs the migration as the owning role, then grants `app_role` what the request path needs.
pub async fn migrate(pool: &PgPool, app_role: &str) -> Result<(), String> {
    use sqlx::SqlSafeStr;
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    let mut migrator = Migrator::with_migrations(vec![Migration::new(
        1,
        "streams".into(),
        MigrationType::Simple,
        MIGRATION_0001_SQL.into_sql_str(),
        false,
    )]);
    migrator.create_schema("ekiben_migrations");
    migrator.dangerous_set_table_name("ekiben_migrations._sqlx_migrations");
    migrator
        .run(pool)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;
    let role = format!("\"{}\"", app_role.replace('"', "\"\""));
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "GRANT USAGE ON SCHEMA ekiben TO {role};
         GRANT SELECT, INSERT ON ekiben.org TO {role};
         GRANT SELECT, INSERT, UPDATE, DELETE ON ekiben.stream TO {role};"
    )))
    .execute(pool)
    .await
    .map_err(|e| format!("grant to {app_role}: {e}"))?;
    Ok(())
}

/// The boot check: refuses a request role that bypasses RLS or owns a table of the schema, a
/// missing table, and any table of `ekiben` without forced RLS and a policy.
pub async fn verify_boot_rls(pool: &PgPool) -> Result<(), String> {
    let (role, sup, bypass): (String, bool, bool) = sqlx::query_as(
        "SELECT rolname::text, rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("boot check: {e}"))?;
    if sup || bypass {
        return Err(format!(
            "boot check refused: request role {role} has SUPERUSER or BYPASSRLS"
        ));
    }
    let tables: Vec<(String, bool, bool, i64, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relrowsecurity, c.relforcerowsecurity,
                (SELECT count(*) FROM pg_policy p WHERE p.polrelid = c.oid),
                pg_get_userbyid(c.relowner) = current_user
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'ekiben' AND c.relkind IN ('r', 'p')",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("boot check: {e}"))?;
    for (table, rls, forced, policies, owned) in &tables {
        if !(*rls && *forced && *policies > 0) {
            return Err(format!(
                "boot check refused: table ekiben.{table} lacks forced row-level security"
            ));
        }
        if *owned {
            return Err(format!(
                "boot check refused: request role {role} owns ekiben.{table}"
            ));
        }
    }
    for required in REQUIRED_TABLES {
        if !tables.iter().any(|t| t.0 == required) {
            return Err(format!(
                "boot check refused: table ekiben.{required} is missing (not migrated)"
            ));
        }
    }
    Ok(())
}

/// The stream definitions of every organization, as one start loads them.
pub struct PgStreams {
    pool: PgPool,
}

impl PgStreams {
    /// Wraps a request-role pool after its boot check.
    pub async fn open(pool: PgPool) -> Result<Self, String> {
        verify_boot_rls(&pool).await?;
        Ok(Self { pool })
    }

    async fn scoped(&self, org: &OrgId) -> Result<Transaction<'_, Postgres>, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SELECT set_config('ekiben.org', $1, true)")
            .bind(&org.0)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(tx)
    }

    /// The saved definitions of `org`, by id.
    pub async fn list(&self, org: &OrgId) -> Result<Vec<(String, Value)>, String> {
        let mut tx = self.scoped(org).await?;
        let rows = sqlx::query_as("SELECT id, posted FROM ekiben.stream ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Starts every saved stream in `manager`. A definition that no longer starts (a variable
    /// gone, a component refused) is reported and kept, so a later fix can still read it.
    pub async fn restore(&self, manager: &StreamManager) -> Result<usize, String> {
        let orgs: Vec<String> = sqlx::query_scalar("SELECT org FROM ekiben.org ORDER BY org")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        let mut started = 0;
        for org in orgs.into_iter().map(OrgId) {
            for (id, posted) in self.list(&org).await? {
                match manager.restore(org.clone(), id.clone(), posted).await {
                    Ok(()) => started += 1,
                    Err(e) => tracing::error!(
                        org = %org.0, stream = %id, error = %e,
                        "ekiben: a saved stream did not start"
                    ),
                }
            }
        }
        Ok(started)
    }
}

impl StreamPersistence for PgStreams {
    fn saved<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a str,
        posted: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = self.scoped(org).await?;
            sqlx::query("INSERT INTO ekiben.org (org) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(&org.0)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            sqlx::query(
                "INSERT INTO ekiben.stream (org, id, posted) VALUES ($1, $2, $3)
                 ON CONFLICT (org, id) DO UPDATE SET posted = EXCLUDED.posted, updated_at = now()",
            )
            .bind(&org.0)
            .bind(id)
            .bind(posted)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }

    fn deleted<'a>(&'a self, org: &'a OrgId, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = self.scoped(org).await?;
            sqlx::query("DELETE FROM ekiben.stream WHERE org = $1 AND id = $2")
                .bind(&org.0)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())
        })
    }
}
