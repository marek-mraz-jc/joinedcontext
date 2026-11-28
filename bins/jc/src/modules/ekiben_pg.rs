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

/// The resources posted to `/resources/{kind}/{id}` (the Portal's `<project>__pipeline_egress`
/// rate limits), as posted (before `${VAR}` interpolation): a stream started at boot may name
/// one before the Portal posts it again.
const MIGRATION_0002_SQL: &str = "
CREATE TABLE IF NOT EXISTS ekiben.resource (
    org TEXT NOT NULL COLLATE \"C\",
    kind TEXT NOT NULL COLLATE \"C\",
    id TEXT NOT NULL COLLATE \"C\",
    posted TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org, kind, id)
);
ALTER TABLE ekiben.resource ENABLE ROW LEVEL SECURITY;
ALTER TABLE ekiben.resource FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON ekiben.resource;
CREATE POLICY tenant_isolation ON ekiben.resource
    USING (org = current_setting('ekiben.org', true))
    WITH CHECK (org = current_setting('ekiben.org', true));
";

/// Tables the boot check requires (every table of the schema must have forced RLS too).
const REQUIRED_TABLES: [&str; 3] = ["org", "stream", "resource"];

/// Runs the migration as the owning role, then grants `app_role` what the request path needs.
pub async fn migrate(pool: &PgPool, app_role: &str) -> Result<(), String> {
    use sqlx::SqlSafeStr;
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    let mut migrator = Migrator::with_migrations(vec![
        Migration::new(
            1,
            "streams".into(),
            MigrationType::Simple,
            MIGRATION_0001_SQL.into_sql_str(),
            false,
        ),
        Migration::new(
            2,
            "resources".into(),
            MigrationType::Simple,
            MIGRATION_0002_SQL.into_sql_str(),
            false,
        ),
    ]);
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
         GRANT SELECT, INSERT, UPDATE, DELETE ON ekiben.stream TO {role};
         GRANT SELECT, INSERT, UPDATE ON ekiben.resource TO {role};"
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
        let mut tx = self.pool.begin().await.map_err(text)?;
        sqlx::query("SELECT set_config('ekiben.org', $1, true)")
            .bind(&org.0)
            .execute(&mut *tx)
            .await
            .map_err(text)?;
        Ok(tx)
    }

    /// The saved definitions of `org`, by id.
    pub async fn list(&self, org: &OrgId) -> Result<Vec<(String, Value)>, String> {
        let mut tx = self.scoped(org).await?;
        let rows = sqlx::query_as("SELECT id, posted FROM ekiben.stream ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .map_err(text)?;
        tx.commit().await.map_err(text)?;
        Ok(rows)
    }

    /// The saved resources of `org`, as (kind, id, posted).
    async fn resources(&self, org: &OrgId) -> Result<Vec<(String, String, String)>, String> {
        let mut tx = self.scoped(org).await?;
        let rows = sqlx::query_as("SELECT kind, id, posted FROM ekiben.resource ORDER BY kind, id")
            .fetch_all(&mut *tx)
            .await
            .map_err(text)?;
        tx.commit().await.map_err(text)?;
        Ok(rows)
    }

    /// Puts back every saved resource of each organization, then starts every saved stream in
    /// `manager`, so a stream naming a resource finds it from its first run (an expiry sweep's
    /// `<project>__pipeline_egress` used to be missing until the Portal's first pass). A
    /// definition that no longer starts (a variable gone, a component refused) is reported and
    /// kept, so a later fix can still read it.
    pub async fn restore(&self, manager: &StreamManager) -> Result<usize, String> {
        let orgs: Vec<String> = sqlx::query_scalar("SELECT org FROM ekiben.org ORDER BY org")
            .fetch_all(&self.pool)
            .await
            .map_err(text)?;
        let mut started = 0;
        for org in orgs.into_iter().map(OrgId) {
            for (kind, id, posted) in self.resources(&org).await? {
                if let Err(e) = manager.restore_resource(&org, &kind, &id, &posted) {
                    tracing::error!(org = %org.0, kind = %kind, resource = %id, error = %e, "ekiben: a saved resource did not build");
                }
            }
            for (id, posted) in self.list(&org).await? {
                match manager.restore(org.clone(), id.clone(), posted).await {
                    Ok(()) => started += 1,
                    Err(e) => {
                        tracing::error!(org = %org.0, stream = %id, error = %e, "ekiben: a saved stream did not start")
                    }
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
                .map_err(text)?;
            sqlx::query(
                "INSERT INTO ekiben.stream (org, id, posted) VALUES ($1, $2, $3)
                 ON CONFLICT (org, id) DO UPDATE SET posted = EXCLUDED.posted, updated_at = now()",
            )
            .bind(&org.0)
            .bind(id)
            .bind(posted)
            .execute(&mut *tx)
            .await
            .map_err(text)?;
            tx.commit().await.map_err(text)
        })
    }

    fn resource_saved<'a>(
        &'a self,
        org: &'a OrgId,
        kind: &'a str,
        id: &'a str,
        posted: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut tx = self.scoped(org).await?;
            sqlx::query("INSERT INTO ekiben.org (org) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(&org.0)
                .execute(&mut *tx)
                .await
                .map_err(text)?;
            sqlx::query(
                "INSERT INTO ekiben.resource (org, kind, id, posted) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (org, kind, id) DO UPDATE SET posted = EXCLUDED.posted, updated_at = now()",
            )
            .bind(&org.0)
            .bind(kind)
            .bind(id)
            .bind(posted)
            .execute(&mut *tx)
            .await
            .map_err(text)?;
            tx.commit().await.map_err(text)
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
                .map_err(text)?;
            tx.commit().await.map_err(text)
        })
    }
}

