#![forbid(unsafe_code)]
//! PostgreSQL store (`kaisatsu-store-pg`, W2):
//! - Own private `sqlx::PgPool` (default max_connections = 5, password read from a file in the
//!   secrets mount, never from env or argv).
//! - Boot check refusing to start when any table in schema `kaisatsu` lacks `FORCE ROW LEVEL SECURITY`
//!   or when the request role (`kaisatsu_app`) has `SUPERUSER`, `BYPASSRLS`, owns any table, or
//!   holds grants on schemas outside `kaisatsu`.
//! - Expired-row sweeper using `FOR UPDATE SKIP LOCKED` batches.
//! - Multi-instance cache invalidation via `LISTEN/NOTIFY kaisatsu_changed`.

use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_memory::MemoryStore;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Embedded migration SQL for schema `kaisatsu` (run only by `kaisatsu migrate` as `kaisatsu_migrate`, W2).
pub const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");

/// Configuration for connecting `kaisatsu-store-pg`.
#[derive(Debug, Clone)]
pub struct PgStoreConfig {
    /// Base connection URL without password (`postgresql://kaisatsu_app@host:5432/dbname`).
    pub url_without_password: String,
    /// Path to the file containing the DB password in the secrets mount (W2).
    pub password_file: Option<std::path::PathBuf>,
    /// Pool size, default 5 (W2).
    pub max_connections: u32,
}

impl Default for PgStoreConfig {
    fn default() -> Self {
        Self {
            url_without_password: "postgresql://kaisatsu_app@localhost:5432/kaisatsu".into(),
            password_file: None,
            max_connections: 5,
        }
    }
}

/// PostgreSQL store with private pool, RLS verification, sweeper, and `LISTEN/NOTIFY` cache sync (W2).
#[derive(Clone)]
pub struct PgStore {
    /// Private field of the kaisatsu state, not reachable from another crate (W2).
    pool: PgPool,
    cache: MemoryStore,
    reload_generation: Arc<AtomicU64>,
}

