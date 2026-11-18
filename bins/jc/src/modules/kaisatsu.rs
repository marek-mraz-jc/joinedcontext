//! kaisatsu (identity provider) inside `jc`: the same state the standalone `kaisatsu serve`
//! builds, on PostgreSQL when `database_url_file` is set (else the in-memory store), with the
//! KEK read from its file.

use crate::app::StartError;
use crate::config::KaisatsuSection;
use crate::module::Module;
use crate::modules::BuildCtx;
use crate::setup;
use axum::Router;
use jc_router::Mount;
use kaisatsu_core::{
    ClientRepresentation, ClientScopeRepresentation, LocalAuthorizer, SIGNATURE_ALG_ATTRIBUTES,
    SigningKeyRecord, Store,
};
use kaisatsu_crypto::Kek;
use kaisatsu_import::{ApplyOptions, apply_realm_files};
use kaisatsu_login::{OutboxDispatcher, StoredSmtpSender};
use kaisatsu_oidc::{OutboundGuard, OutboundPolicy, PrivateHost};
use kaisatsu_server::{KaisatsuState, RealmHosts, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{PgStore, PgStoreConfig};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Opens the configured `Store` for kaisatsu inside `jc` (K16):
/// - When `database_url_file` is set, reads the URL from the file, refuses any URL containing an
///   inline password, connects `PgStore` with `verify_boot_rls`, and spawns the change listener.
/// - `[kaisatsu.database]` does the same from the shared section, migrating first as the
///   migration role when one is set (sqlx's migrator holds an advisory lock).
/// - When neither is set, logs a `WARN` once and falls back to `MemoryStore`.
pub async fn open_kaisatsu_store(
    cfg: &KaisatsuSection,
    kek: Kek,
) -> Result<Arc<dyn Store>, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    if let Some(db) = &cfg.database {
        if let Some(opts) = db.migrate_options().map_err(|e| err(&e))? {
            let pool = crate::pg::connect(opts, 1).await.map_err(|e| err(&e))?;
            PgStore::run_migrations(&pool).await.map_err(|e| err(&e))?;
            pool.close().await;
        }
        let opts = db.app_options().map_err(|e| err(&e))?;
        let pg = PgStore::connect_with(opts, db.max_connections(), kek)
            .await
            .map_err(|e| err(&e))?;
        pg.spawn_change_listener();
        return Ok(Arc::new(pg));
    }
    if let Some(url_file) = &cfg.database_url_file {
        let raw_url = std::fs::read_to_string(url_file).map_err(|e| {
            err(&format!(
                "cannot read database_url_file {}: {e}",
                url_file.display()
            ))
        })?;
        let trimmed = raw_url.trim().to_owned();
        if trimmed.is_empty() {
            return Err(err(&format!(
                "database_url_file {} is empty",
                url_file.display()
            )));
        }
        let pg_cfg = PgStoreConfig {
            url_without_password: trimmed,
            password_file: cfg.db_password_file.clone(),
            max_connections: cfg.db_max_connections.unwrap_or(5),
        };
        let pg = PgStore::connect(&pg_cfg, kek).await.map_err(|e| err(&e))?;
        pg.spawn_change_listener();
        Ok(Arc::new(pg))
    } else {
        tracing::warn!(
            "kaisatsu: database_url_file is not set; mounting kaisatsu on in-memory store (demo/test only)"
        );
        Ok(Arc::new(MemoryStore::new(kek)))
    }
}

/// Applies the realm files of `dir`, their `$(file:...)` secrets read under `secrets_dir`: the
/// files of one `realm` merged in file-name order, each realm on its own (a cutover's export can
/// hold the deployment's realm beside another). A directory without realm files is nothing to
/// apply, not an error: a first start mounts an empty import dir. Answers whether the files define
/// the Portal's sign-in client in `realm`: the deployment then brings that realm itself.
async fn apply_import_dir(
    store: &Arc<dyn Store>,
    dir: &std::path::Path,
    secrets_dir: Option<std::path::PathBuf>,
    outbound_private_hosts: &[String],
    realm: &setup::Realm,
) -> Result<bool, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
    let mut by_realm: std::collections::BTreeMap<String, Vec<(String, serde_json::Value)>> =
        std::collections::BTreeMap::new();
    let mut unnamed = Vec::new();
    for (name, file) in files {
        match file["realm"].as_str() {
            Some(of) => by_realm
                .entry(of.to_owned())
                .or_default()
                .push((name, file)),
            None => unnamed.push((name, file)),
        }
    }
    // A file naming no realm belongs to the one the others name, as when they were all merged.
    if !unnamed.is_empty() {
        if by_realm.len() > 1 {
            return Err(err(&format!(
                "{} names no realm while the import dir holds several",
                unnamed[0].0
            )));
        }
        let key = by_realm.keys().next().cloned().unwrap_or_default();
        by_realm.entry(key).or_default().extend(unnamed);
    }
    let options = ApplyOptions {
        secrets_dir,
        outbound_private_hosts: outbound_private_hosts.to_vec(),
        ..ApplyOptions::default()
    };
    let mut brings_portal = false;
    for (name, files) in &by_realm {
        if name == realm.as_str() {
            let merged = kaisatsu_import::merge_realm_files(files).map_err(|e| err(&e))?;
            brings_portal = merged["clients"].as_array().is_some_and(|clients| {
                clients
                    .iter()
                    .any(|c| c["clientId"].as_str() == Some(setup::PORTAL_CLIENT))
            });
        }
        let report = apply_realm_files(store, files, &options)
            .await
            .map_err(|e| err(&e))?;
        for warning in &report.warnings {
            tracing::warn!(realm = %report.realm, %warning, "kaisatsu realm import");
        }
    }
    Ok(brings_portal)
}

/// Who writes the configured realm: `jc` (its first-start files, on every start) or the
/// deployment (an imported realm `jc` never writes to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealmOwner {
    Jc,
    Import,
}

/// The state file that keeps [`RealmOwner`] of `realm` once decided, so a restart decides the
/// same way whether or not the import dir is still mounted.
fn owner_file(state_dir: &std::path::Path, realm: &setup::Realm) -> std::path::PathBuf {
    state_dir.join(format!("realm-owner-{realm}"))
}

/// Decides, once per state directory, who writes `realm`, and keeps the answer in
/// [`owner_file`]. Without a kept answer the deployment owns the realm when its import dir
/// defines the Portal's client there, or when the store held the realm before this start and it
/// is not `jc`'s default realm (an older `jc` made only that one, and kept no file).
fn realm_owner(
    state_dir: &std::path::Path,
    realm: &setup::Realm,
    existed_before: bool,
    import_brings_portal: bool,
) -> Result<RealmOwner, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let file = owner_file(state_dir, realm);
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            return match text.trim() {
                "jc" => Ok(RealmOwner::Jc),
                "import" => Ok(RealmOwner::Import),
                _ => Err(err(&format!("{} names no realm owner", file.display()))),
            };
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(err(&format!("cannot read {}: {e}", file.display()))),
    }
    let owner =
        if import_brings_portal || (existed_before && realm.as_str() != setup::DEFAULT_REALM) {
            RealmOwner::Import
        } else {
            RealmOwner::Jc
        };
    let text = match owner {
        RealmOwner::Jc => "jc\n",
        RealmOwner::Import => "import\n",
    };
    std::fs::write(&file, text)
        .map_err(|e| err(&format!("cannot write {}: {e}", file.display())))?;
    Ok(owner)
}

/// The account console's client in the Portal's realm (Keycloak's `account-console`): public,
/// authorization code + PKCE S256, redirects only under kaisatsu's own `/account/` path, CORS
/// for that origin alone (`+`), and audience `account`, the only audience the account REST
/// takes. No service account, no direct grants, no realm role in its tokens.
fn account_console_realm_file(
    kaisatsu_public: &str,
    realm: &setup::Realm,
) -> (String, serde_json::Value) {
    let account = format!("{kaisatsu_public}/realms/{realm}/account");
    let client = serde_json::json!({
        "realm": realm.as_str(),
        "clients": [{
            "clientId": kaisatsu_server::ACCOUNT_CONSOLE_CLIENT,
            "name": "Account Console",
            "enabled": true,
            "protocol": "openid-connect",
            "publicClient": true,
            "standardFlowEnabled": true,
            "implicitFlowEnabled": false,
            "directAccessGrantsEnabled": false,
            "serviceAccountsEnabled": false,
            "fullScopeAllowed": false,
            "rootUrl": account,
            "redirectUris": [format!("{account}/*")],
            "webOrigins": ["+"],
            "attributes": {
                "pkce.code.challenge.method": "S256",
                "post.logout.redirect.uris": "+"
            },
            "defaultClientScopes": ["basic", "email", "profile"],
            "optionalClientScopes": [],
            "protocolMappers": [{
                "name": "audience resolve",
                "protocol": "openid-connect",
                "protocolMapper": "oidc-audience-mapper",
                "consentRequired": false,
                "config": {
                    "included.custom.audience": kaisatsu_server::ACCOUNT_AUDIENCE,
                    "access.token.claim": "true",
                    "id.token.claim": "false"
                }
            }]
        }]
    });
    ("40-jc-account-console.json".to_owned(), client)
}

