#![forbid(unsafe_code)]
//! `kaisatsu` CLI binary: `serve`, `migrate`, `apply`, `check`, `plan`, `diff`, `export`, `keys rotate` (W1b, W9).

use clap::{Parser, Subcommand};
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use kaisatsu_core::{LocalAuthorizer, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_import::{
    ApplyMode, ApplyOptions, ApplyPlan, EXIT_CHANGES_OR_APPLIED, EXIT_INVALID_INPUT,
    EXIT_NO_CHANGE, EXIT_STALE_OR_LOCK_TIMEOUT, apply_plan, apply_realm_files, check_realm_value,
    create_plan, export_realm,
};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{PgStore, PgStoreConfig};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "kaisatsu", about = "Rust identity provider replacing Keycloak")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Where the state lives. Without `--database-url` the in-memory store is used (single-binary
/// demo, tests); with it, PostgreSQL as the request role, password from a file (W2).
#[derive(clap::Args, Clone, Default)]
struct DbArgs {
    /// PostgreSQL URL without password (`postgresql://kaisatsu_app@host:5432/db`).
    #[arg(long)]
    database_url: Option<String>,
    /// File holding the database password (secrets mount); never argv or env (W2).
    #[arg(long)]
    db_password_file: Option<PathBuf>,
    /// Pool size.
    #[arg(long, default_value_t = 5)]
    db_max_connections: u32,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the kaisatsu HTTP server.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
        #[arg(long)]
        kek_file: PathBuf,
        #[arg(long)]
        import_dir: Option<PathBuf>,
        /// Poll interval in seconds for watching `--import-dir` and re-applying on change (K17).
        #[arg(long)]
        import_watch: Option<u64>,
        #[arg(long, default_value = "https")]
        scheme: String,
        #[arg(long, default_value = "idm.localhost")]
        host: String,
        #[arg(long, default_value = "")]
        path_prefix: String,
        /// Proxy network (CIDR) whose `X-Forwarded-For/-Proto/-Host` headers are trusted; repeat
        /// for several. Without it the TCP peer is the client address the brute-force counter uses.
        #[arg(long = "trusted-proxy")]
        trusted_proxy: Vec<String>,
        /// Seconds between sweeps of expired session rows (PostgreSQL store).
        #[arg(long, default_value_t = 60)]
        sweep_interval_secs: u64,
        /// File in the secrets mount holding the SMTP password used with each realm's
        /// `smtpServer`. Without it queued e-mails stay pending (K9).
        #[arg(long)]
        smtp_password_file: Option<PathBuf>,
        /// Seconds between e-mail outbox dispatch passes (K9).
        #[arg(long, default_value_t = 10)]
        outbox_interval_secs: u64,
        /// The installation's branding file (the deployment's `global.branding` as YAML, the
        /// Portal's `JC_BRANDING_FILE`), applied at start as every realm's branding.
        #[arg(long)]
        branding_file: Option<PathBuf>,
        #[command(flatten)]
        db: DbArgs,
    },
    /// Run database migrations as `kaisatsu_migrate`.
    Migrate {
        /// URL of the migration role, without password.
        #[arg(long)]
        database_url: String,
        /// File holding the migration role's password.
        #[arg(long)]
        db_password_file: Option<PathBuf>,
    },
    /// Offline validation of realm JSON/YAML files (PR gate, W1b).
    Check {
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        #[arg(long)]
        allow_literal_secrets: bool,
    },
    /// Compute an apply plan against a realm state.
    Plan {
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        #[arg(short = 'o', long = "out")]
        out: PathBuf,
        #[arg(long)]
        prune: bool,
        #[arg(long, default_value = "json")]
        output: String,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Apply realm configuration files or a saved plan.json (W1b).
    Apply {
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        prune: bool,
        #[arg(long)]
        allow_literal_secrets: bool,
        /// Directory `$(file:...)` secret references must stay inside.
        #[arg(long)]
        secrets_dir: Option<PathBuf>,
        #[arg(long, default_value = "json")]
        output: String,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Exit 0 when the files match the stored realm, 2 when applying them would change it.
    Diff {
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Export a realm configuration in keycloak-config-cli format.
    Export {
        #[arg(long)]
        realm: String,
        #[arg(long)]
        with_users: bool,
        #[command(flatten)]
        store: StoreArgs,
    },
}

/// The store of the offline commands: the database plus the KEK that encrypts its secrets.
#[derive(clap::Args, Clone, Default)]
struct StoreArgs {
    /// KEK file; required with `--database-url`.
    #[arg(long)]
    kek_file: Option<PathBuf>,
    #[command(flatten)]
    db: DbArgs,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.into()).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(code) => ExitCode::from(code as u8),
    }
}

/// Prints the reason to stderr and maps it to an exit code. Errors never carry secrets: the
/// password and KEK are read from files and never formatted.
fn fail<E: std::fmt::Display>(code: i32) -> impl Fn(E) -> i32 {
    move |e| {
        eprintln!("kaisatsu: {e}");
        code
    }
}

fn pg_config(db: &DbArgs, url: &str) -> PgStoreConfig {
    PgStoreConfig {
        url_without_password: url.to_owned(),
        password_file: db.db_password_file.clone(),
        max_connections: db.db_max_connections,
    }
}

/// Opens the store the flags name: PostgreSQL (boot check included) or memory.
async fn open_store(
    db: &DbArgs,
    kek: Kek,
) -> std::result::Result<(Arc<dyn Store>, Option<PgStore>), i32> {
    match &db.database_url {
        Some(url) => {
            let pg = PgStore::connect(&pg_config(db, url), kek)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            Ok((Arc::new(pg.clone()), Some(pg)))
        }
        None => Ok((Arc::new(MemoryStore::new(kek)), None)),
    }
}

async fn offline_store(args: &StoreArgs) -> std::result::Result<Arc<dyn Store>, i32> {
    let kek = match (&args.kek_file, &args.db.database_url) {
        (Some(path), _) => Kek::from_file(path).map_err(fail(EXIT_INVALID_INPUT))?,
        (None, Some(_)) => {
            return Err(fail(EXIT_INVALID_INPUT)(
                "--kek-file is required with --database-url",
            ));
        }
        (None, None) => Kek::generate_for_tests(),
    };
    Ok(open_store(&args.db, kek).await?.0)
}

fn print_report<T: serde::Serialize>(output: &str, markdown: impl FnOnce() -> String, report: &T) {
    if output == "markdown" {
        println!("{}", markdown());
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_default()
        );
    }
}

