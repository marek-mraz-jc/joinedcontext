#![forbid(unsafe_code)]
//! Standalone `daiya` binary with CLI subcommands (`serve`, `import`, `export`, `check`,
//! `plan`, `apply`, `allowed`) and exit codes `0/1/2/3/4` (R2, R10a, R11).

use clap::{Parser, Subcommand};
use daiya_import::{
    DashboardImportInput, DesiredBundle, ExitCode, PlanBundle, check_dashboards_offline,
};
use daiya_model::{CrError, DataSourceInfo, InputLimits};
use daiya_server::config::{DaiyaConfig, build_state, cli_ctx, standalone_router};
use daiya_server::execute_cli_command;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Debug, Parser)]
#[command(name = "daiya", about = "JoinedContext dashboard and panel server")]
struct Cli {
    /// Configuration file (JSON): store, API tokens file, secrets, provisioning, outbound
    /// policy. Without it daiya runs on an empty in-memory store with no API tokens.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the HTTP server (listen address and mount from the configuration, these flags
    /// override them).
    Serve {
        #[arg(long)]
        listen: Option<SocketAddr>,
        #[arg(long)]
        scheme: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        path_prefix: Option<String>,
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

/// The exit code for a failed command (R2): a stale plan is 3; a store, I/O or quota failure
/// is 4; anything the input itself caused (bad JSON, refused, not found, denied) is 1.
fn exit_code_for(err: &CrError) -> i32 {
    match err {
        CrError::StalePlan(_) => ExitCode::StalePlan.code(),
        CrError::QuotaExceeded { .. } => ExitCode::StoreOrIoError.code(),
        e if e.status_code() >= 500 => ExitCode::StoreOrIoError.code(),
        _ => ExitCode::Refused.code(),
    }
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
    let mut cfg = match &cli.config {
        Some(path) => match DaiyaConfig::from_file(path) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("{e}");
                process::exit(ExitCode::Refused.code());
            }
        },
        None => DaiyaConfig::default(),
    };
    if let Commands::Serve {
        listen,
        scheme,
        host,
        path_prefix,
    } = &cli.command
    {
        if let Some(l) = listen {
            cfg.listen = *l;
        }
        if let Some(s) = scheme {
            cfg.mount.scheme.clone_from(s);
        }
        if let Some(h) = host {
            cfg.mount.host.clone_from(h);
        }
        if let Some(p) = path_prefix {
            cfg.mount.path_prefix.clone_from(p);
        }
    }
    let built = match build_state(&cfg).await {
        Ok(built) => built,
        Err(e) => {
            eprintln!("daiya cannot start: {e}");
            process::exit(ExitCode::StoreOrIoError.code());
        }
    };
    if let Some(report) = &built.provisioning {
        eprintln!(
            "provisioning: {} datasources, {} deleted, {} folders, {} dashboards",
            report.datasources_upserted,
            report.datasources_deleted,
            report.folders,
            report.dashboards
        );
    }
    let state = built.state;
    let ctx = cli_ctx(&state);

    let exit_code = match cli.command {
        Commands::Serve { .. } => {
            let mount = match cfg.mount() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("invalid mount configuration: {e}");
                    process::exit(ExitCode::Refused.code());
                }
            };
            if state.tokens.is_empty() {
                eprintln!(
                    "warning: no API tokens configured; every authenticated route answers 404"
                );
            }
            let app = standalone_router(state, mount);
            let listener = match tokio::net::TcpListener::bind(cfg.listen).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("failed to bind {}: {e}", cfg.listen);
                    process::exit(ExitCode::StoreOrIoError.code());
                }
            };
            match listener.local_addr() {
                Ok(addr) => eprintln!("daiya listening on {addr}"),
                Err(e) => eprintln!("daiya listening (address unknown: {e})"),
            }
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown)
            .await
            {
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
                exit_code_for(&e)
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
                        exit_code_for(&e)
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                exit_code_for(&e)
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
                    exit_code_for(&e)
                }
            }
        }
        Commands::Plan { file } => match tokio::fs::read_to_string(&file).await {
            Ok(text) => {
                let desired: DesiredBundle = match serde_json::from_str(&text) {
                    Ok(d) => d,
                    Err(e) => {
                        // An unreadable bundle is refused, never planned as an empty one
                        // that silently answers "no changes".
                        eprintln!("invalid bundle file {}: {e}", file.display());
                        process::exit(ExitCode::Refused.code());
                    }
                };
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
                        exit_code_for(&e)
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
                            // The applied import's own verdict: refused dashboards are 1.
                            rep["exitCode"]
                                .as_i64()
                                .and_then(|c| i32::try_from(c).ok())
                                .unwrap_or(ExitCode::Ok.code())
                        }
                        Err(e) => {
                            eprintln!("apply failed: {e}");
                            exit_code_for(&e)
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
                    exit_code_for(&e)
                }
            }
        }
    };

    process::exit(exit_code);
}
