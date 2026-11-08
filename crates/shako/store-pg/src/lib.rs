#![forbid(unsafe_code)]
//! `shako-store-pg`: PostgreSQL implementation of [`shako_core::Store`] on `sqlx` 0.9 with:
//! - Dedicated pool and request role `shako_app` (no `BYPASSRLS`, no `SUPERUSER`, owns 0 tables).
//! - Separate migration role `shako_migrate`.
//! - Database password loaded from a file in the secrets mount (`PgStoreConfig::from_secret_file`).
//! - Boot check (`verify_boot_rls` / `validate_rls_catalog_rows`) refusing to start if any table
//!   in schema `shako` lacks `FORCE ROW LEVEL SECURITY` or if the request role has `BYPASSRLS`.
//! - Per-transaction `SET LOCAL` via `set_config('shako.instance', $1, true)` and
//!   `set_config('shako.org', $2, true)` and `LISTEN/NOTIFY shako_outbox`.

use serde::{Deserialize, Serialize};
use shako_core::{
    BlobRecord, CommitStatusRecord, DeliveryRecord, InstanceId, InstanceRecord, OrgId, OutboxEvent,
    PackageFileRecord, PackageRecord, PackageVersionRecord, ProtectionRuleRecord,
    PullRequestRecord, PushMirrorRecord, QuotaUsageRecord, RepoId, RepoRecord, ReviewRecord,
    ShakoError, ShakoResult, Store, StoreFuture, TagProtectionRecord, WebhookId, WebhookRecord,
};
use shako_store_memory::MemoryStore;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use zeroize::Zeroizing;

pub const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");
pub const MIGRATION_0002_SQL: &str = include_str!("../migrations/0002_store_log.sql");

/// All 16 required tables in schema `shako` that MUST have `relrowsecurity` and `relforcerowsecurity` (S2).
pub const REQUIRED_RLS_TABLES: &[&str] = &[
    "instance",
    "repo",
    "protection_rule",
    "pull_request",
    "review",
    "commit_status",
    "webhook",
    "delivery",
    "push_mirror",
    "package",
    "package_version",
    "package_file",
    "blob",
    "quota_usage",
    "outbox",
    "store_log",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRlsStatus {
    pub table_name: String,
    pub rls_enabled: bool,
    pub rls_forced: bool,
    pub policy_count: i64,
}

/// Validates the PostgreSQL catalog inspection rows at boot (S2).
///
/// `role` is the connected role (`current_user`) the checks were made for; every refusal names it.
pub fn validate_rls_catalog_rows(
    role: &str,
    role_bypass_rls: bool,
    role_super: bool,
    owned_tables: i64,
    external_grants: i64,
    tables: &[TableRlsStatus],
) -> ShakoResult<()> {
    let refused: Vec<&str> = [(role_super, "SUPERUSER"), (role_bypass_rls, "BYPASSRLS")]
        .into_iter()
        .filter_map(|(set, attr)| set.then_some(attr))
        .collect();
    if !refused.is_empty() {
        return Err(ShakoError::RlsNotEnforced(format!(
            "request role {role} has {} (bypasses RLS); connect as a NOSUPERUSER NOBYPASSRLS member of shako_app",
            refused.join(" and ")
        )));
    }
    if owned_tables > 0 {
        return Err(ShakoError::RlsNotEnforced(format!(
            "request role {role} owns {owned_tables} table(s); migrations must run as shako_migrate"
        )));
    }
    if external_grants > 0 {
        return Err(ShakoError::RlsNotEnforced(format!(
            "request role {role} holds {external_grants} grant(s) outside schema shako"
        )));
    }

    for &required in REQUIRED_RLS_TABLES {
        let Some(status) = tables.iter().find(|t| t.table_name == required) else {
            return Err(ShakoError::RlsNotEnforced(format!(
                "required table 'shako.{required}' is missing"
            )));
        };
        if !status.rls_enabled || !status.rls_forced || status.policy_count < 1 {
            return Err(ShakoError::RlsNotEnforced(format!(
                "table 'shako.{}' lacks FORCE ROW LEVEL SECURITY or policy (enabled={}, forced={}, policies={})",
                status.table_name, status.rls_enabled, status.rls_forced, status.policy_count
            )));
        }
    }

    // Refuse ANY extra table created in schema `shako` without forced RLS!
    for status in tables {
        if !status.rls_enabled || !status.rls_forced || status.policy_count < 1 {
            return Err(ShakoError::RlsNotEnforced(format!(
                "table 'shako.{}' lacks FORCE ROW LEVEL SECURITY or policy",
                status.table_name
            )));
        }
    }
    Ok(())
}

/// Configuration for connecting `shako-store-pg` (password read from a file in the secrets mount, S2).
#[derive(Debug, Clone)]
pub struct PgStoreConfig {
    pub url_without_password: String,
    pub password: Option<Zeroizing<String>>,
    pub max_connections: u32,
}

impl PgStoreConfig {
    pub async fn from_secret_file(
        url_without_password: impl Into<String>,
        secret_file_path: impl AsRef<Path>,
        max_connections: u32,
    ) -> ShakoResult<Self> {
        let raw = tokio::fs::read_to_string(secret_file_path.as_ref())
            .await
            .map_err(|e| {
                ShakoError::Storage(format!(
                    "cannot read db password file {}: {e}",
                    secret_file_path.as_ref().display()
                ))
            })?;
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            return Err(ShakoError::BadRequest("db password file is empty".into()));
        }
        Ok(Self {
            url_without_password: url_without_password.into(),
            password: Some(Zeroizing::new(trimmed)),
            max_connections: max_connections.clamp(1, 20),
        })
    }
}

