#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use clap::{Parser, Subcommand};
use ekiben_bloblang::Environment;
use ekiben_core::{EnvAccess, MetricsRegistry, OrgId, OrgQuotas, ResourceManager, StreamConfig};
use ekiben_server::{
    build_full_registry, build_router, load_metrics_config, run_stream_once, OrgTokenFile,
    OrgVarsDir, Scheduler, ServerAuthConfig, ServerState, StreamManager,
};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(
    name = "ekiben",
    version,
    about = "Multi-organization Rust stream processor with Bento's core"
)]
struct Cli {
    /// Set a config field, as Bento's `--set` (only `metrics.*` is read, by `streams`; example:
    /// --set metrics.prometheus.use_histogram_timing=true)
    #[arg(long = "set", global = true)]
    set: Vec<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single stream configuration to completion (one-shot) and print a JSON summary line to stderr
    Run {
        /// Path to the stream configuration YAML/JSON file
        config: PathBuf,
        /// Allow private/loopback HTTP targets
        #[arg(long, default_value_t = false)]
        allow_private_ips: bool,
        /// The organization's quotas as a YAML or JSON file (fields of `OrgQuotas`; a missing
        /// field keeps its default), for a runner sized beyond the defaults
        #[arg(long)]
        quotas: Option<PathBuf>,
        /// Run every `now()` and `timestamp_unix*` at this RFC 3339 instant, to replay a recorded
        /// run on its own clock (example: 2026-11-07T09:16:19Z)
        #[arg(long, value_parser = parse_clock)]
        clock: Option<chrono::DateTime<chrono::Utc>>,
        /// Let `${VAR}` and Bloblang env() read the whole process environment (Bento's
        /// behaviour); without it they read only EKIBEN_VAR_* variables
        #[arg(long, default_value_t = false)]
        host_env: bool,
    },
    /// Run the streams-mode HTTP API server (listens on 127.0.0.1:4195 by default)
    Streams {
        /// Host address to bind to (defaults to 127.0.0.1 for security)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(short, long, default_value_t = 4195)]
        port: u16,
        /// Optional bearer token for authenticating API calls
        #[arg(long, env = "EKIBEN_BEARER_TOKEN")]
        bearer_token: Option<String>,
        /// A directory every replica reads once a second without a restart: each *.yaml is a
        /// stream (id = file stem), quotas.yaml maps organization ids to quotas (example:
        /// /etc/ekiben/streams, a ConfigMap mounted in each pod)
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Refuse every API call without a valid token but /ping and /ready, even when no token
        /// is configured (a hosted server)
        #[arg(long, default_value_t = false)]
        require_auth: bool,
        /// Organization tokens as `<org> <sha256 hex of the token>` lines, re-read on change
        #[arg(long)]
        org_tokens_file: Option<PathBuf>,
        /// A directory of `<org>.yaml` variable maps (`NAME: value`) for `${NAME}` and env()
        /// of each organization's streams, re-read once a second
        #[arg(long)]
        org_vars: Option<PathBuf>,
        /// Address of the shared ingest listener serving every http_server input at
        /// /{org}/{stream}{path} (example: 0.0.0.0:4196)
        #[arg(long)]
        ingest_addr: Option<String>,
        /// A file with the `metrics:` section (Bento's prometheus exporter fields and mapping)
        #[arg(long)]
        metrics: Option<PathBuf>,
        /// Resource config files, as Bento's `-r`: their `metrics:` section and their resources (caches, rate limits, processors, inputs, outputs) for every stream
        #[arg(short = 'r', long = "resources")]
        resources: Vec<PathBuf>,
        /// Optional static stream config files to load at startup
        configs: Vec<PathBuf>,
    },
    /// Run the resident cron/period scheduler over stream configuration files
    Schedule {
        /// Stream configuration files or directories
        configs: Vec<PathBuf>,
    },
    /// Execute a Bloblang mapping over line-delimited stdin and write to stdout (as `bento blobl`)
    Blobl {
        /// Bloblang mapping expression
        mapping: String,
        /// Consume each line as a raw string instead of JSON
        #[arg(short, long, default_value_t = false)]
        raw: bool,
    },
    /// List the registered components and Bloblang functions and methods as JSON (as `bento list --format json`)
    List {
        /// Output format; only `json` is supported
        #[arg(long, default_value = "json", value_parser = ["json"])]
        format: String,
    },
    /// Execute Bento-format config unit tests (`tests:` in a config or its `_bento_test.yaml`), as `bento test`
    Test {
        /// Paths to test: files, directories, or `dir/...` for every file below it
        paths: Vec<PathBuf>,
        /// Resource config files whose resources each test may use
        #[arg(short = 'r', long = "resources")]
        resources: Vec<PathBuf>,
    },
    /// Lint one or more stream configuration files and exit non-zero on errors
    Lint {
        /// Ignore missing environment variables
        #[arg(long, default_value_t = false)]
        chilled: bool,
        /// Paths to configuration files
        configs: Vec<PathBuf>,
    },
}

