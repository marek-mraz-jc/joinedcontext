//! shako (Git forge) inside `jc`: mounts both the forge router (`/{slug}` or `git.{base_host}`)
//! and the host-root `/v2` container registry router (`shako_server::registry_router`).
//!
//! Records live in Postgres when `[shako.database]` is set (`shako_store_pg::PgStore`: boot RLS
//! check, what a previous release left in the store log projected for jc's one tenant before
//! serving), else in memory. Git data
//! lives under `repo_root`, by default `<state_dir>/shako`, so it survives a restart too.

use crate::app::StartError;
use crate::config::ShakoSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use kaisatsu_crypto::Kek;
use shako_core::{
    Action, LocalAuthorizer, MemorySecretStore, OrgId, Principal, RoleBinding, SecretStore,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{PortalBase, ShakoState, registry_router, router};
use shako_store_memory::MemoryStore;
use shako_store_pg::{PgSecretStore, PgStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Shako, e)
}

fn repo_root(configured: Option<&Path>, state_dir: &Path) -> Result<PathBuf, StartError> {
    let dir = match configured {
        Some(dir) => dir.to_path_buf(),
        None => state_dir.join("shako"),
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|e| err(format!("cannot create repo root {}: {e}", dir.display())))?;
    Ok(dir)
}

pub(crate) const INSTANCE: shako_core::InstanceId = shako_core::InstanceId::from_u128(1);
/// The one shako organization jc's forge runs under.
pub(crate) const ORG: OrgId = OrgId::from_u128(1);

/// The configured stores: Postgres (migrated first when a migration role is set, boot RLS
/// check, log projected for `(INSTANCE, org_id)` and trimmed beside the start, the manifests a
/// release before T-7523 left in the rows copied to the blobs under `root`), its hook and mirror secrets sealed in
/// `shako.secret` with the installation's KEK (T-7280); or memory for both without a database
/// section.
async fn open_store(
    cfg: &ShakoSection,
    org_id: OrgId,
    root: &Path,
    setup: &crate::setup::Setup,
) -> Result<(Arc<dyn shako_core::Store>, Arc<dyn SecretStore>), StartError> {
    let Some(db) = &cfg.database else {
        return Ok((
            Arc::new(MemoryStore::new()),
            Arc::new(MemorySecretStore::new()),
        ));
    };
    // Read before anything connects: a database without the key would keep secrets it cannot
    // open after the next start.
    let kek = setup
        .value(crate::setup::Secret::KaisatsuKek)
        .ok_or_else(|| {
            err(
                "shako seals its hook and mirror secrets with the installation's key-encryption \
                 key, and the state directory holds no kaisatsu-kek",
            )
        })?;
    let kek = Kek::from_raw_bytes(kek.as_bytes())
        .map_err(|_| err("the installation's kaisatsu-kek is not a 32-byte key"))?;
    if let Some(opts) = db.migrate_options().map_err(err)? {
        let pool = crate::pg::connect(opts, 1).await.map_err(err)?;
        PgStore::run_migrations(&pool).await.map_err(err)?;
        pool.close().await;
    }
    let store = PgStore::connect_with(db.app_options().map_err(err)?, db.max_connections())
        .await
        .map_err(err)?;
    let projected = store.hydrate(INSTANCE, org_id).await.map_err(err)?;
    tracing::info!(projected, "shako: the typed tables hold the store log");
    // Beside the start: the log entries the tables hold leave once no rollback needs them.
    let trimmer = store.clone();
    tokio::spawn(async move {
        match trimmer.trim_log(INSTANCE, org_id).await {
            Ok(trimmed) => tracing::info!(trimmed, "shako: store log trimmed"),
            Err(e) => {
                tracing::warn!(error = %e, "shako: store log not trimmed; the next start tries again")
            }
        }
    });
    // Before anything listens, so no pull meets a half-copied registry.
    let started = std::time::Instant::now();
    let copied = store
        .copy_manifests(INSTANCE, org_id, |owner, digest, bytes| {
            let path = shako_server::blob_path(root, INSTANCE, &owner, &digest);
            shako_server::write_blob_file(path, bytes)
        })
        .await
        .map_err(err)?;
    tracing::info!(copied, took = ?started.elapsed(), "shako: the manifests are blobs");
    let secrets = PgSecretStore::new(&store, INSTANCE, &kek).map_err(err)?;
    Ok((Arc::new(store), Arc::new(secrets)))
}