/// PostgreSQL store with private pool, forced RLS boot check, and `LISTEN/NOTIFY` outbox wakeup (S2, S14).
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    mirror: Arc<MemoryStore>,
    outbox_notifications: Arc<AtomicU64>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl PgStore {
    pub async fn connect(config: &PgStoreConfig) -> ShakoResult<Self> {
        let mut opts = PgConnectOptions::from_str(&config.url_without_password)
            .map_err(|e| ShakoError::Storage(format!("invalid pg url: {e}")))?;
        if let Some(pw) = &config.password {
            opts = opts.password(pw.as_str());
        }
        Self::connect_with(opts, config.max_connections).await
    }

    /// Connects with explicit options and runs the boot RLS check; refuses a role that bypasses RLS.
    pub async fn connect_with(opts: PgConnectOptions, max_connections: u32) -> ShakoResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.clamp(1, 20))
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .map_err(|e| ShakoError::Storage(format!("pg connect failed: {e}")))?;

        let store = Self {
            pool,
            mirror: Arc::new(MemoryStore::new()),
            outbox_notifications: Arc::new(AtomicU64::new(0)),
            write_lock: Arc::default(),
        };
        store.verify_rls_enforced().await?;
        Ok(store)
    }

    /// Creates a `PgStore` from an existing pool without running the boot check yet (for tests).
    #[must_use]
    pub fn from_pool_unchecked(pool: PgPool) -> Self {
        Self {
            pool,
            mirror: Arc::new(MemoryStore::new()),
            outbox_notifications: Arc::new(AtomicU64::new(0)),
            write_lock: Arc::default(),
        }
    }

    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs the migrations as `shako_migrate` (schema owner). sqlx's migrator holds an advisory
    /// lock while it runs, so replicas booting together migrate once; applied versions are kept
    /// in `shako_migrations._sqlx_migrations`, so a second run changes nothing.
    pub async fn run_migrations(migrate_pool: &PgPool) -> ShakoResult<()> {
        use sqlx::SqlSafeStr;
        use sqlx::migrate::{Migration, MigrationType, Migrator};
        let migration = |version, name: &str, sql: &'static str| {
            Migration::new(
                version,
                name.to_owned().into(),
                MigrationType::Simple,
                sql.into_sql_str(),
                false,
            )
        };
        let mut migrator = Migrator::with_migrations(vec![
            migration(1, "init", MIGRATION_0001_SQL),
            migration(2, "store_log", MIGRATION_0002_SQL),
        ]);
        migrator.create_schema("shako_migrations");
        migrator.dangerous_set_table_name("shako_migrations._sqlx_migrations");
        migrator
            .run(migrate_pool)
            .await
            .map_err(|e| ShakoError::Storage(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Loads one tenant's persisted instance and repositories into the read mirror, so a restart
    /// keeps them, and sets its quota usage from the stored repositories. Reads run inside the
    /// tenant's RLS context: forced RLS lets the request role see no tenant it was not given, so
    /// the caller names each tenant it serves. Returns the number of repositories loaded.
    ///
    /// Only the instance and repository rows are written to PostgreSQL today; protection rules,
    /// pull requests, webhooks, packages and the dispatch marks live in memory only.
    pub async fn hydrate_tenant(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> ShakoResult<usize> {
        let storage = |e: sqlx::Error| ShakoError::Storage(format!("hydrate failed: {e}"));
        let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
        let instance = sqlx::query(
            "SELECT host, path_prefix, max_repos, max_storage_bytes, max_push_bytes, secret_scan_enabled, created_at, updated_at
             FROM shako.instance WHERE instance_id = $1 AND org_id = $2",
        )
        .bind(instance_id.as_uuid())
        .bind(org_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        let repos = sqlx::query(
            "SELECT id, project_id, owner, name, description, default_branch, private, archived, size_bytes, next_pr_number, created_at, updated_at
             FROM shako.repo WHERE instance_id = $1 AND org_id = $2 ORDER BY created_at, id",
        )
        .bind(instance_id.as_uuid())
        .bind(org_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        tx.commit().await.map_err(storage)?;

        let unsigned = |v: i64| u64::try_from(v).unwrap_or(0);
        if let Some(r) = instance {
            self.mirror
                .upsert_instance(InstanceRecord {
                    instance_id,
                    org_id,
                    host: r.get("host"),
                    path_prefix: r.get("path_prefix"),
                    max_repos: unsigned(r.get("max_repos")),
                    max_storage_bytes: unsigned(r.get("max_storage_bytes")),
                    max_push_bytes: unsigned(r.get("max_push_bytes")),
                    secret_scan_enabled: r.get("secret_scan_enabled"),
                    created_at: r.get("created_at"),
                    updated_at: r.get("updated_at"),
                })
                .await?;
        }
        let mut git_bytes: i64 = 0;
        for r in &repos {
            let repo = RepoRecord {
                id: RepoId::from(r.get::<uuid::Uuid, _>("id")),
                instance_id,
                org_id,
                project_id: r.get("project_id"),
                owner: r.get("owner"),
                name: r.get("name"),
                description: r.get("description"),
                default_branch: r.get("default_branch"),
                private: r.get("private"),
                archived: r.get("archived"),
                size_bytes: unsigned(r.get("size_bytes")),
                next_pr_number: unsigned(r.get("next_pr_number")),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            };
            git_bytes = git_bytes.saturating_add(r.get::<i64, _>("size_bytes"));
            match self.mirror.create_repo(repo.clone()).await {
                Ok(_) => {}
                Err(ShakoError::Conflict(_)) => {
                    self.mirror.update_repo(repo).await?;
                }
                Err(e) => return Err(e),
            }
        }
        let held = self.mirror.get_quota_usage(instance_id, org_id).await?;
        let count = i64::try_from(repos.len()).unwrap_or(i64::MAX);
        self.mirror
            .adjust_quota_usage(
                instance_id,
                org_id,
                count.saturating_sub(i64::try_from(held.repo_count).unwrap_or(i64::MAX)),
                git_bytes.saturating_sub(i64::try_from(held.git_bytes).unwrap_or(i64::MAX)),
                0,
            )
            .await?;
        Ok(repos.len())
    }

    /// Begins a transaction with `SET LOCAL` tenant context (`shako.instance` and `shako.org`).
    pub async fn begin_tenant_tx(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> ShakoResult<Transaction<'_, Postgres>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ShakoError::Storage(format!("begin tx failed: {e}")))?;
        sqlx::query(
            "SELECT set_config('shako.instance', $1, true), set_config('shako.org', $2, true)",
        )
        .bind(instance_id.to_string())
        .bind(org_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| ShakoError::Storage(format!("set_config failed: {e}")))?;
        Ok(tx)
    }

    /// Emits `NOTIFY shako_outbox` for low-latency outbox wakeup across replicas (S14).
    pub async fn notify_outbox(&self, instance_id: InstanceId) -> ShakoResult<()> {
        sqlx::query("SELECT pg_notify('shako_outbox', $1)")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| ShakoError::Storage(format!("pg_notify failed: {e}")))?;
        self.outbox_notifications.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn spawn_outbox_listener(&self) -> tokio::task::JoinHandle<()> {
        let pool = self.pool.clone();
        let counter = self.outbox_notifications.clone();
        tokio::spawn(async move {
            while let Ok(mut listener) = PgListener::connect_with(&pool).await {
                if listener.listen("shako_outbox").await.is_err() {
                    break;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                while listener.recv().await.is_ok() {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }
}

impl Store for PgStore {
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let role_row = sqlx::query(
                "SELECT rolname::text AS rolname, rolbypassrls, rolsuper FROM pg_roles WHERE rolname = current_user",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ShakoError::RlsNotEnforced(format!("inspect pg_roles failed: {e}")))?;
            let bypass: bool = role_row.get("rolbypassrls");
            let superuser: bool = role_row.get("rolsuper");
            let role: String = role_row.get("rolname");

            let owned_tables: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                 JOIN pg_roles r ON r.oid = c.relowner
                 WHERE c.relkind = 'r'
                   AND r.rolname = current_user
                   AND n.nspname NOT LIKE 'pg_temp%'",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ShakoError::RlsNotEnforced(format!("inspect table owner failed: {e}")))?;

            let external_grants: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM information_schema.role_table_grants
                 WHERE grantee = current_user
                   AND table_schema NOT IN ('shako', 'pg_catalog', 'information_schema')",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ShakoError::RlsNotEnforced(format!("inspect role grants failed: {e}")))?;

            let table_rows = sqlx::query(
                "SELECT c.relname::text AS table_name,
                        c.relrowsecurity AS rls_enabled,
                        c.relforcerowsecurity AS rls_forced,
                        (SELECT COUNT(*) FROM pg_policy p WHERE p.polrelid = c.oid) AS policy_count
                 FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                 WHERE n.nspname = 'shako'
                   AND c.relkind = 'r'",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ShakoError::RlsNotEnforced(format!("inspect pg_class failed: {e}")))?;

            let statuses: Vec<TableRlsStatus> = table_rows
                .into_iter()
                .map(|r| TableRlsStatus {
                    table_name: r.get("table_name"),
                    rls_enabled: r.get("rls_enabled"),
                    rls_forced: r.get("rls_forced"),
                    policy_count: r.get("policy_count"),
                })
                .collect();

            validate_rls_catalog_rows(
                &role,
                bypass,
                superuser,
                owned_tables,
                external_grants,
                &statuses,
            )
        })
    }

    fn upsert_instance(&self, instance: InstanceRecord) -> StoreFuture<'_, InstanceRecord> {
        Box::pin(async move {
            let _write = self.write_lock.lock().await;
            let mut tx = self
                .begin_tenant_tx(instance.instance_id, instance.org_id)
                .await?;
            sqlx::query(
                "INSERT INTO shako.instance (instance_id, org_id, host, path_prefix, max_repos, max_storage_bytes, max_push_bytes, secret_scan_enabled)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (instance_id, org_id) DO UPDATE SET
                    host = EXCLUDED.host,
                    path_prefix = EXCLUDED.path_prefix,
                    max_repos = EXCLUDED.max_repos,
                    max_storage_bytes = EXCLUDED.max_storage_bytes,
                    max_push_bytes = EXCLUDED.max_push_bytes,
                    secret_scan_enabled = EXCLUDED.secret_scan_enabled,
                    updated_at = now()",
            )
            .bind(instance.instance_id.as_uuid())
            .bind(instance.org_id.as_uuid())
            .bind(&instance.host)
            .bind(&instance.path_prefix)
            .bind(instance.max_repos as i64)
            .bind(instance.max_storage_bytes as i64)
            .bind(instance.max_push_bytes as i64)
            .bind(instance.secret_scan_enabled)
            .execute(&mut *tx)
            .await
            .map_err(|e| ShakoError::Storage(e.to_string()))?;
            append_op(
                &mut tx,
                instance.instance_id,
                instance.org_id,
                &Op::UpsertInstance(instance.clone()),
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
            self.mirror.upsert_instance(instance).await
        })
    }

    fn get_instance(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<InstanceRecord>> {
        self.mirror.get_instance(instance_id, org_id)
    }

    fn get_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        self.mirror.get_quota_usage(instance_id, org_id)
    }

    fn adjust_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_delta: i64,
        git_bytes_delta: i64,
        package_bytes_delta: i64,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        self.logged(
            instance_id,
            org_id,
            Op::AdjustQuotaUsage {
                repo_delta,
                git_bytes_delta,
                package_bytes_delta,
            },
        )
    }

    fn create_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        Box::pin(async move {
            let _write = self.write_lock.lock().await;
            let mut tx = self.begin_tenant_tx(repo.instance_id, repo.org_id).await?;
            let res = sqlx::query(
                "INSERT INTO shako.repo (id, instance_id, org_id, project_id, owner, name, description, default_branch, private, archived, size_bytes, next_pr_number)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(repo.id.as_uuid())
            .bind(repo.instance_id.as_uuid())
            .bind(repo.org_id.as_uuid())
            .bind(&repo.project_id)
            .bind(&repo.owner)
            .bind(&repo.name)
            .bind(&repo.description)
            .bind(&repo.default_branch)
            .bind(repo.private)
            .bind(repo.archived)
            .bind(repo.size_bytes as i64)
            .bind(repo.next_pr_number as i64)
            .execute(&mut *tx)
            .await;
            if let Err(e) = res {
                if e.to_string().contains("duplicate key") || e.to_string().contains("23505") {
                    return Err(ShakoError::Conflict("repository already exists".into()));
                }
                return Err(ShakoError::Storage(e.to_string()));
            }
            append_op(
                &mut tx,
                repo.instance_id,
                repo.org_id,
                &Op::CreateRepo(repo.clone()),
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
            self.mirror.create_repo(repo).await
        })
    }

    fn update_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        Box::pin(async move {
            let _write = self.write_lock.lock().await;
            let mut tx = self.begin_tenant_tx(repo.instance_id, repo.org_id).await?;
            sqlx::query(
                "UPDATE shako.repo SET owner = $1, name = $2, description = $3, default_branch = $4, private = $5, archived = $6, size_bytes = $7, updated_at = now()
                 WHERE instance_id = $8 AND org_id = $9 AND id = $10",
            )
            .bind(&repo.owner)
            .bind(&repo.name)
            .bind(&repo.description)
            .bind(&repo.default_branch)
            .bind(repo.private)
            .bind(repo.archived)
            .bind(repo.size_bytes as i64)
            .bind(repo.instance_id.as_uuid())
            .bind(repo.org_id.as_uuid())
            .bind(repo.id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| ShakoError::Storage(e.to_string()))?;
            append_op(
                &mut tx,
                repo.instance_id,
                repo.org_id,
                &Op::UpdateRepo(repo.clone()),
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
            self.mirror.update_repo(repo).await
        })
    }

    fn get_repo_by_name(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        name: &str,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        self.mirror
            .get_repo_by_name(instance_id, org_id, owner, name)
    }

    fn get_repo_by_id(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        self.mirror.get_repo_by_id(instance_id, org_id, repo_id)
    }

    fn list_repos(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<RepoRecord>> {
        self.mirror.list_repos(instance_id, org_id)
    }

    fn delete_repo(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, bool> {
        Box::pin(async move {
            let _write = self.write_lock.lock().await;
            let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
            sqlx::query(
                "DELETE FROM shako.repo WHERE instance_id = $1 AND org_id = $2 AND id = $3",
            )
            .bind(instance_id.as_uuid())
            .bind(org_id.as_uuid())
            .bind(repo_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| ShakoError::Storage(e.to_string()))?;
            append_op(&mut tx, instance_id, org_id, &Op::DeleteRepo { repo_id }).await?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
            self.mirror.delete_repo(instance_id, org_id, repo_id).await
        })
    }

    fn allocate_pr_number(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, u64> {
        self.logged(instance_id, org_id, Op::AllocatePrNumber { repo_id })
    }

    fn upsert_protection_rule(
        &self,
        rec: ProtectionRuleRecord,
    ) -> StoreFuture<'_, ProtectionRuleRecord> {
        self.logged(rec.instance_id, rec.org_id, Op::UpsertProtectionRule(rec))
    }

    fn get_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, Option<ProtectionRuleRecord>> {
        self.mirror
            .get_protection_rule(instance_id, org_id, repo_id, rule_name)
    }

    fn list_protection_rules(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<ProtectionRuleRecord>> {
        self.mirror
            .list_protection_rules(instance_id, org_id, repo_id)
    }

    fn delete_protection_rule(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        rule_name: &str,
    ) -> StoreFuture<'_, bool> {
        self.logged(
            instance_id,
            org_id,
            Op::DeleteProtectionRule {
                repo_id,
                rule_name: rule_name.to_owned(),
            },
        )
    }

    fn upsert_tag_protection(
        &self,
        rec: TagProtectionRecord,
    ) -> StoreFuture<'_, TagProtectionRecord> {
        self.logged(rec.instance_id, rec.org_id, Op::UpsertTagProtection(rec))
    }

    fn list_tag_protections(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<TagProtectionRecord>> {
        self.mirror
            .list_tag_protections(instance_id, org_id, repo_id)
    }

    fn create_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        self.logged(pr.instance_id, pr.org_id, Op::CreatePullRequest(pr))
    }

    fn update_pull_request(&self, pr: PullRequestRecord) -> StoreFuture<'_, PullRequestRecord> {
        self.logged(pr.instance_id, pr.org_id, Op::UpdatePullRequest(pr))
    }

    fn get_pull_request(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        number: u64,
    ) -> StoreFuture<'_, Option<PullRequestRecord>> {
        self.mirror
            .get_pull_request(instance_id, org_id, repo_id, number)
    }

    fn list_pull_requests(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        state: Option<&str>,
    ) -> StoreFuture<'_, Vec<PullRequestRecord>> {
        self.mirror
            .list_pull_requests(instance_id, org_id, repo_id, state)
    }

    fn create_review(&self, review: ReviewRecord) -> StoreFuture<'_, ReviewRecord> {
        self.logged(review.instance_id, review.org_id, Op::CreateReview(review))
    }

    fn list_reviews(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        pr_number: Option<u64>,
    ) -> StoreFuture<'_, Vec<ReviewRecord>> {
        self.mirror
            .list_reviews(instance_id, org_id, repo_id, pr_number)
    }

    fn upsert_commit_status(
        &self,
        status: CommitStatusRecord,
    ) -> StoreFuture<'_, CommitStatusRecord> {
        self.logged(
            status.instance_id,
            status.org_id,
            Op::UpsertCommitStatus(status),
        )
    }

    fn list_commit_statuses(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        sha: &str,
    ) -> StoreFuture<'_, Vec<CommitStatusRecord>> {
        self.mirror
            .list_commit_statuses(instance_id, org_id, repo_id, sha)
    }

    fn create_webhook(&self, hook: WebhookRecord) -> StoreFuture<'_, WebhookRecord> {
        self.logged(hook.instance_id, hook.org_id, Op::CreateWebhook(hook))
    }

    fn get_webhook(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: WebhookId,
    ) -> StoreFuture<'_, Option<WebhookRecord>> {
        self.mirror.get_webhook(instance_id, org_id, webhook_id)
    }

    fn list_webhooks(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<WebhookRecord>> {
        self.mirror.list_webhooks(instance_id, org_id, repo_id)
    }

    fn record_delivery(&self, delivery: DeliveryRecord) -> StoreFuture<'_, DeliveryRecord> {
        self.logged(
            delivery.instance_id,
            delivery.org_id,
            Op::RecordDelivery(delivery),
        )
    }

    fn list_deliveries(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        webhook_id: Option<WebhookId>,
    ) -> StoreFuture<'_, Vec<DeliveryRecord>> {
        self.mirror.list_deliveries(instance_id, org_id, webhook_id)
    }

    fn upsert_push_mirror(&self, mirror: PushMirrorRecord) -> StoreFuture<'_, PushMirrorRecord> {
        self.logged(
            mirror.instance_id,
            mirror.org_id,
            Op::UpsertPushMirror(mirror),
        )
    }

    fn list_push_mirrors(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<PushMirrorRecord>> {
        self.mirror.list_push_mirrors(instance_id, org_id, repo_id)
    }

    fn delete_push_mirror(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
        remote_name: &str,
    ) -> StoreFuture<'_, bool> {
        self.logged(
            instance_id,
            org_id,
            Op::DeletePushMirror {
                repo_id,
                remote_name: remote_name.to_owned(),
            },
        )
    }

    fn upsert_package(&self, pkg: PackageRecord) -> StoreFuture<'_, PackageRecord> {
        self.logged(pkg.instance_id, pkg.org_id, Op::UpsertPackage(pkg))
    }

    fn list_packages(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageRecord>> {
        self.mirror.list_packages(instance_id, org_id)
    }

    fn upsert_package_version(
        &self,
        ver: PackageVersionRecord,
    ) -> StoreFuture<'_, PackageVersionRecord> {
        self.logged(ver.instance_id, ver.org_id, Op::UpsertPackageVersion(ver))
    }

    fn get_package_version(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_type: &str,
        package_name: &str,
        version_or_digest: &str,
    ) -> StoreFuture<'_, Option<PackageVersionRecord>> {
        self.mirror.get_package_version(
            instance_id,
            org_id,
            owner,
            package_type,
            package_name,
            version_or_digest,
        )
    }

    fn list_package_versions(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: Option<&str>,
        package_type: Option<&str>,
        package_name: Option<&str>,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>> {
        self.mirror
            .list_package_versions(instance_id, org_id, owner, package_type, package_name)
    }

    fn insert_package_file(&self, file: PackageFileRecord) -> StoreFuture<'_, PackageFileRecord> {
        self.logged(file.instance_id, file.org_id, Op::InsertPackageFile(file))
    }

    fn get_package_file(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        package_name: &str,
        version: &str,
        filename: &str,
    ) -> StoreFuture<'_, Option<PackageFileRecord>> {
        self.mirror
            .get_package_file(instance_id, org_id, owner, package_name, version, filename)
    }

    fn list_package_files(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageFileRecord>> {
        self.mirror.list_package_files(instance_id, org_id)
    }

    fn upsert_blob(&self, blob: BlobRecord) -> StoreFuture<'_, BlobRecord> {
        self.logged(blob.instance_id, blob.org_id, Op::UpsertBlob(blob))
    }

    fn get_blob(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo_name: Option<&str>,
        digest: &str,
    ) -> StoreFuture<'_, Option<BlobRecord>> {
        self.mirror
            .get_blob(instance_id, org_id, owner, repo_name, digest)
    }

    fn append_outbox(&self, event: OutboxEvent) -> StoreFuture<'_, OutboxEvent> {
        Box::pin(async move {
            let stored = {
                let _write = self.write_lock.lock().await;
                let mut tx = self
                    .begin_tenant_tx(event.instance_id, event.org_id)
                    .await?;
                sqlx::query(
                    "INSERT INTO shako.outbox (id, instance_id, org_id, repo_id, event_kind, idempotency_key, payload, dispatched)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                     ON CONFLICT (instance_id, idempotency_key) DO NOTHING",
                )
                .bind(event.id)
                .bind(event.instance_id.as_uuid())
                .bind(event.org_id.as_uuid())
                .bind(event.repo_id.map(|r| r.as_uuid()))
                .bind(&event.event_kind)
                .bind(&event.idempotency_key)
                .bind(&event.payload)
                .bind(event.dispatched)
                .execute(&mut *tx)
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
                append_op(
                    &mut tx,
                    event.instance_id,
                    event.org_id,
                    &Op::AppendOutbox(event.clone()),
                )
                .await?;
                tx.commit()
                    .await
                    .map_err(|e| ShakoError::Storage(e.to_string()))?;
                self.mirror.append_outbox(event).await?
            };
            let _ = self.notify_outbox(stored.instance_id).await;
            Ok(stored)
        })
    }

    fn list_undispatched_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        self.mirror.list_undispatched_outbox(instance_id, org_id)
    }

    fn mark_outbox_dispatched(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
    ) -> StoreFuture<'_, bool> {
        self.logged(instance_id, org_id, Op::MarkOutboxDispatched { event_id })
    }

    fn list_outbox_events(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        self.mirror.list_outbox_events(instance_id, org_id)
    }
}