/// Applies the realm the Portal signs in through (first-start setup), when the Portal runs beside
/// kaisatsu and `jc` owns that realm ([`realm_owner`]); an imported realm is left as it came.
/// Apply mode `no-delete`: a restart updates what changed and removes nothing an operator added.
/// Secrets reach the importer as `$(env:...)` values from memory.
async fn apply_portal_realm(
    store: &Arc<dyn Store>,
    ctx: BuildCtx<'_>,
    existed_before: bool,
    import_brings_portal: bool,
) -> Result<(), StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let (Some(base), Some(_)) = (
        ctx.siblings.portal_base(),
        ctx.setup.value(setup::Secret::PortalClientSecret),
    ) else {
        return Ok(());
    };
    let realm = &ctx.setup.realm;
    if realm_owner(&ctx.setup.dir, realm, existed_before, import_brings_portal)?
        == RealmOwner::Import
    {
        tracing::info!(
            %realm,
            "kaisatsu: the deployment brings the Portal's realm; jc writes nothing into it"
        );
        return Ok(());
    }
    let mut files = setup::portal_realm_files(&base, &ctx.siblings.organization_portals, realm)
        .map_err(|e| err(&e))?;
    if ctx
        .setup
        .value(setup::Secret::GatewayClientSecret)
        .is_some()
    {
        files.push(setup::gateway_realm_file(realm));
    }
    if ctx
        .setup
        .value(setup::Secret::AssistantClientSecret)
        .is_some()
    {
        setup::add_assistant_audience(&mut files);
        files.push(setup::assistant_realm_file(realm));
    }
    if let Some(kaisatsu) = ctx.siblings.get(Module::Kaisatsu) {
        files.push(account_console_realm_file(&kaisatsu.public, realm));
    }
    seed_first_admin_once(store, realm, &mut files)
        .await
        .map_err(|e| err(&e))?;
    let options = ApplyOptions {
        env_vars: setup::realm_env(ctx.setup),
        ..ApplyOptions::default()
    };
    let report = apply_realm_files(store, &files, &options)
        .await
        .map_err(|e| err(&e))?;
    tracing::info!(
        realm = %report.realm,
        changed = report.has_changes(),
        "kaisatsu: the Portal's realm is applied"
    );
    Ok(())
}

/// What an Organization's own realm is written from (T-7001): named like the Organization,
/// `jc`'s own (no deployment import writes it), with the Portal's, the gateway's and the
/// assistant's clients redirecting to that Organization's hosts alone. `None` while the Portal
/// does not sign in through kaisatsu.
#[derive(Clone)]
struct OrganizationRealmFiles {
    /// The installation realm's importer variables ([`setup::realm_env`]): each realm gets its
    /// own secrets derived from them ([`setup::organization_realm_env`]), never these.
    env_vars: BTreeMap<String, String>,
    gateway: bool,
    assistant: bool,
    /// Each Organization's working hosts, as URLs (`{scheme}://{portal}`, `portals`).
    bases: BTreeMap<String, Vec<String>>,
}

impl OrganizationRealmFiles {
    fn new(
        ctx: BuildCtx<'_>,
        organizations: &[crate::config::OrganizationSection],
        scheme: &str,
    ) -> Option<Self> {
        if ctx.siblings.portal_base().is_none()
            || ctx.setup.value(setup::Secret::PortalClientSecret).is_none()
        {
            return None;
        }
        Some(Self {
            env_vars: setup::realm_env(ctx.setup),
            gateway: ctx
                .setup
                .value(setup::Secret::GatewayClientSecret)
                .is_some(),
            assistant: ctx
                .setup
                .value(setup::Secret::AssistantClientSecret)
                .is_some(),
            bases: organizations
                .iter()
                .map(|org| {
                    let bases = std::iter::once(&org.hosts.portal)
                        .chain(&org.hosts.portals)
                        .map(|h| format!("{scheme}://{h}"))
                        .collect();
                    (org.name.clone(), bases)
                })
                .collect(),
        })
    }

    /// Applies `organization`'s realm; idempotent, so applying it again changes nothing.
    async fn apply(&self, store: &Arc<dyn Store>, organization: &str) -> Result<(), String> {
        let bases = self
            .bases
            .get(organization)
            .ok_or_else(|| format!("{organization}: not an Organization jc runs"))?;
        let realm = setup::Realm::parse(organization).map_err(|e| e.to_string())?;
        let mut files =
            setup::organization_realm_files(bases, &realm).map_err(|e| e.to_string())?;
        if self.gateway {
            files.push(setup::gateway_realm_file(&realm));
        }
        if self.assistant {
            setup::add_assistant_audience(&mut files);
            files.push(setup::assistant_realm_file(&realm));
        }
        let options = ApplyOptions {
            env_vars: setup::organization_realm_env(&self.env_vars, &realm),
            ..ApplyOptions::default()
        };
        let report = apply_realm_files(store, &files, &options)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            organization,
            changed = report.has_changes(),
            "kaisatsu: the Organization's realm is applied"
        );
        Ok(())
    }
}

/// Applies the realm of each Organization with its own sign-in host (`hosts.idm`, T-7001).
async fn apply_organization_realms(
    store: &Arc<dyn Store>,
    files: Option<&OrganizationRealmFiles>,
    sign_ins: &[super::sign_in::SignIn],
) -> Result<(), StartError> {
    let Some(files) = files else {
        return Ok(());
    };
    for own in sign_ins {
        files
            .apply(store, &own.organization)
            .await
            .map_err(|e| StartError::module(Module::Kaisatsu, &e))?;
    }
    Ok(())
}

/// Keeps kaisatsu on the host registry's sign-in hosts (T-7180): each set jc publishes replaces
/// the pins, so a served sign-in host answers its realm and a removed one is refused from the
/// next request on, under the issuer of the registry's host; and the realm of each Organization
/// in the set is applied (idempotent), so a host served at run time has a realm to answer.
fn follow_sign_ins(
    store: Arc<dyn Store>,
    files: Option<OrganizationRealmFiles>,
    pins: kaisatsu_server::RealmHostsSwap,
    start: &[super::sign_in::SignIn],
    scheme: String,
    tasks: &TaskTracker,
) {
    let tasks = tasks.clone();
    // Every realm pinned in this process: one the registry drops stays refused (`close`).
    let pinned = std::sync::Mutex::new(
        start
            .iter()
            .map(|own| own.organization.clone())
            .collect::<std::collections::BTreeSet<String>>(),
    );
    super::sign_in::follow("kaisatsu", move |set| {
        let mut ever = pinned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ever.extend(set.iter().map(|own| own.organization.clone()));
        match realm_hosts(set, &scheme) {
            Ok(hosts) => pins.replace(ever.iter().fold(hosts, |hosts, r| hosts.close(r))),
            Err(e) => {
                tracing::error!(error = %e, "kaisatsu: sign-in hosts kept");
                return;
            }
        }
        let Some(files) = files.clone() else {
            return;
        };
        let (store, set) = (Arc::clone(&store), set.to_vec());
        tasks.spawn(async move {
            for own in &set {
                if let Err(e) = files.apply(&store, &own.organization).await {
                    tracing::error!(error = %e, "kaisatsu: an Organization's realm is not applied");
                }
            }
        });
    });
}

/// Leaves the first administrator out of `files` once the realm holds it: applied again on every
/// start, its file would enable the account and put it back into the bootstrap group after an
/// administrator disabled it or took it out.
async fn seed_first_admin_once(
    store: &Arc<dyn Store>,
    realm: &setup::Realm,
    files: &mut Vec<(String, serde_json::Value)>,
) -> Result<(), kaisatsu_core::KaisatsuError> {
    if store
        .get_user_by_username_or_email(realm.as_str(), None, setup::FIRST_ADMIN)
        .await?
        .is_some()
    {
        files.retain(|(name, _)| name != setup::FIRST_ADMIN_FILE);
    }
    Ok(())
}

/// The prefix of the per-Endpoint client scopes the Portal makes (`endpoint:{slug}`, its hub
/// scope wave).
const ENDPOINT_SCOPE_PREFIX: &str = "endpoint:";
/// The client APISIX signs people in through (`openid-connect`), when the realm holds one.
const EDGE_CLIENT: &str = "edge";

