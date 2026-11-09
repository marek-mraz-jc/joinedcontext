#![forbid(unsafe_code)]
//! PostgreSQL store (`kaisatsu-store-pg`, W2):
//! - Own private `sqlx::PgPool` (default max_connections = 5, password read from a file in the
//!   secrets mount, never from env or argv).
//! - Every call runs in one transaction that first sets `kaisatsu.realm` and `kaisatsu.orgs`
//!   (transaction-local `set_config`), so the forced row-level security policies of
//!   `migrations/0001_init.sql` decide what the request role sees, whatever the SQL says.
//! - Boot check refusing to start when any table in schema `kaisatsu` lacks `FORCE ROW LEVEL SECURITY`
//!   or when the request role (`kaisatsu_app`) has `SUPERUSER`, `BYPASSRLS`, owns any table, or
//!   holds grants on schemas outside `kaisatsu`.
//! - Client secrets, the SMTP password and signing keys are stored AES-256-GCM encrypted under the
//!   KEK, never in the JSON document column.
//! - Expired-row sweeper using `FOR UPDATE SKIP LOCKED` batches.
//! - `NOTIFY kaisatsu_changed` on configuration writes for sibling processes.

pub mod odata_pg;
pub use odata_pg::PgScopedTable;

use chrono::Utc;
use kaisatsu_core::store::StoreFuture;
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, KeyManager};
use odata_table::{Answer, MemoryTable, QueryOptions, Service, TableInfo, query};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPool, PgPoolOptions};
use sqlx::{PgConnection, Postgres, Transaction};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Embedded migration SQL for schema `kaisatsu` (run only by `kaisatsu migrate` as `kaisatsu_migrate`, W2).
pub const MIGRATION_0001_SQL: &str = include_str!("../migrations/0001_init.sql");
/// Embedded migration 0002 SQL (`webauthn_credential`, `idp_links`, `email_outbox` & domain columns).
pub const MIGRATION_0002_SQL: &str = include_str!("../migrations/0002_k_items.sql");

/// Table holding the applied migration versions (outside schema `kaisatsu`, so the request role
/// has no grant on it and the RLS boot check does not see it).
pub const MIGRATIONS_TABLE: &str = "kaisatsu_migrations._sqlx_migrations";

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

impl PgStoreConfig {
    /// Connection options from the URL and the password file. A password inside the URL is
    /// refused: the password comes from a file in the secrets mount, never argv or env (W2).
    pub fn connect_options(&self) -> Result<PgConnectOptions> {
        if url_carries_password(&self.url_without_password) {
            return Err(KaisatsuError::Storage(
                "the database URL must not carry a password; pass a password file (W2)".into(),
            ));
        }
        let mut opts = PgConnectOptions::from_str(&self.url_without_password)
            .map_err(|e| KaisatsuError::Storage(format!("invalid pg url: {e}")))?;
        if let Some(pw_path) = &self.password_file {
            opts = opts.password(&read_password_file(pw_path)?);
        }
        Ok(opts)
    }
}

/// True when a `postgres://user:password@host/db` URL or its `password=` parameter holds a password.
fn url_carries_password(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (authority, query) = match rest.split_once('?') {
        Some((a, q)) => (a, q),
        None => (rest, ""),
    };
    let authority = authority.split('/').next().unwrap_or_default();
    let userinfo_has_password = authority
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'));
    let query_has_password = query.split('&').any(|kv| {
        kv.split_once('=')
            .map_or(kv, |(k, _)| k)
            .eq_ignore_ascii_case("password")
    });
    userinfo_has_password || query_has_password
}

/// PostgreSQL store with private pool, RLS verification, sweeper, and `LISTEN/NOTIFY` (W2).
#[derive(Clone)]
pub struct PgStore {
    /// Private field of the kaisatsu state, not reachable from another crate (W2).
    pool: PgPool,
    kek: Kek,
    service: Arc<Service>,
    csdl_model: Arc<odata_csdl::Model>,
    reload_generation: Arc<AtomicU64>,
}

type Tx = Transaction<'static, Postgres>;

/// Maps a database error to a store error; a unique violation is a conflict (409).
fn storage(e: sqlx::Error) -> KaisatsuError {
    if let sqlx::Error::Database(db) = &e
        && db.code().as_deref() == Some("23505")
    {
        return KaisatsuError::Conflict("a record with the same key already exists".into());
    }
    KaisatsuError::Storage(format!("postgres: {e}"))
}

fn encode_err(e: serde_json::Error) -> KaisatsuError {
    KaisatsuError::Storage(format!("cannot encode record: {e}"))
}

fn decode_err(e: serde_json::Error) -> KaisatsuError {
    KaisatsuError::Storage(format!("cannot decode stored record: {e}"))
}

fn to_doc<T: Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(encode_err)
}

fn from_doc<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(decode_err)
}

fn from_docs<T: DeserializeOwned>(v: Vec<Value>) -> Result<Vec<T>> {
    v.into_iter().map(from_doc).collect()
}

