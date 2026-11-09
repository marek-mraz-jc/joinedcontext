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
use kaisatsu_core::{ClientRepresentation, ClientScopeRepresentation, LocalAuthorizer, Store};
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

/// Applies the realm the Portal signs in through (first-start setup), when the Portal runs beside
/// kaisatsu. Apply mode `no-delete`: a restart updates what changed and removes nothing an
/// operator added. Secrets reach the importer as `$(env:...)` values from memory.
async fn apply_portal_realm(store: &Arc<dyn Store>, ctx: BuildCtx<'_>) -> Result<(), StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let (Some(base), Some(_)) = (
        ctx.siblings.portal_base(),
        ctx.setup.value(setup::Secret::PortalClientSecret),
    ) else {
        return Ok(());
    };
    let mut files = setup::portal_realm_files(&base).map_err(|e| err(&e))?;
    if ctx
        .setup
        .value(setup::Secret::GatewayClientSecret)
        .is_some()
    {
        files.push(setup::gateway_realm_file());
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
async fn link_pass(store: &Arc<dyn Store>) -> Result<usize, kaisatsu_core::KaisatsuError> {
    let Some(realm) = store.get_realm(setup::PORTAL_REALM).await? else {
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
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            match link_pass(&store).await {
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
    if let Some(dir) = &cfg.import_dir {
        let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
        // A realm file reads no environment here: its secrets are `$(file:...)` under the
        // configured directory.
        let options = ApplyOptions {
            secrets_dir: cfg.import_secrets_dir.clone(),
            ..ApplyOptions::default()
        };
        apply_realm_files(&store, &files, &options)
            .await
            .map_err(|e| err(&e))?;
    }
    apply_portal_realm(&store, ctx).await?;
    follow_endpoint_scopes(store.clone(), shutdown, tasks);
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

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{n:x}{:x}", std::process::id())
    }
}