fn mode(prune: bool) -> ApplyMode {
    if prune {
        ApplyMode::Full
    } else {
        ApplyMode::NoDelete
    }
}

async fn run(cli: CommandsContainer) -> std::result::Result<i32, i32> {
    match cli.0 {
        Commands::Check {
            file,
            allow_literal_secrets,
        } => {
            let files = load_realm_files_from_path(&file).map_err(fail(EXIT_INVALID_INPUT))?;
            let merged =
                kaisatsu_import::merge_realm_files(&files).map_err(fail(EXIT_INVALID_INPUT))?;
            let opts = ApplyOptions {
                allow_literal_secrets,
                ..Default::default()
            }
            .with_process_env();
            check_realm_value(&merged, &opts).map_err(fail(EXIT_INVALID_INPUT))?;
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Plan {
            file,
            out,
            prune,
            output,
            store,
        } => {
            let store = offline_store(&store).await?;
            let files = load_realm_files_from_path(&file).map_err(fail(EXIT_INVALID_INPUT))?;
            let opts = ApplyOptions {
                mode: mode(prune),
                dry_run: true,
                allow_literal_secrets: true,
                ..Default::default()
            }
            .with_process_env();
            let plan = create_plan(&store, &files, &opts)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            let json_bytes = serde_json::to_vec_pretty(&plan).map_err(fail(EXIT_INVALID_INPUT))?;
            std::fs::write(&out, json_bytes).map_err(fail(EXIT_INVALID_INPUT))?;
            print_report(&output, || plan.report.to_markdown(), &plan.report);
            Ok(plan.report.exit_code())
        }
        Commands::Apply {
            file,
            dry_run,
            prune,
            allow_literal_secrets,
            secrets_dir,
            output,
            store,
        } => {
            let store = offline_store(&store).await?;
            let opts = ApplyOptions {
                mode: mode(prune),
                dry_run,
                allow_literal_secrets,
                secrets_dir,
                ..Default::default()
            }
            .with_process_env();
            // A saved plan.json applies only onto the state it was computed against.
            if file.is_file()
                && let Ok(raw) = std::fs::read_to_string(&file)
                && let Ok(plan) = serde_json::from_str::<ApplyPlan>(&raw)
            {
                let report = apply_plan(&store, &plan, &opts).await.map_err(|e| {
                    let code = match e {
                        kaisatsu_core::KaisatsuError::StalePlan(_) => EXIT_STALE_OR_LOCK_TIMEOUT,
                        _ => EXIT_INVALID_INPUT,
                    };
                    fail(code)(e)
                })?;
                print_report(&output, || report.to_markdown(), &report);
                return Ok(report.exit_code());
            }
            let files = load_realm_files_from_path(&file).map_err(fail(EXIT_INVALID_INPUT))?;
            let report = apply_realm_files(&store, &files, &opts)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            print_report(&output, || report.to_markdown(), &report);
            Ok(report.exit_code())
        }
        Commands::Diff { file, store } => {
            let store = offline_store(&store).await?;
            let files = load_realm_files_from_path(&file).map_err(fail(EXIT_INVALID_INPUT))?;
            let opts = ApplyOptions {
                dry_run: true,
                allow_literal_secrets: true,
                ..Default::default()
            }
            .with_process_env();
            let report = apply_realm_files(&store, &files, &opts)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            Ok(if report.has_changes() {
                EXIT_CHANGES_OR_APPLIED
            } else {
                EXIT_NO_CHANGE
            })
        }
        Commands::Export {
            realm,
            with_users,
            store,
        } => {
            let store = offline_store(&store).await?;
            let val = export_realm(&store, &realm, with_users)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Migrate {
            database_url,
            db_password_file,
        } => {
            let db = DbArgs {
                database_url: None,
                db_password_file,
                db_max_connections: 1,
            };
            let opts = pg_config(&db, &database_url)
                .connect_options()
                .map_err(fail(EXIT_INVALID_INPUT))?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .connect_with(opts)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            PgStore::run_migrations(&pool)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            pool.close().await;
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Serve {
            listen,
            kek_file,
            import_dir,
            import_watch,
            scheme,
            host,
            path_prefix,
            trusted_proxy,
            sweep_interval_secs,
            smtp_password_file,
            outbox_interval_secs,
            branding_file,
            db,
        } => {
            let kek = Kek::from_file(&kek_file).map_err(fail(EXIT_INVALID_INPUT))?;
            let mount = Mount::new(
                "kaisatsu",
                scheme.clone(),
                host.clone(),
                path_prefix.clone(),
            )
            .map_err(fail(EXIT_INVALID_INPUT))?;
            let mut builder = JcRouterBuilder::new(scheme, host);
            for cidr in &trusted_proxy {
                let net: ipnet::IpNet = cidr.parse().map_err(fail(EXIT_INVALID_INPUT))?;
                builder = builder.trust_proxy(net);
            }
            let (store, pg) = open_store(&db, kek.clone()).await?;
            if let Some(dir) = import_dir {
                // Applied at start like Keycloak's `--import-realm`; idempotent on a restart.
                let files = load_realm_files_from_path(&dir).map_err(fail(EXIT_INVALID_INPUT))?;
                for realm_files in group_by_realm(files) {
                    apply_realm_files(
                        &store,
                        &realm_files,
                        &ApplyOptions::default().with_process_env(),
                    )
                    .await
                    .map_err(fail(EXIT_INVALID_INPUT))?;
                }
                if let Some(watch_secs) = import_watch.filter(|&s| s > 0) {
                    let mut last_digest =
                        kaisatsu_server::compute_import_dir_digest(&dir).unwrap_or_default();
                    let watch_store = store.clone();
                    let watch_dir = dir.clone();
                    tokio::spawn(async move {
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_secs(watch_secs));
                        loop {
                            tick.tick().await;
                            if let Err(e) = kaisatsu_server::poll_import_dir_once(
                                &watch_store,
                                &watch_dir,
                                &mut last_digest,
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "kaisatsu --import-dir watch apply failed; keeping last good state");
                            }
                        }
                    });
                }
            }
            if let Some(file) = &branding_file {
                // A broken file leaves the pages plain; sign-in stays up.
                match kaisatsu_server::apply_branding_file(&store, file).await {
                    Ok(n) => tracing::info!(realms = n, "branding file applied"),
                    Err(e) => tracing::error!(error = %e, "branding file not applied"),
                }
            }
            if let Some(pg) = pg {
                // Expired rows go in `FOR UPDATE SKIP LOCKED` batches; caches follow
                // `LISTEN kaisatsu_changed` (W2).
                pg.spawn_change_listener();
                let interval = std::time::Duration::from_secs(sweep_interval_secs.max(1));
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(interval);
                    loop {
                        tick.tick().await;
                        if let Err(e) = pg.sweep_expired_batch(500).await {
                            tracing::warn!(error = %e, "kaisatsu sweeper");
                        }
                    }
                });
            }
            if let Some(pw_file) = smtp_password_file {
                let sender = Arc::new(kaisatsu_login::RealmSmtpSender::new(store.clone(), pw_file));
                let dispatcher =
                    kaisatsu_login::OutboxDispatcher::new(store.clone(), kek.clone(), sender);
                let mail_mount = mount.clone();
                let interval = std::time::Duration::from_secs(outbox_interval_secs.max(1));
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(interval);
                    loop {
                        tick.tick().await;
                        if let Err(e) = dispatcher.dispatch_all_realms(&mail_mount).await {
                            tracing::warn!(error = %e.public_detail(), "kaisatsu outbox");
                        }
                    }
                });
            } else {
                tracing::warn!("no --smtp-password-file: queued e-mails stay pending");
            }
            let state = KaisatsuState::new(
                store,
                kek,
                OutboundGuard::default(),
                Arc::new(LocalAuthorizer),
            );
            let app = router(state, mount);
            // Served through the mount router so the client address (trusted proxies honoured)
            // and the forwarded scheme/host reach every handler (W9, W7 brute force per address).
            let jc = builder
                .mount(
                    MountSpec {
                        module: "kaisatsu".into(),
                        host: None,
                        path_prefix: (!path_prefix.is_empty()).then_some(path_prefix),
                    },
                    app,
                )
                .build()
                .map_err(fail(EXIT_INVALID_INPUT))?;
            let listener = tokio::net::TcpListener::bind(&listen)
                .await
                .map_err(fail(EXIT_INVALID_INPUT))?;
            eprintln!(
                "kaisatsu: listening on {}",
                listener.local_addr().map_err(fail(EXIT_INVALID_INPUT))?
            );
            axum::serve(
                listener,
                jc.into_axum_router()
                    .into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(fail(EXIT_INVALID_INPUT))?;
            eprintln!("kaisatsu: stopped");
            Ok(EXIT_NO_CHANGE)
        }
    }
}

/// Resolves on SIGTERM (Kubernetes stopping the pod) or Ctrl-C: open requests finish, new
/// connections are refused, and the process exits 0.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}

/// Splits an import directory into one file list per realm (the `realm` field), keeping
/// file-name order inside each, so several realms can sit in one mounted directory.
fn group_by_realm(
    files: Vec<(String, serde_json::Value)>,
) -> Vec<Vec<(String, serde_json::Value)>> {
    let mut groups: Vec<(String, Vec<(String, serde_json::Value)>)> = Vec::new();
    for (name, doc) in files {
        let realm = doc["realm"].as_str().unwrap_or_default().to_owned();
        match groups.iter_mut().find(|(r, _)| *r == realm) {
            Some((_, list)) => list.push((name, doc)),
            None => groups.push((realm, vec![(name, doc)])),
        }
    }
    groups.into_iter().map(|(_, list)| list).collect()
}

struct CommandsContainer(Commands);
impl From<Cli> for CommandsContainer {
    fn from(c: Cli) -> Self {
        Self(c.command)
    }
}