/// lua-resty-openidc verifies RS256 ID tokens only and signs a person in with any other
/// algorithm's signature unchecked (T-0252): a realm holding the `edge` client must pin RS256 for
/// both its tokens and hold an ACTIVE RS256 key. A realm without `edge` has no edge to check.
async fn check_edge_client(store: &Arc<dyn Store>, realm: &setup::Realm) -> Result<(), StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let Some(realm) = store.get_realm(realm.as_str()).await.map_err(|e| err(&e))? else {
        return Ok(());
    };
    let Some(edge) = store
        .get_client_by_client_id(&realm.id, EDGE_CLIENT)
        .await
        .map_err(|e| err(&e))?
    else {
        return Ok(());
    };
    edge_algorithms(&edge, &realm.realm).map_err(|e| err(&e))?;
    store
        .ensure_signing_key(&realm.id, "RS256")
        .await
        .map_err(|e| err(&e))?;
    let keys = store
        .list_signing_keys(&realm.id)
        .await
        .map_err(|e| err(&e))?;
    active_rs256(&keys, &realm.realm).map_err(|e| err(&e))
}

/// Both token algorithm attributes of `edge` name RS256.
fn edge_algorithms(edge: &ClientRepresentation, realm: &str) -> Result<(), String> {
    for attribute in SIGNATURE_ALG_ATTRIBUTES {
        let alg = edge.attributes.get(attribute).map_or("", String::as_str);
        if alg != "RS256" {
            return Err(format!(
                "client {} of realm {realm}: attribute {attribute} is {alg:?}, the edge needs \"RS256\" (lua-resty-openidc verifies RS256 ID tokens only, T-0252)",
                edge.client_id
            ));
        }
    }
    Ok(())
}

/// The realm holds an ACTIVE RS256 key for the edge client.
fn active_rs256(keys: &[SigningKeyRecord], realm: &str) -> Result<(), String> {
    if keys
        .iter()
        .any(|k| k.algorithm == "RS256" && k.status == "ACTIVE")
    {
        Ok(())
    } else {
        Err(format!(
            "realm {realm} holds client {EDGE_CLIENT} but no ACTIVE RS256 key"
        ))
    }
}

/// The realm's own scope that names every Endpoint: the hub's one default scope, linked nowhere
/// else (ADR-N-025 4b).
const EVERY_ENDPOINT_SCOPE: &str = "endpoint:*";
/// The clients an Endpoint's scope is linked to, and whether `endpoint:*` stays where the realm
/// file put it: the MCP hub, the command line's client and the Portal's own client, whose
/// standard token exchange grants only a scope it holds as optional (T-6063).
const ENDPOINT_SCOPE_CLIENTS: [(&str, bool); 3] = [
    ("mcp-hub", true),
    ("jcctl", false),
    (setup::PORTAL_CLIENT, false),
];
/// How often the linker reads the realm: a new Endpoint's scope is linked this long after the
/// Portal creates it (the deployment's CronJob runs every 5 minutes).
const LINK_EVERY: Duration = Duration::from_secs(5);

/// Whether `scope` is one the Portal made for an Endpoint: named `endpoint:{slug}`, marked
/// `managed-by: joinedcontext` and without a protocol mapper (the mark kaisatsu lets portal-api
/// write; a mapper would put something into a token claim).
fn portal_endpoint_scope(scope: &ClientScopeRepresentation) -> bool {
    scope.name.starts_with(ENDPOINT_SCOPE_PREFIX)
        && scope.name != EVERY_ENDPOINT_SCOPE
        && scope.protocol_mappers.is_empty()
        && scope.attributes.get("managed-by").map(String::as_str) == Some("joinedcontext")
}

/// `client` with every Portal Endpoint scope in `ours` linked as optional and every other
/// `endpoint:` link (unmarked, gone, or linked as a default) removed; `keep_star` leaves the
/// realm's `endpoint:*` where it is. `None` when nothing changes. The rules of the deployment's
/// `sync-admin-permissions.sh`, which jc has no realm admin Job to run.
fn link_endpoint_scopes(
    client: &ClientRepresentation,
    ours: &[&str],
    keep_star: bool,
) -> Option<ClientRepresentation> {
    let stray = |name: &String| {
        name.starts_with(ENDPOINT_SCOPE_PREFIX) && !(keep_star && name == EVERY_ENDPOINT_SCOPE)
    };
    let mut next = client.clone();
    next.default_client_scopes.retain(|n| !stray(n));
    next.optional_client_scopes
        .retain(|n| !stray(n) || ours.contains(&n.as_str()));
    for name in ours {
        if !next.optional_client_scopes.iter().any(|n| n == name) {
            next.optional_client_scopes.push((*name).to_owned());
        }
    }
    (next.default_client_scopes != client.default_client_scopes
        || next.optional_client_scopes != client.optional_client_scopes)
        .then_some(next)
}

/// One pass of the linker over the Portal's realm.
async fn link_pass(
    store: &Arc<dyn Store>,
    realm: &setup::Realm,
) -> Result<usize, kaisatsu_core::KaisatsuError> {
    let Some(realm) = store.get_realm(realm.as_str()).await? else {
        return Ok(0);
    };
    let scopes = store.list_client_scopes(&realm.id).await?;
    let ours: Vec<&str> = scopes
        .iter()
        .filter(|s| portal_endpoint_scope(s))
        .map(|s| s.name.as_str())
        .collect();
    let mut written = 0;
    for (client_id, keep_star) in ENDPOINT_SCOPE_CLIENTS {
        let Some(client) = store.get_client_by_client_id(&realm.id, client_id).await? else {
            continue;
        };
        if let Some(next) = link_endpoint_scopes(&client, &ours, keep_star) {
            store.upsert_client(next).await?;
            written += 1;
        }
    }
    Ok(written)
}

/// Links each Endpoint's scope to the clients that need it, at start and every [`LINK_EVERY`]
/// until `shutdown`: the realm-admin step the deployment runs as the
/// keycloak-admin-permissions-sync CronJob. Only these clients get the scopes; no realm-wide
/// optional scope is set (T-6301).
fn follow_endpoint_scopes(
    store: Arc<dyn Store>,
    realm: setup::Realm,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            match link_pass(&store, &realm).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(clients = n, "kaisatsu: Endpoint scopes linked"),
                Err(e) => tracing::warn!(error = %e, "kaisatsu: Endpoint scopes not linked"),
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(LINK_EVERY) => {}
            }
        }
    });
}

/// How often expired sign-in rows (sessions, codes, revoked token ids) are deleted.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// Deletes expired sign-in rows every `every` until `shutdown`, as `kaisatsu serve` does: the
/// reads already skip them, the sweep keeps the tables from growing with every sign-in.
fn sweep_expired_rows(
    store: Arc<dyn Store>,
    every: Duration,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            if let Err(e) = store.sweep_expired().await {
                tracing::warn!(error = %e, "kaisatsu: expired rows not swept");
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(every) => {}
            }
        }
    });
}

/// How soon a queued invitation or password-reset e-mail goes out (K9).
const OUTBOX_EVERY: Duration = Duration::from_secs(5);

/// Sends the queued e-mails of every realm every `every` until `shutdown` (K9), through each
/// realm's own stored SMTP server ([`StoredSmtpSender`]: read at each send, so the settings the
/// Portal's operators save apply without a restart, T-7152). A realm without one keeps its rows
/// pending.
fn send_queued_mail(
    store: Arc<dyn Store>,
    kek: Kek,
    guard: OutboundGuard,
    mount: Mount,
    every: Duration,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let dispatcher = OutboxDispatcher::new(
        store.clone(),
        kek,
        Arc::new(StoredSmtpSender::new(store, guard)),
    );
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            if let Err(e) = dispatcher.dispatch_all_realms(&mount).await {
                tracing::warn!(error = %e.public_detail(), "kaisatsu: outbox pass");
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(every) => {}
            }
        }
    });
}