fn opt_from_doc<T: DeserializeOwned>(v: Option<Value>) -> Result<Option<T>> {
    v.map(from_doc).transpose()
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn client_secret_aad(realm_id: &str, id: &str) -> String {
    format!("kaisatsu:client-secret:{realm_id}:{id}")
}

fn smtp_aad(realm_id: &str) -> String {
    format!("kaisatsu:smtp-password:{realm_id}")
}

/// Root segment of a group path (`/helsinki/members` -> `helsinki`).
fn path_root(path: &str) -> Option<&str> {
    path.strip_prefix('/').and_then(|p| p.split('/').next())
}

/// Sets the realm and organization scope of a transaction (transaction-local).
async fn set_scope(conn: &mut PgConnection, realm: &str, orgs: Option<&[String]>) -> Result<()> {
    let orgs = match orgs {
        None => "*".to_owned(),
        Some(list) => serde_json::to_string(list).map_err(encode_err)?,
    };
    sqlx::query(
        "SELECT set_config('kaisatsu.realm', $1, true), set_config('kaisatsu.orgs', $2, true)",
    )
    .bind(realm)
    .bind(orgs)
    .execute(conn)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn notify(conn: &mut PgConnection, realm_id: &str) -> Result<()> {
    sqlx::query("SELECT pg_notify('kaisatsu_changed', $1)")
        .bind(realm_id)
        .execute(conn)
        .await
        .map_err(storage)?;
    Ok(())
}

fn attach_children(parent: &mut GroupRepresentation, all: &[GroupRepresentation]) {
    let mut children: Vec<GroupRepresentation> = all
        .iter()
        .filter(|c| {
            c.parent_id.as_deref() == Some(parent.id.as_str())
                || (c.parent_id.is_none()
                    && c.path.starts_with(&format!("{}/", parent.path))
                    && !c.path[parent.path.len() + 1..].contains('/'))
        })
        .cloned()
        .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    for ch in &mut children {
        attach_children(ch, all);
    }
    parent.sub_groups = children;
}

fn matches_or_has_matching_child(
    node: &mut GroupRepresentation,
    q_lower: &str,
    exact: bool,
) -> bool {
    let self_matches = if exact {
        node.name.eq_ignore_ascii_case(q_lower)
    } else {
        node.name.to_ascii_lowercase().contains(q_lower)
    };
    node.sub_groups
        .retain_mut(|ch| matches_or_has_matching_child(ch, q_lower, exact));
    self_matches || !node.sub_groups.is_empty()
}

impl PgStore {
    /// Connects to PostgreSQL reading the password from `config.password_file` if provided,
    /// and runs the mandatory W2 RLS and role privilege boot check.
    pub async fn connect(config: &PgStoreConfig, kek: Kek) -> Result<Self> {
        Self::connect_with(config.connect_options()?, config.max_connections, kek).await
    }

    /// Connects with explicit connection options (pool size clamped to 1..=50) and runs the
    /// boot check before returning the store.
    pub async fn connect_with(
        opts: PgConnectOptions,
        max_connections: u32,
        kek: Kek,
    ) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.clamp(1, 50))
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("pg connect failed: {e}")))?;
        verify_boot_rls(&pool).await?;
        let service = kaisatsu_odata_service();
        let csdl_model = Arc::new(service.model());
        Ok(Self {
            pool,
            kek,
            service: Arc::new(service),
            csdl_model,
            reload_generation: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Runs the migrations as `kaisatsu_migrate` (schema owner). Never called on `serve` (W2).
    /// Applied versions are kept in [`MIGRATIONS_TABLE`]; running twice changes nothing.
    pub async fn run_migrations(migrate_pool: &PgPool) -> Result<()> {
        use sqlx::SqlSafeStr;
        use sqlx::migrate::{Migration, MigrationType, Migrator};
        let mut migrator = Migrator::with_migrations(vec![
            Migration::new(
                1,
                "init".into(),
                MigrationType::Simple,
                MIGRATION_0001_SQL.into_sql_str(),
                false,
            ),
            Migration::new(
                2,
                "k_items".into(),
                MigrationType::Simple,
                MIGRATION_0002_SQL.into_sql_str(),
                false,
            ),
        ]);
        migrator.create_schema("kaisatsu_migrations");
        migrator.dangerous_set_table_name(MIGRATIONS_TABLE);
        migrator
            .run(migrate_pool)
            .await
            .map_err(|e| KaisatsuError::Storage(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Opens a transaction scoped to `realm` and `orgs` (`None` = every organization).
    async fn begin(&self, realm: &str, orgs: Option<&[String]>) -> Result<Tx> {
        if realm.is_empty() || realm == "*" {
            return Err(KaisatsuError::NotFound("realm not found".into()));
        }
        let mut tx = self.pool.begin().await.map_err(storage)?;
        set_scope(&mut tx, realm, orgs).await?;
        Ok(tx)
    }

    /// Opens a transaction that may read every realm row of `kaisatsu.realms` (and nothing else).
    async fn begin_all_realms(&self) -> Result<Tx> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        set_scope(&mut tx, "*", None).await?;
        Ok(tx)
    }

    /// Ensures a realm has active ES256 and RS256 signing keys (serialized per realm).
    pub async fn ensure_realm_keys(&self, realm_id: &str) -> Result<()> {
        self.ensure_keys(realm_id, &["ES256", "RS256"]).await
    }

    /// Ensures a realm has an active key of each of `algorithms`, generating the missing ones
    /// under the realm's key lock in one transaction.
    async fn ensure_keys(&self, realm_id: &str, algorithms: &[&str]) -> Result<()> {
        let mut tx = self.begin(realm_id, None).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('kaisatsu.keys:' || $1, 0))")
            .bind(realm_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let active: Vec<String> = sqlx::query_scalar(
            "SELECT doc->>'algorithm' FROM kaisatsu.signing_keys
             WHERE realm_id = $1 AND doc->>'status' = 'ACTIVE'",
        )
        .bind(realm_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        let mut created = false;
        for algorithm in algorithms {
            if !active.iter().any(|a| a == algorithm) {
                let key = KeyManager::generate(algorithm, realm_id, &self.kek, "ACTIVE")?;
                insert_signing_key(&mut tx, &key).await?;
                created = true;
            }
        }
        if created {
            notify(&mut tx, realm_id).await?;
        }
        tx.commit().await.map_err(storage)?;
        Ok(())
    }

    /// Runs one round of the expired-row sweeper: per realm and table at most `batch_size` rows,
    /// locked with `FOR UPDATE SKIP LOCKED` so several instances never block each other (W2).
    pub async fn sweep_expired_batch(&self, batch_size: i64) -> Result<u64> {
        let realms: Vec<String> = {
            let mut tx = self.begin_all_realms().await?;
            let ids = sqlx::query_scalar("SELECT realm_id FROM kaisatsu.realms ORDER BY seq")
                .fetch_all(&mut *tx)
                .await
                .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            ids
        };
        // sqlx 0.9 refuses format!-built SQL: one static statement per table.
        macro_rules! sweep_sql {
            ($table:literal) => {
                concat!(
                    "DELETE FROM kaisatsu.",
                    $table,
                    " WHERE ctid IN (SELECT ctid FROM kaisatsu.",
                    $table,
                    " WHERE realm_id = $1 AND expires_at < now() LIMIT $2 FOR UPDATE SKIP LOCKED)"
                )
            };
        }
        let statements = [
            sweep_sql!("auth_session"),
            sweep_sql!("auth_code"),
            sweep_sql!("par_request"),
            sweep_sql!("sso_session"),
            sweep_sql!("offline_session"),
            sweep_sql!("refresh_token_family"),
            sweep_sql!("revoked_token"),
        ];
        let mut total = 0u64;
        for realm in realms {
            let mut tx = self.begin(&realm, None).await?;
            for sql in statements {
                let res = sqlx::query(sql)
                    .bind(&realm)
                    .bind(batch_size)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
                total += res.rows_affected();
            }
            // Client sessions die with their SSO session.
            let res = sqlx::query(
                "DELETE FROM kaisatsu.client_session cs WHERE cs.realm_id = $1 AND NOT EXISTS (
                     SELECT 1 FROM kaisatsu.sso_session s
                     WHERE s.realm_id = cs.realm_id AND s.id = cs.sso_session_id)",
            )
            .bind(&realm)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            total += res.rows_affected();
            tx.commit().await.map_err(storage)?;
        }
        Ok(total)
    }

    /// Emits `NOTIFY kaisatsu_changed` so sibling pods invalidate their per-process config caches (W2).
    pub async fn notify_changed(&self, realm_id: &str) -> Result<()> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        notify(&mut conn, realm_id).await?;
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

    pub fn table_info(&self, set_name: &str) -> Option<TableInfo> {
        self.service.info(self.csdl_model.clone(), set_name)
    }

    /// Loads the rows of one entity set of a realm, in the shape of `odata_defs` (W4).
    async fn entity_rows(&self, realm_id: &str, set_name: &str) -> Result<Vec<Value>> {
        if set_name == "Realms" {
            let mut tx = self.begin_all_realms().await?;
            let rows: Vec<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, smtp_password_enc FROM kaisatsu.realms
                 WHERE realm_id = $1 OR name = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            let mut out = Vec::new();
            for (doc, _) in rows {
                let realm: RealmConfig = from_doc(doc)?;
                out.push(odata_defs::realm_to_row(&realm));
            }
            return Ok(out);
        }
        let mut tx = self.begin(realm_id, None).await?;
        macro_rules! docs {
            ($sql:expr) => {
                sqlx::query_scalar::<_, Value>($sql)
                    .bind(realm_id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(storage)?
            };
        }
        let rows: Vec<Value> = match set_name {
            "Organizations" => from_docs::<Organization>(docs!(
                "SELECT doc FROM kaisatsu.organizations WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::org_to_row)
            .collect(),
            "OrganizationDomains" => from_docs::<Organization>(docs!(
                "SELECT doc FROM kaisatsu.organizations WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .flat_map(|o| o.domains.iter())
            .map(odata_defs::org_domain_to_row)
            .collect(),
            "People" => from_docs::<UserRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.people WHERE realm_id = $1 AND NOT transient ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::person_to_row)
            .collect(),
            "Memberships" => from_docs::<Membership>(docs!(
                "SELECT doc FROM kaisatsu.memberships WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::membership_to_row)
            .collect(),
            "Groups" => from_docs::<GroupRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.groups WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::group_to_row)
            .collect(),
            "GroupMembers" => from_docs::<GroupMemberRecord>(docs!(
                "SELECT doc FROM kaisatsu.group_members WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::group_member_to_row)
            .collect(),
            "Roles" => from_docs::<RoleRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.roles WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::role_to_row)
            .collect(),
            "RoleMappings" => from_docs::<RoleMappingRecord>(docs!(
                "SELECT doc FROM kaisatsu.role_mappings WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::role_mapping_to_row)
            .collect(),
            "Applications" => from_docs::<ClientRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.applications WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::client_to_row)
            .collect(),
            "ClientScopes" => from_docs::<ClientScopeRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.client_scopes WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::scope_to_row)
            .collect(),
            "ProtocolMappers" => {
                let clients = from_docs::<ClientRepresentation>(docs!(
                    "SELECT doc FROM kaisatsu.applications WHERE realm_id = $1 ORDER BY seq"
                ))?;
                let scopes = from_docs::<ClientScopeRepresentation>(docs!(
                    "SELECT doc FROM kaisatsu.client_scopes WHERE realm_id = $1 ORDER BY seq"
                ))?;
                clients
                    .iter()
                    .flat_map(|c| c.protocol_mappers.iter())
                    .chain(scopes.iter().flat_map(|s| s.protocol_mappers.iter()))
                    .map(odata_defs::mapper_to_row)
                    .collect()
            }
            "IdentityProviders" => from_docs::<IdentityProviderRepresentation>(docs!(
                "SELECT doc FROM kaisatsu.identity_providers WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::idp_to_row)
            .collect(),
            "Credentials" => {
                let rows: Vec<(Value, Option<String>)> = sqlx::query_as(
                    "SELECT c.doc, p.org_id FROM kaisatsu.credentials c
                     LEFT JOIN kaisatsu.people p ON p.realm_id = c.realm_id AND p.id = c.user_id
                     WHERE c.realm_id = $1 ORDER BY c.seq",
                )
                .bind(realm_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage)?;
                let mut out = Vec::with_capacity(rows.len());
                for (doc, org) in rows {
                    let cred: CredentialRecord = from_doc(doc)?;
                    out.push(odata_defs::credential_to_row(&cred, org.as_deref()));
                }
                out
            }
            "Sessions" => {
                let rows: Vec<(Value, String)> = sqlx::query_as(
                    "SELECT doc, token_hash FROM kaisatsu.sso_session WHERE realm_id = $1 ORDER BY seq",
                )
                .bind(realm_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    out.push(odata_defs::session_to_row(&sso_from_row(row)?));
                }
                out
            }
            "ClientSessions" => from_docs::<ClientSession>(docs!(
                "SELECT doc FROM kaisatsu.client_session WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::client_session_to_row)
            .collect(),
            "OfflineSessions" => from_docs::<OfflineSession>(docs!(
                "SELECT doc FROM kaisatsu.offline_session WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::offline_session_to_row)
            .collect(),
            "Consents" => from_docs::<ConsentRecord>(docs!(
                "SELECT doc FROM kaisatsu.consent WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::consent_to_row)
            .collect(),
            "Events" => from_docs::<AuditEvent>(docs!(
                "SELECT doc FROM kaisatsu.events WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::event_to_row)
            .collect(),
            "AdminEvents" => from_docs::<AdminEvent>(docs!(
                "SELECT doc FROM kaisatsu.admin_events WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::admin_event_to_row)
            .collect(),
            "SigningKeys" => {
                let rows: Vec<(Value, Vec<u8>)> = sqlx::query_as(
                    "SELECT doc, encrypted_private_key FROM kaisatsu.signing_keys
                     WHERE realm_id = $1 ORDER BY seq",
                )
                .bind(realm_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(storage)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    out.push(odata_defs::signing_key_to_row(&key_from_row(row)?));
                }
                out
            }
            "RequiredActions" => from_docs::<RequiredActionConfig>(docs!(
                "SELECT doc FROM kaisatsu.required_actions WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::required_action_to_row)
            .collect(),
            "BrandingBlocks" => from_docs::<BrandingBlock>(docs!(
                "SELECT doc FROM kaisatsu.branding_blocks WHERE realm_id = $1 ORDER BY seq"
            ))?
            .iter()
            .map(odata_defs::branding_to_row)
            .collect(),
            _ => Vec::new(),
        };
        tx.commit().await.map_err(storage)?;
        Ok(rows)
    }

    fn client_from_row(
        &self,
        realm_id: &str,
        row: (Value, Option<Vec<u8>>),
    ) -> Result<ClientRepresentation> {
        let (doc, secret_enc) = row;
        let mut client: ClientRepresentation = from_doc(doc)?;
        if client.realm_id.is_empty() {
            client.realm_id = realm_id.to_owned();
        }
        client.secret = match secret_enc {
            Some(enc) => {
                let plain = self
                    .kek
                    .decrypt(client_secret_aad(realm_id, &client.id).as_bytes(), &enc)?;
                Some(String::from_utf8(plain).map_err(|_| {
                    KaisatsuError::Crypto("stored client secret is not UTF-8".into())
                })?)
            }
            None => None,
        };
        Ok(client)
    }

    fn realm_from_row(&self, row: (Value, Option<Vec<u8>>)) -> Result<RealmConfig> {
        let (doc, smtp_enc) = row;
        let mut realm: RealmConfig = from_doc(doc)?;
        if let (Some(enc), Some(smtp)) = (smtp_enc, realm.smtp_server.as_mut()) {
            let plain = self.kek.decrypt(smtp_aad(&realm.id).as_bytes(), &enc)?;
            smtp.password = String::from_utf8(plain)
                .map_err(|_| KaisatsuError::Crypto("stored SMTP password is not UTF-8".into()))?;
        }
        Ok(realm)
    }

    async fn write_client(&self, tx: &mut Tx, client: &ClientRepresentation) -> Result<()> {
        let mut doc_client = client.clone();
        doc_client.secret = None;
        let secret_enc = match &client.secret {
            Some(s) => Some(self.kek.encrypt(
                client_secret_aad(&client.realm_id, &client.id).as_bytes(),
                s.as_bytes(),
            )?),
            None => None,
        };
        sqlx::query("DELETE FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2")
            .bind(&client.realm_id)
            .bind(&client.id)
            .execute(&mut **tx)
            .await
            .map_err(storage)?;
        sqlx::query(
            "INSERT INTO kaisatsu.applications (realm_id, id, org_id, client_id, doc, secret_enc)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&client.realm_id)
        .bind(&client.id)
        .bind(&client.org_id)
        .bind(&client.client_id)
        .bind(to_doc(&doc_client)?)
        .bind(secret_enc)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        Ok(())
    }
}

fn sso_from_row(row: (Value, String)) -> Result<SsoSession> {
    let (mut doc, handle) = row;
    // The handle hash lives in `token_hash`; the serialized session never carries it.
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("handleHash".into(), Value::String(String::new()));
    }
    let mut s: SsoSession = from_doc(doc)?;
    s.handle_hash = handle;
    Ok(s)
}

fn key_from_row(row: (Value, Vec<u8>)) -> Result<SigningKeyRecord> {
    let (mut doc, enc) = row;
    // The ciphertext lives in its own column; the document never carries it.
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("encryptedPrivateKey".into(), Value::Array(Vec::new()));
    }
    let mut k: SigningKeyRecord = from_doc(doc)?;
    k.encrypted_private_key = enc;
    Ok(k)
}

async fn insert_signing_key(conn: &mut PgConnection, key: &SigningKeyRecord) -> Result<()> {
    sqlx::query("DELETE FROM kaisatsu.signing_keys WHERE realm_id = $1 AND kid = $2")
        .bind(&key.realm_id)
        .bind(&key.kid)
        .execute(&mut *conn)
        .await
        .map_err(storage)?;
    sqlx::query(
        "INSERT INTO kaisatsu.signing_keys (realm_id, kid, priority, doc, encrypted_private_key)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&key.realm_id)
    .bind(&key.kid)
    .bind(key.priority)
    .bind(to_doc(key)?)
    .bind(&key.encrypted_private_key)
    .execute(&mut *conn)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn load_user(
    conn: &mut PgConnection,
    realm_id: &str,
    user_id: &str,
) -> Result<Option<UserRepresentation>> {
    let doc: Option<Value> =
        sqlx::query_scalar("SELECT doc FROM kaisatsu.people WHERE realm_id = $1 AND id = $2")
            .bind(realm_id)
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(storage)?;
    opt_from_doc(doc)
}

async fn save_user_doc(conn: &mut PgConnection, user: &UserRepresentation) -> Result<()> {
    sqlx::query("UPDATE kaisatsu.people SET doc = $3 WHERE realm_id = $1 AND id = $2")
        .bind(&user.realm_id)
        .bind(&user.id)
        .bind(to_doc(user)?)
        .execute(&mut *conn)
        .await
        .map_err(storage)?;
    Ok(())
}

async fn insert_user(conn: &mut PgConnection, user: &UserRepresentation) -> Result<()> {
    sqlx::query(
        "INSERT INTO kaisatsu.people (realm_id, id, org_id, username_lower, email_lower, transient, doc)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&user.realm_id)
    .bind(&user.id)
    .bind(&user.org_id)
    .bind(user.username.to_ascii_lowercase())
    .bind(user.email.as_ref().map(|e| e.to_ascii_lowercase()))
    .bind(user.transient)
    .bind(to_doc(user)?)
    .execute(&mut *conn)
    .await
    .map_err(storage)?;
    Ok(())
}

/// True when another person of the realm holds the username or e-mail (all organizations).
async fn user_key_taken(
    conn: &mut PgConnection,
    user: &UserRepresentation,
    except_id: Option<&str>,
) -> Result<bool> {
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM kaisatsu.people
         WHERE realm_id = $1 AND ($4::text IS NULL OR id <> $4)
           AND (username_lower = $2 OR ($3::text IS NOT NULL AND email_lower = $3)))",
    )
    .bind(&user.realm_id)
    .bind(user.username.to_ascii_lowercase())
    .bind(user.email.as_ref().map(|e| e.to_ascii_lowercase()))
    .bind(except_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(storage)?;
    Ok(taken)
}

/// Marks refresh token families revoked (doc and column) for one filter.
macro_rules! revoke_families {
    ($conn:expr, $where:literal, $($bind:expr),+) => {
        sqlx::query(concat!(
            "UPDATE kaisatsu.refresh_token_family
             SET revoked = true, doc = jsonb_set(doc, '{revoked}', 'true'::jsonb) WHERE ",
            $where
        ))
        $(.bind($bind))+
        .execute(&mut *$conn)
        .await
        .map_err(storage)
    };
}

impl Store for PgStore {
    fn query_entity_set<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        authorizer: &'a dyn Authorizer,
        set_name: &'a str,
        options: &'a QueryOptions,
        max_page: u64,
    ) -> StoreFuture<'a, Answer> {
        Box::pin(async move {
            let info = self
                .table_info(set_name)
                .ok_or_else(|| KaisatsuError::NotFound(format!("unknown entity set {set_name}")))?;
            let scope = scope_for_context(ctx, authorizer, set_name);
            if !scope.holds("read") {
                return Err(KaisatsuError::NotFound(format!(
                    "{set_name} not accessible"
                )));
            }
            let table = PgScopedTable::new(self, info, ctx.realm_id.clone(), ctx.allowed_orgs());
            let _ = MemoryTable::new;
            let _ = Self::entity_rows;
            Ok(query(&table, options, &scope, max_page).await?)
        })
    }

    fn get_realm<'a>(&'a self, realm_name: &'a str) -> StoreFuture<'a, Option<RealmConfig>> {
        Box::pin(async move {
            let mut tx = self.begin_all_realms().await?;
            let row: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, smtp_password_enc FROM kaisatsu.realms
                 WHERE name = $1 OR realm_id = $1 ORDER BY (name = $1) DESC LIMIT 1",
            )
            .bind(realm_name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            row.map(|r| self.realm_from_row(r)).transpose()
        })
    }

    fn upsert_realm<'a>(&'a self, mut realm: RealmConfig) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if realm.id.is_empty() {
                realm.id = realm.realm.clone();
            }
            let smtp_enc = match realm.smtp_server.as_ref() {
                Some(s) if !s.password.is_empty() => Some(
                    self.kek
                        .encrypt(smtp_aad(&realm.id).as_bytes(), s.password.as_bytes())?,
                ),
                _ => None,
            };
            let mut tx = self.begin_all_realms().await?;
            sqlx::query("DELETE FROM kaisatsu.realms WHERE realm_id = $1 OR name = $2")
                .bind(&realm.id)
                .bind(&realm.realm)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.realms (realm_id, name, doc, smtp_password_enc)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&realm.id)
            .bind(&realm.realm)
            .bind(to_doc(&realm)?)
            .bind(smtp_enc)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            notify(&mut tx, &realm.id).await?;
            tx.commit().await.map_err(storage)?;
            self.ensure_realm_keys(&realm.id).await
        })
    }

    fn list_realms<'a>(&'a self) -> StoreFuture<'a, Vec<RealmConfig>> {
        Box::pin(async move {
            let mut tx = self.begin_all_realms().await?;
            let rows: Vec<(Value, Option<Vec<u8>>)> =
                sqlx::query_as("SELECT doc, smtp_password_enc FROM kaisatsu.realms ORDER BY name")
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            rows.into_iter().map(|r| self.realm_from_row(r)).collect()
        })
    }

    fn upsert_organization<'a>(&'a self, mut org: Organization) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if org.id.is_empty() {
                org.id = org.alias.clone();
            }
            let mut tx = self.begin(&org.realm_id, None).await?;
            let removed: Vec<String> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.organizations WHERE realm_id = $1 AND (org_id = $2 OR alias = $3)
                 RETURNING org_id",
            )
            .bind(&org.realm_id)
            .bind(&org.id)
            .bind(&org.alias)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "DELETE FROM kaisatsu.organization_domains WHERE realm_id = $1 AND (org_id = ANY($2) OR org_id = $3)",
            )
            .bind(&org.realm_id)
            .bind(&removed)
            .bind(&org.id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.organizations (realm_id, org_id, alias, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&org.realm_id)
            .bind(&org.id)
            .bind(&org.alias)
            .bind(to_doc(&org)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            for d in &org.domains {
                sqlx::query(
                    "INSERT INTO kaisatsu.organization_domains (realm_id, org_id, domain_lower, verified)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (realm_id, org_id, domain_lower) DO UPDATE SET verified = EXCLUDED.verified",
                )
                .bind(&org.realm_id)
                .bind(&org.id)
                .bind(d.name.to_ascii_lowercase())
                .bind(d.verified)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_organization<'a>(
        &'a self,
        realm_id: &'a str,
        org_id_or_alias: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.organizations WHERE realm_id = $1 AND (org_id = $2 OR alias = $2)
                 ORDER BY seq LIMIT 1",
            )
            .bind(realm_id)
            .bind(org_id_or_alias)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn list_organizations<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<Organization>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.organizations WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn find_org_by_domain<'a>(
        &'a self,
        realm_id: &'a str,
        domain: &'a str,
    ) -> StoreFuture<'a, Option<Organization>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT o.doc FROM kaisatsu.organizations o
                 JOIN kaisatsu.organization_domains d ON d.realm_id = o.realm_id AND d.org_id = o.org_id
                 WHERE o.realm_id = $1 AND d.domain_lower = $2 AND d.verified
                   AND coalesce((o.doc->>'enabled')::boolean, false)
                 ORDER BY o.seq LIMIT 1",
            )
            .bind(realm_id)
            .bind(domain.to_ascii_lowercase())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn upsert_membership<'a>(&'a self, m: Membership) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&m.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.memberships WHERE realm_id = $1 AND org_id = $2 AND person_id = $3",
            )
            .bind(&m.realm_id)
            .bind(&m.org_id)
            .bind(&m.person_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.memberships (realm_id, org_id, person_id, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&m.realm_id)
            .bind(&m.org_id)
            .bind(&m.person_id)
            .bind(to_doc(&m)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_memberships_for_person<'a>(
        &'a self,
        realm_id: &'a str,
        person_id: &'a str,
    ) -> StoreFuture<'a, Vec<Membership>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.memberships WHERE realm_id = $1 AND person_id = $2 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(person_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn create_user<'a>(
        &'a self,
        mut user: UserRepresentation,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            if user.id.is_empty() {
                user.id = new_id();
            }
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            if user.created_timestamp.is_none() {
                user.created_timestamp = Some(Utc::now().timestamp_millis());
            }
            let mut tx = self.begin(&user.realm_id, None).await?;
            if user_key_taken(&mut tx, &user, None).await? {
                return Err(KaisatsuError::Conflict(
                    "user with same username or email already exists".into(),
                ));
            }
            insert_user(&mut tx, &user).await?;
            tx.commit().await.map_err(storage)?;
            Ok(user)
        })
    }

    fn update_user<'a>(
        &'a self,
        mut user: UserRepresentation,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            let mut tx = self.begin(&user.realm_id, None).await?;
            if load_user(&mut tx, &user.realm_id, &user.id)
                .await?
                .is_none()
            {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            if user_key_taken(&mut tx, &user, Some(&user.id)).await? {
                return Err(KaisatsuError::Conflict(
                    "another user already holds that username or email".into(),
                ));
            }
            sqlx::query(
                "UPDATE kaisatsu.people SET org_id = $3, username_lower = $4, email_lower = $5,
                     transient = $6, doc = $7
                 WHERE realm_id = $1 AND id = $2",
            )
            .bind(&user.realm_id)
            .bind(&user.id)
            .bind(&user.org_id)
            .bind(&user.username)
            .bind(&user.email)
            .bind(user.transient)
            .bind(to_doc(&user)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(user)
        })
    }

    fn delete_user<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            // RLS decides visibility: an invisible person is not deleted.
            let deleted: Option<String> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.people WHERE realm_id = $1 AND id = $2 AND NOT transient RETURNING id",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if deleted.is_none() {
                return Ok(false);
            }
            // The person is gone; their dependent rows go in every organization.
            set_scope(&mut tx, realm_id, None).await?;
            for sql in [
                "DELETE FROM kaisatsu.credentials WHERE realm_id = $1 AND user_id = $2",
                "DELETE FROM kaisatsu.group_members WHERE realm_id = $1 AND user_id = $2",
                "DELETE FROM kaisatsu.sso_session WHERE realm_id = $1 AND user_id = $2",
                "DELETE FROM kaisatsu.webauthn_credential WHERE realm_id = $1 AND user_id = $2",
                "DELETE FROM kaisatsu.idp_links WHERE realm_id = $1 AND user_id = $2",
            ] {
                sqlx::query(sql)
                    .bind(realm_id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(true)
        })
    }

    fn get_user_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        user_id: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.people WHERE realm_id = $1 AND id = $2 AND NOT transient",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn get_user_by_username_or_email<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        identifier: &'a str,
    ) -> StoreFuture<'a, Option<UserRepresentation>> {
        Box::pin(async move {
            let lower = identifier.trim().to_ascii_lowercase();
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.people
                 WHERE realm_id = $1 AND (username_lower = $2 OR email_lower = $2) AND NOT transient
                 ORDER BY seq LIMIT 1",
            )
            .bind(realm_id)
            .bind(&lower)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
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
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.people WHERE realm_id = $1 AND NOT transient
                 ORDER BY username_lower",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            let users: Vec<UserRepresentation> = from_docs(docs)?;
            let search_lower = search.map(str::to_ascii_lowercase);
            let email_lower = email.map(str::to_ascii_lowercase);
            let user_lower = username.map(str::to_ascii_lowercase);
            let field_matches = |value: &str, wanted: &Option<String>| match wanted {
                None => true,
                Some(w) if exact => value == w,
                Some(w) => value.contains(w.as_str()),
            };
            Ok(users
                .into_iter()
                .filter(|u| {
                    let u_em = u.email.as_deref().unwrap_or_default().to_ascii_lowercase();
                    if !field_matches(&u_em, &email_lower)
                        || !field_matches(&u.username.to_ascii_lowercase(), &user_lower)
                    {
                        return false;
                    }
                    match &search_lower {
                        Some(q) if !q.is_empty() => format!(
                            "{} {} {} {}",
                            u.username,
                            u.email.as_deref().unwrap_or_default(),
                            u.first_name.as_deref().unwrap_or_default(),
                            u.last_name.as_deref().unwrap_or_default(),
                        )
                        .to_ascii_lowercase()
                        .contains(q.as_str()),
                        _ => true,
                    }
                })
                .skip(first)
                .take(max.max(1))
                .collect())
        })
    }

    fn upsert_credential<'a>(&'a self, mut cred: CredentialRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if cred.id.is_empty() {
                cred.id = new_id();
            }
            if cred.created_date == 0 {
                cred.created_date = Utc::now().timestamp_millis();
            }
            let mut tx = self.begin(&cred.realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.credentials WHERE realm_id = $1 AND id = $2")
                .bind(&cred.realm_id)
                .bind(&cred.id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            if cred.credential_type == "password" {
                // The previous password becomes a history entry (passwordHistory policy).
                let prev: Option<Value> = sqlx::query_scalar(
                    "DELETE FROM kaisatsu.credentials
                     WHERE realm_id = $1 AND user_id = $2 AND credential_type = 'password'
                     RETURNING doc",
                )
                .bind(&cred.realm_id)
                .bind(&cred.user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?;
                if let Some(prev) = prev {
                    let mut hist: CredentialRecord = from_doc(prev)?;
                    hist.id = new_id();
                    hist.credential_type = "password-history".into();
                    insert_credential(&mut tx, &hist).await?;
                }
            }
            insert_credential(&mut tx, &cred).await?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<CredentialRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.credentials WHERE realm_id = $1 AND user_id = $2 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn delete_credential<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        cred_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let res = sqlx::query(
                "DELETE FROM kaisatsu.credentials WHERE realm_id = $1 AND user_id = $2 AND id = $3",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(cred_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn upsert_group<'a>(
        &'a self,
        group: GroupRepresentation,
    ) -> StoreFuture<'a, GroupRepresentation> {
        Box::pin(async move {
            let realm_id = group.realm_id.clone();
            let mut tx = self.begin(&realm_id, None).await?;
            let mut stack = vec![group];
            let mut first: Option<GroupRepresentation> = None;
            while let Some(mut g) = stack.pop() {
                if g.id.is_empty() {
                    g.id = new_id();
                }
                if g.path.is_empty() {
                    g.path = format!("/{}", g.name);
                }
                if g.org_id.is_none()
                    && let Some(seg) = path_root(&g.path)
                {
                    let is_org: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM kaisatsu.organizations
                         WHERE realm_id = $1 AND (org_id = $2 OR alias = $2))",
                    )
                    .bind(&g.realm_id)
                    .bind(seg)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
                    if is_org {
                        g.org_id = Some(seg.to_owned());
                    }
                }
                let subs = std::mem::take(&mut g.sub_groups);
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM kaisatsu.groups WHERE realm_id = $1 AND (id = $2 OR path = $3)
                     ORDER BY (id = $2) DESC LIMIT 1",
                )
                .bind(&g.realm_id)
                .bind(&g.id)
                .bind(&g.path)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?;
                if let Some(id) = existing {
                    g.id = id;
                }
                sqlx::query("DELETE FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2")
                    .bind(&g.realm_id)
                    .bind(&g.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
                sqlx::query(
                    "INSERT INTO kaisatsu.groups (realm_id, id, org_id, path, doc) VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(&g.realm_id)
                .bind(&g.id)
                .bind(&g.org_id)
                .bind(&g.path)
                .bind(to_doc(&g)?)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
                for mut sub in subs.into_iter().rev() {
                    sub.realm_id = g.realm_id.clone();
                    sub.parent_id = Some(g.id.clone());
                    if sub.path.is_empty() {
                        sub.path = format!("{}/{}", g.path.trim_end_matches('/'), sub.name);
                    }
                    if sub.org_id.is_none() {
                        sub.org_id = g.org_id.clone();
                    }
                    stack.push(sub);
                }
                if first.is_none() {
                    first = Some(g);
                }
            }
            tx.commit().await.map_err(storage)?;
            first.ok_or_else(|| KaisatsuError::Storage("group upsert wrote nothing".into()))
        })
    }

    fn get_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn get_group_by_path<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        path: &'a str,
    ) -> StoreFuture<'a, Option<GroupRepresentation>> {
        Box::pin(async move {
            let norm = if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("/{path}")
            };
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.groups WHERE realm_id = $1 AND path = $2",
            )
            .bind(realm_id)
            .bind(&norm)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn list_groups<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        search: Option<&'a str>,
        exact: bool,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.groups WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            let visible: Vec<GroupRepresentation> = from_docs(docs)?;
            let mut roots: Vec<GroupRepresentation> = visible
                .iter()
                .filter(|g| {
                    g.parent_id.is_none()
                        && g.path.strip_prefix('/').is_some_and(|p| !p.contains('/'))
                })
                .cloned()
                .collect();
            roots.sort_by(|a, b| a.name.cmp(&b.name));
            for r in &mut roots {
                attach_children(r, &visible);
            }
            let Some(q) = search.filter(|s| !s.is_empty()) else {
                return Ok(roots);
            };
            let q_lower = q.to_ascii_lowercase();
            roots.retain_mut(|r| matches_or_has_matching_child(r, &q_lower, exact));
            Ok(roots)
        })
    }

    fn delete_group<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let removed: Option<Value> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2 RETURNING doc",
            )
            .bind(realm_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(removed) = removed else {
                return Ok(false);
            };
            let removed: GroupRepresentation = from_doc(removed)?;
            set_scope(&mut tx, realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.group_members WHERE realm_id = $1 AND group_id = $2")
                .bind(realm_id)
                .bind(group_id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            let holders: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.people
                 WHERE realm_id = $1 AND (doc->'groups' ? $2 OR doc->'groups' ? $3)",
            )
            .bind(realm_id)
            .bind(&removed.path)
            .bind(&removed.name)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            for doc in holders {
                let mut u: UserRepresentation = from_doc(doc)?;
                u.groups
                    .retain(|p| p != &removed.path && p != &removed.name);
                save_user_doc(&mut tx, &u).await?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(true)
        })
    }

    fn add_user_to_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let group: GroupRepresentation =
                from_doc(doc.ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?)?;
            let member_org = group
                .org_id
                .clone()
                .or_else(|| path_root(&group.path).map(str::to_owned));
            let member = GroupMemberRecord {
                id: new_id(),
                realm_id: realm_id.to_owned(),
                org_id: group.org_id.clone(),
                group_id: group_id.to_owned(),
                user_id: user_id.to_owned(),
            };
            sqlx::query(
                "INSERT INTO kaisatsu.group_members (realm_id, group_id, user_id, org_id, doc)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (realm_id, group_id, user_id) DO NOTHING",
            )
            .bind(realm_id)
            .bind(group_id)
            .bind(user_id)
            .bind(member_org)
            .bind(to_doc(&member)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(mut u) = load_user(&mut tx, realm_id, user_id).await?
                && !u.groups.contains(&group.path)
            {
                u.groups.push(group.path);
                save_user_doc(&mut tx, &u).await?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn remove_user_from_group<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        group_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let path: Option<String> = sqlx::query_scalar(
                "SELECT path FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "DELETE FROM kaisatsu.group_members WHERE realm_id = $1 AND user_id = $2 AND group_id = $3",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(path) = path
                && let Some(mut u) = load_user(&mut tx, realm_id, user_id).await?
            {
                u.groups.retain(|p| p != &path);
                save_user_doc(&mut tx, &u).await?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_user_groups<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<GroupRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let paths = load_user(&mut tx, realm_id, user_id)
                .await?
                .map(|u| u.groups)
                .unwrap_or_default();
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT g.doc FROM kaisatsu.groups g
                 WHERE g.realm_id = $1 AND (
                     g.id IN (SELECT gm.group_id FROM kaisatsu.group_members gm
                              WHERE gm.realm_id = $1 AND gm.user_id = $2)
                     OR g.path = ANY($3) OR (g.doc->>'name') = ANY($3))
                 ORDER BY g.path",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(&paths)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn list_group_members<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        group_id: &'a str,
        max: usize,
    ) -> StoreFuture<'a, Vec<UserRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let path: Option<String> = sqlx::query_scalar(
                "SELECT path FROM kaisatsu.groups WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(path) = path else {
                return Err(KaisatsuError::NotFound("group not found".into()));
            };
            // The group is visible to the caller; its members are listed whatever their home.
            set_scope(&mut tx, realm_id, None).await?;
            let limit = i64::try_from(max.max(1)).unwrap_or(i64::MAX);
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT p.doc FROM kaisatsu.people p
                 WHERE p.realm_id = $1 AND (
                     p.id IN (SELECT gm.user_id FROM kaisatsu.group_members gm
                              WHERE gm.realm_id = $1 AND gm.group_id = $2)
                     OR p.doc->'groups' ? $3)
                 ORDER BY p.seq LIMIT $4",
            )
            .bind(realm_id)
            .bind(group_id)
            .bind(&path)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn upsert_role<'a>(
        &'a self,
        mut role: RoleRepresentation,
    ) -> StoreFuture<'a, RoleRepresentation> {
        Box::pin(async move {
            if role.id.is_empty() {
                role.id = new_id();
            }
            let mut tx = self.begin(&role.realm_id, None).await?;
            let existing: Option<String> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.roles
                 WHERE realm_id = $1 AND container_id IS NOT DISTINCT FROM $2 AND name = $3
                 RETURNING id",
            )
            .bind(&role.realm_id)
            .bind(&role.container_id)
            .bind(&role.name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(id) = existing {
                role.id = id;
            }
            sqlx::query(
                "INSERT INTO kaisatsu.roles (realm_id, id, container_id, client_role, name, doc)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&role.realm_id)
            .bind(&role.id)
            .bind(&role.container_id)
            .bind(role.client_role)
            .bind(&role.name)
            .bind(to_doc(&role)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(role)
        })
    }

    fn list_roles<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.roles WHERE realm_id = $1 AND (
                     ($2::text IS NULL AND NOT client_role)
                     OR ($2::text IS NOT NULL AND client_role AND container_id = $2))
                 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(container_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn delete_role<'a>(
        &'a self,
        realm_id: &'a str,
        container_id: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let res = sqlx::query(
                "DELETE FROM kaisatsu.roles
                 WHERE realm_id = $1 AND name = $2 AND container_id IS NOT DISTINCT FROM $3",
            )
            .bind(realm_id)
            .bind(role_name)
            .bind(container_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "DELETE FROM kaisatsu.role_mappings
                 WHERE realm_id = $1 AND role_name = $2 AND client_uuid IS NOT DISTINCT FROM $3",
            )
            .bind(realm_id)
            .bind(role_name)
            .bind(container_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn add_role_mapping<'a>(&'a self, mut mapping: RoleMappingRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if mapping.id.is_empty() {
                mapping.id = new_id();
            }
            let mut tx = self.begin(&mapping.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.role_mappings
                 WHERE realm_id = $1 AND principal_kind = $2 AND principal_id = $3
                   AND client_uuid IS NOT DISTINCT FROM $4 AND role_name = $5",
            )
            .bind(&mapping.realm_id)
            .bind(&mapping.principal_kind)
            .bind(&mapping.principal_id)
            .bind(&mapping.client_uuid)
            .bind(&mapping.role_name)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if mapping.principal_kind == "user"
                && let Some(mut u) =
                    load_user(&mut tx, &mapping.realm_id, &mapping.principal_id).await?
            {
                let client_id: Option<String> = match &mapping.client_uuid {
                    Some(uuid) => sqlx::query_scalar(
                        "SELECT client_id FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2",
                    )
                    .bind(&mapping.realm_id)
                    .bind(uuid)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage)?,
                    None => None,
                };
                if let Some(cid) = client_id {
                    let list = u.client_roles.entry(cid).or_default();
                    if !list.contains(&mapping.role_name) {
                        list.push(mapping.role_name.clone());
                    }
                } else if !u.realm_roles.contains(&mapping.role_name) {
                    u.realm_roles.push(mapping.role_name.clone());
                }
                save_user_doc(&mut tx, &u).await?;
            }
            sqlx::query(
                "INSERT INTO kaisatsu.role_mappings
                     (realm_id, id, principal_kind, principal_id, client_uuid, role_name, doc)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&mapping.realm_id)
            .bind(&mapping.id)
            .bind(&mapping.principal_kind)
            .bind(&mapping.principal_id)
            .bind(&mapping.client_uuid)
            .bind(&mapping.role_name)
            .bind(to_doc(&mapping)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn remove_role_mapping<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
        role_name: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.role_mappings
                 WHERE realm_id = $1 AND principal_kind = $2 AND principal_id = $3
                   AND client_uuid IS NOT DISTINCT FROM $4 AND role_name = $5",
            )
            .bind(realm_id)
            .bind(principal_kind)
            .bind(principal_id)
            .bind(client_uuid)
            .bind(role_name)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if principal_kind == "user"
                && let Some(mut u) = load_user(&mut tx, realm_id, principal_id).await?
            {
                let client_id: Option<String> = match client_uuid {
                    Some(uuid) => sqlx::query_scalar(
                        "SELECT client_id FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2",
                    )
                    .bind(realm_id)
                    .bind(uuid)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage)?,
                    None => None,
                };
                if let Some(cid) = client_id {
                    if let Some(list) = u.client_roles.get_mut(&cid) {
                        list.retain(|r| r != role_name);
                    }
                } else {
                    u.realm_roles.retain(|r| r != role_name);
                }
                save_user_doc(&mut tx, &u).await?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_role_mappings_for_principal<'a>(
        &'a self,
        realm_id: &'a str,
        principal_kind: &'a str,
        principal_id: &'a str,
        client_uuid: Option<&'a str>,
    ) -> StoreFuture<'a, Vec<RoleMappingRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.role_mappings
                 WHERE realm_id = $1 AND principal_kind = $2 AND principal_id = $3
                   AND client_uuid IS NOT DISTINCT FROM $4
                 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(principal_kind)
            .bind(principal_id)
            .bind(client_uuid)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn list_principals_with_client_role<'a>(
        &'a self,
        realm_id: &'a str,
        client_uuid: &'a str,
        role_name: &'a str,
        principal_kind: &'a str,
    ) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT principal_id FROM kaisatsu.role_mappings
                 WHERE realm_id = $1 AND client_uuid = $2 AND role_name = $3 AND principal_kind = $4
                 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(client_uuid)
            .bind(role_name)
            .bind(principal_kind)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(ids)
        })
    }

    fn upsert_client<'a>(
        &'a self,
        mut client: ClientRepresentation,
    ) -> StoreFuture<'a, ClientRepresentation> {
        Box::pin(async move {
            if client.id.is_empty() {
                client.id = new_id();
            }
            for m in &mut client.protocol_mappers {
                if m.id.is_empty() {
                    m.id = new_id();
                }
                m.realm_id = client.realm_id.clone();
            }
            let realm_id = client.realm_id.clone();
            let mut tx = self.begin(&realm_id, None).await?;
            let existing: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, secret_enc FROM kaisatsu.applications
                 WHERE realm_id = $1 AND (id = $2 OR client_id = $3)
                 ORDER BY (id = $2) DESC LIMIT 1",
            )
            .bind(&realm_id)
            .bind(&client.id)
            .bind(&client.client_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(row) = existing {
                let existing = self.client_from_row(&realm_id, row)?;
                if existing.id != client.id {
                    sqlx::query(
                        "DELETE FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2",
                    )
                    .bind(&realm_id)
                    .bind(&existing.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
                }
                client.id = existing.id;
                if client.secret.is_none() {
                    client.secret = existing.secret;
                }
            } else if !client.public_client && client.secret.is_none() {
                client.secret = Some(kaisatsu_crypto::random_handle_256());
            }
            if let Some(mark) = client.attributes.get("managed-by") {
                client.managed_by = Some(mark.clone());
            }
            self.write_client(&mut tx, &client).await?;
            // W3: confidential client with serviceAccountsEnabled gets `service-account-{clientId}`
            if !client.public_client && client.service_accounts_enabled {
                let sa = UserRepresentation {
                    id: new_id(),
                    realm_id: realm_id.clone(),
                    org_id: client.org_id.clone(),
                    username: format!("service-account-{}", client.client_id).to_ascii_lowercase(),
                    enabled: true,
                    email_verified: false,
                    service_account_client_id: Some(client.client_id.clone()),
                    created_timestamp: Some(Utc::now().timestamp_millis()),
                    ..Default::default()
                };
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM kaisatsu.people WHERE realm_id = $1 AND username_lower = $2)",
                )
                .bind(&realm_id)
                .bind(&sa.username)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
                if !exists {
                    insert_user(&mut tx, &sa).await?;
                }
            }
            notify(&mut tx, &realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(client)
        })
    }

    fn get_client_by_uuid<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let row: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, secret_enc FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(uuid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            row.map(|r| self.client_from_row(realm_id, r)).transpose()
        })
    }

    fn get_client_by_client_id<'a>(
        &'a self,
        realm_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let row: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, secret_enc FROM kaisatsu.applications WHERE realm_id = $1 AND client_id = $2",
            )
            .bind(realm_id)
            .bind(client_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            row.map(|r| self.client_from_row(realm_id, r)).transpose()
        })
    }

    fn list_clients<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        client_id_filter: Option<&'a str>,
        search: bool,
    ) -> StoreFuture<'a, Vec<ClientRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let rows: Vec<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, secret_enc FROM kaisatsu.applications
                 WHERE realm_id = $1 AND (
                     $2::text IS NULL
                     OR ($3 AND strpos(client_id, $2) > 0)
                     OR (NOT $3 AND client_id = $2))
                 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(client_id_filter)
            .bind(search)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            rows.into_iter()
                .map(|r| self.client_from_row(realm_id, r))
                .collect()
        })
    }

    fn delete_client<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        uuid: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let removed: Option<String> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.applications WHERE realm_id = $1 AND id = $2 RETURNING client_id",
            )
            .bind(realm_id)
            .bind(uuid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(client_id) = removed else {
                return Ok(false);
            };
            set_scope(&mut tx, realm_id, None).await?;
            // Only the client's own service account goes with it, never a person of that name.
            sqlx::query(
                "DELETE FROM kaisatsu.people
                 WHERE realm_id = $1 AND username_lower = $2 AND doc->>'serviceAccountClientId' = $3",
            )
            .bind(realm_id)
            .bind(format!("service-account-{client_id}").to_ascii_lowercase())
            .bind(&client_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            notify(&mut tx, realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(true)
        })
    }

    fn upsert_client_scope<'a>(
        &'a self,
        mut scope: ClientScopeRepresentation,
    ) -> StoreFuture<'a, ClientScopeRepresentation> {
        Box::pin(async move {
            if scope.id.is_empty() {
                scope.id = new_id();
            }
            for m in &mut scope.protocol_mappers {
                if m.id.is_empty() {
                    m.id = new_id();
                }
                m.realm_id = scope.realm_id.clone();
            }
            if let Some(mark) = scope.attributes.get("managed-by") {
                scope.managed_by = Some(mark.clone());
            }
            let mut tx = self.begin(&scope.realm_id, None).await?;
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT id FROM kaisatsu.client_scopes WHERE realm_id = $1 AND (id = $2 OR name = $3)
                 ORDER BY seq LIMIT 1",
            )
            .bind(&scope.realm_id)
            .bind(&scope.id)
            .bind(&scope.name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(id) = existing {
                scope.id = id;
            }
            sqlx::query("DELETE FROM kaisatsu.client_scopes WHERE realm_id = $1 AND id = $2")
                .bind(&scope.realm_id)
                .bind(&scope.id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.client_scopes (realm_id, id, name, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&scope.realm_id)
            .bind(&scope.id)
            .bind(&scope.name)
            .bind(to_doc(&scope)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            notify(&mut tx, &scope.realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(scope)
        })
    }

    fn get_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, Option<ClientScopeRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.client_scopes WHERE realm_id = $1 AND (id = $2 OR name = $2)
                 ORDER BY seq LIMIT 1",
            )
            .bind(realm_id)
            .bind(scope_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn list_client_scopes<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<ClientScopeRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.client_scopes WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn delete_client_scope<'a>(
        &'a self,
        realm_id: &'a str,
        scope_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let removed: Option<String> = sqlx::query_scalar(
                "DELETE FROM kaisatsu.client_scopes WHERE realm_id = $1 AND id = $2 RETURNING name",
            )
            .bind(realm_id)
            .bind(scope_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(name) = removed else {
                return Ok(false);
            };
            let rows: Vec<(Value, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT doc, secret_enc FROM kaisatsu.applications
                 WHERE realm_id = $1 AND (doc->'defaultClientScopes' ? $2 OR doc->'optionalClientScopes' ? $2)",
            )
            .bind(realm_id)
            .bind(&name)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            for row in rows {
                let mut c = self.client_from_row(realm_id, row)?;
                c.default_client_scopes.retain(|n| n != &name);
                c.optional_client_scopes.retain(|n| n != &name);
                let mut doc_client = c.clone();
                doc_client.secret = None;
                sqlx::query(
                    "UPDATE kaisatsu.applications SET doc = $3 WHERE realm_id = $1 AND id = $2",
                )
                .bind(realm_id)
                .bind(&c.id)
                .bind(to_doc(&doc_client)?)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            notify(&mut tx, realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(true)
        })
    }

    fn upsert_idp<'a>(&'a self, mut idp: IdentityProviderRepresentation) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if idp.internal_id.is_empty() {
                idp.internal_id = new_id();
            }
            let mut tx = self.begin(&idp.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.identity_providers WHERE realm_id = $1 AND alias = $2",
            )
            .bind(&idp.realm_id)
            .bind(&idp.alias)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.identity_providers (realm_id, alias, doc) VALUES ($1, $2, $3)",
            )
            .bind(&idp.realm_id)
            .bind(&idp.alias)
            .bind(to_doc(&idp)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            notify(&mut tx, &idp.realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_idp<'a>(
        &'a self,
        realm_id: &'a str,
        alias: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.identity_providers WHERE realm_id = $1 AND alias = $2",
            )
            .bind(realm_id)
            .bind(alias)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn list_idps<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderRepresentation>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.identity_providers WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn upsert_signing_key<'a>(&'a self, key: SigningKeyRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&key.realm_id, None).await?;
            insert_signing_key(&mut tx, &key).await?;
            notify(&mut tx, &key.realm_id).await?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_signing_keys<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<SigningKeyRecord>> {
        Box::pin(async move {
            self.ensure_realm_keys(realm_id).await?;
            let mut tx = self.begin(realm_id, None).await?;
            let rows: Vec<(Value, Vec<u8>)> = sqlx::query_as(
                "SELECT doc, encrypted_private_key FROM kaisatsu.signing_keys
                 WHERE realm_id = $1 ORDER BY priority DESC, seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            rows.into_iter().map(key_from_row).collect()
        })
    }

    fn ensure_signing_key<'a>(
        &'a self,
        realm_id: &'a str,
        algorithm: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move { self.ensure_keys(realm_id, &[algorithm]).await })
    }

    fn put_auth_session<'a>(&'a self, session: AuthSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&session.realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.auth_session WHERE realm_id = $1 AND id = $2")
                .bind(&session.realm_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.auth_session (realm_id, id, doc, expires_at) VALUES ($1, $2, $3, $4)",
            )
            .bind(&session.realm_id)
            .bind(&session.id)
            .bind(to_doc(&session)?)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_auth_session_by_id<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
    ) -> StoreFuture<'a, Option<AuthSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.auth_session WHERE realm_id = $1 AND id = $2 AND expires_at > now()",
            )
            .bind(realm_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn delete_auth_session<'a>(&'a self, realm_id: &'a str, id: &'a str) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.auth_session WHERE realm_id = $1 AND id = $2")
                .bind(realm_id)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn put_auth_code<'a>(&'a self, code: AuthorizationCodeRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&code.realm_id, None).await?;
            sqlx::query(
                "INSERT INTO kaisatsu.auth_code (realm_id, token_hash, consumed, doc, expires_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&code.realm_id)
            .bind(&code.code_hash)
            .bind(code.consumed)
            .bind(to_doc(&code)?)
            .bind(code.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn consume_auth_code<'a>(
        &'a self,
        realm_id: &'a str,
        code_hash: &'a str,
    ) -> StoreFuture<'a, (Option<AuthorizationCodeRecord>, bool)> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            // One UPDATE decides the race: only one caller flips `consumed`.
            let fresh: Option<Value> = sqlx::query_scalar(
                "UPDATE kaisatsu.auth_code
                 SET consumed = true, doc = jsonb_set(doc, '{consumed}', 'true'::jsonb)
                 WHERE realm_id = $1 AND token_hash = $2 AND NOT consumed
                 RETURNING doc",
            )
            .bind(realm_id)
            .bind(code_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let out = match fresh {
                Some(doc) => (Some(from_doc(doc)?), false),
                None => {
                    let used: Option<Value> = sqlx::query_scalar(
                        "SELECT doc FROM kaisatsu.auth_code WHERE realm_id = $1 AND token_hash = $2",
                    )
                    .bind(realm_id)
                    .bind(code_hash)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage)?;
                    match used {
                        Some(doc) => (Some(from_doc(doc)?), true),
                        None => (None, false),
                    }
                }
            };
            tx.commit().await.map_err(storage)?;
            Ok(out)
        })
    }

    fn put_par_request<'a>(&'a self, par: ParRequestRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&par.realm_id, None).await?;
            sqlx::query(
                "INSERT INTO kaisatsu.par_request (realm_id, token_hash, consumed, doc, expires_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&par.realm_id)
            .bind(&par.request_uri_hash)
            .bind(par.consumed)
            .bind(to_doc(&par)?)
            .bind(par.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn consume_par_request<'a>(
        &'a self,
        realm_id: &'a str,
        request_uri_hash: &'a str,
    ) -> StoreFuture<'a, Option<ParRequestRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "UPDATE kaisatsu.par_request
                 SET consumed = true, doc = jsonb_set(doc, '{consumed}', 'true'::jsonb)
                 WHERE realm_id = $1 AND token_hash = $2 AND NOT consumed AND expires_at > now()
                 RETURNING doc",
            )
            .bind(realm_id)
            .bind(request_uri_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn upsert_sso_session<'a>(&'a self, session: SsoSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let realm_id = session.realm_id.clone();
            let mut tx = self.begin(&realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.sso_session WHERE realm_id = $1 AND id = $2")
                .bind(&realm_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            let mut client_ids: Vec<&String> = session.clients.keys().collect();
            client_ids.sort();
            for cid in client_ids {
                let now = Utc::now();
                let cs = ClientSession {
                    id: new_id(),
                    realm_id: realm_id.clone(),
                    org_id: session.org_id.clone(),
                    sso_session_id: session.id.clone(),
                    client_id: cid.clone(),
                    user_id: session.user_id.clone(),
                    scopes: vec!["openid".into()],
                    created_at: now,
                    last_access: now,
                };
                sqlx::query(
                    "INSERT INTO kaisatsu.client_session
                         (realm_id, id, org_id, sso_session_id, client_id, user_id, doc)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)
                     ON CONFLICT (realm_id, sso_session_id, client_id) DO NOTHING",
                )
                .bind(&realm_id)
                .bind(&cs.id)
                .bind(&cs.org_id)
                .bind(&cs.sso_session_id)
                .bind(&cs.client_id)
                .bind(&cs.user_id)
                .bind(to_doc(&cs)?)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            sqlx::query(
                "INSERT INTO kaisatsu.sso_session (realm_id, id, org_id, token_hash, user_id, doc, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&realm_id)
            .bind(&session.id)
            .bind(&session.org_id)
            .bind(&session.handle_hash)
            .bind(&session.user_id)
            .bind(to_doc(&session)?)
            .bind(session.idle_expires_at.min(session.max_expires_at))
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let row: Option<(Value, String)> = sqlx::query_as(
                "SELECT doc, token_hash FROM kaisatsu.sso_session
                 WHERE realm_id = $1 AND id = $2 AND expires_at > now()",
            )
            .bind(realm_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            row.map(sso_from_row).transpose()
        })
    }

    fn get_sso_session_by_handle<'a>(
        &'a self,
        realm_id: &'a str,
        handle_hash: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let row: Option<(Value, String)> = sqlx::query_as(
                "SELECT doc, token_hash FROM kaisatsu.sso_session
                 WHERE realm_id = $1 AND token_hash = $2 AND expires_at > now()
                 ORDER BY seq LIMIT 1",
            )
            .bind(realm_id)
            .bind(handle_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            row.map(sso_from_row).transpose()
        })
    }

    fn list_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let rows: Vec<(Value, String)> = sqlx::query_as(
                "SELECT doc, token_hash FROM kaisatsu.sso_session
                 WHERE realm_id = $1 AND user_id = $2 AND expires_at > now() ORDER BY seq",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            rows.into_iter().map(sso_from_row).collect()
        })
    }

    fn delete_sso_session<'a>(
        &'a self,
        realm_id: &'a str,
        session_id: &'a str,
    ) -> StoreFuture<'a, Option<SsoSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let row: Option<(Value, String)> = sqlx::query_as(
                "DELETE FROM kaisatsu.sso_session WHERE realm_id = $1 AND id = $2 RETURNING doc, token_hash",
            )
            .bind(realm_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(row) = row else {
                return Ok(None);
            };
            sqlx::query(
                "DELETE FROM kaisatsu.client_session WHERE realm_id = $1 AND sso_session_id = $2",
            )
            .bind(realm_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            revoke_families!(
                tx,
                "realm_id = $1 AND sso_session_id = $2",
                realm_id,
                session_id
            )?;
            tx.commit().await.map_err(storage)?;
            Ok(Some(sso_from_row(row)?))
        })
    }

    fn delete_user_sso_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<SsoSession>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let rows: Vec<(Value, String)> = sqlx::query_as(
                "DELETE FROM kaisatsu.sso_session WHERE realm_id = $1 AND user_id = $2 RETURNING doc, token_hash",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query("DELETE FROM kaisatsu.client_session WHERE realm_id = $1 AND user_id = $2")
                .bind(realm_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            revoke_families!(tx, "realm_id = $1 AND user_id = $2", realm_id, user_id)?;
            tx.commit().await.map_err(storage)?;
            rows.into_iter().map(sso_from_row).collect()
        })
    }

    fn upsert_offline_session<'a>(&'a self, session: OfflineSession) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&session.realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.offline_session WHERE realm_id = $1 AND id = $2")
                .bind(&session.realm_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.offline_session (realm_id, id, org_id, user_id, client_id, doc, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&session.realm_id)
            .bind(&session.id)
            .bind(&session.org_id)
            .bind(&session.user_id)
            .bind(&session.client_id)
            .bind(to_doc(&session)?)
            .bind(session.idle_expires_at.min(session.max_expires_at))
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn delete_user_client_offline_sessions<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.offline_session WHERE realm_id = $1 AND user_id = $2 AND client_id = $3",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(client_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            revoke_families!(
                tx,
                "realm_id = $1 AND user_id = $2 AND client_id = $3",
                realm_id,
                user_id,
                client_id
            )?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn upsert_refresh_family<'a>(&'a self, family: RefreshTokenFamily) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&family.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.refresh_token_family WHERE realm_id = $1 AND id = $2",
            )
            .bind(&family.realm_id)
            .bind(&family.id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.refresh_token_family
                     (realm_id, id, token_hash, sso_session_id, user_id, client_id, revoked, doc, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&family.realm_id)
            .bind(&family.id)
            .bind(&family.current_jti_hash)
            .bind(&family.sso_session_id)
            .bind(&family.user_id)
            .bind(&family.client_id)
            .bind(family.revoked)
            .bind(to_doc(&family)?)
            .bind(family.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_refresh_family<'a>(
        &'a self,
        realm_id: &'a str,
        family_id: &'a str,
    ) -> StoreFuture<'a, Option<RefreshTokenFamily>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.refresh_token_family WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(family_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn rotate_refresh_family<'a>(
        &'a self,
        family: RefreshTokenFamily,
        expected_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(&family.realm_id, None).await?;
            let res = sqlx::query(
                "UPDATE kaisatsu.refresh_token_family
                 SET token_hash = $4, revoked = $5, doc = $6, expires_at = $7
                 WHERE realm_id = $1 AND id = $2 AND token_hash = $3 AND NOT revoked",
            )
            .bind(&family.realm_id)
            .bind(&family.id)
            .bind(expected_hash)
            .bind(&family.current_jti_hash)
            .bind(family.revoked)
            .bind(to_doc(&family)?)
            .bind(family.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(res.rows_affected() == 1)
        })
    }

    fn revoke_refresh_families_for_session<'a>(
        &'a self,
        realm_id: &'a str,
        sso_session_id: &'a str,
        client_id: Option<&'a str>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            revoke_families!(
                tx,
                "realm_id = $1 AND sso_session_id = $2 AND ($3::text IS NULL OR client_id = $3)",
                realm_id,
                sso_session_id,
                client_id
            )?;
            if let Some(cid) = client_id {
                sqlx::query(
                    "DELETE FROM kaisatsu.client_session
                     WHERE realm_id = $1 AND sso_session_id = $2 AND client_id = $3",
                )
                .bind(realm_id)
                .bind(sso_session_id)
                .bind(cid)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn mark_token_revoked<'a>(&'a self, rec: RevokedTokenRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&rec.realm_id, None).await?;
            let inserted: Option<String> = sqlx::query_scalar(
                "INSERT INTO kaisatsu.revoked_token (realm_id, token_hash, expires_at) VALUES ($1, $2, $3)
                 ON CONFLICT (realm_id, token_hash) DO NOTHING RETURNING token_hash",
            )
            .bind(&rec.realm_id)
            .bind(&rec.jti_hash)
            .bind(rec.expires_at)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            if inserted.is_none() {
                return Err(KaisatsuError::Conflict(
                    "token or jti already used/revoked".into(),
                ));
            }
            Ok(())
        })
    }

    fn is_token_revoked<'a>(
        &'a self,
        realm_id: &'a str,
        jti_hash: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let revoked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM kaisatsu.revoked_token
                 WHERE realm_id = $1 AND token_hash = $2 AND expires_at > now())",
            )
            .bind(realm_id)
            .bind(jti_hash)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(revoked)
        })
    }

    fn get_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, Option<LoginFailureRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.login_failure WHERE realm_id = $1 AND target_key = $2",
            )
            .bind(realm_id)
            .bind(target_key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn upsert_login_failure<'a>(&'a self, rec: LoginFailureRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&rec.realm_id, None).await?;
            sqlx::query(
                "INSERT INTO kaisatsu.login_failure (realm_id, target_key, doc) VALUES ($1, $2, $3)
                 ON CONFLICT (realm_id, target_key) DO UPDATE SET doc = EXCLUDED.doc",
            )
            .bind(&rec.realm_id)
            .bind(&rec.target_key)
            .bind(to_doc(&rec)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn clear_login_failure<'a>(
        &'a self,
        realm_id: &'a str,
        target_key: &'a str,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.login_failure WHERE realm_id = $1 AND target_key = $2",
            )
            .bind(realm_id)
            .bind(target_key)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn upsert_consent<'a>(&'a self, mut consent: ConsentRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if consent.id.is_empty() {
                consent.id = new_id();
            }
            let mut tx = self.begin(&consent.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.consent WHERE realm_id = $1 AND user_id = $2 AND client_id = $3",
            )
            .bind(&consent.realm_id)
            .bind(&consent.user_id)
            .bind(&consent.client_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.consent (realm_id, user_id, client_id, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&consent.realm_id)
            .bind(&consent.user_id)
            .bind(&consent.client_id)
            .bind(to_doc(&consent)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_consents<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<ConsentRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.consent WHERE realm_id = $1 AND user_id = $2 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn delete_consent<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
        client_id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let res = sqlx::query(
                "DELETE FROM kaisatsu.consent WHERE realm_id = $1 AND user_id = $2 AND client_id = $3",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(client_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "DELETE FROM kaisatsu.offline_session WHERE realm_id = $1 AND user_id = $2 AND client_id = $3",
            )
            .bind(realm_id)
            .bind(user_id)
            .bind(client_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            revoke_families!(
                tx,
                "realm_id = $1 AND user_id = $2 AND client_id = $3",
                realm_id,
                user_id,
                client_id
            )?;
            tx.commit().await.map_err(storage)?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn upsert_required_action<'a>(&'a self, mut ra: RequiredActionConfig) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if ra.id.is_empty() {
                ra.id = format!("{}:{}", ra.realm_id, ra.alias);
            }
            let mut tx = self.begin(&ra.realm_id, None).await?;
            sqlx::query("DELETE FROM kaisatsu.required_actions WHERE realm_id = $1 AND alias = $2")
                .bind(&ra.realm_id)
                .bind(&ra.alias)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.required_actions (realm_id, alias, doc) VALUES ($1, $2, $3)",
            )
            .bind(&ra.realm_id)
            .bind(&ra.alias)
            .bind(to_doc(&ra)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_required_actions<'a>(
        &'a self,
        realm_id: &'a str,
    ) -> StoreFuture<'a, Vec<RequiredActionConfig>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.required_actions WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn upsert_branding<'a>(&'a self, branding: BrandingBlock) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let branding = branding.sanitised();
            let org_key = branding.org_id.clone().unwrap_or_default();
            let mut tx = self.begin(&branding.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.branding_blocks WHERE realm_id = $1 AND org_key = $2",
            )
            .bind(&branding.realm_id)
            .bind(&org_key)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.branding_blocks (realm_id, org_key, org_id, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&branding.realm_id)
            .bind(&org_key)
            .bind(&branding.org_id)
            .bind(to_doc(&branding)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_branding<'a>(
        &'a self,
        realm_id: &'a str,
        org_id: Option<&'a str>,
    ) -> StoreFuture<'a, Option<BrandingBlock>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            // The organization's own block first, the realm's block otherwise.
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.branding_blocks
                 WHERE realm_id = $1 AND (org_key = '' OR ($2::text IS NOT NULL AND org_key = $2))
                 ORDER BY (org_key = '') LIMIT 1",
            )
            .bind(realm_id)
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn append_event<'a>(&'a self, mut event: AuditEvent) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if event.id.is_empty() {
                event.id = new_id();
            }
            let mut tx = self.begin(&event.realm_id, None).await?;
            sqlx::query(
                "INSERT INTO kaisatsu.events (realm_id, id, org_id, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&event.realm_id)
            .bind(&event.id)
            .bind(&event.org_id)
            .bind(to_doc(&event)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AuditEvent>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.events WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn append_admin_event<'a>(&'a self, mut event: AdminEvent) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if event.id.is_empty() {
                event.id = new_id();
            }
            let mut tx = self.begin(&event.realm_id, None).await?;
            sqlx::query(
                "INSERT INTO kaisatsu.admin_events (realm_id, id, org_id, doc) VALUES ($1, $2, $3, $4)",
            )
            .bind(&event.realm_id)
            .bind(&event.id)
            .bind(&event.org_id)
            .bind(to_doc(&event)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn list_admin_events<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<AdminEvent>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.admin_events WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn insert_webauthn_credential<'a>(
        &'a self,
        mut cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, WebAuthnCredentialRecord> {
        Box::pin(async move {
            if cred.id.is_empty() {
                cred.id = new_id();
            }
            let mut tx = self.begin(&cred.realm_id, None).await?;
            // Include public_key_cose_json inside the stored JSON doc in Postgres.
            let mut doc = to_doc(&cred)?;
            if let Some(obj) = doc.as_object_mut() {
                obj.insert(
                    "publicKeyCoseJson".into(),
                    Value::String(cred.public_key_cose_json.clone()),
                );
            }
            sqlx::query(
                "INSERT INTO kaisatsu.webauthn_credential
                     (realm_id, id, org_id, user_id, user_handle, credential_id, sign_count, doc, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&cred.realm_id)
            .bind(&cred.id)
            .bind(&cred.org_id)
            .bind(&cred.user_id)
            .bind(&cred.user_handle)
            .bind(&cred.credential_id)
            .bind(i64::from(cred.sign_count))
            .bind(doc)
            .bind(cred.created_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(cred)
        })
    }

    fn update_webauthn_credential<'a>(
        &'a self,
        cred: WebAuthnCredentialRecord,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(&cred.realm_id, None).await?;
            let mut doc = to_doc(&cred)?;
            if let Some(obj) = doc.as_object_mut() {
                obj.insert(
                    "publicKeyCoseJson".into(),
                    Value::String(cred.public_key_cose_json.clone()),
                );
            }
            let res = sqlx::query(
                "UPDATE kaisatsu.webauthn_credential
                 SET sign_count = $3, doc = $4
                 WHERE realm_id = $1 AND id = $2",
            )
            .bind(&cred.realm_id)
            .bind(&cred.id)
            .bind(i64::from(cred.sign_count))
            .bind(doc)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if res.rows_affected() == 0 {
                return Err(KaisatsuError::NotFound(
                    "WebAuthn credential not found".into(),
                ));
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_webauthn_by_credential_id<'a>(
        &'a self,
        realm_id: &'a str,
        credential_id: &'a str,
    ) -> StoreFuture<'a, Option<WebAuthnCredentialRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.webauthn_credential
                 WHERE realm_id = $1 AND credential_id = $2",
            )
            .bind(realm_id)
            .bind(credential_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            doc.map(webauthn_from_doc).transpose()
        })
    }

    fn list_webauthn_credentials<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<WebAuthnCredentialRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.webauthn_credential
                 WHERE realm_id = $1 AND user_id = $2 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            docs.into_iter().map(webauthn_from_doc).collect()
        })
    }

    fn delete_webauthn_credential<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let res = sqlx::query(
                "DELETE FROM kaisatsu.webauthn_credential WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(res.rows_affected() > 0)
        })
    }

    fn update_user_with_outbox<'a>(
        &'a self,
        mut user: UserRepresentation,
        mut outbox: EmailOutboxRecord,
    ) -> StoreFuture<'a, UserRepresentation> {
        Box::pin(async move {
            user.username = user.username.to_ascii_lowercase();
            if let Some(em) = &mut user.email {
                *em = em.to_ascii_lowercase();
            }
            if outbox.id.is_empty() {
                outbox.id = new_id();
            }
            let mut tx = self.begin(&user.realm_id, None).await?;
            if load_user(&mut tx, &user.realm_id, &user.id)
                .await?
                .is_none()
            {
                return Err(KaisatsuError::NotFound("user not found".into()));
            }
            if user_key_taken(&mut tx, &user, Some(&user.id)).await? {
                return Err(KaisatsuError::Conflict(
                    "another user already holds that username or email".into(),
                ));
            }
            sqlx::query(
                "UPDATE kaisatsu.people SET org_id = $3, username_lower = $4, email_lower = $5,
                     transient = $6, doc = $7
                 WHERE realm_id = $1 AND id = $2",
            )
            .bind(&user.realm_id)
            .bind(&user.id)
            .bind(&user.org_id)
            .bind(&user.username)
            .bind(&user.email)
            .bind(user.transient)
            .bind(to_doc(&user)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            insert_outbox(&mut tx, &outbox).await?;
            tx.commit().await.map_err(storage)?;
            Ok(user)
        })
    }

    fn enqueue_outbox<'a>(&'a self, mut outbox: EmailOutboxRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if outbox.id.is_empty() {
                outbox.id = new_id();
            }
            let mut tx = self.begin(&outbox.realm_id, None).await?;
            insert_outbox(&mut tx, &outbox).await?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn claim_outbox_batch<'a>(
        &'a self,
        realm_id: &'a str,
        limit: usize,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        Box::pin(async move {
            let cap = i64::try_from(limit.clamp(1, 50)).unwrap_or(50);
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "UPDATE kaisatsu.email_outbox
                 SET status = 'sending',
                     attempts = attempts + 1,
                     doc = jsonb_set(
                         jsonb_set(doc, '{status}', '\"sending\"'::jsonb),
                         '{attempts}',
                         to_jsonb(attempts + 1)
                     )
                 WHERE (realm_id, id) IN (
                     SELECT realm_id, id FROM kaisatsu.email_outbox
                     WHERE realm_id = $1 AND status = 'pending' AND next_attempt_at <= now()
                     ORDER BY seq
                     LIMIT $2
                     FOR UPDATE SKIP LOCKED
                 )
                 RETURNING doc",
            )
            .bind(realm_id)
            .bind(cap)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn complete_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        id: &'a str,
        sent: bool,
        error: Option<String>,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        max_attempts: u32,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.email_outbox WHERE realm_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(realm_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(doc) = doc {
                let mut rec: EmailOutboxRecord = from_doc(doc)?;
                if sent {
                    rec.status = "sent".into();
                    rec.sent_at = Some(Utc::now());
                    rec.last_error = None;
                } else {
                    rec.last_error = error.clone();
                    if rec.attempts >= max_attempts {
                        rec.status = "failed".into();
                    } else {
                        rec.status = "pending".into();
                        rec.next_attempt_at = next_attempt_at
                            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(5));
                    }
                }
                sqlx::query(
                    "UPDATE kaisatsu.email_outbox
                     SET status = $3, last_error = $4, next_attempt_at = $5, doc = $6
                     WHERE realm_id = $1 AND id = $2",
                )
                .bind(realm_id)
                .bind(id)
                .bind(&rec.status)
                .bind(&rec.last_error)
                .bind(rec.next_attempt_at)
                .bind(to_doc(&rec)?)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn retry_outbox_row<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
        id: &'a str,
    ) -> StoreFuture<'a, Option<EmailOutboxRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.email_outbox WHERE realm_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(realm_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            let Some(doc) = doc else {
                return Ok(None);
            };
            let mut rec: EmailOutboxRecord = from_doc(doc)?;
            rec.status = "pending".into();
            rec.attempts = 0;
            rec.last_error = None;
            rec.next_attempt_at = Utc::now();
            sqlx::query(
                "UPDATE kaisatsu.email_outbox
                 SET status = 'pending', attempts = 0, last_error = NULL,
                     next_attempt_at = $3, doc = $4
                 WHERE realm_id = $1 AND id = $2",
            )
            .bind(realm_id)
            .bind(id)
            .bind(rec.next_attempt_at)
            .bind(to_doc(&rec)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(Some(rec))
        })
    }

    fn list_outbox_rows<'a>(
        &'a self,
        realm_id: &'a str,
        allowed_orgs: Option<&'a [String]>,
    ) -> StoreFuture<'a, Vec<EmailOutboxRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, allowed_orgs).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.email_outbox WHERE realm_id = $1 ORDER BY seq",
            )
            .bind(realm_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn upsert_idp_link<'a>(&'a self, mut link: IdentityProviderLinkRecord) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if link.id.is_empty() {
                link.id = new_id();
            }
            let mut tx = self.begin(&link.realm_id, None).await?;
            sqlx::query(
                "DELETE FROM kaisatsu.idp_links
                 WHERE realm_id = $1 AND idp_alias = $2 AND external_user_id = $3",
            )
            .bind(&link.realm_id)
            .bind(&link.idp_alias)
            .bind(&link.external_user_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO kaisatsu.idp_links
                     (realm_id, id, org_id, user_id, idp_alias, external_user_id, doc, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(&link.realm_id)
            .bind(&link.id)
            .bind(&link.org_id)
            .bind(&link.user_id)
            .bind(&link.idp_alias)
            .bind(&link.external_user_id)
            .bind(to_doc(&link)?)
            .bind(link.created_at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            Ok(())
        })
    }

    fn get_idp_link_by_external<'a>(
        &'a self,
        realm_id: &'a str,
        idp_alias: &'a str,
        external_user_id: &'a str,
    ) -> StoreFuture<'a, Option<IdentityProviderLinkRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let doc: Option<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.idp_links
                 WHERE realm_id = $1 AND idp_alias = $2 AND external_user_id = $3",
            )
            .bind(realm_id)
            .bind(idp_alias)
            .bind(external_user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            opt_from_doc(doc)
        })
    }

    fn list_idp_links_for_user<'a>(
        &'a self,
        realm_id: &'a str,
        user_id: &'a str,
    ) -> StoreFuture<'a, Vec<IdentityProviderLinkRecord>> {
        Box::pin(async move {
            let mut tx = self.begin(realm_id, None).await?;
            let docs: Vec<Value> = sqlx::query_scalar(
                "SELECT doc FROM kaisatsu.idp_links
                 WHERE realm_id = $1 AND user_id = $2 ORDER BY seq",
            )
            .bind(realm_id)
            .bind(user_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            from_docs(docs)
        })
    }

    fn sweep_expired<'a>(&'a self) -> StoreFuture<'a, u64> {
        Box::pin(async move {
            let mut total = 0u64;
            // Bounded: under a constant inflow of expiring rows the next call continues.
            for _ in 0..100 {
                let n = self.sweep_expired_batch(1000).await?;
                total += n;
                if n == 0 {
                    break;
                }
            }
            Ok(total)
        })
    }
}