/// One mutation of the store, as written to `shako.store_log`. The request path reads from the
/// in-memory mirror; [`PgStore::hydrate`] replays a tenant's log into it at start, so every
/// record kind survives a restart, not only the ones with a typed table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Op {
    UpsertInstance(InstanceRecord),
    AdjustQuotaUsage {
        repo_delta: i64,
        git_bytes_delta: i64,
        package_bytes_delta: i64,
    },
    CreateRepo(RepoRecord),
    UpdateRepo(RepoRecord),
    DeleteRepo {
        repo_id: RepoId,
    },
    AllocatePrNumber {
        repo_id: RepoId,
    },
    UpsertProtectionRule(ProtectionRuleRecord),
    DeleteProtectionRule {
        repo_id: RepoId,
        rule_name: String,
    },
    UpsertTagProtection(TagProtectionRecord),
    CreatePullRequest(PullRequestRecord),
    UpdatePullRequest(PullRequestRecord),
    CreateReview(ReviewRecord),
    UpsertCommitStatus(CommitStatusRecord),
    CreateWebhook(WebhookRecord),
    RecordDelivery(DeliveryRecord),
    UpsertPushMirror(PushMirrorRecord),
    DeletePushMirror {
        repo_id: RepoId,
        remote_name: String,
    },
    UpsertPackage(PackageRecord),
    UpsertPackageVersion(PackageVersionRecord),
    InsertPackageFile(PackageFileRecord),
    UpsertBlob(BlobRecord),
    AppendOutbox(OutboxEvent),
    MarkOutboxDispatched {
        event_id: uuid::Uuid,
    },
}

