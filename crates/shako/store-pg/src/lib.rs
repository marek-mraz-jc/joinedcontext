#![forbid(unsafe_code)]
//! `shako-store-pg`: PostgreSQL implementation of [`shako_core::Store`] on `sqlx` 0.9 with:
//! - Dedicated pool and request role `shako_app` (no `BYPASSRLS`, no `SUPERUSER`, owns 0 tables).
//! - Separate migration role `shako_migrate`.
//! - Database password loaded from a file in the secrets mount (`PgStoreConfig::from_secret_file`).
//! - Boot check (`verify_boot_rls` / `validate_rls_catalog_rows`) refusing to start if any table
//!   in schema `shako` lacks `FORCE ROW LEVEL SECURITY` or if the request role has `BYPASSRLS`.
//! - Per-transaction `SET LOCAL` via `set_config('shako.instance', $1, true)` and
//!   `set_config('shako.org', $2, true)`.
//! - No state in the process: every read is of the typed tables inside such a transaction, so
//!   every replica answers alike (T-7508).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shako_core::{
    BlobRecord, CommitStatusRecord, DeliveryRecord, InstanceId, InstanceRecord, OrgId, OutboxEvent,
    PackageFileRecord, PackageRecord, PackageVersionRecord, ProtectionRuleRecord,
    PullRequestRecord, PushMirrorRecord, QuotaUsageRecord, RepoId, RepoRecord, ReviewRecord,
    SecretStore, ShakoError, ShakoResult, Store, StoreFuture, TagProtectionRecord, WebhookId,
    WebhookRecord,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

mod projection;
mod tables;

pub const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");
pub const MIGRATION_0002_SQL: &str = include_str!("../migrations/0002_store_log.sql");
pub const MIGRATION_0003_SQL: &str = include_str!("../migrations/0003_secrets.sql");
/// In a ledger of its own, which a previous release never reads (T-7508).
pub const PROJECTION_0001_SQL: &str = include_str!("../migrations/projection/0001_projection.sql");

/// How long a claimed event is this process's to send; after it another may claim it again, so
/// an event is sent once, or again when its sender died or took longer.
const OUTBOX_LEASE: Duration = Duration::from_secs(300);

/// All 17 required tables in schema `shako` that MUST have `relrowsecurity` and `relforcerowsecurity` (S2).
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
    "projection",
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

/// PostgreSQL store with private pool and forced RLS boot check (S2, S14).
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    /// The tenants whose log this process saw projected; never their records.
    ready: Arc<std::sync::Mutex<HashSet<(InstanceId, OrgId)>>>,
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
            ready: Arc::default(),
        };
        store.verify_rls_enforced().await?;
        Ok(store)
    }

    /// Creates a `PgStore` from an existing pool without running the boot check yet (for tests).
    #[must_use]
    pub fn from_pool_unchecked(pool: PgPool) -> Self {
        Self {
            pool,
            ready: Arc::default(),
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
            migration(3, "secrets", MIGRATION_0003_SQL),
        ]);
        migrator.create_schema("shako_migrations");
        migrator.dangerous_set_table_name("shako_migrations._sqlx_migrations");
        migrator
            .run(migrate_pool)
            .await
            .map_err(|e| ShakoError::Storage(format!("migration failed: {e}")))?;
        let mut projection =
            Migrator::with_migrations(vec![migration(1, "projection", PROJECTION_0001_SQL)]);
        projection.create_schema("shako_projection_migrations");
        projection.dangerous_set_table_name("shako_projection_migrations._sqlx_migrations");
        projection
            .run(migrate_pool)
            .await
            .map_err(|e| ShakoError::Storage(format!("projection migration failed: {e}")))?;
        Ok(())
    }

    /// Brings tenant `(instance_id, org_id)`'s typed tables up to `shako.store_log`, which is
    /// work once: after an update from, or a rollback to, a release that wrote only the log.
    /// Serving does it by itself; a host calls it at start so that a log it cannot read stops the
    /// start. Never run this release beside such a writer (replace it, do not roll).
    /// Returns the number of operations read.
    pub async fn hydrate(&self, instance_id: InstanceId, org_id: OrgId) -> ShakoResult<usize> {
        let tenant = (instance_id, org_id);
        let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
        projection::lock(&mut tx, tenant).await?;
        let count = projection::catch_up(&mut tx, tenant).await?;
        tx.commit().await.map_err(tables::failed)?;
        self.ready_tenants().insert(tenant);
        Ok(count)
    }

    fn ready_tenants(&self) -> std::sync::MutexGuard<'_, HashSet<(InstanceId, OrgId)>> {
        // a holder that panicked left the set whole: an insert is one step
        self.ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A transaction to read `tenant` in: its tables hold its log and its RLS context is set.
    async fn read_tx(&self, tenant: (InstanceId, OrgId)) -> ShakoResult<Transaction<'_, Postgres>> {
        let ready = self.ready_tenants().contains(&tenant);
        if !ready {
            self.hydrate(tenant.0, tenant.1).await?;
        }
        self.begin_tenant_tx(tenant.0, tenant.1).await
    }

    async fn rows<T: DeserializeOwned>(
        &self,
        tenant: (InstanceId, OrgId),
        table: &tables::Table,
        rest: &'static str,
        binds: &[Option<String>],
    ) -> ShakoResult<Vec<T>> {
        let mut tx = self.read_tx(tenant).await?;
        let rows = tables::select(&mut tx, tenant, table, rest, binds).await?;
        tx.commit().await.map_err(tables::failed)?;
        Ok(rows)
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
}