/// shako's signing key: the operator's or the one setup keeps in the state directory (generated
/// once), so a restart keeps every stored image signature and issued token verifiable. No key
/// held refuses the start, never a fresh key per boot.
fn signing_key(setup: &crate::setup::Setup) -> Result<ShakoSigningKey, StartError> {
    let text = setup
        .value(crate::setup::Secret::ShakoSigningKey)
        .ok_or_else(|| err("shako has no signing key (JCBIN_SHAKO_SIGNING_KEY_FILE)"))?;
    ShakoSigningKey::from_text(text.as_bytes()).map_err(|_| {
        err("shako's signing key is not a P-256 key (64 hex, raw 32 bytes or PKCS#8 PEM)")
    })
}

pub async fn build_state(
    cfg: &ShakoSection,
    setup: &crate::setup::Setup,
) -> Result<ShakoState, StartError> {
    let root = repo_root(cfg.repo_root.as_deref(), &setup.dir)?;
    let org_id = ORG;
    let (store, secrets) = open_store(cfg, org_id, &root, setup).await?;
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
        org_id,
        principal: Principal::User("admin".into()),
        actions: vec![
            Action::Admin,
            Action::Write,
            Action::Approve,
            Action::Publish,
        ],
        project_id: None,
        repo: None,
    }]));

    let key = signing_key(setup)?;

    let mut static_tokens = Vec::new();
    if let Some(token) = setup.value(crate::setup::Secret::ShakoToken) {
        static_tokens.push(StaticTokenBinding::new(
            token,
            Subject::authenticated("admin", "admin", vec![org_id]),
        ));
    }

    let verifier = TokenVerifier::new(key).with_static_tokens(static_tokens);
    let mut state = ShakoState::new(
        INSTANCE,
        root,
        store,
        authz,
        verifier,
        shako_core::OutboundGuard::default(),
    );
    state.secret_store = secrets;
    Ok(state)
}

/// How often the outbox is looked at beside each write's own dispatch.
const OUTBOX_POLL: Duration = Duration::from_secs(30);

/// Sends what the outbox still holds: the events of a process that died before it sent them,
/// which wait for no next write (T-7508), and the push mirrors still due (T-7522).
pub fn spawn_outbox(state: ShakoState, shutdown: CancellationToken, tasks: &TaskTracker) {
    poll_outbox(state, OUTBOX_POLL, shutdown, tasks);
}

fn poll_outbox(
    state: ShakoState,
    every: Duration,
    shutdown: CancellationToken,
    tasks: &TaskTracker,
) {
    tasks.spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(every) => {
                    if let Err(e) = shako_server::dispatch_pending_outbox(&state, ORG).await {
                        tracing::warn!(error = %e, "shako: the outbox was not sent, trying again");
                    }
                    shako_server::drain_mirrors(&state, INSTANCE, ORG);
                }
            }
        }
    });
}

