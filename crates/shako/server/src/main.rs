#![forbid(unsafe_code)]
//! Standalone `shako` binary with subcommands `serve`, `migrate`, `apply -f`, `check -f`,
//! `compact`, and `fsck` (Contract S1).

use clap::{Parser, Subcommand};
use jc_router::Mount;
use shako_core::{
    Action, LocalAuthorizer, OrgId, RoleBinding, StaticTokenBinding, Subject,
};
use shako_server::{
    ShakoManifestBundle, ShakoState, apply_manifest_bundle, check_manifest_bundle,
    compact_all_repositories, fsck_all_repositories, router,
};
use shako_store_memory::MemoryStore;
use shako_store_pg::PgStore;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(name = "shako", version, about = "shako Git forge")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the HTTP server.
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        listen: SocketAddr,
        #[arg(long, default_value = "https")]
        scheme: String,
        #[arg(long, default_value = "localhost")]
        host: String,
        #[arg(long, default_value = "")]
        path_prefix: String,
        #[arg(long)]
        repo_root: Option<PathBuf>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    /// Run PostgreSQL migrations as `shako_migrate`.
    Migrate {
        #[arg(long)]
        db_url: String,
        #[arg(long)]
        password_file: Option<PathBuf>,
    },
    /// Offline validation of a declarative ForgeManifest file.
    Check {
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    /// Idempotently apply a declarative ForgeManifest file.
    Apply {
        #[arg(short = 'f', long)]
        file: PathBuf,
        #[arg(long)]
        repo_root: Option<PathBuf>,
    },
    /// Compact packs across repositories.
    Compact {
        #[arg(long)]
        repo_root: Option<PathBuf>,
    },
    /// Verify pack integrity and ref connectivity across repositories.
    Fsck {
        #[arg(long)]
        repo_root: Option<PathBuf>,
    },
}

fn build_standalone_state(
    repo_root: Option<PathBuf>,
    token_file: Option<PathBuf>,
) -> Result<(ShakoState, OrgId), String> {
    let root = repo_root
        .unwrap_or_else(|| std::env::temp_dir().join(format!("shako-{}", std::process::id())));
    std::fs::create_dir_all(&root).map_err(|e| format!("create repo root: {e}"))?;
    let org_id = OrgId::from_u128(1);
    let store = Arc::new(MemoryStore::new());
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
        org_id,
        principal: "admin".into(),
        actions: vec![Action::Admin],
        project_id: None,
        repo: None,
    }]));
    let mut tokens = Vec::new();
    if let Some(tf) = token_file {
        let raw = std::fs::read_to_string(&tf)
            .map_err(|e| format!("cannot read token file {}: {e}", tf.display()))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("token file is empty".into());
        }
        tokens.push(StaticTokenBinding::new(
            trimmed,
            Subject::authenticated("admin", "admin", vec![org_id], vec![]),
        ));
    }
    let state = ShakoState::for_standalone(root, store, authz, org_id, tokens)
        .map_err(|e| e.to_string())?;
    Ok((state, org_id))
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Serve {
            listen,
            scheme,
            host,
            path_prefix,
            repo_root,
            token_file,
        } => {
            let (state, _) = match build_standalone_state(repo_root, token_file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("shako: {e}");
                    return ExitCode::from(2);
                }
            };
            let mount = match Mount::new("shako", scheme, host, path_prefix) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("shako: invalid mount: {e}");
                    return ExitCode::from(2);
                }
            };
            let app = router(state, mount);
            let listener = match tokio::net::TcpListener::bind(listen).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("shako: bind {listen}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("shako: serve error: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Commands::Migrate {
            db_url,
            password_file,
        } => {
            let mut opts = match PgConnectOptions::from_str(&db_url) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("shako: invalid db_url: {e}");
                    return ExitCode::from(2);
                }
            };
            if let Some(pf) = password_file {
                let Ok(pw) = std::fs::read_to_string(&pf) else {
                    eprintln!("shako: cannot read password file {}", pf.display());
                    return ExitCode::from(2);
                };
                opts = opts.password(pw.trim());
            }
            let pool = match PgPoolOptions::new().max_connections(2).connect_with(opts).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("shako: db connect failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = PgStore::run_migrations(&pool).await {
                eprintln!("shako: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Commands::Check { file } => {
            let (state, org_id) = match build_standalone_state(None, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("shako: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let Ok(text) = std::fs::read_to_string(&file) else {
                eprintln!("shako: cannot read {}", file.display());
                return ExitCode::from(2);
            };
            let Ok(bundle) = serde_json::from_str::<ShakoManifestBundle>(&text) else {
                eprintln!("shako: invalid manifest JSON in {}", file.display());
                return ExitCode::from(2);
            };
            match check_manifest_bundle(&bundle, &state, org_id) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("shako check failed: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Commands::Apply { file, repo_root } => {
            let (state, org_id) = match build_standalone_state(repo_root, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("shako: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let Ok(text) = std::fs::read_to_string(&file) else {
                eprintln!("shako: cannot read {}", file.display());
                return ExitCode::from(2);
            };
            let Ok(bundle) = serde_json::from_str::<ShakoManifestBundle>(&text) else {
                eprintln!("shako: invalid manifest JSON in {}", file.display());
                return ExitCode::from(2);
            };
            match apply_manifest_bundle(&state, org_id, &bundle).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("shako apply failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Compact { repo_root } => {
            let (state, org_id) = match build_standalone_state(repo_root, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("shako: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match compact_all_repositories(&state, org_id).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("shako compact failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Fsck { repo_root } => {
            let (state, org_id) = match build_standalone_state(repo_root, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("shako: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match fsck_all_repositories(&state, org_id).await {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("shako fsck failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