/// What an operation answers, so one apply path serves the request and the replay.
pub trait FromApplied: Sized {
    fn from_applied(value: Applied) -> ShakoResult<Self>;
}

/// The answer of [`Op::apply`].
#[derive(Debug)]
pub enum Applied {
    Instance(InstanceRecord),
    Quota(QuotaUsageRecord),
    Repo(RepoRecord),
    Bool(bool),
    Number(u64),
    ProtectionRule(ProtectionRuleRecord),
    TagProtection(TagProtectionRecord),
    PullRequest(PullRequestRecord),
    Review(ReviewRecord),
    CommitStatus(CommitStatusRecord),
    Webhook(WebhookRecord),
    Delivery(DeliveryRecord),
    PushMirror(PushMirrorRecord),
    Package(PackageRecord),
    PackageVersion(PackageVersionRecord),
    PackageFile(PackageFileRecord),
    Blob(BlobRecord),
    Outbox(OutboxEvent),
}

macro_rules! from_applied {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl FromApplied for $ty {
            fn from_applied(value: Applied) -> ShakoResult<Self> {
                match value {
                    Applied::$variant(v) => Ok(v),
                    other => Err(ShakoError::Storage(format!("unexpected store answer {other:?}"))),
                }
            }
        }
    )*};
}

from_applied!(
    InstanceRecord => Instance,
    QuotaUsageRecord => Quota,
    RepoRecord => Repo,
    bool => Bool,
    u64 => Number,
    ProtectionRuleRecord => ProtectionRule,
    TagProtectionRecord => TagProtection,
    PullRequestRecord => PullRequest,
    ReviewRecord => Review,
    CommitStatusRecord => CommitStatus,
    WebhookRecord => Webhook,
    DeliveryRecord => Delivery,
    PushMirrorRecord => PushMirror,
    PackageRecord => Package,
    PackageVersionRecord => PackageVersion,
    PackageFileRecord => PackageFile,
    BlobRecord => Blob,
    OutboxEvent => Outbox,
);

