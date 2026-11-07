#![forbid(unsafe_code)]
//! `controlroom-store-pg`: PostgreSQL implementation of [`ControlroomStore`] on `sqlx 0.9` with:
//! - Dedicated pool and roles (`controlroom_app` without `BYPASSRLS`, migrations by `controlroom_migrate`).
//! - Password loaded from a file in the secrets mount (`PgPoolConfig::from_secret_file`).
//! - Boot check (`verify_rls_enforced` / `validate_rls_catalog_rows`) refusing to start if the role
//!   has `BYPASSRLS` or if any `cr_*` table lacks `FORCE ROW LEVEL SECURITY` (R8, §5.3).
//! - Per-transaction `SELECT set_config('cr.org', $1, true)` and `SELECT ... FOR UPDATE` on
//!   `cr_quota` during write transactions (§5.3).

use controlroom_model::{
    ControlroomStore, CrError, CrResult, DashboardRecord, DashboardVersionRecord, DatasourceRecord,
    FolderRecord, ImportReportRecord, InstallationId, InstanceId, LibraryPanelRecord, OrgId,
    OrgQuotas, PublicDashboardRecord, QueryCacheStatRecord, ShareType, store::StoreFuture,
};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use zeroize::Zeroizing;

pub const MIGRATION_SQL: &str = include_str!("../migrations/0001_initial.sql");

/// Required tables that MUST have both `relrowsecurity` and `relforcerowsecurity` enabled at boot (§5.3, R8).
pub const REQUIRED_RLS_TABLES: &[&str] = &[
    "cr_quota",
    "cr_folder",
    "cr_dashboard",
    "cr_dashboard_version",
    "cr_datasource",
    "cr_library_panel",
    "cr_public_dashboard",
    "cr_alert_rule",
    "cr_alert_state",
    "cr_query_cache",
    "cr_import_report",
];

/// Pool configuration for `controlroom-store-pg` (R8, R11: passed as a struct, password from secret file).
#[derive(Debug, Clone)]
pub struct PgPoolConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub app_role: String,
    pub password: Zeroizing<String>,
    pub max_connections: u32,
}

impl PgPoolConfig {
    /// Reads the database password from `secret_file_path` in the secrets mount (R8).
    pub async fn from_secret_file(
        host: impl Into<String>,
        port: u16,
        database: impl Into<String>,
        app_role: impl Into<String>,
        secret_file_path: impl AsRef<Path>,
        max_connections: u32,
    ) -> CrResult<Self> {
        let raw = tokio::fs::read_to_string(secret_file_path.as_ref())
            .await
            .map_err(|e| {
                CrError::Internal(format!("failed to read postgres password file: {e}"))
            })?;
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            return Err(CrError::BadRequest("postgres secret file is empty".into()));
        }
        Ok(Self {
            host: host.into(),
            port,
            database: database.into(),
            app_role: app_role.into(),
            password: Zeroizing::new(trimmed),
            max_connections: max_connections.max(1),
        })
    }

    pub async fn connect(&self) -> CrResult<PgPool> {
        let opts = PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.database)
            .username(&self.app_role)
            .password(self.password.as_str());
        PgPoolOptions::new()
            .max_connections(self.max_connections)
            .connect_with(opts)
            .await
            .map_err(|e| CrError::Internal(format!("failed to connect to postgres: {e}")))
    }
}

/// Row returned by the `pg_class` / `pg_policy` boot inspection query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRlsStatus {
    pub table_name: String,
    pub rls_enabled: bool,
    pub rls_forced: bool,
    pub policy_count: i64,
}

/// Validates the boot-check catalog rows: refuses if the current role has `BYPASSRLS` or `SUPERUSER`,
/// if any required table is missing, or if any `cr_*` table in the schema lacks forced RLS or a policy (R8).
pub fn validate_rls_catalog_rows(
    role_bypass_rls: bool,
    role_super: bool,
    tables: &[TableRlsStatus],
) -> CrResult<()> {
    if role_bypass_rls {
        return Err(CrError::RlsNotEnforced(
            "application database role has BYPASSRLS enabled".into(),
        ));
    }
    if role_super {
        return Err(CrError::RlsNotEnforced(
            "application database role is SUPERUSER (bypasses RLS)".into(),
        ));
    }

    for &required in REQUIRED_RLS_TABLES {
        let Some(status) = tables.iter().find(|t| t.table_name == required) else {
            return Err(CrError::RlsNotEnforced(format!(
                "required table '{required}' is missing from schema"
            )));
        };
        if !status.rls_enabled || !status.rls_forced {
            return Err(CrError::RlsNotEnforced(format!(
                "table '{}' does not have FORCE ROW LEVEL SECURITY (enabled={}, forced={})",
                status.table_name, status.rls_enabled, status.rls_forced
            )));
        }
        if status.policy_count < 1 {
            return Err(CrError::RlsNotEnforced(format!(
                "table '{}' has no RLS policy attached",
                status.table_name
            )));
        }
    }

    // Also refuse ANY extra `cr_*` table created in the schema without forced RLS!
    for status in tables {
        if !status.rls_enabled || !status.rls_forced || status.policy_count < 1 {
            return Err(CrError::RlsNotEnforced(format!(
                "table '{}' lacks forced RLS or policy (enabled={}, forced={}, policies={})",
                status.table_name, status.rls_enabled, status.rls_forced, status.policy_count
            )));
        }
    }

    Ok(())
}