/// A driver error as the stream manager's persistence reports it.
fn text(e: sqlx::Error) -> String {
    e.to_string()
}

/// The store on real Postgres (`JC_PG_TEST_DATABASE_URL`, a superuser that only prepares):
/// the owner migrates, the request role reads its own organization's rows alone, and every
/// refusal of the boot check and of a statement reaches the caller.
#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{EnvAccess, ResourceManager};
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;

    const GENERATE: &str = r#"{"input": {"generate": {"interval": "1h", "mapping": "root = {}"}}, "output": {"drop": {}}}"#;
    const UNSET: &str = r#"{"input": {"generate": {"interval": "1h", "mapping": "root = \"${EKIBEN_PG_UNSET_T7820}\""}}, "output": {"drop": {}}}"#;

    struct Db {
        admin: PgPool,
        base: PgConnectOptions,
        name: String,
    }

    impl Db {
        async fn login(&self, role: &str) -> PgPool {
            PgPoolOptions::new()
                .max_connections(2)
                .connect_with(
                    self.base
                        .clone()
                        .database(&self.name)
                        .username(role)
                        .password(&format!("pw-{role}-7820")),
                )
                .await
                .expect("the role logs in")
        }

        async fn as_admin(&self) -> PgPool {
            PgPoolOptions::new()
                .max_connections(2)
                .connect_with(self.base.clone().database(&self.name))
                .await
                .expect("the admin logs in")
        }
    }

    /// The roles are shared by the tests of this module: one `ALTER ROLE` at a time.
    static ROLES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn prepare(tag: &str) -> Db {
        let url = crate::config::portal_env("JC_PG_TEST_DATABASE_URL")
            .expect("JC_PG_TEST_DATABASE_URL is set by ci.yml root-test");
        let base = PgConnectOptions::from_str(&url).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let name = format!("jc_ekpg_{tag}_{}", std::process::id());
        let mut statements = vec![
            format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
            format!("CREATE DATABASE {name}"),
        ];
        for (role, create) in [
            ("jc_ekpg_owner", true),
            ("jc_ekpg_app", false),
            ("jc_ekpg_ro", false),
            ("jc_ekpg_none", false),
        ] {
            let create = if create { "CREATE, " } else { "" };
            statements.push(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD 'pw-{role}-7820';
                 GRANT {create}CONNECT ON DATABASE {name} TO {role};"
            ));
        }
        let _roles = ROLES.lock().await;
        for statement in statements {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&admin)
                .await
                .unwrap();
        }
        Db { admin, base, name }
    }

    /// A manager as jc builds it, its organizations' files under `root`.
    fn manager(root: &std::path::Path) -> StreamManager {
        StreamManager::new(
            ekiben_server::build_full_registry(),
            ResourceManager::new(),
            Some(root.to_path_buf()),
            EnvAccess::None,
        )
    }

    fn org(name: &str) -> OrgId {
        OrgId(name.into())
    }

    #[tokio::test]
    async fn saved_streams_and_resources_come_back_per_organization() {
        let db = prepare("rt").await;
        let owner = db.login("jc_ekpg_owner").await;
        migrate(&owner, "jc_ekpg_app").await.unwrap();
        // A second start finds the migration done and grants again.
        migrate(&owner, "jc_ekpg_app").await.unwrap();
        let app = db.login("jc_ekpg_app").await;
        let streams = PgStreams::open(app).await.expect("the boot check passes");
        let (city, town) = (org("city"), org("town"));
        let generate: Value = serde_json::from_str(GENERATE).unwrap();
        let unset: Value = serde_json::from_str(UNSET).unwrap();
        streams.saved(&city, "b.gen", &generate).await.unwrap();
        // The same id again replaces the definition.
        streams.saved(&city, "b.gen", &generate).await.unwrap();
        streams.saved(&city, "a.unset", &unset).await.unwrap();
        streams.saved(&town, "t.gen", &generate).await.unwrap();
        streams
            .resource_saved(&city, "output", "sink", "drop: {}")
            .await
            .unwrap();
        streams
            .resource_saved(&city, "nope", "odd", "drop: {}")
            .await
            .unwrap();
        let ids: Vec<String> = streams
            .list(&city)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(
            ids,
            ["a.unset", "b.gen"],
            "town's row is out of city's reach"
        );
        assert_eq!(streams.list(&org("nobody")).await.unwrap(), Vec::new());
        assert_eq!(streams.resources(&city).await.unwrap().len(), 2);
        assert!(streams.resources(&town).await.unwrap().is_empty());

        // A start puts back every resource and starts what still starts; the rest stays saved.
        let root = tempfile::tempdir().unwrap();
        let manager = manager(root.path());
        assert_eq!(
            streams.restore(&manager).await.unwrap(),
            2,
            "b.gen and t.gen"
        );
        assert_eq!(
            manager.list(Some(&city)).await.len(),
            1,
            "each in its organization"
        );
        assert_eq!(manager.list(Some(&town)).await.len(), 1);
        assert_eq!(
            streams.list(&city).await.unwrap().len(),
            2,
            "a.unset is kept"
        );
        manager.stop_all(std::time::Duration::from_secs(1)).await;

        streams.deleted(&city, "b.gen").await.unwrap();
        streams.deleted(&city, "never-saved").await.unwrap();
        let ids: Vec<String> = streams
            .list(&city)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(ids, ["a.unset"]);
        assert_eq!(
            streams.list(&town).await.unwrap().len(),
            1,
            "another org's delete leaves it"
        );
    }

    #[tokio::test]
    async fn the_boot_check_refuses_each_unsafe_request_role_and_table() {
        let db = prepare("boot").await;
        let owner = db.login("jc_ekpg_owner").await;
        // Before the migration the schema has no table: missing.
        let app = db.login("jc_ekpg_app").await;
        let e = verify_boot_rls(&app).await.unwrap_err();
        assert!(e.contains("ekiben.org is missing"), "{e}");
        migrate(&owner, "jc_ekpg_app").await.unwrap();
        verify_boot_rls(&app).await.unwrap();
        let admin = db.as_admin().await;
        // A request login whose search path finds another `pg_class` before the catalog's: the
        // table check fails with the server's reason.
        sqlx::raw_sql(
            "CREATE SCHEMA jc_ekpg_shadow;
             CREATE TABLE jc_ekpg_shadow.pg_class (x int);
             GRANT USAGE ON SCHEMA jc_ekpg_shadow TO PUBLIC;
             GRANT SELECT ON jc_ekpg_shadow.pg_class TO PUBLIC;",
        )
        .execute(&admin)
        .await
        .unwrap();
        let shadowed = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                db.base
                    .clone()
                    .database(&db.name)
                    .username("jc_ekpg_app")
                    .password("pw-jc_ekpg_app-7820")
                    .options([("search_path", "jc_ekpg_shadow,pg_catalog")]),
            )
            .await
            .unwrap();
        let e = verify_boot_rls(&shadowed).await.unwrap_err();
        assert!(
            e.starts_with("boot check: ") && e.contains("relname"),
            "{e}"
        );
        shadowed.close().await;
        let e = verify_boot_rls(&admin).await.unwrap_err();
        assert!(e.contains("has SUPERUSER or BYPASSRLS"), "{e}");
        assert!(PgStreams::open(admin.clone()).await.is_err());
        let e = verify_boot_rls(&owner).await.unwrap_err();
        assert!(e.contains("owns ekiben."), "{e}");
        sqlx::raw_sql("ALTER TABLE ekiben.resource NO FORCE ROW LEVEL SECURITY")
            .execute(&owner)
            .await
            .unwrap();
        let e = verify_boot_rls(&app).await.unwrap_err();
        assert!(
            e.contains("ekiben.resource lacks forced row-level security"),
            "{e}"
        );
        sqlx::raw_sql("DROP TABLE ekiben.resource")
            .execute(&owner)
            .await
            .unwrap();
        let e = verify_boot_rls(&app).await.unwrap_err();
        assert!(e.contains("ekiben.resource is missing"), "{e}");
        // A grant to a role the server does not know fails the start, naming the role.
        let e = migrate(&owner, "jc_ekpg_no_such_role").await.unwrap_err();
        assert!(e.contains("grant to jc_ekpg_no_such_role"), "{e}");
        // A role that may not create the schema fails the migration.
        let e = migrate(&app, "jc_ekpg_app").await.unwrap_err();
        assert!(e.starts_with("migration failed"), "{e}");
        db.admin.close().await;
    }

    #[tokio::test]
    async fn a_refused_statement_or_commit_reaches_the_caller() {
        let db = prepare("fail").await;
        let owner = db.login("jc_ekpg_owner").await;
        migrate(&owner, "jc_ekpg_app").await.unwrap();
        // jc_ekpg_ro may name an organization and read it, nothing else; a write of
        // organization `refused` passes its statement and fails at commit.
        sqlx::raw_sql(
            "GRANT USAGE ON SCHEMA ekiben TO jc_ekpg_ro;
             GRANT SELECT, INSERT ON ekiben.org TO jc_ekpg_ro;
             CREATE FUNCTION ekiben.refuse() RETURNS trigger LANGUAGE plpgsql AS
               $$ BEGIN RAISE EXCEPTION 'refused at commit'; END $$;
             CREATE CONSTRAINT TRIGGER refuse_stream AFTER INSERT OR DELETE ON ekiben.stream
               DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION ekiben.refuse();
             CREATE CONSTRAINT TRIGGER refuse_resource AFTER INSERT ON ekiben.resource
               DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION ekiben.refuse();",
        )
        .execute(&owner)
        .await
        .unwrap();
        let generate: Value = serde_json::from_str(GENERATE).unwrap();
        let app = PgStreams {
            pool: db.login("jc_ekpg_app").await,
        };
        let city = org("city");
        let e = app.saved(&city, "s", &generate).await.unwrap_err();
        assert!(e.contains("refused at commit"), "{e}");
        let e = app
            .resource_saved(&city, "output", "o", "drop: {}")
            .await
            .unwrap_err();
        assert!(e.contains("refused at commit"), "{e}");
        // The org row and a stream row exist only when written past the trigger.
        sqlx::raw_sql(
            "ALTER TABLE ekiben.stream DISABLE TRIGGER refuse_stream;
             INSERT INTO ekiben.stream (org, id, posted) VALUES ('city', 'kept', '{}');
             INSERT INTO ekiben.org (org) VALUES ('city');
             ALTER TABLE ekiben.stream ENABLE TRIGGER refuse_stream;",
        )
        .execute(&db.as_admin().await)
        .await
        .unwrap();
        let e = app.deleted(&city, "kept").await.unwrap_err();
        assert!(e.contains("refused at commit"), "{e}");

        let ro = PgStreams {
            pool: db.login("jc_ekpg_ro").await,
        };
        for e in [
            ro.list(&city).await.unwrap_err(),
            ro.resources(&city).await.unwrap_err(),
            ro.saved(&city, "s", &generate).await.unwrap_err(),
            ro.resource_saved(&city, "output", "o", "drop: {}")
                .await
                .unwrap_err(),
            ro.deleted(&city, "kept").await.unwrap_err(),
            ro.restore(&manager(std::env::temp_dir().as_path()))
                .await
                .unwrap_err(),
        ] {
            assert!(e.contains("permission denied"), "{e}");
        }

        // jc_ekpg_none may not even name an organization.
        sqlx::raw_sql("GRANT USAGE ON SCHEMA ekiben TO jc_ekpg_none")
            .execute(&owner)
            .await
            .unwrap();
        let none = PgStreams {
            pool: db.login("jc_ekpg_none").await,
        };
        for e in [
            none.saved(&city, "s", &generate).await.unwrap_err(),
            none.resource_saved(&city, "output", "o", "drop: {}")
                .await
                .unwrap_err(),
        ] {
            assert!(e.contains("permission denied for table org"), "{e}");
        }

        // A pool that is gone: every call answers the error, none panics.
        let gone = PgStreams {
            pool: db.login("jc_ekpg_app").await,
        };
        gone.pool.close().await;
        assert!(gone.list(&city).await.is_err());
        assert!(
            gone.restore(&manager(std::env::temp_dir().as_path()))
                .await
                .is_err()
        );
        assert!(gone.saved(&city, "s", &generate).await.is_err());
        assert!(
            gone.resource_saved(&city, "output", "o", "x")
                .await
                .is_err()
        );
        assert!(gone.deleted(&city, "s").await.is_err());
        assert!(
            verify_boot_rls(&gone.pool)
                .await
                .unwrap_err()
                .starts_with("boot check:")
        );
        assert!(
            migrate(&gone.pool, "jc_ekpg_app")
                .await
                .unwrap_err()
                .starts_with("migration failed")
        );
    }
}