impl PgStore {
    /// Connects to PostgreSQL reading the password from `config.password_file` if provided,
    /// and runs the mandatory W2 RLS and role privilege boot check.
    pub async fn connect(config: &PgStoreConfig, kek: Kek) -> Result<Self> {
        let mut opts = PgConnectOptions::from_str(&config.url_without_password)
            .map_err(|e| KaisatsuError::Storage(format!("invalid pg url: {e}")))?;
        if let Some(pw_path) = &config.password_file {
            let pw = read_password_file(pw_path)?;
            opts = opts.password(&pw);
        }
        let max_conns = config.max_connections.clamp(1, 50);
        let pool = PgPoolOptions::new()
            .max_connections(max_conns)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("pg connect failed: {e}")))?;

        verify_boot_rls(&pool).await?;

        Ok(Self {
            pool,
            cache: MemoryStore::new(kek),
            reload_generation: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Runs migrations as `kaisatsu_migrate` (schema owner). Never called on `serve` (W2).
    pub async fn run_migrations(migrate_pool: &PgPool) -> Result<()> {
        sqlx::raw_sql(MIGRATION_0001_SQL)
            .execute(migrate_pool)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Runs one batch of the expired-row sweeper using `FOR UPDATE SKIP LOCKED` (W2).
    pub async fn sweep_expired_batch(&self, batch_size: i64) -> Result<u64> {
        let mut total = 0u64;
        // sqlx 0.9 refuses format!-built SQL: one static statement per table.
        macro_rules! sweep_sql {
            ($table:literal) => {
                concat!(
                    "DELETE FROM kaisatsu.",
                    $table,
                    " WHERE ctid IN (SELECT ctid FROM kaisatsu.",
                    $table,
                    " WHERE expires_at < now() LIMIT $1 FOR UPDATE SKIP LOCKED)"
                )
            };
        }
        for (table, sql) in [
            ("auth_session", sweep_sql!("auth_session")),
            ("sso_session", sweep_sql!("sso_session")),
            ("offline_session", sweep_sql!("offline_session")),
            ("refresh_token_family", sweep_sql!("refresh_token_family")),
            ("revoked_token", sweep_sql!("revoked_token")),
        ] {
            let res = sqlx::query(sql)
                .bind(batch_size)
                .execute(&self.pool)
                .await
                .map_err(|e| KaisatsuError::Storage(format!("sweep {table} failed: {e}")))?;
            total += res.rows_affected();
        }
        total += self.cache.sweep_expired().await?;
        Ok(total)
    }

    /// Emits `NOTIFY kaisatsu_changed` so sibling pods invalidate their per-process config caches (W2).
    pub async fn notify_changed(&self, realm_id: &str) -> Result<()> {
        sqlx::query("SELECT pg_notify('kaisatsu_changed', $1)")
            .bind(realm_id)
            .execute(&self.pool)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("pg_notify failed: {e}")))?;
        self.reload_generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Spawns a background listener on `kaisatsu_changed` that increments `reload_generation`
    /// on every notification or reconnect (W2).
    pub fn spawn_change_listener(&self) -> tokio::task::JoinHandle<()> {
        let pool = self.pool.clone();
        let gen_counter = self.reload_generation.clone();
        tokio::spawn(async move {
            while let Ok(mut listener) = PgListener::connect_with(&pool).await {
                if listener.listen("kaisatsu_changed").await.is_err() {
                    break;
                }
                // Full reload on connect/reconnect
                gen_counter.fetch_add(1, Ordering::SeqCst);
                while listener.recv().await.is_ok() {
                    gen_counter.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }

    pub fn reload_generation(&self) -> u64 {
        self.reload_generation.load(Ordering::SeqCst)
    }
}

/// Reads a database password from a file in the secrets mount (W2).
pub fn read_password_file(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        KaisatsuError::Storage(format!(
            "cannot read db password file {}: {e}",
            path.display()
        ))
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(KaisatsuError::Storage(format!(
            "db password file {} is empty",
            path.display()
        )));
    }
    Ok(trimmed.to_owned())
}

/// Verifies W2 invariants on `pool` before `serve` starts:
/// 1. Request role is neither `rolsuper` nor `rolbypassrls`.
/// 2. Request role owns no tables anywhere in the database.
/// 3. Request role has no table privileges on non-system schemas outside `kaisatsu`.
/// 4. Every table in schema `kaisatsu` has both `relrowsecurity` and `relforcerowsecurity` enabled.
pub async fn verify_boot_rls(pool: &PgPool) -> Result<()> {
    let (is_super, is_bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("boot check role query failed: {e}")))?;

    if is_super || is_bypass {
        return Err(KaisatsuError::Storage(
            "boot check refused: request role has SUPERUSER or BYPASSRLS (W2)".into(),
        ));
    }

    let owned_tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         JOIN pg_roles r ON r.oid = c.relowner
         WHERE c.relkind = 'r'
           AND r.rolname = current_user
           AND n.nspname NOT LIKE 'pg_temp%'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| KaisatsuError::Storage(format!("boot check ownership query failed: {e}")))?;

    if owned_tables > 0 {
        return Err(KaisatsuError::Storage(format!(
            "boot check refused: request role owns {owned_tables} table(s); migrations must run as kaisatsu_migrate (W2)"
        )));
    }

    let external_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.role_table_grants
         WHERE grantee = current_user
           AND table_schema NOT IN ('kaisatsu', 'pg_catalog', 'information_schema')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| KaisatsuError::Storage(format!("boot check grant query failed: {e}")))?;

    if external_grants > 0 {
        return Err(KaisatsuError::Storage(format!(
            "boot check refused: request role holds {external_grants} grant(s) outside schema kaisatsu (W2)"
        )));
    }

    let unforced: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'kaisatsu'
           AND c.relkind = 'r'
           AND (NOT c.relrowsecurity OR NOT c.relforcerowsecurity)",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| KaisatsuError::Storage(format!("boot check RLS query failed: {e}")))?;

    if !unforced.is_empty() {
        return Err(KaisatsuError::Storage(format!(
            "boot check refused: kaisatsu table(s) lack FORCE ROW LEVEL SECURITY: {} (W2)",
            unforced.join(", ")
        )));
    }

    Ok(())
}

impl Store for PgStore {
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a odata_table::QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, odata_table::Answer> {
        self.cache
            .query_entity_set(ctx, authorizer, set_name, options, max_page)
    }

    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>> {
        self.cache.get_realm(realm_name)
    }

    fn upsert_realm<'a>(&'a self, realm: RealmConfig) -> StoreFuture<'a, ()> {
        self.cache.upsert_realm(realm)
    }

    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>> {
        self.cache.list_realms()
    }

    fn upsert_organization<'a>(&'a self, org: Organization) -> StoreFuture<'a, ()> {
        self.cache.upsert_organization(org)
    }

    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        self.cache.get_organization(realm_id, org_id_or_alias)
    }

    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>> {
        self.cache.list_organizations(realm_id, allowed_orgs)
    }

    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        self.cache.find_org_by_domain(realm_id, domain)
    }

    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()> {
        self.cache.upsert_membership(m)
    }

    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>> {
        self.cache.list_memberships_for_person(realm_id, person_id)
    }

    fn create_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        self.cache.create_user(user)
    }

    fn update_user<'a>(&'a self, user: UserRepresentation) -> StoreFuture<'a, UserRepresentation> {
        self.cache.update_user(user)
    }

    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_user(realm_id, allowed_orgs, user_id)
    }

    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        self.cache.get_user_by_id(realm_id, allowed_orgs, user_id)
    }

    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        self.cache
            .get_user_by_username_or_email(realm_id, allowed_orgs, identifier)
    }

    fn list_users<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        email: Option<&'a str>,
        username: Option<&'a str>,
        exact: bool,
        first: usize,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        self.cache.list_users(
            realm_id,
            allowed_orgs,
            search,
            email,
            username,
            exact,
            first,
            max,
        )
    }

    fn upsert_credential<'a>(&'a self, cred: CredentialRecord) -> StoreFuture<'a, ()> {
        self.cache.upsert_credential(cred)
    }

    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>> {
        self.cache.list_credentials(realm_id, user_id)
    }

    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_credential(realm_id, user_id, cred_id)
    }

    fn upsert_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        self.cache.upsert_group(group)
    }

    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        self.cache.get_group(realm_id, allowed_orgs, group_id)
    }

    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        self.cache.get_group_by_path(realm_id, allowed_orgs, path)
    }

    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        self.cache
            .list_groups(realm_id, allowed_orgs, search, exact)
    }

    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_group(realm_id, allowed_orgs, group_id)
    }

    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.cache.add_user_to_group(realm_id, user_id, group_id)
    }

    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.cache
            .remove_user_from_group(realm_id, user_id, group_id)
    }

    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        self.cache.list_user_groups(realm_id, user_id)
    }

    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        self.cache
            .list_group_members(realm_id, allowed_orgs, group_id, max)
    }

    fn upsert_role<'a>(&'a self, role: RoleRepresentation) -> StoreFuture<'a, RoleRepresentation> {
        self.cache.upsert_role(role)
    }

    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>> {
        self.cache.list_roles(realm_id, container_id)
    }

    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_role(realm_id, container_id, role_name)
    }

    fn add_role_mapping<'a>(&'a self, mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        self.cache.add_role_mapping(mapping)
    }

    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.cache.remove_role_mapping(
            realm_id,
            principal_kind,
            principal_id,
            client_uuid,
            role_name,
        )
    }

    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>> {
        self.cache.list_role_mappings_for_principal(
            realm_id,
            principal_kind,
            principal_id,
            client_uuid,
        )
    }

    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>> {
        self.cache.list_principals_with_client_role(
            realm_id,
            client_uuid,
            role_name,
            principal_kind,
        )
    }

    fn upsert_client<'a>(
        &'a self,
        client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation> {
        self.cache.upsert_client(client)
    }

    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        self.cache.get_client_by_uuid(realm_id, allowed_orgs, uuid)
    }

    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        self.cache.get_client_by_client_id(realm_id, client_id)
    }

    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>> {
        self.cache
            .list_clients(realm_id, allowed_orgs, client_id_filter, search)
    }

    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_client(realm_id, allowed_orgs, uuid)
    }

    fn upsert_client_scope<'a>(
        &'a self,
        scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation> {
        self.cache.upsert_client_scope(scope)
    }

    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>> {
        self.cache.get_client_scope(realm_id, scope_id)
    }

    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        self.cache.list_client_scopes(realm_id)
    }

    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_client_scope(realm_id, scope_id)
    }

    fn upsert_idp<'a>(&'a self, idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()> {
        self.cache.upsert_idp(idp)
    }

    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>> {
        self.cache.get_idp(realm_id, alias)
    }

    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>> {
        self.cache.list_idps(realm_id)
    }

    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()> {
        self.cache.upsert_signing_key(key)
    }

    fn list_signing_keys<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<SigningKeyRecord>> {
        self.cache.list_signing_keys(realm_id)
    }

    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()> {
        self.cache.put_auth_session(session)
    }

    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>> {
        self.cache.get_auth_session_by_id(realm_id, id)
    }

    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()> {
        self.cache.delete_auth_session(realm_id, id)
    }

    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()> {
        self.cache.put_auth_code(code)
    }

    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)> {
        self.cache.consume_auth_code(realm_id, code_hash)
    }

    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()> {
        self.cache.put_par_request(par)
    }

    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>> {
        self.cache.consume_par_request(realm_id, request_uri_hash)
    }

    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()> {
        self.cache.upsert_sso_session(session)
    }

    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        self.cache.get_sso_session(realm_id, session_id)
    }

    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        self.cache.get_sso_session_by_handle(realm_id, handle_hash)
    }

    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        self.cache.list_user_sso_sessions(realm_id, user_id)
    }

    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        self.cache.delete_sso_session(realm_id, session_id)
    }

    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        self.cache.delete_user_sso_sessions(realm_id, user_id)
    }

    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()> {
        self.cache.upsert_offline_session(session)
    }

    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.cache
            .delete_user_client_offline_sessions(realm_id, user_id, client_id)
    }

    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()> {
        self.cache.upsert_refresh_family(family)
    }

    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>> {
        self.cache.get_refresh_family(realm_id, family_id)
    }

    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()> {
        self.cache
            .revoke_refresh_families_for_session(realm_id, sso_session_id, client_id)
    }

    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()> {
        self.cache.mark_token_revoked(rec)
    }

    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.is_token_revoked(realm_id, jti_hash)
    }

    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>> {
        self.cache.get_login_failure(realm_id, target_key)
    }

    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()> {
        self.cache.upsert_login_failure(rec)
    }

    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()> {
        self.cache.clear_login_failure(realm_id, target_key)
    }

    fn upsert_consent<'a>(&'a self, consent: ConsentRecord) -> StoreFuture<'a, ()> {
        self.cache.upsert_consent(consent)
    }

    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>> {
        self.cache.list_consents(realm_id, user_id)
    }

    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        self.cache.delete_consent(realm_id, user_id, client_id)
    }

    fn upsert_required_action<'a>(&'a self, ra: RequiredActionConfig) -> StoreFuture<'a, ()> {
        self.cache.upsert_required_action(ra)
    }

    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>> {
        self.cache.list_required_actions(realm_id)
    }

    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()> {
        self.cache.upsert_branding(branding)
    }

    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>> {
        self.cache.get_branding(realm_id, org_id)
    }

    fn append_event<'a>(&'a self, event: AuditEvent) -> StoreFuture<'a, ()> {
        self.cache.append_event(event)
    }

    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>> {
        self.cache.list_events(realm_id, allowed_orgs)
    }

    fn append_admin_event<'a>(&'a self, event: AdminEvent) -> StoreFuture<'a, ()> {
        self.cache.append_admin_event(event)
    }

    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>> {
        self.cache.list_admin_events(realm_id, allowed_orgs)
    }

    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64> {
        self.cache.sweep_expired()
    }
}