/// PostgreSQL-backed [`ControlroomStore`] enforcing forced RLS on every transaction (R8, §5.3).
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs the schema migrations (`0001_initial.sql`).
    pub async fn run_migrations(&self) -> CrResult<()> {
        sqlx::raw_sql(MIGRATION_SQL)
            .execute(&self.pool)
            .await
            .map_err(|e| CrError::Internal(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Begins a transaction scoped to `org_id` via `SELECT set_config('cr.org', $1, true)` (§5.3).
    async fn begin_org_tx(&self, org_id: OrgId) -> CrResult<Transaction<'_, Postgres>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CrError::Internal(format!("begin tx failed: {e}")))?;
        sqlx::query("SELECT set_config('cr.org', $1, true)")
            .bind(org_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(format!("set_config cr.org failed: {e}")))?;
        Ok(tx)
    }

    /// Locks and reads the organization's quota row (`SELECT ... FOR UPDATE`) inside `tx` (§5.3).
    async fn lock_org_quotas(
        tx: &mut Transaction<'_, Postgres>,
        installation_id: InstallationId,
        org_id: OrgId,
        instance_id: InstanceId,
    ) -> CrResult<OrgQuotas> {
        let defaults = OrgQuotas::default();
        sqlx::query(
            "INSERT INTO cr_quota (installation_id, org_id, instance_id, max_dashboards, max_panels_per_dashboard, max_datasources, max_wasm_plugins, max_queries_per_minute, max_concurrent_queries, max_cache_bytes, max_alert_rules, max_evaluations_per_minute, max_public_range_seconds, min_public_interval_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (org_id) DO NOTHING",
        )
        .bind(installation_id.as_uuid())
        .bind(org_id.as_uuid())
        .bind(instance_id.as_uuid())
        .bind(defaults.max_dashboards as i64)
        .bind(defaults.max_panels_per_dashboard as i64)
        .bind(defaults.max_datasources as i64)
        .bind(defaults.max_wasm_plugins as i64)
        .bind(defaults.max_queries_per_minute as i64)
        .bind(defaults.max_concurrent_queries as i64)
        .bind(defaults.max_cache_bytes as i64)
        .bind(defaults.max_alert_rules as i64)
        .bind(defaults.max_evaluations_per_minute as i64)
        .bind(defaults.max_public_range_seconds)
        .bind(defaults.min_public_interval_ms)
        .execute(&mut **tx)
        .await
        .map_err(|e| CrError::Internal(format!("ensure quota row failed: {e}")))?;

        let row = sqlx::query(
            "SELECT max_dashboards, max_panels_per_dashboard, max_datasources, max_wasm_plugins,
                    max_queries_per_minute, max_concurrent_queries, max_cache_bytes,
                    max_alert_rules, max_evaluations_per_minute, max_public_range_seconds, min_public_interval_ms
             FROM cr_quota WHERE org_id = $1 FOR UPDATE",
        )
        .bind(org_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| CrError::Internal(format!("lock quota row failed: {e}")))?;

        Ok(OrgQuotas {
            max_dashboards: row.get::<i64, _>("max_dashboards") as u64,
            max_panels_per_dashboard: row.get::<i64, _>("max_panels_per_dashboard") as u64,
            max_datasources: row.get::<i64, _>("max_datasources") as u64,
            max_wasm_plugins: row.get::<i64, _>("max_wasm_plugins") as u64,
            max_queries_per_minute: row.get::<i64, _>("max_queries_per_minute") as u64,
            max_concurrent_queries: row.get::<i64, _>("max_concurrent_queries") as u64,
            max_cache_bytes: row.get::<i64, _>("max_cache_bytes") as u64,
            max_alert_rules: row.get::<i64, _>("max_alert_rules") as u64,
            max_evaluations_per_minute: row.get::<i64, _>("max_evaluations_per_minute") as u64,
            max_public_range_seconds: row.get("max_public_range_seconds"),
            min_public_interval_ms: row.get("min_public_interval_ms"),
        })
    }
}

impl ControlroomStore for PgStore {
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let role_row = sqlx::query(
                "SELECT rolbypassrls, rolsuper FROM pg_roles WHERE rolname = current_user",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CrError::RlsNotEnforced(format!("failed to inspect pg_roles: {e}")))?;

            let bypass: bool = role_row.get("rolbypassrls");
            let superuser: bool = role_row.get("rolsuper");

            let table_rows = sqlx::query(
                "SELECT c.relname AS table_name,
                        c.relrowsecurity AS rls_enabled,
                        c.relforcerowsecurity AS rls_forced,
                        (SELECT COUNT(*) FROM pg_policy p WHERE p.polrelid = c.oid) AS policy_count
                 FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                 WHERE n.nspname = current_schema()
                   AND c.relkind = 'r'
                   AND c.relname LIKE 'cr_%'",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CrError::RlsNotEnforced(format!("failed to inspect pg_class: {e}")))?;

            let statuses: Vec<TableRlsStatus> = table_rows
                .into_iter()
                .map(|r| TableRlsStatus {
                    table_name: r.get("table_name"),
                    rls_enabled: r.get("rls_enabled"),
                    rls_forced: r.get("rls_forced"),
                    policy_count: r.get("policy_count"),
                })
                .collect();

            validate_rls_catalog_rows(bypass, superuser, &statuses)
        })
    }

    fn get_quotas(&self, org_id: OrgId) -> StoreFuture<'_, OrgQuotas> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let opt = sqlx::query(
                "SELECT max_dashboards, max_panels_per_dashboard, max_datasources, max_wasm_plugins,
                        max_queries_per_minute, max_concurrent_queries, max_cache_bytes,
                        max_alert_rules, max_evaluations_per_minute, max_public_range_seconds, min_public_interval_ms
                 FROM cr_quota WHERE org_id = $1",
            )
            .bind(org_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;

            Ok(opt.map_or_else(OrgQuotas::default, |row| OrgQuotas {
                max_dashboards: row.get::<i64, _>("max_dashboards") as u64,
                max_panels_per_dashboard: row.get::<i64, _>("max_panels_per_dashboard") as u64,
                max_datasources: row.get::<i64, _>("max_datasources") as u64,
                max_wasm_plugins: row.get::<i64, _>("max_wasm_plugins") as u64,
                max_queries_per_minute: row.get::<i64, _>("max_queries_per_minute") as u64,
                max_concurrent_queries: row.get::<i64, _>("max_concurrent_queries") as u64,
                max_cache_bytes: row.get::<i64, _>("max_cache_bytes") as u64,
                max_alert_rules: row.get::<i64, _>("max_alert_rules") as u64,
                max_evaluations_per_minute: row.get::<i64, _>("max_evaluations_per_minute") as u64,
                max_public_range_seconds: row.get("max_public_range_seconds"),
                min_public_interval_ms: row.get("min_public_interval_ms"),
            }))
        })
    }

    fn set_quotas(&self, org_id: OrgId, q: OrgQuotas) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            sqlx::query(
                "INSERT INTO cr_quota (installation_id, org_id, instance_id, max_dashboards, max_panels_per_dashboard, max_datasources, max_wasm_plugins, max_queries_per_minute, max_concurrent_queries, max_cache_bytes, max_alert_rules, max_evaluations_per_minute, max_public_range_seconds, min_public_interval_ms)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                 ON CONFLICT (org_id) DO UPDATE SET
                    max_dashboards = EXCLUDED.max_dashboards,
                    max_panels_per_dashboard = EXCLUDED.max_panels_per_dashboard,
                    max_datasources = EXCLUDED.max_datasources,
                    max_wasm_plugins = EXCLUDED.max_wasm_plugins,
                    max_queries_per_minute = EXCLUDED.max_queries_per_minute,
                    max_concurrent_queries = EXCLUDED.max_concurrent_queries,
                    max_cache_bytes = EXCLUDED.max_cache_bytes,
                    max_alert_rules = EXCLUDED.max_alert_rules,
                    max_evaluations_per_minute = EXCLUDED.max_evaluations_per_minute,
                    max_public_range_seconds = EXCLUDED.max_public_range_seconds,
                    min_public_interval_ms = EXCLUDED.min_public_interval_ms,
                    updated_at = NOW()",
            )
            .bind(InstallationId::from_u128(1).as_uuid())
            .bind(org_id.as_uuid())
            .bind(InstanceId::from_u128(1).as_uuid())
            .bind(q.max_dashboards as i64)
            .bind(q.max_panels_per_dashboard as i64)
            .bind(q.max_datasources as i64)
            .bind(q.max_wasm_plugins as i64)
            .bind(q.max_queries_per_minute as i64)
            .bind(q.max_concurrent_queries as i64)
            .bind(q.max_cache_bytes as i64)
            .bind(q.max_alert_rules as i64)
            .bind(q.max_evaluations_per_minute as i64)
            .bind(q.max_public_range_seconds)
            .bind(q.min_public_interval_ms)
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(())
        })
    }

    fn list_folders(&self, org_id: OrgId) -> StoreFuture<'_, Vec<FolderRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, parent_uid, title, version, managed_by, created_by, updated_by, created_at, updated_at
                 FROM cr_folder WHERE org_id = $1 ORDER BY uid",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows.into_iter().map(row_to_folder).collect())
        })
    }

    fn get_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<FolderRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, parent_uid, title, version, managed_by, created_by, updated_by, created_at, updated_at
                 FROM cr_folder WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_folder))
        })
    }

    fn upsert_folder(&self, folder: FolderRecord) -> StoreFuture<'_, FolderRecord> {
        Box::pin(async move {
            if folder.uid == "general" {
                return Err(CrError::BadRequest(
                    "cannot create or overwrite reserved folder uid 'general'".into(),
                ));
            }
            let mut tx = self.begin_org_tx(folder.org_id).await?;
            let row = sqlx::query(
                "INSERT INTO cr_folder (installation_id, org_id, instance_id, project_id, uid, parent_uid, title, version, managed_by, created_by, updated_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8, $9, $10)
                 ON CONFLICT (org_id, uid) DO UPDATE SET
                    parent_uid = EXCLUDED.parent_uid,
                    title = EXCLUDED.title,
                    version = cr_folder.version + 1,
                    managed_by = EXCLUDED.managed_by,
                    updated_by = EXCLUDED.updated_by,
                    updated_at = NOW()
                 RETURNING installation_id, org_id, instance_id, project_id, uid, parent_uid, title, version, managed_by, created_by, updated_by, created_at, updated_at",
            )
            .bind(folder.installation_id.as_uuid())
            .bind(folder.org_id.as_uuid())
            .bind(folder.instance_id.as_uuid())
            .bind(&folder.project_id)
            .bind(&folder.uid)
            .bind(&folder.parent_uid)
            .bind(&folder.title)
            .bind(&folder.managed_by)
            .bind(&folder.created_by)
            .bind(&folder.updated_by)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row_to_folder(row))
        })
    }

    fn delete_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            sqlx::query("DELETE FROM cr_dashboard WHERE org_id = $1 AND folder_uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            let res = sqlx::query("DELETE FROM cr_folder WHERE org_id = $1 AND uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn list_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DashboardRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, folder_uid, title, slug, tags, panel_count, datasource_uids, version, schema_version, is_provisioned, provisioned_external_id, allow_ui_updates, model, created_by, updated_by, created_at, updated_at
                 FROM cr_dashboard WHERE org_id = $1 ORDER BY uid",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows.into_iter().map(row_to_dashboard).collect())
        })
    }

    fn get_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<DashboardRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, folder_uid, title, slug, tags, panel_count, datasource_uids, version, schema_version, is_provisioned, provisioned_external_id, allow_ui_updates, model, created_by, updated_by, created_at, updated_at
                 FROM cr_dashboard WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_dashboard))
        })
    }

    fn upsert_dashboard(
        &self,
        mut dashboard: DashboardRecord,
        message: &str,
        overwrite: bool,
    ) -> StoreFuture<'_, DashboardRecord> {
        let msg = message.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(dashboard.org_id).await?;
            let quotas = Self::lock_org_quotas(
                &mut tx,
                dashboard.installation_id,
                dashboard.org_id,
                dashboard.instance_id,
            )
            .await?;

            if dashboard.panel_count as u64 > quotas.max_panels_per_dashboard {
                return Err(CrError::QuotaExceeded {
                    resource: "panels_per_dashboard".into(),
                    limit: quotas.max_panels_per_dashboard,
                });
            }

            let existing_row = sqlx::query(
                "SELECT version, is_provisioned, allow_ui_updates FROM cr_dashboard WHERE org_id = $1 AND uid = $2 FOR UPDATE",
            )
            .bind(dashboard.org_id.as_uuid())
            .bind(&dashboard.uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;

            let parent_version = if let Some(ex) = existing_row {
                let ex_ver: i32 = ex.get("version");
                let ex_prov: bool = ex.get("is_provisioned");
                let ex_allow: bool = ex.get("allow_ui_updates");
                if !overwrite && dashboard.version != ex_ver {
                    return Err(CrError::PreconditionFailed(format!(
                        "dashboard '{}' version mismatch",
                        dashboard.uid
                    )));
                }
                if ex_prov && !ex_allow && !dashboard.is_provisioned {
                    return Err(CrError::Forbidden(
                        "cannot update provisioned dashboard when allowUiUpdates is false".into(),
                    ));
                }
                dashboard.version = ex_ver + 1;
                ex_ver
            } else {
                let cnt: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM cr_dashboard WHERE org_id = $1")
                        .bind(dashboard.org_id.as_uuid())
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| CrError::Internal(e.to_string()))?;
                if cnt as u64 >= quotas.max_dashboards {
                    return Err(CrError::QuotaExceeded {
                        resource: "dashboards".into(),
                        limit: quotas.max_dashboards,
                    });
                }
                dashboard.version = 1;
                0
            };

            if let Some(obj) = dashboard.data.as_object_mut() {
                obj.insert("version".to_owned(), Value::from(dashboard.version));
                obj.insert("uid".to_owned(), Value::String(dashboard.uid.clone()));
            }

            let tags_json = serde_json::to_value(&dashboard.tags).unwrap_or(Value::Array(vec![]));
            let ds_json =
                serde_json::to_value(&dashboard.datasource_uids).unwrap_or(Value::Array(vec![]));

            let saved = sqlx::query(
                "INSERT INTO cr_dashboard (installation_id, org_id, instance_id, project_id, uid, folder_uid, title, slug, tags, panel_count, datasource_uids, version, schema_version, is_provisioned, provisioned_external_id, allow_ui_updates, model, created_by, updated_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
                 ON CONFLICT (org_id, uid) DO UPDATE SET
                    folder_uid = EXCLUDED.folder_uid,
                    title = EXCLUDED.title,
                    slug = EXCLUDED.slug,
                    tags = EXCLUDED.tags,
                    panel_count = EXCLUDED.panel_count,
                    datasource_uids = EXCLUDED.datasource_uids,
                    version = EXCLUDED.version,
                    schema_version = EXCLUDED.schema_version,
                    is_provisioned = EXCLUDED.is_provisioned,
                    provisioned_external_id = EXCLUDED.provisioned_external_id,
                    allow_ui_updates = EXCLUDED.allow_ui_updates,
                    model = EXCLUDED.model,
                    updated_by = EXCLUDED.updated_by,
                    updated_at = NOW()
                 RETURNING installation_id, org_id, instance_id, project_id, uid, folder_uid, title, slug, tags, panel_count, datasource_uids, version, schema_version, is_provisioned, provisioned_external_id, allow_ui_updates, model, created_by, updated_by, created_at, updated_at",
            )
            .bind(dashboard.installation_id.as_uuid())
            .bind(dashboard.org_id.as_uuid())
            .bind(dashboard.instance_id.as_uuid())
            .bind(&dashboard.project_id)
            .bind(&dashboard.uid)
            .bind(&dashboard.folder_uid)
            .bind(&dashboard.title)
            .bind(&dashboard.slug)
            .bind(&tags_json)
            .bind(dashboard.panel_count)
            .bind(&ds_json)
            .bind(dashboard.version)
            .bind(dashboard.schema_version)
            .bind(dashboard.is_provisioned)
            .bind(&dashboard.provisioned_external_id)
            .bind(dashboard.allow_ui_updates)
            .bind(&dashboard.data)
            .bind(&dashboard.created_by)
            .bind(&dashboard.updated_by)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;

            sqlx::query(
                "INSERT INTO cr_dashboard_version (installation_id, org_id, instance_id, dashboard_uid, version, parent_version, restored_from, message, model, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $8, $9)",
            )
            .bind(dashboard.installation_id.as_uuid())
            .bind(dashboard.org_id.as_uuid())
            .bind(dashboard.instance_id.as_uuid())
            .bind(&dashboard.uid)
            .bind(dashboard.version)
            .bind(parent_version)
            .bind(if msg.is_empty() { "Saved" } else { &msg })
            .bind(&dashboard.data)
            .bind(&dashboard.updated_by)
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;

            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row_to_dashboard(saved))
        })
    }

    fn delete_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            sqlx::query(
                "DELETE FROM cr_dashboard_version WHERE org_id = $1 AND dashboard_uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            sqlx::query("DELETE FROM cr_public_dashboard WHERE org_id = $1 AND dashboard_uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            let res = sqlx::query("DELETE FROM cr_dashboard WHERE org_id = $1 AND uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn list_dashboard_versions(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Vec<DashboardVersionRecord>> {
        let uid = dashboard_uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, dashboard_uid, version, parent_version, restored_from, message, model, created_by, created_at
                 FROM cr_dashboard_version WHERE org_id = $1 AND dashboard_uid = $2 ORDER BY version ASC",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows
                .into_iter()
                .map(|r| DashboardVersionRecord {
                    installation_id: InstallationId(r.get("installation_id")),
                    org_id: OrgId(r.get("org_id")),
                    instance_id: InstanceId(r.get("instance_id")),
                    dashboard_uid: r.get("dashboard_uid"),
                    version: r.get("version"),
                    parent_version: r.get("parent_version"),
                    restored_from: r.get("restored_from"),
                    message: r.get("message"),
                    data: r.get("model"),
                    created_by: r.get("created_by"),
                    created_at: r.get("created_at"),
                })
                .collect())
        })
    }

    fn list_datasources(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DatasourceRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, name, kind, access, url, is_default, read_only, json_data, secret_refs, allow_hosts, wasm_digest, version, created_at, updated_at
                 FROM cr_datasource WHERE org_id = $1 ORDER BY name",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows.into_iter().map(row_to_datasource).collect())
        })
    }

    fn get_datasource(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, name, kind, access, url, is_default, read_only, json_data, secret_refs, allow_hosts, wasm_digest, version, created_at, updated_at
                 FROM cr_datasource WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_datasource))
        })
    }

    fn get_datasource_by_name(
        &self,
        org_id: OrgId,
        name: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        let name = name.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, project_id, uid, name, kind, access, url, is_default, read_only, json_data, secret_refs, allow_hosts, wasm_digest, version, created_at, updated_at
                 FROM cr_datasource WHERE org_id = $1 AND name = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_datasource))
        })
    }

    fn upsert_datasource(&self, ds: DatasourceRecord) -> StoreFuture<'_, DatasourceRecord> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(ds.org_id).await?;
            let quotas =
                Self::lock_org_quotas(&mut tx, ds.installation_id, ds.org_id, ds.instance_id)
                    .await?;

            let exists: Option<i32> = sqlx::query_scalar(
                "SELECT version FROM cr_datasource WHERE org_id = $1 AND uid = $2",
            )
            .bind(ds.org_id.as_uuid())
            .bind(&ds.uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;

            if exists.is_none() {
                let cnt: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM cr_datasource WHERE org_id = $1")
                        .bind(ds.org_id.as_uuid())
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| CrError::Internal(e.to_string()))?;
                if cnt as u64 >= quotas.max_datasources {
                    return Err(CrError::QuotaExceeded {
                        resource: "datasources".into(),
                        limit: quotas.max_datasources,
                    });
                }
            }

            if ds.is_default {
                sqlx::query(
                    "UPDATE cr_datasource SET is_default = FALSE WHERE org_id = $1 AND uid <> $2",
                )
                .bind(ds.org_id.as_uuid())
                .bind(&ds.uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            }

            let secret_refs_json = serde_json::to_value(&ds.secret_refs).unwrap_or_default();
            let allow_hosts_json = serde_json::to_value(&ds.allow_hosts).unwrap_or_default();

            let row = sqlx::query(
                "INSERT INTO cr_datasource (installation_id, org_id, instance_id, project_id, uid, name, kind, access, url, is_default, read_only, json_data, secret_refs, allow_hosts, wasm_digest, version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 1)
                 ON CONFLICT (org_id, uid) DO UPDATE SET
                    name = EXCLUDED.name,
                    kind = EXCLUDED.kind,
                    access = EXCLUDED.access,
                    url = EXCLUDED.url,
                    is_default = EXCLUDED.is_default,
                    read_only = EXCLUDED.read_only,
                    json_data = EXCLUDED.json_data,
                    secret_refs = EXCLUDED.secret_refs,
                    allow_hosts = EXCLUDED.allow_hosts,
                    wasm_digest = EXCLUDED.wasm_digest,
                    version = cr_datasource.version + 1,
                    updated_at = NOW()
                 RETURNING installation_id, org_id, instance_id, project_id, uid, name, kind, access, url, is_default, read_only, json_data, secret_refs, allow_hosts, wasm_digest, version, created_at, updated_at",
            )
            .bind(ds.installation_id.as_uuid())
            .bind(ds.org_id.as_uuid())
            .bind(ds.instance_id.as_uuid())
            .bind(&ds.project_id)
            .bind(&ds.uid)
            .bind(&ds.name)
            .bind(&ds.kind)
            .bind(&ds.access)
            .bind(&ds.url)
            .bind(ds.is_default)
            .bind(ds.read_only)
            .bind(&ds.json_data)
            .bind(&secret_refs_json)
            .bind(&allow_hosts_json)
            .bind(&ds.wasm_digest)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row_to_datasource(row))
        })
    }

    fn delete_datasource(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let ro: Option<bool> = sqlx::query_scalar(
                "SELECT read_only FROM cr_datasource WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            if ro == Some(true) {
                return Err(CrError::Forbidden(
                    "Cannot delete read-only data source".into(),
                ));
            }
            let res = sqlx::query("DELETE FROM cr_datasource WHERE org_id = $1 AND uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn list_library_panels(&self, org_id: OrgId) -> StoreFuture<'_, Vec<LibraryPanelRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, folder_uid, uid, name, panel_type, description, model, version, created_at, updated_at
                 FROM cr_library_panel WHERE org_id = $1 ORDER BY uid",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows.into_iter().map(row_to_library_panel).collect())
        })
    }

    fn get_library_panel(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<LibraryPanelRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, folder_uid, uid, name, panel_type, description, model, version, created_at, updated_at
                 FROM cr_library_panel WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_library_panel))
        })
    }

    fn upsert_library_panel(
        &self,
        panel: LibraryPanelRecord,
    ) -> StoreFuture<'_, LibraryPanelRecord> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(panel.org_id).await?;
            let row = sqlx::query(
                "INSERT INTO cr_library_panel (installation_id, org_id, instance_id, folder_uid, uid, name, panel_type, description, model, version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1)
                 ON CONFLICT (org_id, uid) DO UPDATE SET
                    folder_uid = EXCLUDED.folder_uid,
                    name = EXCLUDED.name,
                    panel_type = EXCLUDED.panel_type,
                    description = EXCLUDED.description,
                    model = EXCLUDED.model,
                    version = cr_library_panel.version + 1,
                    updated_at = NOW()
                 RETURNING installation_id, org_id, instance_id, folder_uid, uid, name, panel_type, description, model, version, created_at, updated_at",
            )
            .bind(panel.installation_id.as_uuid())
            .bind(panel.org_id.as_uuid())
            .bind(panel.instance_id.as_uuid())
            .bind(&panel.folder_uid)
            .bind(&panel.uid)
            .bind(&panel.name)
            .bind(&panel.panel_type)
            .bind(&panel.description)
            .bind(&panel.model)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row_to_library_panel(row))
        })
    }

    fn delete_library_panel(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let res = sqlx::query("DELETE FROM cr_library_panel WHERE org_id = $1 AND uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn list_public_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<PublicDashboardRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by, created_at, updated_at
                 FROM cr_public_dashboard WHERE org_id = $1 ORDER BY uid",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows.into_iter().map(row_to_public_dashboard).collect())
        })
    }

    fn get_public_dashboard(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by, created_at, updated_at
                 FROM cr_public_dashboard WHERE org_id = $1 AND uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_public_dashboard))
        })
    }

    fn get_public_dashboard_by_dashboard_uid(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        let duid = dashboard_uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by, created_at, updated_at
                 FROM cr_public_dashboard WHERE org_id = $1 AND dashboard_uid = $2",
            )
            .bind(org_id.as_uuid())
            .bind(&duid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(row.map(row_to_public_dashboard))
        })
    }

    fn get_public_dashboard_by_token_hash(
        &self,
        token_hash: &str,
    ) -> StoreFuture<'_, Option<(PublicDashboardRecord, DashboardRecord)>> {
        let th = token_hash.to_owned();
        Box::pin(async move {
            // Forced RLS hides every row while `cr.org` is unset; the `public_token_lookup`
            // policy shows the one row whose hash this transaction names.
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            sqlx::query("SELECT set_config('cr.public_token', $1, true)")
                .bind(&th)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            let row = sqlx::query(
                "SELECT installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by, created_at, updated_at
                 FROM cr_public_dashboard WHERE access_token_hash = $1",
            )
            .bind(&th)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;

            let Some(pd_row) = row else {
                return Ok(None);
            };
            let pd = row_to_public_dashboard(pd_row);
            let dash = self.get_dashboard(pd.org_id, &pd.dashboard_uid).await?;
            Ok(dash.map(|d| (pd, d)))
        })
    }

    fn upsert_public_dashboard(
        &self,
        mut pubdash: PublicDashboardRecord,
    ) -> StoreFuture<'_, PublicDashboardRecord> {
        let raw_token = pubdash.access_token.clone();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(pubdash.org_id).await?;
            let row = sqlx::query(
                "INSERT INTO cr_public_dashboard (installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                 ON CONFLICT (org_id, uid) DO UPDATE SET
                    access_token_hash = EXCLUDED.access_token_hash,
                    is_enabled = EXCLUDED.is_enabled,
                    time_selection_enabled = EXCLUDED.time_selection_enabled,
                    annotations_enabled = EXCLUDED.annotations_enabled,
                    show_query_text = EXCLUDED.show_query_text,
                    share = EXCLUDED.share,
                    updated_by = EXCLUDED.updated_by,
                    updated_at = NOW()
                 RETURNING installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash, is_enabled, time_selection_enabled, annotations_enabled, show_query_text, share, created_by, updated_by, created_at, updated_at",
            )
            .bind(pubdash.installation_id.as_uuid())
            .bind(pubdash.org_id.as_uuid())
            .bind(pubdash.instance_id.as_uuid())
            .bind(&pubdash.uid)
            .bind(&pubdash.dashboard_uid)
            .bind(&pubdash.access_token_hash)
            .bind(pubdash.is_enabled)
            .bind(pubdash.time_selection_enabled)
            .bind(pubdash.annotations_enabled)
            .bind(pubdash.show_query_text)
            .bind(pubdash.share.as_str())
            .bind(&pubdash.created_by)
            .bind(&pubdash.updated_by)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            pubdash = row_to_public_dashboard(row);
            pubdash.access_token = raw_token;
            Ok(pubdash)
        })
    }

    fn delete_public_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let res = sqlx::query("DELETE FROM cr_public_dashboard WHERE org_id = $1 AND uid = $2")
                .bind(org_id.as_uuid())
                .bind(&uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn list_query_cache_stats(&self, org_id: OrgId) -> StoreFuture<'_, Vec<QueryCacheStatRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT id, org_id, instance_id, datasource_uid, hits, misses, entries, bytes_used
                 FROM cr_query_cache WHERE org_id = $1 ORDER BY id",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows
                .into_iter()
                .map(|r| QueryCacheStatRecord {
                    id: r.get("id"),
                    org_id: OrgId(r.get("org_id")),
                    instance_id: InstanceId(r.get("instance_id")),
                    datasource_uid: r.get("datasource_uid"),
                    hits: r.get::<i64, _>("hits") as u64,
                    misses: r.get::<i64, _>("misses") as u64,
                    entries: r.get::<i64, _>("entries") as u64,
                    bytes_used: r.get::<i64, _>("bytes_used") as u64,
                })
                .collect())
        })
    }

    fn upsert_query_cache_stat(&self, stat: QueryCacheStatRecord) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(stat.org_id).await?;
            sqlx::query(
                "INSERT INTO cr_query_cache (installation_id, org_id, instance_id, id, datasource_uid, hits, misses, entries, bytes_used)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (org_id, id) DO UPDATE SET
                    hits = EXCLUDED.hits,
                    misses = EXCLUDED.misses,
                    entries = EXCLUDED.entries,
                    bytes_used = EXCLUDED.bytes_used,
                    updated_at = NOW()",
            )
            .bind(InstallationId::from_u128(1).as_uuid())
            .bind(stat.org_id.as_uuid())
            .bind(stat.instance_id.as_uuid())
            .bind(&stat.id)
            .bind(&stat.datasource_uid)
            .bind(stat.hits as i64)
            .bind(stat.misses as i64)
            .bind(stat.entries as i64)
            .bind(stat.bytes_used as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(())
        })
    }

    fn list_import_reports(&self, org_id: OrgId) -> StoreFuture<'_, Vec<ImportReportRecord>> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(org_id).await?;
            let rows = sqlx::query(
                "SELECT id, org_id, instance_id, dry_run, status, dashboard_count, refused_count, mapped_count, summary, created_at
                 FROM cr_import_report WHERE org_id = $1 ORDER BY created_at DESC",
            )
            .bind(org_id.as_uuid())
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(rows
                .into_iter()
                .map(|r| ImportReportRecord {
                    id: r.get("id"),
                    org_id: OrgId(r.get("org_id")),
                    instance_id: InstanceId(r.get("instance_id")),
                    dry_run: r.get("dry_run"),
                    status: r.get("status"),
                    dashboard_count: r.get("dashboard_count"),
                    refused_count: r.get("refused_count"),
                    mapped_count: r.get("mapped_count"),
                    summary: r.get("summary"),
                    created_at: r.get("created_at"),
                })
                .collect())
        })
    }

    fn insert_import_report(&self, report: ImportReportRecord) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut tx = self.begin_org_tx(report.org_id).await?;
            sqlx::query(
                "INSERT INTO cr_import_report (installation_id, org_id, instance_id, id, dry_run, status, dashboard_count, refused_count, mapped_count, summary, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(InstallationId::from_u128(1).as_uuid())
            .bind(report.org_id.as_uuid())
            .bind(report.instance_id.as_uuid())
            .bind(&report.id)
            .bind(report.dry_run)
            .bind(&report.status)
            .bind(report.dashboard_count)
            .bind(report.refused_count)
            .bind(report.mapped_count)
            .bind(&report.summary)
            .bind(report.created_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| CrError::Internal(e.to_string()))?;
            Ok(())
        })
    }
}