/// Keeps each Organization's sign-in pages its `Organization.spec.appearance` over the realm's
/// block (UIX-16, T-6777): one block per Organization, keyed by its Portal host, which the
/// sign-in finds by the redirect URI the Portal there sends. A member the Organization leaves
/// out, or a removed appearance, shows the realm's block again.
#[cfg(feature = "portal")]
fn follow_appearances(
    store: &Arc<dyn Store>,
    base: &kaisatsu_core::BrandingBlock,
    organizations: &[crate::config::OrganizationSection],
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let hosts: std::collections::BTreeMap<String, (String, bool)> = organizations
        .iter()
        .map(|org| {
            let own_realm = org.hosts.idm.is_some();
            (org.name.clone(), (org.hosts.portal.clone(), own_realm))
        })
        .collect();
    let (store, base) = (store.clone(), base.clone());
    super::appearance::follow(
        hosts.keys().cloned().collect(),
        shutdown,
        tasks,
        move |name, look| {
            let (store, host) = (store.clone(), hosts.get(&name).cloned());
            let base = with_look(base.clone(), super::appearance::installation().as_ref());
            async move {
                let Some((host, own_realm)) = host else {
                    return;
                };
                let block = organization_block(base, &host, look.as_ref());
                if own_realm {
                    own_realm_branding(&store, &name, &block).await;
                }
                match store.upsert_branding(block).await {
                    Ok(()) => {
                        tracing::info!(organization = %name, appearance = look.is_some(), "kaisatsu: sign-in pages follow the Organization")
                    }
                    Err(e) => {
                        tracing::warn!(organization = %name, error = %e, "kaisatsu: Organization branding not stored")
                    }
                }
            }
        },
    );
}

/// Keeps the installation realm's sign-in pages the look its operators approved in the Portal
/// over `base`, the block the branding file gave the realm (T-7188); no look shows `base`.
#[cfg(feature = "portal")]
fn follow_installation_look(
    store: &Arc<dyn Store>,
    base: &kaisatsu_core::BrandingBlock,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let (store, base) = (store.clone(), base.clone());
    super::appearance::follow_installation(shutdown, tasks, move |look| {
        let (store, block) = (store.clone(), with_look(base.clone(), look.as_ref()));
        async move {
            match store.upsert_branding(block).await {
                Ok(()) => {
                    tracing::info!(
                        look = look.is_some(),
                        "kaisatsu: sign-in pages follow the installation's look"
                    )
                }
                Err(e) => {
                    tracing::warn!(error = %e, "kaisatsu: the installation's look not stored")
                }
            }
        }
    });
}

/// The block of the realm named `realm` as the branding file left it: the stored one when a file
/// was applied (`from_file`), else the platform defaults, since a stored block then holds the
/// look the operators approved before; `None` without the realm.
#[cfg(feature = "portal")]
async fn realm_branding(
    store: &Arc<dyn Store>,
    realm: &str,
    from_file: bool,
) -> kaisatsu_core::Result<Option<kaisatsu_core::BrandingBlock>> {
    let Some(realm) = store.get_realm(realm).await? else {
        return Ok(None);
    };
    let base = if from_file {
        store.get_branding(&realm.id, None).await?
    } else {
        None
    };
    let base = base.unwrap_or_default();
    Ok(Some(kaisatsu_core::BrandingBlock {
        id: if base.id.is_empty() {
            format!("{}:installation", realm.id)
        } else {
            base.id.clone()
        },
        realm_id: realm.id,
        org_id: None,
        ..base
    }))
}

/// The Organization's look as its own realm's block (T-7001): every page of its sign-in host
/// shows it. A realm not created yet takes it at the next appearance change.
#[cfg(feature = "portal")]
async fn own_realm_branding(
    store: &Arc<dyn Store>,
    realm: &str,
    block: &kaisatsu_core::BrandingBlock,
) {
    let found = match store.get_realm(realm).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(%realm, error = %e, "kaisatsu: Organization realm unreadable");
            return;
        }
    };
    let Some(own) = found else { return };
    let block = kaisatsu_core::BrandingBlock {
        id: own.id.clone(),
        realm_id: own.id,
        org_id: None,
        ..block.clone()
    };
    if let Err(e) = store.upsert_branding(block).await {
        tracing::warn!(%realm, error = %e, "kaisatsu: Organization realm branding not stored");
    }
}

/// The realm's block `base` as the Organization on Portal host `host` shows it: its
/// appearance over `base`, member by member.
#[cfg(feature = "portal")]
fn organization_block(
    mut base: kaisatsu_core::BrandingBlock,
    host: &str,
    look: Option<&jc_core::kinds::OrganizationAppearance>,
) -> kaisatsu_core::BrandingBlock {
    base.id = format!("{}:{host}", base.realm_id);
    base.org_id = Some(host.to_owned());
    with_look(base, look)
}

/// `base` with `look` over it, member by member.
#[cfg(feature = "portal")]
fn with_look(
    mut base: kaisatsu_core::BrandingBlock,
    look: Option<&jc_core::kinds::OrganizationAppearance>,
) -> kaisatsu_core::BrandingBlock {
    if let Some(look) = look {
        let over = |target: &mut String, value: Option<&String>| {
            if let Some(value) = value {
                value.clone_into(target);
            }
        };
        over(&mut base.instance_name, look.instance_name.as_ref());
        over(&mut base.short_name, super::appearance::short_name(look));
        over(&mut base.city, look.city.as_ref());
        over(
            &mut base.organisation,
            look.publisher.as_ref().or(look.instance_name.as_ref()),
        );
        over(&mut base.contact_email, look.contact_email.as_ref());
        let colours = &look.colours;
        over(&mut base.colours.primary, colours.primary.as_ref());
        over(&mut base.colours.secondary, colours.secondary.as_ref());
        over(&mut base.colours.accent, colours.accent.as_ref());
        over(&mut base.colours.background, colours.background.as_ref());
        over(&mut base.colours.text, colours.text.as_ref());
        over(&mut base.logo_svg, look.logo_svg.as_ref());
    }
    base.sanitised()
}

/// kaisatsu's outgoing calls: the Kubernetes API and `[kaisatsu] outbound_private_hosts` (checked
/// at config load) may resolve to a private address, nothing else.
fn outbound_guard(cfg: &KaisatsuSection) -> OutboundGuard {
    OutboundGuard::new(OutboundPolicy {
        private_hosts: cfg
            .outbound_private_hosts
            .iter()
            .filter_map(|h| PrivateHost::parse(h).ok())
            .collect(),
        ..OutboundPolicy::default()
    })
}

pub async fn build(
    cfg: &KaisatsuSection,
    organizations: &[crate::config::OrganizationSection],
    ctx: BuildCtx<'_>,
    mount: Mount,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let kek = ctx
        .setup
        .value(setup::Secret::KaisatsuKek)
        .ok_or_else(|| err(&"no key-encryption key"))?;
    let kek = Kek::from_raw_bytes(kek.as_bytes()).map_err(|e| err(&e))?;
    let store = open_kaisatsu_store(cfg, kek.clone()).await?;
    let realm = &ctx.setup.realm;
    let existed_before = store
        .get_realm(realm.as_str())
        .await
        .map_err(|e| err(&e))?
        .is_some();
    let mut import_brings_portal = false;
    if let Some(dir) = &cfg.import_dir {
        // A realm file reads no environment here: its secrets are `$(file:...)` under the
        // configured directory.
        import_brings_portal = apply_import_dir(
            &store,
            dir,
            cfg.import_secrets_dir.clone(),
            &cfg.outbound_private_hosts,
            realm,
        )
        .await?;
    }
    apply_portal_realm(&store, ctx, existed_before, import_brings_portal).await?;
    let sign_ins = super::sign_in::from_config(organizations);
    let realm_files = OrganizationRealmFiles::new(ctx, organizations, &mount.scheme);
    apply_organization_realms(&store, realm_files.as_ref(), &sign_ins).await?;
    if let Some(file) = &cfg.branding_file {
        // A broken file leaves the pages plain; sign-in stays up.
        match kaisatsu_server::apply_branding_file(&store, file).await {
            Ok(n) => tracing::info!(realms = n, "kaisatsu branding file applied"),
            Err(e) => tracing::error!(error = %e, "kaisatsu branding file not applied"),
        }
    }
    check_edge_client(&store, realm).await?;
    follow_endpoint_scopes(store.clone(), realm.clone(), shutdown, tasks);
    sweep_expired_rows(store.clone(), SWEEP_EVERY, shutdown, tasks);
    send_queued_mail(
        store.clone(),
        kek.clone(),
        outbound_guard(cfg),
        mount.clone(),
        OUTBOX_EVERY,
        shutdown,
        tasks,
    );
    #[cfg(feature = "portal")]
    match realm_branding(&store, realm.as_str(), cfg.branding_file.is_some()).await {
        Ok(Some(base)) => {
            follow_installation_look(&store, &base, shutdown, tasks);
            follow_appearances(&store, &base, organizations, shutdown, tasks);
        }
        Ok(None) => {
            tracing::warn!("kaisatsu: no installation realm; the sign-in pages keep their look")
        }
        Err(e) => {
            tracing::warn!(error = %e, "kaisatsu: realm branding unreadable; the sign-in pages keep their look")
        }
    }
    let state = KaisatsuState::new(store, kek, outbound_guard(cfg), Arc::new(LocalAuthorizer))
        .with_second_factor(match cfg.second_factor {
            crate::config::SecondFactorFloor::Optional => kaisatsu_server::SecondFactor::Optional,
            crate::config::SecondFactorFloor::Required => kaisatsu_server::SecondFactor::Required,
        })
        .with_realm_hosts(realm_hosts(&sign_ins, &mount.scheme).map_err(|e| err(&e))?);
    follow_sign_ins(
        state.store().clone(),
        realm_files,
        state.realm_hosts(),
        &sign_ins,
        mount.scheme.clone(),
        tasks,
    );
    Ok(router(state, mount))
}