/// The hook and mirror secrets in `shako.secret` (S6-09, T-7280), sealed with the installation's
/// KEK (`kaisatsu_crypto::Kek`, under a subkey of its own): a restart and every replica read what
/// one wrote, the table holds no plaintext, and forced RLS keeps each Organization to its rows.
pub struct PgSecretStore {
    pool: PgPool,
    instance_id: InstanceId,
    seal: kaisatsu_crypto::Kek,
}

impl PgSecretStore {
    /// `kek` is the installation's key-encryption key; the secrets are sealed under its subkey
    /// `shako-secrets`, never under the key kaisatsu seals its own rows with.
    pub fn new(
        store: &PgStore,
        instance_id: InstanceId,
        kek: &kaisatsu_crypto::Kek,
    ) -> ShakoResult<Self> {
        let seal = kaisatsu_crypto::Kek::from_bytes(kek.derive_subkey("shako-secrets"));
        Ok(Self {
            pool: store.pool().clone(),
            instance_id,
            seal,
        })
    }

    /// The associated data: a sealed value opens only for its tenant and reference.
    fn aad(&self, org_id: OrgId, secret_ref: &str) -> Vec<u8> {
        format!("shako-secret\0{}\0{org_id}\0{secret_ref}", self.instance_id).into_bytes()
    }

    async fn tenant_tx(&self, org_id: OrgId) -> ShakoResult<Transaction<'_, Postgres>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ShakoError::Storage(format!("begin tx failed: {e}")))?;
        sqlx::query(
            "SELECT set_config('shako.instance', $1, true), set_config('shako.org', $2, true)",
        )
        .bind(self.instance_id.to_string())
        .bind(org_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| ShakoError::Storage(format!("set_config failed: {e}")))?;
        Ok(tx)
    }
}