fn row_to_folder(r: sqlx::postgres::PgRow) -> FolderRecord {
    FolderRecord {
        installation_id: InstallationId(r.get("installation_id")),
        org_id: OrgId(r.get("org_id")),
        instance_id: InstanceId(r.get("instance_id")),
        project_id: r.get("project_id"),
        uid: r.get("uid"),
        parent_uid: r.get("parent_uid"),
        title: r.get("title"),
        version: r.get("version"),
        managed_by: r.get("managed_by"),
        created_by: r.get("created_by"),
        updated_by: r.get("updated_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_dashboard(r: sqlx::postgres::PgRow) -> DashboardRecord {
    let tags_val: Value = r.get("tags");
    let ds_val: Value = r.get("datasource_uids");
    DashboardRecord {
        installation_id: InstallationId(r.get("installation_id")),
        org_id: OrgId(r.get("org_id")),
        instance_id: InstanceId(r.get("instance_id")),
        project_id: r.get("project_id"),
        uid: r.get("uid"),
        folder_uid: r.get("folder_uid"),
        title: r.get("title"),
        slug: r.get("slug"),
        tags: serde_json::from_value(tags_val).unwrap_or_default(),
        panel_count: r.get("panel_count"),
        datasource_uids: serde_json::from_value(ds_val).unwrap_or_default(),
        version: r.get("version"),
        schema_version: r.get("schema_version"),
        is_provisioned: r.get("is_provisioned"),
        provisioned_external_id: r.get("provisioned_external_id"),
        allow_ui_updates: r.get("allow_ui_updates"),
        data: r.get("model"),
        created_by: r.get("created_by"),
        updated_by: r.get("updated_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_datasource(r: sqlx::postgres::PgRow) -> DatasourceRecord {
    let secret_refs_val: Value = r.get("secret_refs");
    let allow_hosts_val: Value = r.get("allow_hosts");
    DatasourceRecord {
        installation_id: InstallationId(r.get("installation_id")),
        org_id: OrgId(r.get("org_id")),
        instance_id: InstanceId(r.get("instance_id")),
        project_id: r.get("project_id"),
        uid: r.get("uid"),
        name: r.get("name"),
        kind: r.get("kind"),
        access: r.get("access"),
        url: r.get("url"),
        is_default: r.get("is_default"),
        read_only: r.get("read_only"),
        json_data: r.get("json_data"),
        secret_refs: serde_json::from_value::<BTreeMap<String, String>>(secret_refs_val)
            .unwrap_or_default(),
        allow_hosts: serde_json::from_value(allow_hosts_val).unwrap_or_default(),
        wasm_digest: r.get("wasm_digest"),
        version: r.get("version"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_library_panel(r: sqlx::postgres::PgRow) -> LibraryPanelRecord {
    LibraryPanelRecord {
        installation_id: InstallationId(r.get("installation_id")),
        org_id: OrgId(r.get("org_id")),
        instance_id: InstanceId(r.get("instance_id")),
        folder_uid: r.get("folder_uid"),
        uid: r.get("uid"),
        name: r.get("name"),
        panel_type: r.get("panel_type"),
        description: r.get("description"),
        model: r.get("model"),
        version: r.get("version"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_public_dashboard(r: sqlx::postgres::PgRow) -> PublicDashboardRecord {
    let share_str: String = r.get("share");
    PublicDashboardRecord {
        installation_id: InstallationId(r.get("installation_id")),
        org_id: OrgId(r.get("org_id")),
        instance_id: InstanceId(r.get("instance_id")),
        uid: r.get("uid"),
        dashboard_uid: r.get("dashboard_uid"),
        access_token_hash: r.get("access_token_hash"),
        access_token: None,
        is_enabled: r.get("is_enabled"),
        time_selection_enabled: r.get("time_selection_enabled"),
        annotations_enabled: r.get("annotations_enabled"),
        show_query_text: r.get("show_query_text"),
        share: if share_str == "email" {
            ShareType::Email
        } else {
            ShareType::Public
        },
        created_by: r.get("created_by"),
        updated_by: r.get("updated_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

/// Helper to connect a test [`PgStore`] from `CONTROLROOM_TEST_DATABASE_URL`.
pub async fn connect_test_pg_store(url: &str) -> CrResult<PgStore> {
    let opts = PgConnectOptions::from_str(url)
        .map_err(|e| CrError::Internal(format!("invalid test db url: {e}")))?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .map_err(|e| CrError::Internal(format!("failed to connect test db: {e}")))?;
    let store = PgStore::new(pool);
    store.run_migrations().await?;
    Ok(store)
}