impl Op {
    /// Applies the operation to `store` for tenant `(instance_id, org_id)`.
    pub async fn apply(
        self,
        store: &MemoryStore,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> ShakoResult<Applied> {
        Ok(match self {
            Self::UpsertInstance(r) => Applied::Instance(store.upsert_instance(r).await?),
            Self::AdjustQuotaUsage {
                repo_delta,
                git_bytes_delta,
                package_bytes_delta,
            } => Applied::Quota(
                store
                    .adjust_quota_usage(
                        instance_id,
                        org_id,
                        repo_delta,
                        git_bytes_delta,
                        package_bytes_delta,
                    )
                    .await?,
            ),
            Self::CreateRepo(r) => Applied::Repo(store.create_repo(r).await?),
            Self::UpdateRepo(r) => Applied::Repo(store.update_repo(r).await?),
            Self::DeleteRepo { repo_id } => {
                Applied::Bool(store.delete_repo(instance_id, org_id, repo_id).await?)
            }
            Self::AllocatePrNumber { repo_id } => Applied::Number(
                store
                    .allocate_pr_number(instance_id, org_id, repo_id)
                    .await?,
            ),
            Self::UpsertProtectionRule(r) => {
                Applied::ProtectionRule(store.upsert_protection_rule(r).await?)
            }
            Self::DeleteProtectionRule { repo_id, rule_name } => Applied::Bool(
                store
                    .delete_protection_rule(instance_id, org_id, repo_id, &rule_name)
                    .await?,
            ),
            Self::UpsertTagProtection(r) => {
                Applied::TagProtection(store.upsert_tag_protection(r).await?)
            }
            Self::CreatePullRequest(r) => Applied::PullRequest(store.create_pull_request(r).await?),
            Self::UpdatePullRequest(r) => Applied::PullRequest(store.update_pull_request(r).await?),
            Self::CreateReview(r) => Applied::Review(store.create_review(r).await?),
            Self::UpsertCommitStatus(r) => {
                Applied::CommitStatus(store.upsert_commit_status(r).await?)
            }
            Self::CreateWebhook(r) => Applied::Webhook(store.create_webhook(r).await?),
            Self::RecordDelivery(r) => Applied::Delivery(store.record_delivery(r).await?),
            Self::UpsertPushMirror(r) => Applied::PushMirror(store.upsert_push_mirror(r).await?),
            Self::DeletePushMirror {
                repo_id,
                remote_name,
            } => Applied::Bool(
                store
                    .delete_push_mirror(instance_id, org_id, repo_id, &remote_name)
                    .await?,
            ),
            Self::UpsertPackage(r) => Applied::Package(store.upsert_package(r).await?),
            Self::UpsertPackageVersion(r) => {
                Applied::PackageVersion(store.upsert_package_version(r).await?)
            }
            Self::InsertPackageFile(r) => Applied::PackageFile(store.insert_package_file(r).await?),
            Self::UpsertBlob(r) => Applied::Blob(store.upsert_blob(r).await?),
            Self::AppendOutbox(r) => Applied::Outbox(store.append_outbox(r).await?),
            Self::MarkOutboxDispatched { event_id } => Applied::Bool(
                store
                    .mark_outbox_dispatched(instance_id, org_id, event_id)
                    .await?,
            ),
        })
    }
}

/// Appends `op` to `shako.store_log` inside a tenant transaction.
async fn append_op(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: InstanceId,
    org_id: OrgId,
    op: &Op,
) -> ShakoResult<()> {
    let doc = serde_json::to_value(op).map_err(|e| ShakoError::Storage(e.to_string()))?;
    sqlx::query("INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)")
        .bind(instance_id.as_uuid())
        .bind(org_id.as_uuid())
        .bind(doc)
        .execute(&mut **tx)
        .await
        .map_err(|e| ShakoError::Storage(format!("store log write failed: {e}")))?;
    Ok(())
}

impl PgStore {
    /// Writes `op` to the log, then applies it to the mirror. Writes are serialized, so the log
    /// order is the order the mirror saw (a replay rebuilds the same state).
    fn logged<T: FromApplied + Send + 'static>(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        op: Op,
    ) -> StoreFuture<'_, T> {
        Box::pin(async move {
            let _write = self.write_lock.lock().await;
            let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
            append_op(&mut tx, instance_id, org_id, &op).await?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(e.to_string()))?;
            T::from_applied(op.apply(&self.mirror, instance_id, org_id).await?)
        })
    }

    /// Loads tenant `(instance_id, org_id)` from `shako.store_log` into the mirror, in log order.
    /// Call once before serving. An operation the mirror refused when it was first made is
    /// refused again and skipped, as it was then. Returns the number of operations read.
    pub async fn hydrate(&self, instance_id: InstanceId, org_id: OrgId) -> ShakoResult<usize> {
        let _write = self.write_lock.lock().await;
        let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
        let rows: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT op FROM shako.store_log ORDER BY seq")
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| ShakoError::Storage(format!("store log read failed: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| ShakoError::Storage(e.to_string()))?;
        let count = rows.len();
        for doc in rows {
            let op: Op = serde_json::from_value(doc)
                .map_err(|e| ShakoError::Storage(format!("store log entry unreadable: {e}")))?;
            if let Err(e) = op.apply(&self.mirror, instance_id, org_id).await {
                tracing::debug!(error = %e, "shako store log: an operation refused again on replay");
            }
        }
        Ok(count)
    }
}