impl SecretStore for PgSecretStore {
    fn get_secret<'a>(
        &'a self,
        org_id: OrgId,
        secret_ref: &'a str,
    ) -> StoreFuture<'a, Option<String>> {
        Box::pin(async move {
            let mut tx = self.tenant_tx(org_id).await?;
            let sealed: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT sealed FROM shako.secret WHERE instance_id = $1 AND org_id = $2 AND secret_ref = $3",
            )
            .bind(self.instance_id.as_uuid())
            .bind(org_id.as_uuid())
            .bind(secret_ref)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| ShakoError::Storage(format!("secret read failed: {e}")))?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(format!("secret read failed: {e}")))?;
            let Some(sealed) = sealed else {
                return Ok(None);
            };
            // The reference is never in the error: it names a repository, not the value, but a
            // failing open says only that the key does not fit.
            let plain = Zeroizing::new(
                self.seal
                    .decrypt(&self.aad(org_id, secret_ref), &sealed)
                    .map_err(|_| {
                        ShakoError::Storage(
                            "a stored secret does not open with this installation's key".into(),
                        )
                    })?,
            );
            String::from_utf8(plain.to_vec())
                .map(Some)
                .map_err(|_| ShakoError::Storage("a stored secret is not text".into()))
        })
    }

    fn put_secret<'a>(
        &'a self,
        org_id: OrgId,
        secret_ref: &'a str,
        value: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let sealed = self
                .seal
                .encrypt(&self.aad(org_id, secret_ref), value.as_bytes())
                .map_err(|_| ShakoError::Storage("the secret could not be sealed".into()))?;
            let mut tx = self.tenant_tx(org_id).await?;
            sqlx::query(
                "INSERT INTO shako.secret (instance_id, org_id, secret_ref, sealed) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (instance_id, org_id, secret_ref) DO UPDATE SET sealed = EXCLUDED.sealed, updated_at = now()",
            )
            .bind(self.instance_id.as_uuid())
            .bind(org_id.as_uuid())
            .bind(secret_ref)
            .bind(sealed)
            .execute(&mut *tx)
            .await
            .map_err(|e| ShakoError::Storage(format!("secret write failed: {e}")))?;
            tx.commit()
                .await
                .map_err(|e| ShakoError::Storage(format!("secret write failed: {e}")))
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
        self.logged(
            instance.instance_id,
            instance.org_id,
            Op::UpsertInstance(instance),
        )
    }

    fn get_instance(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<InstanceRecord>> {
        Box::pin(async move {
            Ok(self
                .rows((instance_id, org_id), &tables::INSTANCE, "", &[])
                .await?
                .pop())
        })
    }

    fn get_quota_usage(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, QuotaUsageRecord> {
        Box::pin(async move {
            let held = self
                .rows((instance_id, org_id), &tables::QUOTA_USAGE, "", &[])
                .await?
                .pop();
            Ok(held.unwrap_or_else(|| QuotaUsageRecord {
                instance_id,
                org_id,
                repo_count: 0,
                git_bytes: 0,
                package_bytes: 0,
                updated_at: jc_time::TimestampTz::now(),
            }))
        })
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
        self.logged(repo.instance_id, repo.org_id, Op::CreateRepo(repo))
    }

    fn update_repo(&self, repo: RepoRecord) -> StoreFuture<'_, RepoRecord> {
        self.logged(repo.instance_id, repo.org_id, Op::UpdateRepo(repo))
    }

    fn get_repo_by_name(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        name: &str,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        let (owner, name) = (owner.to_owned(), name.to_owned());
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::REPO,
                    "AND t.owner = $3 AND t.name = $4",
                    &[Some(owner), Some(name)],
                )
                .await?
                .pop())
        })
    }

    fn get_repo_by_id(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Option<RepoRecord>> {
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::REPO,
                    "AND t.id = $3::uuid",
                    &[Some(repo_id.to_string())],
                )
                .await?
                .pop())
        })
    }

    fn list_repos(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<RepoRecord>> {
        Box::pin(async move {
            self.rows((instance_id, org_id), &tables::REPO, "ORDER BY t.id", &[])
                .await
        })
    }

    fn delete_repo(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, bool> {
        self.logged(instance_id, org_id, Op::DeleteRepo { repo_id })
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
        let rule_name = rule_name.to_owned();
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::PROTECTION_RULE,
                    "AND t.repo_id = $3::uuid AND t.kind = 'branch' AND t.rule_name = $4",
                    &[Some(repo_id.to_string()), Some(rule_name)],
                )
                .await?
                .pop())
        })
    }

    fn list_protection_rules(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: RepoId,
    ) -> StoreFuture<'_, Vec<ProtectionRuleRecord>> {
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::PROTECTION_RULE,
                "AND t.repo_id = $3::uuid AND t.kind = 'branch' ORDER BY t.rule_name",
                &[Some(repo_id.to_string())],
            )
            .await
        })
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
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::PROTECTION_RULE,
                "AND t.repo_id = $3::uuid AND t.kind = 'tag' ORDER BY t.rule_name",
                &[Some(repo_id.to_string())],
            )
            .await
        })
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
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::PULL_REQUEST,
                    "AND t.repo_id = $3::uuid AND t.number = $4::bigint",
                    &[Some(repo_id.to_string()), Some(number.to_string())],
                )
                .await?
                .pop())
        })
    }

    fn list_pull_requests(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
        state: Option<&str>,
    ) -> StoreFuture<'_, Vec<PullRequestRecord>> {
        let state = state.filter(|s| *s != "all").map(str::to_owned);
        Box::pin(async move {
            self
                .rows(
                    (instance_id, org_id),
                    &tables::PULL_REQUEST,
                    "AND ($3::uuid IS NULL OR t.repo_id = $3::uuid) AND ($4::text IS NULL OR lower(t.state) = lower($4)) ORDER BY t.updated_at DESC, t.number DESC, t.repo_id",
                    &[repo_id.map(|id| id.to_string()), state],
                )
                .await
        })
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
        Box::pin(async move {
            self
                .rows(
                    (instance_id, org_id),
                    &tables::REVIEW,
                    "AND ($3::uuid IS NULL OR t.repo_id = $3::uuid) AND ($4::bigint IS NULL OR t.pr_number = $4::bigint) ORDER BY t.position, t.created_at, t.id",
                    &[repo_id.map(|id| id.to_string()), pr_number.map(|n| n.to_string())],
                )
                .await
        })
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
        let sha = sha.to_owned();
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::COMMIT_STATUS,
                "AND t.repo_id = $3::uuid AND t.sha = $4 ORDER BY t.context",
                &[Some(repo_id.to_string()), Some(sha)],
            )
            .await
        })
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
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::WEBHOOK,
                    "AND t.id = $3::uuid",
                    &[Some(webhook_id.to_string())],
                )
                .await?
                .pop())
        })
    }

    fn list_webhooks(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        repo_id: Option<RepoId>,
    ) -> StoreFuture<'_, Vec<WebhookRecord>> {
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::WEBHOOK,
                "AND ($3::uuid IS NULL OR t.repo_id = $3::uuid) ORDER BY t.id",
                &[repo_id.map(|id| id.to_string())],
            )
            .await
        })
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
        Box::pin(async move {
            self
                .rows(
                    (instance_id, org_id),
                    &tables::DELIVERY,
                    "AND ($3::uuid IS NULL OR t.webhook_id = $3::uuid) ORDER BY t.position, t.delivered_at, t.id",
                    &[webhook_id.map(|id| id.to_string())],
                )
                .await
        })
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
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::PUSH_MIRROR,
                "AND ($3::uuid IS NULL OR t.repo_id = $3::uuid) ORDER BY t.repo_id, t.remote_name",
                &[repo_id.map(|id| id.to_string())],
            )
            .await
        })
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
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::PACKAGE,
                "ORDER BY t.owner, t.package_type, t.name",
                &[],
            )
            .await
        })
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
        let (owner, package_type) = (owner.to_owned(), package_type.to_owned());
        let (package_name, wanted) = (package_name.to_owned(), version_or_digest.to_owned());
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::PACKAGE_VERSION,
                    "AND t.owner = $3 AND t.package_type = $4 AND t.package_name = $5 AND (t.version = $6 OR t.digest = $6) ORDER BY (t.version = $6), t.version DESC",
                    &[Some(owner), Some(package_type), Some(package_name), Some(wanted)],
                )
                .await?
                .pop())
        })
    }

    fn list_package_versions(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: Option<&str>,
        package_type: Option<&str>,
        package_name: Option<&str>,
    ) -> StoreFuture<'_, Vec<PackageVersionRecord>> {
        let owner = owner.map(str::to_owned);
        let package_type = package_type.map(str::to_owned);
        let package_name = package_name.map(str::to_owned);
        Box::pin(async move {
            self
                .rows(
                    (instance_id, org_id),
                    &tables::PACKAGE_VERSION,
                    "AND ($3::text IS NULL OR t.owner = $3) AND ($4::text IS NULL OR t.package_type = $4) AND ($5::text IS NULL OR t.package_name = $5) ORDER BY t.owner, t.package_type, t.package_name, t.version",
                    &[owner, package_type, package_name],
                )
                .await
        })
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
        let (owner, package_name) = (owner.to_owned(), package_name.to_owned());
        let (version, filename) = (version.to_owned(), filename.to_owned());
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::PACKAGE_FILE,
                    "AND t.owner = $3 AND t.package_name = $4 AND t.version = $5 AND t.filename = $6",
                    &[Some(owner), Some(package_name), Some(version), Some(filename)],
                )
                .await?
                .pop())
        })
    }

    fn list_package_files(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<PackageFileRecord>> {
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::PACKAGE_FILE,
                "ORDER BY t.owner, t.package_name, t.version, t.filename",
                &[],
            )
            .await
        })
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
        // Without a repository: a mount within the SAME owner only (Contract S13, Threat T13).
        let (owner, digest) = (owner.to_owned(), digest.to_owned());
        let repo_name = repo_name.map(str::to_owned);
        Box::pin(async move {
            Ok(self
                .rows(
                    (instance_id, org_id),
                    &tables::BLOB,
                    "AND t.owner = $3 AND ($4::text IS NULL OR t.repo_name = $4) AND t.digest = $5 ORDER BY t.repo_name DESC",
                    &[Some(owner), repo_name, Some(digest)],
                )
                .await?
                .pop())
        })
    }

    fn append_outbox(&self, event: OutboxEvent) -> StoreFuture<'_, OutboxEvent> {
        self.logged(event.instance_id, event.org_id, Op::AppendOutbox(event))
    }

    fn list_undispatched_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::OUTBOX,
                "AND NOT t.dispatched ORDER BY t.position, t.created_at, t.id",
                &[],
            )
            .await
        })
    }

    fn mark_outbox_dispatched(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        event_id: uuid::Uuid,
    ) -> StoreFuture<'_, bool> {
        self.logged(instance_id, org_id, Op::MarkOutboxDispatched { event_id })
    }

    fn claim_outbox(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Option<OutboxEvent>> {
        Box::pin(async move {
            let mut tx = self.read_tx((instance_id, org_id)).await?;
            // The oldest alone, so events keep their order. A second claimer waits on the row,
            // then finds it claimed and takes nothing.
            let row: Option<serde_json::Value> = sqlx::query_scalar(
                "WITH head AS (
                     SELECT id FROM shako.outbox
                     WHERE instance_id = $1 AND org_id = $2 AND NOT dispatched
                     ORDER BY position, created_at, id LIMIT 1)
                 UPDATE shako.outbox t SET claimed_until = now() + make_interval(secs => $3)
                 FROM head
                 WHERE t.id = head.id AND t.instance_id = $1 AND t.org_id = $2 AND NOT t.dispatched
                   AND (t.claimed_until IS NULL OR t.claimed_until <= now())
                 RETURNING to_jsonb(t)",
            )
            .bind(instance_id.as_uuid())
            .bind(org_id.as_uuid())
            .bind(OUTBOX_LEASE.as_secs_f64())
            .fetch_optional(&mut *tx)
            .await
            .map_err(tables::failed)?;
            tx.commit().await.map_err(tables::failed)?;
            row.map(|row| {
                serde_json::from_value(tables::to_record(&tables::OUTBOX, row)?)
                    .map_err(|e| ShakoError::Storage(e.to_string()))
            })
            .transpose()
        })
    }

    fn list_outbox_events(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
    ) -> StoreFuture<'_, Vec<OutboxEvent>> {
        Box::pin(async move {
            self.rows(
                (instance_id, org_id),
                &tables::OUTBOX,
                "ORDER BY t.position, t.created_at, t.id",
                &[],
            )
            .await
        })
    }
}

