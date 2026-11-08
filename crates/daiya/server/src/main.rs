#![forbid(unsafe_code)]
//! Standalone `daiya` binary with CLI subcommands (`serve`, `import`, `export`, `check`,
//! `plan`, `apply`, `allowed`) and exit codes `0/1/2/3/4` (R2, R10a, R11).

use clap::{Parser, Subcommand};
use daiya_import::{
    DashboardImportInput, DesiredBundle, ExitCode, PlanBundle, check_dashboards_offline,
};
use daiya_model::{CrError, DataSourceInfo, InputLimits, OrgId, RequestCtx, Subject};
use daiya_server::{DaiyaState, ServerConfig, execute_cli_command, router};
use daiya_store_memory::MemoryStore;
use jc_router::Mount;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(name = "daiya", about = "JoinedContext dashboard and panel server")]
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
    },
    /// Offline validation and migration check of dashboard JSON files or directories (R2, §7.3).
    Check {
        #[arg(short = 'f', long)]
        file: PathBuf,
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Import dashboard JSON files or directories (supports `--dry-run`, R2).
    Import {
        #[arg(short = 'f', long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Export a dashboard by UID as canonical Grafana JSON at schemaVersion 42 (R2).
    Export {
        #[arg(long)]
        uid: String,
    },
    /// Compute a diff plan bundle with cryptographic state etag (R2, §7.3).
    Plan {
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    /// Apply a verified plan bundle, refusing stale plans with exit code 3 (R2, §7.3).
    Apply {
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    /// Check whether an operation on an object is allowed (R10, R10a).
    Allowed {
        #[arg(long)]
        operation: String,
        #[arg(long)]
        object: Option<String>,
    },
}

async fn load_dashboard_inputs(path: &Path) -> Result<Vec<DashboardImportInput>, CrError> {
    let mut inputs = Vec::new();
    if path.is_dir() {
        let mut dir = tokio::fs::read_dir(path)
            .await
            .map_err(|e| CrError::Internal(format!("failed to read directory: {e}")))?;
        let mut entries = Vec::new();
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| CrError::Internal(e.to_string()))?
        {
            entries.push(entry.path());
        }
        entries.sort();
        for p in entries {
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                let text = tokio::fs::read_to_string(&p)
                    .await
                    .map_err(|e| CrError::Internal(e.to_string()))?;
                let val: Value = serde_json::from_str(&text)
                    .map_err(|e| CrError::BadRequest(format!("{}: {e}", p.display())))?;
                inputs.push(DashboardImportInput {
                    raw_json: val,
                    folder_uid: None,
                    is_provisioned: true,
                    allow_ui_updates: true,
                    external_id: Some(p.display().to_string()),
                });
            }
        }
    } else {
        let text = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| CrError::Internal(format!("failed to read file: {e}")))?;
        let val: Value = serde_json::from_str(&text)
            .map_err(|e| CrError::BadRequest(format!("invalid JSON: {e}")))?;
        inputs.push(DashboardImportInput {
            raw_json: val,
            folder_uid: None,
            is_provisioned: true,
            allow_ui_updates: true,
            external_id: Some(path.display().to_string()),
        });
    }
    Ok(inputs)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let org_id = OrgId::from_u128(1);
    let store = Arc::new(MemoryStore::new());
    let state = DaiyaState::for_standalone(store, org_id);
    let ctx = RequestCtx::new(
        state.installation_id,
        org_id,
        state.default_instance_id,
        Subject::authenticated("admin", vec![org_id], vec!["platform-admin".into()]),
    );

    let exit_code = match cli.command {
        Commands::Serve {
            listen,
            scheme,
            host,
            path_prefix,
        } => {
            let cfg = ServerConfig {
                listen_addr: listen,
                ..ServerConfig::default()
            };
            let mount = match Mount::new("daiya", scheme, host, path_prefix) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("invalid mount configuration: {e}");
                    process::exit(ExitCode::Refused.code());
                }
            };
            let app = router(state, mount);
            let listener = match tokio::net::TcpListener::bind(cfg.listen_addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("failed to bind {}: {e}", cfg.listen_addr);
                    process::exit(ExitCode::StoreOrIoError.code());
                }
            };
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("server error: {e}");
                process::exit(ExitCode::StoreOrIoError.code());
            }
            ExitCode::Ok.code()
        }
        Commands::Check { file, format } => match load_dashboard_inputs(&file).await {
            Ok(inputs) => {
                let ds = vec![DataSourceInfo {
                    uid: "prometheus".into(),
                    name: "Prometheus".into(),
                    ds_type: "prometheus".into(),
                    api_version: String::new(),
                    is_default: true,
                }];
                match check_dashboards_offline(
                    &inputs,
                    &ds,
                    &BTreeSet::new(),
                    &InputLimits::default(),
                ) {
                    Ok(report) => {
                        if format == "markdown" || format == "md" {
                            println!("{}", report.to_markdown());
                        } else {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
                            );
                        }
                        report.exit_code
                    }
                    Err(e) => {
                        eprintln!("check failed: {e}");
                        ExitCode::Refused.code()
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::StoreOrIoError.code()
            }
        },
        Commands::Import {
            file,
            dry_run,
            format,
        } => match load_dashboard_inputs(&file).await {
            Ok(inputs) => {
                match execute_cli_command(
                    &state,
                    &ctx,
                    "import",
                    json!({
                        "dryRun": dry_run,
                        "dashboards": inputs,
                    }),
                )
                .await
                {
                    Ok(rep_val) => {
                        if let Ok(rep) =
                            serde_json::from_value::<daiya_import::ImportReport>(rep_val.clone())
                        {
                            if format == "markdown" || format == "md" {
                                println!("{}", rep.to_markdown());
                            } else {
                                println!(
                                    "{}",
                                    serde_json::to_string_pretty(&rep_val).unwrap_or_default()
                                );
                            }
                            rep.exit_code
                        } else {
                            ExitCode::Ok.code()
                        }
                    }
                    Err(e) => {
                        eprintln!("import failed: {e}");
                        ExitCode::Refused.code()
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::StoreOrIoError.code()
            }
        },
        Commands::Export { uid } => {
            match execute_cli_command(&state, &ctx, "export", json!({ "uid": uid })).await {
                Ok(val) => {
                    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
                    ExitCode::Ok.code()
                }
                Err(e) => {
                    eprintln!("export failed: {e}");
                    ExitCode::StoreOrIoError.code()
                }
            }
        }
        Commands::Plan { file } => match tokio::fs::read_to_string(&file).await {
            Ok(text) => {
                let desired: DesiredBundle =
                    serde_json::from_str(&text).unwrap_or_else(|_| DesiredBundle::default());
                match execute_cli_command(&state, &ctx, "plan", json!({ "bundle": desired })).await
                {
                    Ok(plan_val) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&plan_val).unwrap_or_default()
                        );
                        if plan_val["hasChanges"].as_bool() == Some(true) {
                            ExitCode::ChangesOrWarnings.code()
                        } else {
                            ExitCode::Ok.code()
                        }
                    }
                    Err(e) => {
                        eprintln!("plan failed: {e}");
                        ExitCode::Refused.code()
                    }
                }
            }
            Err(e) => {
                eprintln!("failed to read {}: {e}", file.display());
                ExitCode::StoreOrIoError.code()
            }
        },
        Commands::Apply { file } => match tokio::fs::read_to_string(&file).await {
            Ok(text) => match serde_json::from_str::<PlanBundle>(&text) {
                Ok(plan) => {
                    match execute_cli_command(&state, &ctx, "apply", json!({ "plan": plan })).await
                    {
                        Ok(rep) => {
                            println!("{}", serde_json::to_string_pretty(&rep).unwrap_or_default());
                            ExitCode::Ok.code()
                        }
                        Err(CrError::StalePlan(msg)) => {
                            eprintln!("stale plan: {msg}");
                            ExitCode::StalePlan.code()
                        }
                        Err(e) => {
                            eprintln!("apply failed: {e}");
                            ExitCode::StoreOrIoError.code()
                        }
                    }
                }
                Err(e) => {
                    eprintln!("invalid plan file: {e}");
                    ExitCode::Refused.code()
                }
            },
            Err(e) => {
                eprintln!("failed to read {}: {e}", file.display());
                ExitCode::StoreOrIoError.code()
            }
        },
        Commands::Allowed { operation, object } => {
            match execute_cli_command(
                &state,
                &ctx,
                "allowed",
                json!({ "operation": operation, "object": object }),
            )
            .await
            {
                Ok(dec) => {
                    println!("{}", serde_json::to_string_pretty(&dec).unwrap_or_default());
                    if dec["allowed"].as_bool() == Some(true) {
                        ExitCode::Ok.code()
                    } else {
                        ExitCode::Refused.code()
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::Refused.code()
                }
            }
        }
    };

    process::exit(exit_code);
}
