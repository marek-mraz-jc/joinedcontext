#![forbid(unsafe_code)]
//! Configuration of a standalone `daiya` (R2, R8, R11): read from a JSON file the binary is
//! pointed at, passed to the library as a struct. Secrets are never in it: the Postgres
//! password, the API token digests and the datasource secrets are files in the secrets mount.
//!
//! ```json
//! {
//!   "listen": "0.0.0.0:3000",
//!   "mount": { "scheme": "https", "host": "dash.example.org", "pathPrefix": "" },
//!   "organization": { "id": "0190...", "name": "Main Org." },
//!   "store": { "kind": "postgres", "url": "postgres://daiya_app_login@db:5432/daiya",
//!              "passwordFile": "/run/secrets/daiya-db", "migrate": {
//!              "url": "postgres://daiya_migrate_login@db:5432/daiya",
//!              "passwordFile": "/run/secrets/daiya-migrate" } },
//!   "tokensFile": "/run/secrets/daiya-tokens.json",
//!   "secretsDir": "/run/secrets/datasources",
//!   "provisioningDir": "/etc/daiya/provisioning",
//!   "outbound": { "allowHttp": true, "allowedNetworks": ["10.43.0.0/16"] },
//!   "serverAdmin": { "user": "admin", "passwordFile": "/run/secrets/daiya-admin" }
//! }
//! ```

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use daiya_api::{DaiyaState, TokenBinding};
use daiya_import::{ProvisioningLoadReport, ProvisioningTarget, load_provisioning_dir};
use daiya_model::{
    CrError, CrResult, DaiyaStore, GrafanaRole, InstallationId, InstanceId, OrgId,
    PolicyAuthorizer, RoleBinding, Subject,
};
use daiya_query::{FileSecretResolver, MapSecretResolver, OutboundPolicy};
use daiya_store_memory::MemoryStore;
use daiya_store_pg::{PgPoolConfig, PgStore};
use ipnet::IpNet;
use jc_router::{ClientAddr, Mount};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The actor the CLI subcommands act as: the operator who holds the configuration.
pub const CLI_ACTOR: &str = "daiya-cli";

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 3000))
}

/// Top-level configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaiyaConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub mount: MountConfig,
    #[serde(default)]
    pub organization: OrganizationConfig,
    /// Organizations held from the start under their Grafana numbers (`{"id": 4, "name":
    /// "bbsk"}`), so provisioning files can name them by `orgId`.
    #[serde(default)]
    pub organizations: Vec<NumberedOrganization>,
    #[serde(default)]
    pub store: StoreConfig,
    /// JSON list of [`TokenEntry`]: API credentials by SHA-256, never the token itself.
    #[serde(default)]
    pub tokens_file: Option<PathBuf>,
    /// Datasource secrets: `<dir>/<org-uuid>/<secretRef>`.
    #[serde(default)]
    pub secrets_dir: Option<PathBuf>,
    /// Grafana provisioning root (`datasources/`, `dashboards/`), loaded at start.
    #[serde(default)]
    pub provisioning_dir: Option<PathBuf>,
    /// `$NAME` values of the provisioning files: never read from the file, the binary fills it
    /// from its `DAIYA_VAR_*` environment variables ([`provisioning_vars_from`]).
    #[serde(skip)]
    pub provisioning_vars: BTreeMap<String, String>,
    #[serde(default)]
    pub outbound: OutboundConfig,
    /// Grafana's local administrator (`provision.sh`, the Portal's `JC_PORTAL_GRAFANA_USER`):
    /// Basic auth, admin of every organization.
    #[serde(default)]
    pub server_admin: Option<ServerAdminConfig>,
}

/// The server administrator's name and the file holding its password (one line).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerAdminConfig {
    pub user: String,
    pub password_file: PathBuf,
}

impl Default for DaiyaConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            mount: MountConfig::default(),
            organization: OrganizationConfig::default(),
            organizations: Vec::new(),
            store: StoreConfig::default(),
            tokens_file: None,
            secrets_dir: None,
            provisioning_dir: None,
            provisioning_vars: BTreeMap::new(),
            outbound: OutboundConfig::default(),
            server_admin: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MountConfig {
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default)]
    pub path_prefix: String,
}

fn default_scheme() -> String {
    "https".into()
}

fn default_host() -> String {
    "localhost".into()
}

