#![forbid(unsafe_code)]
//! `kaisatsu` CLI binary: `serve`, `migrate`, `apply`, `check`, `plan`, `diff`, `export`, `keys rotate` (W1b, W9).

use clap::{Parser, Subcommand};
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use kaisatsu_core::{LocalAuthorizer, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_import::{
    ApplyMode, ApplyOptions, ApplyPlan, EXIT_CHANGES_OR_APPLIED, EXIT_INVALID_INPUT,
    EXIT_NO_CHANGE, EXIT_STALE_OR_LOCK_TIMEOUT, apply_plan, apply_realm_files, check_realm_value,
    compute_realm_state_checksum, create_plan, export_realm,
};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "kaisatsu", about = "Rust identity provider replacing Keycloak")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    },
    /// Run database migrations as `kaisatsu_migrate`.
    Migrate {
        #[arg(long)]
        database_url: String,
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
        #[arg(long, default_value = "json")]
        output: String,
    },
    /// Check for configuration drift since the last apply.
    Diff {
        #[arg(short = 'f', long = "file")]
        file: PathBuf,
    },
    /// Export a realm configuration in keycloak-config-cli format.
    Export {
        #[arg(long)]
        realm: String,
        #[arg(long)]
        with_users: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.into()).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(code) => ExitCode::from(code as u8),
    }
}

async fn run(cli: CommandsContainer) -> std::result::Result<i32, i32> {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    match cli.0 {
        Commands::Check {
            file,
            allow_literal_secrets,
        } => {
            let files = load_realm_files_from_path(&file).map_err(|_| EXIT_INVALID_INPUT)?;
            let merged =
                kaisatsu_import::merge_realm_files(&files).map_err(|_| EXIT_INVALID_INPUT)?;
            let opts = ApplyOptions {
                allow_literal_secrets,
                ..Default::default()
            };
            check_realm_value(&merged, &opts).map_err(|_| EXIT_INVALID_INPUT)?;
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Plan {
            file,
            out,
            prune,
            output,
        } => {
            let files = load_realm_files_from_path(&file).map_err(|_| EXIT_INVALID_INPUT)?;
            let opts = ApplyOptions {
                mode: if prune {
                    ApplyMode::Full
                } else {
                    ApplyMode::NoDelete
                },
                dry_run: true,
                allow_literal_secrets: true,
                ..Default::default()
            };
            let plan = create_plan(&store, &files, &opts)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            let json_bytes = serde_json::to_vec_pretty(&plan).map_err(|_| EXIT_INVALID_INPUT)?;
            std::fs::write(&out, json_bytes).map_err(|_| EXIT_INVALID_INPUT)?;
            if output == "markdown" {
                println!("{}", plan.report.to_markdown());
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan.report).unwrap_or_default()
                );
            }
            Ok(plan.report.exit_code())
        }
        Commands::Apply {
            file,
            dry_run,
            prune,
            allow_literal_secrets,
            output,
        } => {
            let opts = ApplyOptions {
                mode: if prune {
                    ApplyMode::Full
                } else {
                    ApplyMode::NoDelete
                },
                dry_run,
                allow_literal_secrets,
                ..Default::default()
            };
            // Check if `file` is a saved ApplyPlan JSON
            if file.is_file()
                && let Ok(raw) = std::fs::read_to_string(&file)
                && let Ok(plan) = serde_json::from_str::<ApplyPlan>(&raw)
            {
                let report = apply_plan(&store, &plan, &opts)
                    .await
                    .map_err(|e| match e {
                        kaisatsu_core::KaisatsuError::StalePlan(_) => EXIT_STALE_OR_LOCK_TIMEOUT,
                        _ => EXIT_INVALID_INPUT,
                    })?;
                if output == "markdown" {
                    println!("{}", report.to_markdown());
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                }
                return Ok(report.exit_code());
            }
            let files = load_realm_files_from_path(&file).map_err(|_| EXIT_INVALID_INPUT)?;
            let report = apply_realm_files(&store, &files, &opts)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            if output == "markdown" {
                println!("{}", report.to_markdown());
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_default()
                );
            }
            Ok(report.exit_code())
        }
        Commands::Diff { file } => {
            let files = load_realm_files_from_path(&file).map_err(|_| EXIT_INVALID_INPUT)?;
            let merged =
                kaisatsu_import::merge_realm_files(&files).map_err(|_| EXIT_INVALID_INPUT)?;
            let realm_name = merged["realm"].as_str().unwrap_or("default");
            let sum = compute_realm_state_checksum(&store, realm_name)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            if sum == "empty" {
                return Ok(EXIT_CHANGES_OR_APPLIED);
            }
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Export { realm, with_users } => {
            let val = export_realm(&store, &realm, with_users)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Migrate { database_url } => {
            let pool = sqlx::PgPool::connect(&database_url)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            kaisatsu_store_pg::PgStore::run_migrations(&pool)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            Ok(EXIT_NO_CHANGE)
        }
        Commands::Serve {
            listen,
            kek_file,
            import_dir,
            scheme,
            host,
            path_prefix,
            trusted_proxy,
        } => {
            let kek = Kek::from_file(&kek_file).map_err(|_| EXIT_INVALID_INPUT)?;
            let mem_store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
            if let Some(dir) = import_dir {
                let files = load_realm_files_from_path(&dir).map_err(|_| EXIT_INVALID_INPUT)?;
                let _ = apply_realm_files(&mem_store, &files, &ApplyOptions::default())
                    .await
                    .map_err(|_| EXIT_INVALID_INPUT)?;
            }
            let mount = Mount::new(
                "kaisatsu",
                scheme.clone(),
                host.clone(),
                path_prefix.clone(),
            )
            .map_err(|_| EXIT_INVALID_INPUT)?;
            let state = KaisatsuState::new(
                mem_store,
                kek,
                OutboundGuard::default(),
                Arc::new(LocalAuthorizer),
            );
            let app = router(state, mount);
            // Served through the mount router so the client address (trusted proxies honoured)
            // and the forwarded scheme/host reach every handler (W9, W7 brute force per address).
            let mut builder = JcRouterBuilder::new(scheme, host);
            for cidr in &trusted_proxy {
                let net: ipnet::IpNet = cidr.parse().map_err(|_| EXIT_INVALID_INPUT)?;
                builder = builder.trust_proxy(net);
            }
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
                .map_err(|_| EXIT_INVALID_INPUT)?;
            let listener = tokio::net::TcpListener::bind(&listen)
                .await
                .map_err(|_| EXIT_INVALID_INPUT)?;
            axum::serve(
                listener,
                jc.into_axum_router()
                    .into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .map_err(|_| EXIT_INVALID_INPUT)?;
            Ok(EXIT_NO_CHANGE)
        }
    }
}

struct CommandsContainer(Commands);
impl From<Cli> for CommandsContainer {
    fn from(c: Cli) -> Self {
        Self(c.command)
    }
}