fn webauthn_from_doc(mut doc: Value) -> Result<WebAuthnCredentialRecord> {
    let cose = doc
        .as_object_mut()
        .and_then(|o| o.remove("publicKeyCoseJson"))
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("publicKeyCoseJson".into(), Value::String(cose.clone()));
    }
    let mut rec: WebAuthnCredentialRecord = from_doc(doc)?;
    rec.public_key_cose_json = cose;
    Ok(rec)
}

async fn insert_outbox(conn: &mut PgConnection, outbox: &EmailOutboxRecord) -> Result<()> {
    sqlx::query(
        "INSERT INTO kaisatsu.email_outbox
             (realm_id, id, org_id, status, attempts, last_error, next_attempt_at, doc, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&outbox.realm_id)
    .bind(&outbox.id)
    .bind(&outbox.org_id)
    .bind(&outbox.status)
    .bind(i32::try_from(outbox.attempts).unwrap_or(0))
    .bind(&outbox.last_error)
    .bind(outbox.next_attempt_at)
    .bind(to_doc(outbox)?)
    .bind(outbox.created_at)
    .execute(&mut *conn)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn insert_credential(conn: &mut PgConnection, cred: &CredentialRecord) -> Result<()> {
    sqlx::query(
        "INSERT INTO kaisatsu.credentials (realm_id, id, user_id, credential_type, doc)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&cred.realm_id)
    .bind(&cred.id)
    .bind(&cred.user_id)
    .bind(&cred.credential_type)
    .bind(to_doc(cred)?)
    .execute(&mut *conn)
    .await
    .map_err(storage)?;
    Ok(())
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
         WHERE c.relkind IN ('r', 'p')
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

    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'kaisatsu' AND c.relkind IN ('r', 'p')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| KaisatsuError::Storage(format!("boot check schema query failed: {e}")))?;
    if tables == 0 {
        return Err(KaisatsuError::Storage(
            "boot check refused: schema kaisatsu has no tables; run `kaisatsu migrate` first (W2)"
                .into(),
        ));
    }

    let unforced: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'kaisatsu'
           AND c.relkind IN ('r', 'p')
           AND (NOT c.relrowsecurity OR NOT c.relforcerowsecurity)
         ORDER BY 1",
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