impl Default for MountConfig {
    fn default() -> Self {
        Self {
            scheme: default_scheme(),
            host: default_host(),
            path_prefix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizationConfig {
    #[serde(default = "default_org_id")]
    pub id: OrgId,
    #[serde(default = "default_org_name")]
    pub name: String,
}

/// One organization held from the start: its Grafana number (2 and up) and its name.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NumberedOrganization {
    pub id: i64,
    pub name: String,
}

/// Holds `organizations` in `state`'s directory (see `compat::declare_organization`).
///
/// # Errors
/// A number below 2, or a number or name held twice.
pub fn declare_organizations(
    state: &DaiyaState,
    organizations: &[NumberedOrganization],
) -> CrResult<()> {
    for org in organizations {
        daiya_api::compat::declare_organization(state, org.id, &org.name)?;
    }
    Ok(())
}

fn default_org_id() -> OrgId {
    OrgId::from_u128(1)
}

fn default_org_name() -> String {
    "Main Org.".into()
}

impl Default for OrganizationConfig {
    fn default() -> Self {
        Self {
            id: default_org_id(),
            name: default_org_name(),
        }
    }
}

/// Where dashboards live.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StoreConfig {
    /// In memory: everything is gone when the process ends.
    #[default]
    Memory,
    /// Postgres with forced RLS, on the application role.
    #[serde(rename_all = "camelCase")]
    Postgres {
        /// `postgres://role@host:port/database`, no password.
        url: String,
        password_file: PathBuf,
        #[serde(default = "default_max_connections")]
        max_connections: u32,
        /// Run the migration first, as the owning role.
        #[serde(default)]
        migrate: Option<MigrateConfig>,
    },
}

fn default_max_connections() -> u32 {
    8
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrateConfig {
    pub url: String,
    pub password_file: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundConfig {
    #[serde(default)]
    pub allow_http: bool,
    /// CIDRs of non-public networks datasources may reach (for example the service network).
    #[serde(default)]
    pub allowed_networks: Vec<String>,
}

/// One API credential of the tokens file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenEntry {
    /// Lower-case hex SHA-256 of the token.
    pub token_sha256: String,
    pub actor: String,
    pub role: GrafanaRole,
    /// Scope the role to one folder (organization-wide when absent).
    #[serde(default)]
    pub folder_uid: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

fn config_err(msg: impl Into<String>) -> CrError {
    CrError::BadRequest(format!("configuration: {}", msg.into()))
}

impl DaiyaConfig {
    /// Reads and parses a configuration file (unknown keys are refused).
    pub fn from_file(path: &Path) -> CrResult<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| config_err(format!("cannot read '{}': {e}", path.display())))?;
        serde_json::from_str(&text).map_err(|e| config_err(format!("'{}': {e}", path.display())))
    }

    pub fn mount(&self) -> CrResult<Mount> {
        Mount::new(
            "daiya",
            self.mount.scheme.clone(),
            self.mount.host.clone(),
            self.mount.path_prefix.clone(),
        )
        .map_err(|e| config_err(e.to_string()))
    }

    pub fn outbound_policy(&self) -> CrResult<OutboundPolicy> {
        let allowed_networks = self
            .outbound
            .allowed_networks
            .iter()
            .map(|cidr| {
                cidr.trim()
                    .parse::<IpNet>()
                    .map_err(|_| config_err(format!("'{cidr}' is not a CIDR")))
            })
            .collect::<CrResult<Vec<_>>>()?;
        Ok(OutboundPolicy {
            allow_http: self.outbound.allow_http,
            allowed_networks,
            ..OutboundPolicy::default()
        })
    }
}

/// Reads the tokens file: a JSON list of [`TokenEntry`].
pub fn read_tokens_file(path: &Path) -> CrResult<Vec<TokenEntry>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| config_err(format!("cannot read tokens file '{}': {e}", path.display())))?;
    let entries: Vec<TokenEntry> = serde_json::from_str(&text)
        .map_err(|e| config_err(format!("tokens file '{}': {e}", path.display())))?;
    for entry in &entries {
        let digest = entry.token_sha256.trim();
        if digest.len() != 64
            || !digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return Err(config_err(format!(
                "token of '{}' must be a lower-case hex SHA-256",
                entry.actor
            )));
        }
        if entry.actor.trim().is_empty()
            || entry.actor == CLI_ACTOR
            || entry.actor == daiya_model::SERVER_ADMIN_ACTOR
        {
            return Err(config_err(format!(
                "token actor '{}' is empty or reserved",
                entry.actor
            )));
        }
    }
    Ok(entries)
}

/// Opens the configured store. Postgres: the migration first when configured, then the
/// application pool and the boot check, which refuses a role or schema without forced RLS.
pub async fn open_store(cfg: &StoreConfig) -> CrResult<Arc<dyn DaiyaStore>> {
    match cfg {
        StoreConfig::Memory => Ok(Arc::new(MemoryStore::new())),
        StoreConfig::Postgres {
            url,
            password_file,
            max_connections,
            migrate,
        } => {
            if let Some(m) = migrate {
                let owner =
                    PgPoolConfig::from_url_and_secret_file(&m.url, &m.password_file, 1).await?;
                let pool = owner.connect().await?;
                daiya_store_pg::migrate(&pool).await?;
                pool.close().await;
            }
            let app = PgPoolConfig::from_url_and_secret_file(url, password_file, *max_connections)
                .await?;
            let store = PgStore::new(app.connect().await?);
            store.verify_rls_enforced().await?;
            Ok(Arc::new(store))
        }
    }
}

/// A started instance: its state, the CLI's request context and what provisioning wrote.
pub struct Built {
    pub state: DaiyaState,
    pub provisioning: Option<ProvisioningLoadReport>,
}

