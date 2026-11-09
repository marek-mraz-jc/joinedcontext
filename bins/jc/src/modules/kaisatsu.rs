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
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{PgStore, PgStoreConfig};
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
        apply_realm_files(store, files, &options)
            .await
            .map_err(|e| err(&e))?;
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

pub async fn build(
    cfg: &KaisatsuSection,
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
        import_brings_portal =
            apply_import_dir(&store, dir, cfg.import_secrets_dir.clone(), realm).await?;
    }
    apply_portal_realm(&store, ctx, existed_before, import_brings_portal).await?;
    check_edge_client(&store, realm).await?;
    follow_endpoint_scopes(store.clone(), realm.clone(), shutdown, tasks);
    let state = KaisatsuState::new(
        store,
        kek,
        OutboundGuard::default(),
        Arc::new(LocalAuthorizer),
    );
    Ok(router(state, mount))
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
        assert!(client(setup::PROXY_CLIENT).await.service_accounts_enabled);
        let portal = client(setup::PORTAL_CLIENT).await;
        assert!(audience(&portal).contains(&setup::ASSISTANT_CLIENT.to_owned()));
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
    #[tokio::test]
    async fn an_empty_import_dir_applies_nothing_and_a_realm_file_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let kek = Kek::from_raw_bytes(&[7u8; 32]).unwrap();
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek));
        let dev = setup::Realm::parse("dev").unwrap();
        assert!(
            !apply_import_dir(&store, dir.path(), None, &dev)
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
            !apply_import_dir(&store, dir.path(), None, &dev)
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
            apply_import_dir(&store, dir.path(), None, &dev)
                .await
                .unwrap()
        );
        assert!(
            !apply_import_dir(&store, dir.path(), None, &other)
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
            apply_import_dir(&store, dir.path(), None, &dev)
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
        let mount = Mount::new("kaisatsu", "https", "jc.example", "/id").unwrap();
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
            serde_json::json!(["https://jc.example/id/realms/jc/account/*"])
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