/// `file()` for the `blobl` command line tool: the person running it reads their own files.
struct HostFileImporter;

impl ekiben_bloblang::Importer for HostFileImporter {
    fn import(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("open {path}: {e}"))
    }
}

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    let exit_code = match run_cli(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    };
    std::process::exit(exit_code);
}

/// Reads an `OrgQuotas` file for `ekiben run --quotas`; an unknown field is refused, so a typo
/// never silently keeps a default.
fn load_quotas(path: &std::path::Path) -> Result<OrgQuotas, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read quotas '{}': {e}", path.display()))?;
    let value: serde_norway::Value =
        serde_norway::from_str(&text).map_err(|e| format!("quotas '{}': {e}", path.display()))?;
    let known: Vec<String> = serde_json::to_value(OrgQuotas::default())
        .ok()
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default();
    if let Some(map) = value.as_mapping() {
        for key in map.keys() {
            let name = key.as_str().unwrap_or_default();
            if !known.iter().any(|k| k == name) {
                return Err(format!(
                    "quotas '{}': unknown field `{name}`",
                    path.display()
                ));
            }
        }
    }
    serde_norway::from_value(value).map_err(|e| format!("quotas '{}': {e}", path.display()))
}

async fn run_cli(cli: Cli) -> Result<i32, String> {
    let global_sets = cli.set;
    match cli.command {
        Commands::Run {
            config,
            allow_private_ips,
            quotas,
            clock,
            host_env,
        } => {
            if let Some(at) = clock {
                ekiben_bloblang::functions::freeze_clock(at);
            }
            let access = if host_env {
                EnvAccess::Process
            } else {
                EnvAccess::AllowListed
            };
            let raw = std::fs::read_to_string(&config)
                .map_err(|e| format!("failed to read '{}': {e}", config.display()))?;
            let (stream_conf, _) =
                StreamConfig::parse_str_with_env(&raw, false, |k| access.lookup(k))
                    .map_err(|e| e.to_string())?;
            let reg = build_full_registry();
            let resources = ResourceManager::new();
            if allow_private_ips || quotas.is_some() {
                let mut org_quotas = match &quotas {
                    Some(path) => load_quotas(path)?,
                    None => OrgQuotas::default(),
                };
                org_quotas.allow_private_ips |= allow_private_ips;
                resources.set_org_quotas(&stream_conf.org, org_quotas);
            }
            let stream_id = config
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "run".to_string());

            let summary = run_stream_once(stream_id, stream_conf, reg, resources, None, access)
                .await
                .map_err(|e| e.to_string())?;

            let summary_json = serde_json::to_string(&summary).map_err(|e| e.to_string())?;
            // stdout belongs to the stream (a stdout output), as with `bento -c`.
            eprintln!("{summary_json}");

            if summary.success {
                Ok(0)
            } else {
                Ok(1)
            }
        }
        Commands::Streams {
            host,
            port,
            bearer_token,
            config_dir,
            require_auth,
            org_tokens_file,
            org_vars,
            ingest_addr,
            metrics,
            resources: resource_files,
            configs,
        } => {
            ekiben_io_http::http_server::set_streams_mode();
            let reg = build_full_registry();
            let resources = ResourceManager::new();
            let metrics_conf =
                load_metrics_config(metrics.as_ref(), &resource_files, &global_sets)?;
            let metrics_registry =
                MetricsRegistry::with_config(metrics_conf).map_err(|e| e.to_string())?;
            // Streams mode reads only EKIBEN_VAR_* process variables and each organization's
            // own variables (E1): a hosted process's environment holds other modules' secrets.
            let manager = StreamManager::new(reg, resources.clone(), None, EnvAccess::AllowListed)
                .with_metrics_registry(metrics_registry);

            if let Some(dir) = org_vars {
                let mut vars = OrgVarsDir::new(dir);
                let errors = vars.sync(&resources);
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
                tokio::spawn(vars.watch(resources.clone()));
            }

            // Bento's `-r`: each file's caches, rate limits and processors are shared by every
            // stream of the default organization (the one an admin token posts into).
            for path in &resource_files {
                let raw = std::fs::read_to_string(path)
                    .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
                manager
                    .register_resources(&OrgId::default(), &raw)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }

            if let Some(addr) = ingest_addr {
                let listener = TcpListener::bind(&addr)
                    .await
                    .map_err(|e| format!("failed to bind ingest listener {addr}: {e}"))?;
                ekiben_io_http::ingest::activate();
                tokio::spawn(async move {
                    if let Err(e) = ekiben_io_http::ingest::serve(listener).await {
                        tracing::error!("ingest listener: {e}");
                    }
                });
            }

            for path in configs {
                let raw = std::fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
                let (conf, _) = manager
                    .parse_config(&raw, false)
                    .map_err(|e| e.to_string())?;
                let lints = ekiben_io_http::http_server::lint_streams_mode(&conf.raw_source);
                if !lints.is_empty() {
                    return Err(format!("{}: {}", path.display(), lints.join("; ")));
                }
                let id = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "stream".to_string());
                manager
                    .create_posted(id, conf, StreamConfig::posted_json(&raw))
                    .await
                    .map_err(|e| e.to_string())?;
            }

            if let Some(dir) = config_dir {
                let mut watcher = ekiben_server::config_dir::ConfigDir::new(dir);
                // The first pass must hold: a pod that cannot read its config is not ready.
                let errors = watcher.sync(&manager).await;
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
                tokio::spawn(watcher.watch(manager.clone()));
            }

            let org_token_file = match org_tokens_file {
                Some(p) => Some(Arc::new(OrgTokenFile::load(p)?)),
                None => None,
            };
            let auth = ServerAuthConfig {
                admin_bearer_token: bearer_token,
                org_tokens: Vec::new(),
                require_auth,
                org_token_file,
                // A standalone server keeps /ready open for its orchestrator's probe.
                guard_probes: false,
            };
            let state = ServerState::new(manager, auth);
            let app = build_router(state);

            let bind_addr = format!("{host}:{port}");
            let listener = TcpListener::bind(&bind_addr)
                .await
                .map_err(|e| format!("failed to bind {bind_addr}: {e}"))?;
            axum::serve(listener, app)
                .await
                .map_err(|e| format!("server error: {e}"))?;
            Ok(0)
        }
        Commands::Schedule { configs } => {
            let reg = build_full_registry();
            let resources = ResourceManager::new();
            let access = EnvAccess::AllowListed;
            let scheduler = Scheduler::new(reg, resources, None, access);

            for path in configs {
                let raw = std::fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
                let (conf, _) = StreamConfig::parse_str_with_env(&raw, false, |k| access.lookup(k))
                    .map_err(|e| e.to_string())?;
                let id = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "scheduled".to_string());
                scheduler
                    .add_stream(id, conf, Utc::now())
                    .map_err(|e| e.to_string())?;
            }

            loop {
                let _ = scheduler.trigger_due(Utc::now());
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        Commands::Blobl { mapping, raw } => {
            // As `bento blobl`: env() and file() read the host, a failed line goes to
            // stderr and the next line runs, a deleted message prints an empty line.
            let env = Environment::new()
                .with_host_env(true)
                .with_host_file(true)
                .with_importer(Arc::new(HostFileImporter));
            let exec = match env.compile_mapping(&mapping) {
                Ok(exec) => exec,
                Err(e) => {
                    eprintln!("failed to parse mapping: {e}");
                    return Ok(1);
                }
            };
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout().lock();
            for line_res in stdin.lock().split(b'\n') {
                let line = line_res.map_err(|e| e.to_string())?;
                let line = line.strip_suffix(b"\r").unwrap_or(&line);
                match ekiben_bloblang::cli::run_line(&exec, line, raw) {
                    Ok(out) => {
                        let _ = writeln!(stdout, "{out}");
                    }
                    Err(err) => eprintln!("{err}"),
                }
            }
            let _ = stdout.flush();
            Ok(0)
        }
        Commands::List { format: _ } => {
            let mut names: serde_json::Map<String, serde_json::Value> = build_full_registry()
                .component_names()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into()))
                .collect();
            let env = Environment::new();
            names.insert("bloblang-functions".into(), env.function_names().into());
            names.insert("bloblang-methods".into(), env.method_names().into());
            // Names Bento refuses: a config using one of them runs on ekiben only.
            let mut extensions: Vec<String> = Vec::new();
            #[cfg(feature = "ngsild")]
            for (kind, name) in ekiben_io_ngsild::EXTENSION_COMPONENTS {
                extensions.push(format!("{kind}/{name}"));
            }
            for f in ekiben_bloblang::ngsild::EXTENSION_FUNCTIONS {
                extensions.push(format!("bloblang-functions/{f}"));
            }
            for m in ekiben_bloblang::ngsild::EXTENSION_METHODS {
                extensions.push(format!("bloblang-methods/{m}"));
            }
            names.insert("ekiben-extensions".into(), extensions.into());
            println!("{}", serde_json::Value::Object(names));
            Ok(0)
        }
        Commands::Test { paths, resources } => {
            let reg = build_full_registry();
            let mut out = std::io::stdout().lock();
            let mut err = std::io::stderr().lock();
            let ok = ekiben_server::unit_test::run_all(reg, &paths, &resources, &mut out, &mut err)
                .await;
            let _ = out.flush();
            Ok(if ok { 0 } else { 1 })
        }
        Commands::Lint { chilled, configs } => {
            let reg = build_full_registry();
            let mut any_errors = false;
            for path in configs {
                let raw = std::fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
                match StreamConfig::parse_str_with_env(&raw, chilled, |k| std::env::var(k).ok()) {
                    Ok((conf, env_lints)) => {
                        let report = reg.lint_stream_config(
                            &conf,
                            if chilled { Vec::new() } else { env_lints },
                        );
                        if !report.is_ok() {
                            any_errors = true;
                            for err in report.lint_errors {
                                eprintln!("{}: {err}", path.display());
                            }
                        }
                    }
                    Err(e) => {
                        any_errors = true;
                        eprintln!("{}: {e}", path.display());
                    }
                }
            }
            let _ = OrgId::default();
            Ok(if any_errors { 1 } else { 0 })
        }
    }
}

fn parse_clock(text: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| format!("not an RFC 3339 time such as 2026-11-07T09:16:19Z: {e}"))
}

#[cfg(test)]
mod tests {
    use super::load_quotas;

    fn file(text: &str) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(f.path(), text).expect("write");
        f
    }

    #[test]
    fn a_quotas_file_sets_what_it_names_and_keeps_the_rest() {
        let f = file("max_in_flight_messages: 100000\n");
        let quotas = load_quotas(f.path()).expect("quotas");
        assert_eq!(quotas.max_in_flight_messages, 100_000);
        assert_eq!(quotas.max_concurrent_streams, 64);
        assert!(!quotas.allow_private_ips);
    }

    #[test]
    fn a_quotas_file_with_an_unknown_field_is_refused() {
        let f = file("max_inflight_messages: 100000\n");
        let err = load_quotas(f.path()).expect_err("typo");
        assert!(
            err.contains("unknown field `max_inflight_messages`"),
            "{err}"
        );
    }
}