/// `portal` is the Portal's public base: a browser asking shako's pages without a credential is
/// sent there (shako keeps no sessions).
pub fn build(state: ShakoState, mount: Mount, portal: &str) -> (Router, Router) {
    let main_router =
        router(state.clone(), mount.clone()).layer(axum::Extension(PortalBase(portal.into())));
    let v2_router = registry_router(state, mount);
    (main_router, v2_router)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn the_poller_sends_what_no_write_dispatched() {
        let dir = tempfile::tempdir().unwrap();
        let state = ShakoState::new(
            INSTANCE,
            dir.path().to_path_buf(),
            Arc::new(MemoryStore::new()),
            Arc::new(LocalAuthorizer::new(Vec::new())),
            TokenVerifier::new(ShakoSigningKey::from_text("1".repeat(64).as_bytes()).unwrap()),
            shako_core::OutboundGuard::default(),
        );
        let event = serde_json::json!({"id": "0190a000-0000-7000-8000-000000000001", "instanceId": INSTANCE,
            "orgId": ORG, "repoId": null, "eventKind": "push", "idempotencyKey": "left",
            "payload": {}, "dispatched": false, "createdAt": "2026-11-20T10:00:00Z",
            "dispatchedAt": null});
        let event = serde_json::from_value(event).unwrap();
        state.store().append_outbox(event).await.unwrap();

        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        poll_outbox(
            state.clone(),
            Duration::from_millis(5),
            shutdown.clone(),
            &tasks,
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while !state
                .store()
                .list_undispatched_outbox(INSTANCE, ORG)
                .await
                .unwrap()
                .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the event is sent");
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
    }

    #[test]
    fn the_repo_root_is_made_private_where_configured_or_under_the_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let default = repo_root(None, tmp.path()).unwrap();
        assert_eq!(default, tmp.path().join("shako"));
        let configured = tmp.path().join("a/b/repos");
        assert_eq!(
            repo_root(Some(&configured), tmp.path()).unwrap(),
            configured
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&configured).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let e = repo_root(Some(&file.join("repos")), tmp.path())
            .unwrap_err()
            .to_string();
        assert!(e.contains("cannot create repo root"), "{e}");
    }

    fn database(dir: &Path, migrate: bool) -> crate::pg::DatabaseSection {
        let password = dir.join("password");
        std::fs::write(&password, "pw").unwrap();
        // A bound socket that never listens: the connection is refused at once.
        crate::pg::DatabaseSection {
            url: "postgres://shako_app@127.0.0.1:1/shako".into(),
            password_file: password.clone(),
            max_connections: None,
            migrate_url: migrate.then(|| "postgres://shako_owner@127.0.0.1:1/shako".into()),
            migrate_password_file: migrate.then_some(password),
            maintenance_url: None,
            maintenance_password_file: None,
        }
    }

    fn with_kek(dir: &Path, kek: Option<&str>) -> crate::setup::Setup {
        let mut given = BTreeMap::new();
        if let Some(kek) = kek {
            given.insert(
                crate::setup::Secret::KaisatsuKek,
                crate::setup::Given {
                    secret: jc_secret::Secret::new(kek.into()),
                    source: jc_secret::Source::Env,
                    path: None,
                },
            );
        }
        let roles: &[Module] = if kek.is_some() {
            &[Module::Shako, Module::Kaisatsu]
        } else {
            &[Module::Shako]
        };
        crate::setup::Setup::prepare(dir, roles, &given).unwrap()
    }

    async fn open_err(cfg: &ShakoSection, setup: &crate::setup::Setup, root: &Path) -> String {
        open_store(cfg, ORG, root, setup)
            .await
            .err()
            .expect("the store must not open")
            .to_string()
    }

    #[tokio::test]
    async fn a_database_store_refuses_to_start_without_a_usable_key_or_server() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ShakoSection {
            database: Some(database(tmp.path(), false)),
            ..ShakoSection::default()
        };
        let e = open_err(&cfg, &with_kek(tmp.path(), None), tmp.path()).await;
        assert!(e.contains("holds no kaisatsu-kek"), "{e}");
        let e = open_err(&cfg, &with_kek(tmp.path(), Some("short")), tmp.path()).await;
        assert!(e.contains("not a 32-byte key"), "{e}");
        assert!(
            !e.contains("short"),
            "the key never reaches the message: {e}"
        );
        let kek = "k".repeat(32);
        let mut unreadable = database(tmp.path(), false);
        unreadable.password_file = tmp.path().join("missing");
        let cfg_unreadable = ShakoSection {
            database: Some(unreadable),
            ..ShakoSection::default()
        };
        let setup = with_kek(tmp.path(), Some(&kek));
        let e = open_err(&cfg_unreadable, &setup, tmp.path()).await;
        assert!(e.contains("shako"), "{e}");
        let e = open_err(&cfg, &setup, tmp.path()).await;
        assert!(
            e.contains("shako"),
            "a refused server refuses the start: {e}"
        );
        let migrating = ShakoSection {
            database: Some(database(tmp.path(), true)),
            ..ShakoSection::default()
        };
        let e = open_err(&migrating, &setup, tmp.path()).await;
        assert!(e.contains("cannot connect to Postgres"), "{e}");
    }

    /// shako on real Postgres (`SHAKO_TEST_DATABASE_URL`, a superuser that only prepares, as
    /// dataroom's split test): the NOSUPERUSER owner migrates, a login that owns nothing serves;
    /// the restart projects what a release before T-7523 left in the store log and copies its
    /// manifest to a blob under the repo root. An owner without CREATE on the database and a
    /// request login with BYPASSRLS each refuse the start.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // The test reads its database URL, as dataroom's split test does.
    async fn a_postgres_forge_migrates_as_the_owner_and_serves_as_a_login_that_owns_nothing() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        let admin_url = std::env::var("SHAKO_TEST_DATABASE_URL")
            .expect("SHAKO_TEST_DATABASE_URL is set by ci.yml root-test");
        let base = PgConnectOptions::from_str(&admin_url).unwrap();
        let server = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let db = format!("jc_shako_{}", std::process::id());
        let tmp = tempfile::tempdir().unwrap();
        for statement in [
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
            // The cluster-wide roles shako's first migration makes when they are missing, made
            // by the superuser as a deployment does: the NOCREATEROLE owner cannot.
            "DO $$ BEGIN CREATE ROLE shako_migrate NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
             NOBYPASSRLS; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
             DO $$ BEGIN CREATE ROLE shako_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
             NOBYPASSRLS; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;"
                .to_owned(),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&server)
                .await
                .unwrap();
        }
        let login = |role: &str, grants: &str| {
            let pw = format!("pw-{role}-{}", std::process::id());
            let file = tmp.path().join(role);
            std::fs::write(&file, &pw).unwrap();
            let statement = format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEROLE PASSWORD '{pw}';
                 {grants}"
            );
            let url = format!(
                "postgres://{role}@{}:{}/{db}",
                base.get_host(),
                base.get_port()
            );
            (statement, url, file)
        };
        let owner = login(
            "jc_shako_owner",
            &format!("GRANT CREATE, CONNECT ON DATABASE {db} TO jc_shako_owner;"),
        );
        let app = login(
            "jc_shako_app",
            &format!(
                "GRANT CONNECT ON DATABASE {db} TO jc_shako_app; GRANT shako_app TO jc_shako_app;"
            ),
        );
        let guest = login(
            "jc_shako_guest",
            &format!("GRANT CONNECT ON DATABASE {db} TO jc_shako_guest;"),
        );
        for (statement, _, _) in [&owner, &app, &guest] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement.clone()))
                .execute(&server)
                .await
                .unwrap();
        }
        let section = |migrate: &(String, String, PathBuf)| crate::pg::DatabaseSection {
            url: app.1.clone(),
            password_file: app.2.clone(),
            max_connections: Some(2),
            migrate_url: Some(migrate.1.clone()),
            migrate_password_file: Some(migrate.2.clone()),
            maintenance_url: None,
            maintenance_password_file: None,
        };
        let setup = with_kek(tmp.path(), Some(&"k".repeat(32)));
        let repos = tmp.path().join("repos");
        let cfg = |migrate: &(String, String, PathBuf)| ShakoSection {
            database: Some(section(migrate)),
            repo_root: Some(repos.clone()),
            ..ShakoSection::default()
        };

        // An owner that may not create the schema refuses the start, the database left empty.
        let e = build_state(&cfg(&guest), &setup)
            .await
            .err()
            .expect("no CREATE, no migration")
            .to_string();
        assert!(e.contains("shako"), "{e}");

        let (log, _guard) = super::super::capture_log();
        let first = build_state(&cfg(&owner), &setup)
            .await
            .expect("migrated and served");
        let token = setup
            .value(crate::setup::Secret::ShakoToken)
            .unwrap()
            .to_owned();
        let (_, v2) = build(
            first,
            jc_router::Mount::new("shako", "http", "localhost", "/shako").unwrap(),
            "https://portal.city.test/",
        );
        use tower::ServiceExt as _;
        let request = axum::http::Request::get("/v2/")
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(v2.oneshot(request).await.unwrap().status().is_success());

        // What a release before T-7523 left: a manifest in the store log, no blob of it.
        let bytes =
            br#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json"}"#;
        let digest = format!("sha256:{}", shako_core::sha256_hex(bytes));
        let record = serde_json::json!({
            "id": "0190a000-0000-7000-8000-0000000077a1", "instanceId": INSTANCE, "orgId": ORG,
            "packageId": "0190a000-0000-7000-8000-0000000077a2", "owner": "city",
            "packageType": "container", "packageName": "image", "version": "old",
            "digest": digest, "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "subjectDigest": null, "artifactType": null, "sizeBytes": bytes.len(),
            "signed": false, "createdAt": "2026-11-01T00:00:00Z", "manifestBytes": bytes.to_vec(),
        });
        let in_db = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone().database(&db))
            .await
            .unwrap();
        sqlx::query("INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)")
            .bind(INSTANCE.as_uuid())
            .bind(ORG.as_uuid())
            .bind(serde_json::json!({"op": "upsert_package_version", "args": record}))
            .execute(&in_db)
            .await
            .unwrap();
        build_state(&cfg(&owner), &setup)
            .await
            .expect("a restart serves");
        let blobs: Vec<_> = walk(&repos)
            .into_iter()
            .filter(|f| std::fs::read(f).is_ok_and(|b| b == bytes))
            .collect();
        assert_eq!(blobs.len(), 1, "the manifest is a blob: {blobs:?}");
        /// Whether the trim was logged, asked after a tick that lets the spawned trim run.
        async fn trimmed(log: &Path) -> bool {
            tokio::time::sleep(Duration::from_millis(20)).await;
            std::fs::read_to_string(log)
                .unwrap()
                .contains("store log trimmed")
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while !trimmed(log.path()).await {}
        })
        .await
        .expect("the log is trimmed beside the start");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            text.contains("the manifests are blobs") && text.contains("copied=1"),
            "{text}"
        );
        assert!(!text.contains("pw-jc_shako"), "no password is logged");

        // A trim the server refuses leaves the log to the next start, and this one still serves.
        sqlx::raw_sql(
            "CREATE FUNCTION public.jc_refuse_trim() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'the test refuses the trim'; END $$;
             CREATE TRIGGER jc_refuse_trim BEFORE DELETE ON shako.store_log
                 FOR EACH STATEMENT EXECUTE FUNCTION public.jc_refuse_trim();",
        )
        .execute(&in_db)
        .await
        .unwrap();
        build_state(&cfg(&owner), &setup)
            .await
            .expect("a start whose trim is refused serves");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !std::fs::read_to_string(log.path())
                .unwrap()
                .contains("store log not trimmed")
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the refused trim is logged");
        sqlx::raw_sql("DROP TRIGGER jc_refuse_trim ON shako.store_log")
            .execute(&in_db)
            .await
            .unwrap();

        // An outbox the store cannot read is logged and polled again; the poller still stops.
        let polled = build_state(&cfg(&owner), &setup).await.expect("serves");
        sqlx::raw_sql("REVOKE SELECT ON shako.outbox FROM jc_shako_app, shako_app")
            .execute(&in_db)
            .await
            .unwrap();
        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        poll_outbox(polled, Duration::from_millis(5), shutdown.clone(), &tasks);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !std::fs::read_to_string(log.path())
                .unwrap()
                .contains("the outbox was not sent")
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the refused outbox is logged");
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
        sqlx::raw_sql("GRANT SELECT ON shako.outbox TO shako_app")
            .execute(&in_db)
            .await
            .unwrap();

        // A request login that passes row level security by refuses the start.
        sqlx::raw_sql("ALTER ROLE jc_shako_app BYPASSRLS")
            .execute(&server)
            .await
            .unwrap();
        let e = build_state(&cfg(&owner), &setup)
            .await
            .err()
            .expect("BYPASSRLS refused")
            .to_string();
        assert!(e.contains("shako"), "{e}");
        for statement in [
            "ALTER ROLE jc_shako_app NOBYPASSRLS".to_owned(),
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&server)
                .await
                .unwrap();
        }
    }

    /// Every file under `dir`.
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    #[tokio::test]
    async fn a_memory_forge_starts_with_the_admin_token_and_refuses_without_a_key() {
        let tmp = tempfile::tempdir().unwrap();
        let setup =
            crate::setup::Setup::prepare(tmp.path(), &[Module::Shako], &BTreeMap::new()).unwrap();
        let token = setup
            .value(crate::setup::Secret::ShakoToken)
            .unwrap()
            .to_owned();
        let state = build_state(&ShakoSection::default(), &setup).await.unwrap();
        assert!(tmp.path().join("shako").is_dir(), "the default repo root");
        let (main, v2) = build(
            state,
            jc_router::Mount::new("shako", "http", "localhost", "/shako").unwrap(),
            "https://portal.city.test/",
        );
        use tower::ServiceExt as _;
        let ask = |router: Router, path: &str, bearer: Option<&str>| {
            let mut request = axum::http::Request::get(path);
            if let Some(token) = bearer {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let request = request.body(axum::body::Body::empty()).unwrap();
            async move { router.oneshot(request).await.unwrap().status() }
        };
        assert_eq!(
            ask(v2.clone(), "/v2/", None).await,
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert!(ask(v2.clone(), "/v2/", Some(&token)).await.is_success());
        assert_eq!(
            ask(v2, "/v2/", Some("not-the-token")).await,
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert_ne!(
            ask(main, "/shako/", None).await,
            axum::http::StatusCode::NOT_FOUND
        );

        let without =
            crate::setup::Setup::prepare(tmp.path(), &[Module::Antares], &BTreeMap::new()).unwrap();
        let e = build_state(&ShakoSection::default(), &without)
            .await
            .err()
            .expect("no signing key")
            .to_string();
        assert!(e.contains("no signing key"), "{e}");
    }

    /// A forge without the admin token (its container holds the signing key alone, as a role
    /// split hands it) starts and answers no bearer at all.
    #[tokio::test]
    async fn without_the_admin_token_no_bearer_reaches_the_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let mut given = BTreeMap::new();
        given.insert(
            crate::setup::Secret::ShakoSigningKey,
            crate::setup::Given {
                secret: jc_secret::Secret::new("1".repeat(64)),
                source: jc_secret::Source::Env,
                path: None,
            },
        );
        let setup = crate::setup::Setup::prepare_with(
            tmp.path(),
            &[Module::Antares],
            &given,
            &[crate::setup::Secret::ShakoSigningKey],
        )
        .unwrap();
        assert!(setup.value(crate::setup::Secret::ShakoToken).is_none());
        let state = build_state(&ShakoSection::default(), &setup).await.unwrap();
        let (_, v2) = build(
            state,
            jc_router::Mount::new("shako", "http", "localhost", "/shako").unwrap(),
            "https://portal.city.test/",
        );
        use tower::ServiceExt as _;
        for bearer in ["", "1111111111111111", "not-a-token"] {
            let request = axum::http::Request::get("/v2/")
                .header("authorization", format!("Bearer {bearer}"))
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                v2.clone().oneshot(request).await.unwrap().status(),
                axum::http::StatusCode::UNAUTHORIZED,
                "{bearer:?}"
            );
        }
    }

    #[test]
    fn a_signing_key_that_is_not_p256_refuses_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        let mut given = BTreeMap::new();
        given.insert(
            crate::setup::Secret::ShakoSigningKey,
            crate::setup::Given {
                secret: jc_secret::Secret::new("not-a-key".into()),
                source: jc_secret::Source::Env,
                path: None,
            },
        );
        let setup = crate::setup::Setup::prepare(tmp.path(), &[Module::Shako], &given).unwrap();
        let e = signing_key(&setup).unwrap_err().to_string();
        assert!(e.contains("not a P-256 key"), "{e}");
        assert!(
            !e.contains("not-a-key"),
            "the key never reaches the message: {e}"
        );
    }

    #[test]
    fn signing_key_is_kept_across_boots_and_missing_refuses_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        let boot = || crate::setup::Setup::prepare(tmp.path(), &[Module::Shako], &BTreeMap::new());
        let first = signing_key(&boot().unwrap()).unwrap();
        let second = signing_key(&boot().unwrap()).unwrap();
        assert_eq!(first.kid, second.kid, "same key after a restart");
        let payload = b"image digest";
        assert!(second.verify_cosign_payload(payload, &first.sign_cosign_payload(payload)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("shako-signing-key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let without =
            crate::setup::Setup::prepare(tmp.path(), &[Module::Antares], &BTreeMap::new()).unwrap();
        let e = signing_key(&without).unwrap_err().to_string();
        assert!(e.contains("no signing key"), "{e}");
    }
}