/// Each Organization with a sign-in host of its own signs in there, in the realm named like it
/// (T-7001), under `{scheme}://{host}`; the sibling modules reach that realm's back channel on
/// loopback.
fn realm_hosts(
    sign_ins: &[super::sign_in::SignIn],
    scheme: &str,
) -> Result<RealmHosts, jc_router::RouterError> {
    let mut hosts = ["localhost", "127.0.0.1", "[::1]"]
        .into_iter()
        .fold(RealmHosts::default(), RealmHosts::internal);
    for own in sign_ins {
        hosts = hosts.pin(
            &own.organization,
            Mount::new("kaisatsu", scheme, &own.host, "")?,
        );
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};

    fn scope(name: &str, mark: Option<&str>, mapper: bool) -> ClientScopeRepresentation {
        let mut s = ClientScopeRepresentation {
            name: name.to_owned(),
            ..ClientScopeRepresentation::default()
        };
        if let Some(mark) = mark {
            s.attributes.insert("managed-by".into(), mark.into());
        }
        if mapper {
            s.protocol_mappers.push(Default::default());
        }
        s
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|n| (*n).to_owned()).collect()
    }

    /// An Organization's sign-in block (UIX-16, T-6777): keyed by its Portal host, its
    /// appearance over the realm's block; without one the realm's block under that key.
    #[cfg(feature = "portal")]
    #[test]
    fn an_organizations_sign_in_block_is_its_appearance_over_the_realms() {
        let mut realm = kaisatsu_core::BrandingBlock {
            realm_id: "r1".into(),
            organisation: "Installation".into(),
            city: "Slovakia".into(),
            ..kaisatsu_core::BrandingBlock::default()
        };
        realm.colours.secondary = "#222222".into();
        let plain = organization_block(realm.clone(), "portal.praha.example", None);
        assert_eq!(plain.org_id.as_deref(), Some("portal.praha.example"));
        assert_eq!(plain.id, "r1:portal.praha.example");
        assert_eq!(
            kaisatsu_core::BrandingBlock {
                id: realm.id.clone(),
                org_id: None,
                ..plain
            },
            realm.clone().sanitised(),
            "no appearance: the realm's look"
        );
        let look: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "instanceName": "Banskobystrický kraj",
                "colours": { "primary": "#0b5394" },
            }))
            .expect("an appearance");
        let own = organization_block(realm, "portal.bbsk.example:8443", Some(&look));
        assert_eq!(
            own.organisation, "Banskobystrický kraj",
            "no publisher: its name"
        );
        assert_eq!(own.instance_name, "Banskobystrický kraj");
        assert_eq!(own.colours.primary, "#0b5394");
        assert_eq!(own.colours.secondary, "#222222", "left out: the realm's");
        assert_eq!(own.city, "Slovakia");
    }

    /// T-7188: the installation's look goes over the realm's block, an Organization's over that,
    /// and an empty look leaves the block the file gave.
    #[cfg(feature = "portal")]
    #[test]
    fn the_installation_look_goes_under_each_organizations_and_over_the_realms() {
        let mut realm = kaisatsu_core::BrandingBlock {
            id: "r1:installation".into(),
            realm_id: "r1".into(),
            city: "Slovakia".into(),
            ..kaisatsu_core::BrandingBlock::default()
        };
        realm.colours.secondary = "#222222".into();
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>"##;
        let installation: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "instanceName": "Smart City", "colours": { "primary": "#0b5394" }, "logoSvg": svg,
            }))
            .expect("a look");
        assert_eq!(
            with_look(realm.clone(), None),
            realm.clone().sanitised(),
            "no look: the file's block"
        );
        let shown = with_look(realm.clone(), Some(&installation));
        assert_eq!(
            (shown.id.as_str(), shown.org_id.as_deref()),
            ("r1:installation", None)
        );
        assert_eq!(shown.instance_name, "Smart City");
        assert_eq!(shown.colours.primary, "#0b5394");
        assert_eq!(shown.colours.secondary, "#222222");
        assert_eq!(shown.logo_svg, svg, "the sign-in pages draw the logo");
        let org: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({ "colours": { "primary": "#aa0000" } }))
                .expect("an appearance");
        let own = organization_block(shown, "portal.bbsk.example", Some(&org));
        assert_eq!(own.colours.primary, "#aa0000", "the Organization's own");
        assert_eq!(
            own.instance_name, "Smart City",
            "left out: the installation's"
        );
        assert_eq!(own.logo_svg, svg);
    }

    #[test]
    fn only_marked_mapperless_endpoint_scopes_are_the_portals() {
        assert!(portal_endpoint_scope(&scope(
            "endpoint:abc",
            Some("joinedcontext"),
            false
        )));
        assert!(!portal_endpoint_scope(&scope("endpoint:abc", None, false)));
        assert!(!portal_endpoint_scope(&scope(
            "endpoint:abc",
            Some("someone"),
            false
        )));
        assert!(!portal_endpoint_scope(&scope(
            "endpoint:abc",
            Some("joinedcontext"),
            true
        )));
        assert!(!portal_endpoint_scope(&scope(
            "endpoint:*",
            Some("joinedcontext"),
            false
        )));
        assert!(!portal_endpoint_scope(&scope(
            "connector:abc",
            Some("joinedcontext"),
            false
        )));
    }

    #[test]
    fn endpoint_scopes_are_linked_optional_and_strays_unlinked() {
        let client = ClientRepresentation {
            default_client_scopes: names(&["basic", "endpoint:*", "endpoint:default-linked"]),
            optional_client_scopes: names(&["gateway-hub", "endpoint:gone", "endpoint:a"]),
            ..ClientRepresentation::default()
        };
        let ours = ["endpoint:a", "endpoint:b"];
        // The hub keeps the realm's `endpoint:*` default; a default-linked or unknown
        // `endpoint:` scope goes, the Portal's are optional.
        let hub = link_endpoint_scopes(&client, &ours, true).expect("a change");
        assert_eq!(hub.default_client_scopes, names(&["basic", "endpoint:*"]));
        assert_eq!(
            hub.optional_client_scopes,
            names(&["gateway-hub", "endpoint:a", "endpoint:b"])
        );
        // Anywhere else `endpoint:*` goes too: no token names every Endpoint.
        let portal = link_endpoint_scopes(&client, &ours, false).expect("a change");
        assert_eq!(portal.default_client_scopes, names(&["basic"]));
        assert!(
            !portal
                .optional_client_scopes
                .contains(&"endpoint:*".to_owned())
        );
        // A second pass writes nothing.
        assert!(link_endpoint_scopes(&portal, &ours, false).is_none());
        assert!(link_endpoint_scopes(&hub, &ours, true).is_none());
    }

    /// T-7001 (T-6991 §7): Organization A's realm on idm.a.test answers its sign-in for A's
    /// working host and refuses B's as `redirect_uri`, so a code of A never reaches B; the realm
    /// is 404 on B's sign-in host.
    #[tokio::test]
    async fn a_code_for_one_organization_never_goes_to_anothers_working_host() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let roles = crate::module::Module::ALL;
        let setup = setup::Setup::prepare(dir.path(), &roles, &Default::default()).unwrap();
        let kek = Kek::from_raw_bytes("07".repeat(32).as_bytes()).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
        let a = setup::Realm::parse("a").unwrap();
        let files =
            setup::organization_realm_files(&["https://smartcity.a.test".to_owned()], &a).unwrap();
        let options = ApplyOptions {
            env_vars: setup::realm_env(&setup),
            ..ApplyOptions::default()
        };
        apply_realm_files(&store, &files, &options).await.unwrap();
        assert!(
            store
                .get_user_by_username_or_email("a", None, setup::FIRST_ADMIN)
                .await
                .unwrap()
                .is_none(),
            "an Organization realm holds no first administrator"
        );
        let idm = |h: &str| Mount::new("kaisatsu", "https", h, "").unwrap();
        let pins = RealmHosts::default()
            .pin("a", idm("idm.a.test"))
            .pin("b", idm("idm.b.test"));
        let state = KaisatsuState::new(
            store,
            kek,
            OutboundGuard::default(),
            Arc::new(LocalAuthorizer),
        )
        .with_realm_hosts(pins);
        let app = router(state, idm("login.city.test"));
        let auth = |host: &str, working: &str| {
            let uri = format!(
                "/realms/a/protocol/openid-connect/auth?client_id={}&response_type=code&scope=openid\
                 &state=s1&nonce=n1&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM\
                 &code_challenge_method=S256&redirect_uri=https%3A%2F%2F{working}%2Fapi%2Fv1%2Fauth%2Fcallback",
                setup::PORTAL_CLIENT
            );
            let req = Request::get(uri)
                .extension(idm(host))
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(req)
        };
        let own = auth("idm.a.test", "smartcity.a.test").await.unwrap();
        // A's own working host: on to the sign-in page of A's host, never to a working host.
        assert_eq!(own.status(), StatusCode::SEE_OTHER);
        let next = own.headers()[header::LOCATION].to_str().unwrap();
        assert!(
            next.contains("/realms/a/") && !next.contains("smartcity."),
            "{next}"
        );
        let sibling = auth("idm.a.test", "smartcity.b.test").await.unwrap();
        assert_eq!(sibling.status(), StatusCode::BAD_REQUEST);
        assert!(sibling.headers().get(header::LOCATION).is_none());
        let elsewhere = auth("idm.b.test", "smartcity.a.test").await.unwrap();
        assert_eq!(elsewhere.status(), StatusCode::NOT_FOUND);
    }

    /// S1-04: each Organization realm holds its own client secrets. A's `portal-reconciler`
    /// (manage-users) secret buys a token in A's realm and is refused by B's token endpoint and
    /// the installation's; neither realm holds the installation's secret.
    #[tokio::test]
    async fn one_organizations_client_secret_is_refused_by_another_realm() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let setup =
            setup::Setup::prepare(dir.path(), &crate::module::Module::ALL, &Default::default())
                .unwrap();
        let kek = Kek::from_raw_bytes("07".repeat(32).as_bytes()).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
        let installation = setup::realm_env(&setup);
        let base = |o: &str| vec![format!("https://smartcity.{o}.test")];
        let files = OrganizationRealmFiles {
            env_vars: installation.clone(),
            gateway: true,
            assistant: true,
            bases: BTreeMap::from([("a".to_owned(), base("a")), ("b".to_owned(), base("b"))]),
        };
        files.apply(&store, "a").await.unwrap();
        files.apply(&store, "b").await.unwrap();
        let realm_env = |o: &str| {
            setup::organization_realm_env(&installation, &setup::Realm::parse(o).unwrap())
        };
        let (a, b) = (realm_env("a"), realm_env("b"));
        assert_eq!(a.len(), 5, "every client has its own secret");
        assert!(!a.contains_key("JC_SETUP_FIRST_ADMIN_PASSWORD"));
        for (name, value) in &a {
            assert_ne!(Some(value), b.get(name), "{name}: one value in two realms");
            assert_ne!(
                Some(value),
                installation.get(name),
                "{name}: the installation's"
            );
        }
        let idm = |h: &str| Mount::new("kaisatsu", "https", h, "").unwrap();
        let pins = RealmHosts::default()
            .pin("a", idm("idm.a.test"))
            .pin("b", idm("idm.b.test"));
        let state = KaisatsuState::new(
            store,
            kek,
            OutboundGuard::default(),
            Arc::new(LocalAuthorizer),
        )
        .with_realm_hosts(pins);
        let app = router(state, idm("login.city.test"));
        let token = |realm: &str, secret: &str| {
            let form = format!(
                "grant_type=client_credentials&client_id={}&client_secret={secret}",
                setup::PORTAL_ADMIN_CLIENT
            );
            let req = Request::post(format!("/realms/{realm}/protocol/openid-connect/token"))
                .extension(idm(&format!("idm.{realm}.test")))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap();
            app.clone().oneshot(req)
        };
        let secret_of_a = &a["CLIENT_SECRET_PORTAL_RECONCILER"];
        assert_eq!(
            token("a", secret_of_a).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            token("b", secret_of_a).await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "A's secret in B's realm"
        );
        assert_eq!(
            token("b", &installation["CLIENT_SECRET_PORTAL_RECONCILER"])
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
            "the installation's secret in B's realm"
        );
    }

    /// T-7180: kaisatsu's pins follow the host registry. A sign-in host served at run time
    /// answers its realm under the registry's issuer whatever `Host` or `X-Forwarded-Host` the
    /// request names; once the registry serves another set the realm is refused there, and its
    /// realm file applies once (a second apply changes nothing).
    #[tokio::test]
    async fn a_sign_in_host_served_at_run_time_answers_under_the_registry_issuer() {
        use crate::modules::sign_in::SignIn;
        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let setup =
            setup::Setup::prepare(dir.path(), &crate::module::Module::ALL, &Default::default())
                .unwrap();
        let kek = Kek::from_raw_bytes("07".repeat(32).as_bytes()).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
        let files = OrganizationRealmFiles {
            env_vars: setup::realm_env(&setup),
            gateway: false,
            assistant: false,
            bases: BTreeMap::from([("a".to_owned(), vec!["https://smartcity.a.test".to_owned()])]),
        };
        files.apply(&store, "a").await.unwrap();
        let realm = store.get_realm("a").await.unwrap().expect("a's realm");
        files.apply(&store, "a").await.unwrap();
        assert_eq!(
            store.get_realm("a").await.unwrap().map(|r| r.id),
            Some(realm.id),
            "applied once"
        );
        assert!(
            files.apply(&store, "zz").await.is_err(),
            "not an Organization jc runs"
        );
        let state = KaisatsuState::new(
            store,
            kek,
            OutboundGuard::default(),
            Arc::new(LocalAuthorizer),
        );
        let pins = state.realm_hosts();
        let idm = |h: &str| Mount::new("kaisatsu", "https", h, "").unwrap();
        let app = router(state, idm("login.city.test"));
        let discover = |host: &str| {
            let req = Request::get("/realms/a/.well-known/openid-configuration")
                .extension(idm(host))
                .header(header::HOST, "evil.test")
                .header("x-forwarded-host", "evil.test")
                .header("x-forwarded-proto", "http")
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(req)
        };
        let own = |organization: &str, host: &str| SignIn {
            organization: organization.into(),
            host: host.into(),
        };
        pins.replace(realm_hosts(&[own("a", "idm.a.test")], "https").unwrap());
        let served = discover("idm.a.test").await.unwrap();
        assert_eq!(served.status(), StatusCode::OK);
        let body = axum::body::to_bytes(served.into_body(), 1 << 20)
            .await
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(document["issuer"], "https://idm.a.test/realms/a");
        assert_eq!(
            discover("login.city.test").await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "a pinned realm answers on its own host alone"
        );
        pins.replace(
            realm_hosts(&[own("b", "idm.a.test")], "https")
                .unwrap()
                .close("a"),
        );
        assert_eq!(
            discover("idm.a.test").await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "the host serves another realm now"
        );
        assert_eq!(
            discover("login.city.test").await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "a realm whose host was removed never falls back to the installation's host"
        );
    }

    /// The realm `apply_portal_realm` writes when the assistant runs: kaisatsu takes it, the
    /// assistant's client asks scope `embedder` and introspects for the proxy, and portal-api's
    /// tokens name `jc-assistant`.
    #[tokio::test]
    async fn the_assistants_clients_and_the_portals_audience_apply() {
        let dir = tempfile::tempdir().unwrap();
        let roles = crate::module::Module::ALL;
        let setup = setup::Setup::prepare(dir.path(), &roles, &Default::default()).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(
            Kek::from_raw_bytes("07".repeat(32).as_bytes()).unwrap(),
        ));
        let mut files =
            setup::portal_realm_files("https://portal.city.example/", &[], &setup.realm).unwrap();
        setup::add_assistant_audience(&mut files);
        files.push(setup::assistant_realm_file(&setup.realm));
        let options = ApplyOptions {
            env_vars: setup::realm_env(&setup),
            ..ApplyOptions::default()
        };
        apply_realm_files(&store, &files, &options).await.unwrap();
        let realm = store
            .get_realm(setup.realm.as_str())
            .await
            .unwrap()
            .unwrap();
        let client = |id: &'static str| {
            let store = store.clone();
            let realm = realm.id.clone();
            async move {
                store
                    .get_client_by_client_id(&realm, id)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        let assistant = client(setup::ASSISTANT_CLIENT).await;
        assert!(assistant.service_accounts_enabled);
        assert_eq!(
            assistant.optional_client_scopes,
            vec![setup::EMBEDDER_SCOPE.to_owned()]
        );
        let audience = |c: &kaisatsu_core::model::ClientRepresentation| -> Vec<String> {
            c.protocol_mappers
                .iter()
                .filter_map(|m| m.config.get("included.custom.audience").cloned())
                .collect()
        };
        assert_eq!(audience(&assistant), vec![setup::PROXY_CLIENT.to_owned()]);
        let proxy = client(setup::PROXY_CLIENT).await;
        assert!(proxy.service_accounts_enabled);
        // The proxy reads a run's context on the Portal's internal listener, which admits only a
        // token for `portal-internal` (T-7149: without it every conversation got 401 there).
        assert_eq!(audience(&proxy), vec!["portal-internal".to_owned()]);
        let portal = client(setup::PORTAL_CLIENT).await;
        assert!(audience(&portal).contains(&setup::ASSISTANT_CLIENT.to_owned()));
    }

    /// The first administrator is seeded on the first start only: one an administrator disabled
    /// stays disabled when `jc` starts again with changed realm files (an upgrade, an added
    /// Organization), which an unchanged file's checksum would skip.
    #[tokio::test]
    async fn a_disabled_first_admin_stays_disabled_on_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let setup = setup::Setup::prepare(dir.path(), &Module::ALL, &Default::default()).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(
            Kek::from_raw_bytes("07".repeat(32).as_bytes()).unwrap(),
        ));
        let options = ApplyOptions {
            env_vars: setup::realm_env(&setup),
            ..ApplyOptions::default()
        };
        let (setup, store, options) = (&setup, &store, &options);
        let start = |portals: Vec<String>| async move {
            let mut files =
                setup::portal_realm_files("https://portal.city.example/", &portals, &setup.realm)
                    .unwrap();
            seed_first_admin_once(store, &setup.realm, &mut files)
                .await
                .unwrap();
            apply_realm_files(store, &files, options).await.unwrap();
        };
        let admin = || async {
            store
                .get_user_by_username_or_email(setup.realm.as_str(), None, setup::FIRST_ADMIN)
                .await
                .unwrap()
                .expect("the first admin")
        };
        start(vec![]).await;
        let mut first = admin().await;
        assert!(first.enabled);
        first.enabled = false;
        store.update_user(first).await.unwrap();
        start(vec!["https://portal.org.city.example".into()]).await;
        assert!(!admin().await.enabled);
    }

    fn edge(attrs: &[(&str, &str)]) -> ClientRepresentation {
        let mut c = ClientRepresentation {
            client_id: EDGE_CLIENT.into(),
            ..ClientRepresentation::default()
        };
        for (k, v) in attrs {
            c.attributes.insert((*k).into(), (*v).into());
        }
        c
    }

    #[test]
    fn the_edge_client_needs_rs256_for_both_tokens() {
        let id = "id.token.signed.response.alg";
        let access = "access.token.signed.response.alg";
        edge_algorithms(&edge(&[(id, "RS256"), (access, "RS256")]), "dev").unwrap();
        for attrs in [
            vec![],
            vec![(id, "RS256")],
            vec![(id, "RS256"), (access, "ES256")],
            vec![(id, ""), (access, "RS256")],
        ] {
            let e = edge_algorithms(&edge(&attrs), "dev").unwrap_err();
            assert!(e.contains("client edge"), "{e}");
        }
        let e = edge_algorithms(&edge(&[(access, "RS256")]), "dev").unwrap_err();
        assert!(e.contains(id), "{e}");
    }

    #[test]
    fn a_realm_without_an_active_rs256_key_is_refused() {
        let kek = Kek::from_raw_bytes(&[7u8; 32]).unwrap();
        let rs = kaisatsu_crypto::KeyManager::generate("RS256", "dev", &kek, "ACTIVE").unwrap();
        let es = kaisatsu_crypto::KeyManager::generate("ES256", "dev", &kek, "ACTIVE").unwrap();
        let mut retired = rs.clone();
        retired.status = "RETIRED".into();
        active_rs256(&[es.clone(), rs], "dev").unwrap();
        let e = active_rs256(&[es, retired], "dev").unwrap_err();
        assert!(e.contains("no ACTIVE RS256 key"), "{e}");
    }

    /// An empty import dir (first start) applies nothing; a realm file in it is applied.
    /// jc sweeps expired rows as `kaisatsu serve` does: an expired revoked-token row is gone
    /// once the sweeper ran, so a sweep afterwards finds nothing left.
    #[tokio::test]
    async fn the_sweeper_deletes_expired_rows() {
        let kek = Kek::from_raw_bytes(&[7u8; 32]).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek));
        store
            .mark_token_revoked(kaisatsu_core::RevokedTokenRecord {
                jti_hash: "gone".into(),
                realm_id: "dev".into(),
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
            })
            .await
            .unwrap();
        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        sweep_expired_rows(store.clone(), Duration::from_millis(10), &shutdown, &tasks);
        tokio::time::sleep(Duration::from_millis(100)).await;
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
        assert_eq!(store.sweep_expired().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn an_empty_import_dir_applies_nothing_and_a_realm_file_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let kek = Kek::from_raw_bytes(&[7u8; 32]).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek));
        let dev = setup::Realm::parse("dev").unwrap();
        assert!(
            !apply_import_dir(&store, dir.path(), None, &[], &dev)
                .await
                .unwrap()
        );
        assert!(store.list_realms().await.unwrap().is_empty());
        std::fs::write(
            dir.path().join("realm.json"),
            r#"{"realm": "rehearsal", "enabled": true}"#,
        )
        .unwrap();
        assert!(
            !apply_import_dir(&store, dir.path(), None, &[], &dev)
                .await
                .unwrap()
        );
        assert!(store.get_realm("rehearsal").await.unwrap().is_some());
    }

    /// Each realm of the import dir is applied on its own, and only the configured realm's
    /// `portal-api` says the deployment brings that realm.
    #[tokio::test]
    async fn every_realm_of_the_import_dir_is_applied_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(MemoryStore::new(Kek::from_raw_bytes(&[7u8; 32]).unwrap()));
        let client = |realm: &str, id: &str| {
            serde_json::json!({
                "realm": realm,
                "enabled": true,
                "clients": [{ "clientId": id, "enabled": true, "protocol": "openid-connect" }]
            })
        };
        for (file, value) in [
            ("00-dev.json", client("dev", "portal-api")),
            ("10-dev-more.json", client("dev", "extra")),
            ("20-jc.json", client("jc", "portal-api")),
        ] {
            std::fs::write(dir.path().join(file), value.to_string()).unwrap();
        }
        let dev = setup::Realm::parse("dev").unwrap();
        let other = setup::Realm::parse("smartcity").unwrap();
        assert!(
            apply_import_dir(&store, dir.path(), None, &[], &dev)
                .await
                .unwrap()
        );
        assert!(
            !apply_import_dir(&store, dir.path(), None, &[], &other)
                .await
                .unwrap()
        );
        for (realm, id) in [
            ("dev", "portal-api"),
            ("dev", "extra"),
            ("jc", "portal-api"),
        ] {
            assert!(
                store
                    .get_client_by_client_id(realm, id)
                    .await
                    .unwrap()
                    .is_some(),
                "{realm}/{id}"
            );
        }
        assert!(
            store
                .get_client_by_client_id("jc", "extra")
                .await
                .unwrap()
                .is_none(),
            "one realm's files never merge into another's"
        );
        // A file naming no realm is ambiguous among several.
        std::fs::write(dir.path().join("30-none.json"), r#"{"enabled": true}"#).unwrap();
        assert!(
            apply_import_dir(&store, dir.path(), None, &[], &dev)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password()
     {
        let dir = tempfile::tempdir().unwrap();
        let kek_path = dir.path().join("kek");
        std::fs::write(&kek_path, "07".repeat(32)).unwrap();

        // Unknown field in [kaisatsu] is refused by deny_unknown_fields.
        let bad_toml = format!(
            "[kaisatsu]\nkek_file = {:?}\ndatabase_url_inline = \"postgres://x\"\n",
            kek_path
        );
        assert!(Config::parse_file(dir.path(), &bad_toml).is_err());

        // Valid database_url_file is parsed and merged.
        let url_file = dir.path().join("db_url");
        std::fs::write(
            &url_file,
            "postgresql://kaisatsu_app:leaked@localhost:5432/db\n",
        )
        .unwrap();
        let good_toml = format!(
            "role = \"kaisatsu\"\n[kaisatsu]\nkek_file = {:?}\ndatabase_url_file = {:?}\n",
            kek_path, url_file
        );
        let parsed = Config::parse_file(dir.path(), &good_toml).unwrap();
        let resolved = Config::merge(Cli::default(), parsed).unwrap();
        assert_eq!(
            resolved.kaisatsu.database_url_file.as_deref(),
            Some(url_file.as_path())
        );

        // Inline password inside database_url_file is refused before any network connection.
        let kek = Kek::from_file(&kek_path).unwrap();
        let err = open_kaisatsu_store(&resolved.kaisatsu, kek.clone())
            .await
            .err()
            .expect("inline password must be refused");
        assert!(err.to_string().contains("must not carry a password"));

        // Without database_url_file, falls back to MemoryStore.
        let mem_section = KaisatsuSection {
            kek_file: Some(kek_path),
            import_dir: None,
            import_secrets_dir: None,
            database_url_file: None,
            db_password_file: None,
            db_max_connections: None,
            database: None,
            branding_file: None,
            second_factor: Default::default(),
            outbound_private_hosts: Vec::new(),
        };
        let store = open_kaisatsu_store(&mem_section, kek).await.unwrap();
        assert!(store.list_realms().await.unwrap().is_empty());
    }

    /// K16 Postgres path (runs with `KAISATSU_TEST_DATABASE_URL`, a superuser URL): a superuser
    /// URL is refused by the boot RLS check; the request role on a migrated database opens a
    /// `PgStore` whose writes persist across connections.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // The test database URL.
    async fn k16_postgres_store_runs_the_boot_rls_check_and_persists() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        let Ok(url) = std::env::var("KAISATSU_TEST_DATABASE_URL") else {
            println!("SKIP k16 postgres path: KAISATSU_TEST_DATABASE_URL is unset");
            return;
        };
        let base = PgConnectOptions::from_str(&url).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let kek_path = dir.path().join("kek");
        std::fs::write(&kek_path, "07".repeat(32)).unwrap();
        let kek = Kek::from_file(&kek_path).unwrap();
        let host = format!("{}:{}", base.get_host(), base.get_port());
        let section = |user: &str, db: &str, pw: &str| {
            let url_file = dir.path().join(format!("url-{user}"));
            std::fs::write(&url_file, format!("postgres://{user}@{host}/{db}\n")).unwrap();
            let pw_file = dir.path().join(format!("pw-{user}"));
            std::fs::write(&pw_file, pw).unwrap();
            KaisatsuSection {
                kek_file: Some(kek_path.clone()),
                import_dir: None,
                import_secrets_dir: None,
                database_url_file: Some(url_file),
                db_password_file: Some(pw_file),
                db_max_connections: Some(2),
                database: None,
                branding_file: None,
                second_factor: Default::default(),
                outbound_private_hosts: Vec::new(),
            }
        };

        // 1. The superuser connects but the boot check refuses it.
        let su_pw = url
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('@'))
            .and_then(|(cred, _)| cred.split_once(':'))
            .map(|(_, pw)| pw.to_owned())
            .unwrap_or_default();
        let err = open_kaisatsu_store(
            &section(
                base.get_username(),
                base.get_database().unwrap_or("postgres"),
                &su_pw,
            ),
            kek.clone(),
        )
        .await
        .err()
        .expect("a superuser request role is refused");
        assert!(err.to_string().contains("boot check refused"), "{err}");

        // 2. kaisatsu_app on a database migrated by kaisatsu_migrate opens and persists.
        let app_pw = uuid_like();
        let mig_pw = uuid_like();
        for (role, pw) in [("kaisatsu_app", &app_pw), ("kaisatsu_migrate", &mig_pw)] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
            )))
            .execute(&admin)
            .await
            .unwrap();
        }
        let db = format!("kaisatsu_jc_{}", uuid_like());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {db} OWNER kaisatsu_migrate"
        )))
        .execute(&admin)
        .await
        .unwrap();
        let mig = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                base.clone()
                    .database(&db)
                    .username("kaisatsu_migrate")
                    .password(&mig_pw),
            )
            .await
            .unwrap();
        PgStore::run_migrations(&mig).await.unwrap();
        mig.close().await;
        let app = section("kaisatsu_app", &db, &app_pw);
        let store = open_kaisatsu_store(&app, kek.clone()).await.unwrap();
        store
            .upsert_realm(kaisatsu_core::RealmConfig {
                id: "k16".into(),
                realm: "k16".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        drop(store);
        let again = open_kaisatsu_store(&app, kek).await.unwrap();
        assert!(
            again.get_realm("k16").await.unwrap().is_some(),
            "persisted in PostgreSQL"
        );
        drop(again);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {db} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }

    #[test]
    fn the_realm_owner_is_decided_once_and_kept() {
        let jc = setup::Realm::default();
        let dev = setup::Realm::parse("dev").unwrap();
        let owner = |realm: &setup::Realm, before: bool, import: bool| {
            let dir = tempfile::tempdir().unwrap();
            realm_owner(dir.path(), realm, before, import).unwrap()
        };
        // A fresh store: jc makes the realm, whatever its name.
        assert_eq!(owner(&jc, false, false), RealmOwner::Jc);
        assert_eq!(owner(&dev, false, false), RealmOwner::Jc);
        // The import dir brings the Portal's client: the deployment's realm.
        assert_eq!(owner(&dev, false, true), RealmOwner::Import);
        assert_eq!(owner(&jc, true, true), RealmOwner::Import);
        // Held before this start, no kept answer: an older jc made only `jc`.
        assert_eq!(owner(&jc, true, false), RealmOwner::Jc);
        assert_eq!(owner(&dev, true, false), RealmOwner::Import);

        // Once decided, the kept answer wins: a restart without the import dir still leaves an
        // imported realm alone, and a realm jc made stays jc's.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            realm_owner(dir.path(), &jc, false, true).unwrap(),
            RealmOwner::Import
        );
        assert_eq!(
            realm_owner(dir.path(), &jc, true, false).unwrap(),
            RealmOwner::Import
        );
        assert_eq!(
            realm_owner(dir.path(), &dev, false, false).unwrap(),
            RealmOwner::Jc
        );
        assert_eq!(
            realm_owner(dir.path(), &dev, true, true).unwrap(),
            RealmOwner::Jc
        );
        std::fs::write(owner_file(dir.path(), &dev), "someone").unwrap();
        assert!(realm_owner(dir.path(), &dev, true, true).is_err());
    }

    #[tokio::test]
    async fn the_linker_works_in_the_configured_realm_only() {
        let store: Arc<dyn Store> =
            Arc::new(MemoryStore::new(Kek::from_raw_bytes(&[7; 32]).unwrap()));
        for name in ["jc", "dev"] {
            store
                .upsert_realm(kaisatsu_core::RealmConfig {
                    id: name.into(),
                    realm: name.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
            let mut s = scope("endpoint:aaaa", Some("joinedcontext"), false);
            s.id = format!("{name}-endpoint-scope");
            s.realm_id = name.into();
            store.upsert_client_scope(s).await.unwrap();
            store
                .upsert_client(ClientRepresentation {
                    id: format!("{name}-portal-api"),
                    realm_id: name.into(),
                    client_id: setup::PORTAL_CLIENT.into(),
                    ..ClientRepresentation::default()
                })
                .await
                .unwrap();
        }
        let dev = setup::Realm::parse("dev").unwrap();
        assert_eq!(link_pass(&store, &dev).await.unwrap(), 1);
        let linked = |realm: &'static str| {
            let store = store.clone();
            async move {
                store
                    .get_client_by_client_id(realm, setup::PORTAL_CLIENT)
                    .await
                    .unwrap()
                    .unwrap()
                    .optional_client_scopes
            }
        };
        let scope = "endpoint:aaaa".to_owned();
        assert!(linked("dev").await.contains(&scope));
        assert!(
            !linked("jc").await.contains(&scope),
            "the other realm is left alone"
        );
    }

    #[test]
    fn the_account_console_client_is_public_pkce_and_bound_to_kaisatsus_own_path() {
        let mount = Mount::new("kaisatsu", "https", "jc.example", "/login").unwrap();
        let (name, file) =
            account_console_realm_file(&mount.public_base(), &setup::Realm::default());
        assert!(name.ends_with(".json"));
        let client = &file["clients"][0];
        assert_eq!(client["clientId"], "account-console");
        assert_eq!(client["publicClient"], true);
        assert_eq!(client["directAccessGrantsEnabled"], false);
        assert_eq!(client["serviceAccountsEnabled"], false);
        assert_eq!(client["attributes"]["pkce.code.challenge.method"], "S256");
        assert_eq!(
            client["redirectUris"],
            serde_json::json!(["https://jc.example/login/realms/jc/account/*"])
        );
        assert_eq!(client["webOrigins"], serde_json::json!(["+"]));
        assert_eq!(
            client["protocolMappers"][0]["config"]["included.custom.audience"],
            "account"
        );
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{n:x}{:x}", std::process::id())
    }
}