/// The provisioning variables among `vars` (the process environment, handed in by the binary):
/// `DAIYA_VAR_NAME` becomes `NAME`; every other variable is left out.
pub fn provisioning_vars_from(
    vars: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    vars.into_iter()
        .filter_map(|(k, v)| k.strip_prefix("DAIYA_VAR_").map(|n| (n.to_owned(), v)))
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

/// Builds the state of a standalone instance from `cfg`: store, API tokens and their roles,
/// the CLI actor (admin of the organization), datasource secrets, the outbound policy, and
/// the provisioning directory loaded into the store.
pub async fn build_state(cfg: &DaiyaConfig) -> CrResult<Built> {
    let store = open_store(&cfg.store).await?;
    let org = cfg.organization.id;
    let entries = match &cfg.tokens_file {
        Some(path) => read_tokens_file(path)?,
        None => Vec::new(),
    };
    let mut bindings = vec![RoleBinding::from_grafana_role(
        org,
        CLI_ACTOR,
        GrafanaRole::Admin,
        None,
    )];
    let mut tokens = Vec::with_capacity(entries.len());
    for entry in entries {
        bindings.push(RoleBinding::from_grafana_role(
            org,
            entry.actor.clone(),
            entry.role,
            entry.folder_uid.clone(),
        ));
        let mut subject = Subject::authenticated(entry.actor.clone(), vec![org], Vec::new());
        subject.read_only = entry.read_only;
        tokens.push(TokenBinding {
            token_sha256: entry.token_sha256.trim().to_owned(),
            subject,
        });
    }
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        org,
        InstanceId::from_u128(1),
        store,
        Arc::new(PolicyAuthorizer::new(bindings)),
    )
    .with_tokens(tokens);
    state.secret_resolver = match &cfg.secrets_dir {
        Some(dir) => Arc::new(FileSecretResolver::new(dir.clone())),
        None => Arc::new(MapSecretResolver::new()),
    };
    state.outbound_policy = cfg.outbound_policy()?;
    if let Some(admin) = &cfg.server_admin {
        let password = std::fs::read_to_string(&admin.password_file).map_err(|e| {
            config_err(format!(
                "cannot read the server admin password file '{}': {}",
                admin.password_file.display(),
                e.kind()
            ))
        })?;
        state = state.with_server_admin(&admin.user, password.trim_end_matches(['\r', '\n']))?;
    }
    {
        let mut directory = state.org_registry.write();
        if cfg.organization.name != default_org_name() {
            directory.by_name.remove(&default_org_name());
            directory
                .by_name
                .insert(cfg.organization.name.clone(), (1, org));
            directory
                .by_num
                .insert(1, (cfg.organization.name.clone(), org));
        }
    }
    declare_organizations(&state, &cfg.organizations)?;
    let provisioning = match &cfg.provisioning_dir {
        Some(dir) => Some(provision(&state, dir, &cfg.provisioning_vars).await?),
        None => None,
    };
    Ok(Built {
        state,
        provisioning,
    })
}

/// Loads the provisioning directory `dir` into the organizations of `state` its `orgId`s name
/// (1 the default organization, another number one the directory holds).
pub async fn provision(
    state: &DaiyaState,
    dir: &Path,
    vars: &BTreeMap<String, String>,
) -> CrResult<ProvisioningLoadReport> {
    // The registry's read guard ends here, before the load awaits.
    let organizations = state
        .org_registry
        .read()
        .by_num
        .iter()
        .filter(|(num, _)| **num != 1)
        .map(|(num, (_, org))| (*num, *org))
        .collect();
    load_provisioning_dir(
        state.store.as_ref(),
        ProvisioningTarget {
            installation_id: state.installation_id,
            org_id: state.default_org_id,
            instance_id: state.default_instance_id,
            organizations,
        },
        dir,
        &state.limits,
        vars,
    )
    .await
}

/// The request context of the CLI subcommands: the operator, admin of the organization.
#[must_use]
pub fn cli_ctx(state: &DaiyaState) -> daiya_model::RequestCtx {
    daiya_model::RequestCtx::new(
        state.installation_id,
        state.default_org_id,
        state.default_instance_id,
        Subject::authenticated(CLI_ACTOR, vec![state.default_org_id], Vec::new()),
    )
}

async fn client_addr_from_connection(mut req: Request<Body>, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    if let Some(ip) = peer
        && req.extensions().get::<ClientAddr>().is_none()
    {
        req.extensions_mut().insert(ClientAddr(ip));
    }
    next.run(req).await
}

/// The router of a standalone server: [`crate::router`] plus the peer address as
/// [`ClientAddr`], so the public-dashboard rate limit counts per source address (served with
/// `into_make_service_with_connect_info::<SocketAddr>()`).
pub fn standalone_router(state: DaiyaState, mount: Mount) -> Router {
    crate::router(state, mount).layer(middleware::from_fn(client_addr_from_connection))
}