/// One mutation of the store, as written to `shako.store_log` beside its rows in the typed
/// tables: what a previous release replays after a rollback, and what [`PgStore::hydrate`]
/// projects of what that release wrote.
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
    CreateWebhook(#[serde(with = "hidden::webhook")] WebhookRecord),
    RecordDelivery(DeliveryRecord),
    UpsertPushMirror(#[serde(with = "hidden::push_mirror")] PushMirrorRecord),
    DeletePushMirror {
        repo_id: RepoId,
        remote_name: String,
    },
    UpsertPackage(PackageRecord),
    UpsertPackageVersion(#[serde(with = "hidden::package_version")] PackageVersionRecord),
    InsertPackageFile(PackageFileRecord),
    UpsertBlob(BlobRecord),
    AppendOutbox(OutboxEvent),
    MarkOutboxDispatched {
        event_id: uuid::Uuid,
    },
}

/// The fields a record keeps out of every API answer (`skip_serializing` in shako-core) are state
/// all the same: the store log writes them beside the rest, or a replay loses them (a package
/// version without `manifestBytes` stopped the store from starting).
pub(crate) mod hidden {
    use serde::{Serialize, Serializer, ser::Error as _};
    use serde_json::{Value, json};

    fn with<S: Serializer>(
        record: &impl Serialize,
        fields: Value,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let mut doc = serde_json::to_value(record).map_err(S::Error::custom)?;
        if let (Some(doc), Value::Object(fields)) = (doc.as_object_mut(), fields) {
            doc.extend(fields);
        }
        doc.serialize(s)
    }

    macro_rules! hidden {
        ($module:ident, $ty:ty, |$r:ident| $fields:expr) => {
            pub mod $module {
                use super::*;
                pub fn serialize<S: Serializer>($r: &$ty, s: S) -> Result<S::Ok, S::Error> {
                    with($r, $fields, s)
                }
                // Deserialize reads every field, the hidden ones included.
                pub fn deserialize<'de, D: serde::Deserializer<'de>>(
                    d: D,
                ) -> Result<$ty, D::Error> {
                    serde::Deserialize::deserialize(d)
                }
            }
        };
    }

    hidden!(
        package_version,
        shako_core::PackageVersionRecord,
        |r| json!({"manifestBytes": r.manifest_bytes})
    );
    hidden!(
        webhook,
        shako_core::WebhookRecord,
        |r| json!({"secretRef": r.secret_ref,
        "previousSecretRef": r.previous_secret_ref})
    );
    hidden!(
        push_mirror,
        shako_core::PushMirrorRecord,
        |r| json!({"credentialSecretRef": r.credential_secret_ref})
    );
}

impl PgStore {
    /// Logs `op` and applies it to the typed tables in one transaction under the tenant's lock.
    /// What is refused leaves neither an entry nor a row.
    fn logged<T: DeserializeOwned + Send + 'static>(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        op: Op,
    ) -> StoreFuture<'_, T> {
        Box::pin(async move {
            let tenant = (instance_id, org_id);
            let mut tx = self.begin_tenant_tx(instance_id, org_id).await?;
            projection::lock(&mut tx, tenant).await?;
            projection::catch_up(&mut tx, tenant).await?;
            let at = projection::append(&mut tx, tenant, &op).await?;
            let answer = projection::apply(&mut tx, tenant, op, &at).await?;
            projection::projected(&mut tx, tenant, at.seq()).await?;
            let answer = serde_json::from_value(answer)
                .map_err(|e| ShakoError::Storage(format!("unexpected store answer: {e}")))?;
            tx.commit().await.map_err(tables::failed)?;
            self.ready_tenants().insert(tenant);
            Ok(answer)
        })
    }
}

#[cfg(test)]
mod store_log_tests {
    use super::Op;
    use serde_json::json;

    fn replayed(op: &Op) -> Op {
        let logged = serde_json::to_value(op).expect("log entry");
        serde_json::from_value(logged).expect("a log entry reads back")
    }

    // The fields no API answer shows (`skip_serializing`) are still state: a replay without
    // them refused to start (`missing field manifestBytes`) or served an empty OCI manifest.
    #[test]
    fn a_replay_keeps_the_fields_the_api_never_shows() {
        let ids = json!({"instanceId": "0190a000-0000-7000-8000-000000000001",
                         "orgId": "0190a000-0000-7000-8000-000000000002"});
        let mut ver = json!({"id": "0190a000-0000-7000-8000-000000000003",
            "packageId": "0190a000-0000-7000-8000-000000000004", "owner": "jc-apps",
            "packageType": "container", "packageName": "app-x", "version": "v1",
            "digest": "sha256:00", "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "manifestBytes": [123, 125], "subjectDigest": null, "artifactType": null,
            "sizeBytes": 2, "signed": false, "createdAt": "2026-11-09T00:00:00Z"});
        let mut hook = json!({"id": "0190a000-0000-7000-8000-000000000005",
            "repoId": "0190a000-0000-7000-8000-000000000006", "url": "https://hook.example/",
            "events": ["push"], "active": true, "secretRef": "hooks/a", "previousSecretRef": "hooks/b",
            "format": "shako", "createdAt": "2026-11-09T00:00:00Z"});
        let mut mirror = json!({"id": "0190a000-0000-7000-8000-000000000007",
            "repoId": "0190a000-0000-7000-8000-000000000006", "remoteName": "up",
            "remoteAddress": "https://git.example/x.git", "credentialSecretRef": "mirrors/up",
            "interval": "8h", "syncOnCommit": false, "lastSync": null, "lastError": "",
            "createdAt": "2026-11-09T00:00:00Z"});
        for v in [&mut ver, &mut hook, &mut mirror] {
            v.as_object_mut()
                .expect("object")
                .extend(ids.as_object().expect("ids").clone());
        }
        let ops = [
            Op::UpsertPackageVersion(serde_json::from_value(ver).expect("version")),
            Op::CreateWebhook(serde_json::from_value(hook).expect("hook")),
            Op::UpsertPushMirror(serde_json::from_value(mirror).expect("mirror")),
        ];
        for op in &ops {
            assert_eq!(format!("{:?}", replayed(op)), format!("{op:?}"));
        }
        // A version logged before the bytes were kept still reads, so the store starts.
        let mut old = serde_json::to_value(&ops[0]).expect("log entry");
        old["args"]
            .as_object_mut()
            .expect("args")
            .remove("manifestBytes");
        assert!(serde_json::from_value::<Op>(old).is_ok());
    }
}
