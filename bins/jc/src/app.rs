//! Assembles the mount table from the configuration, builds every module's router, and serves them
//! all on one listener through `jc-router` (plus an optional internal admin listener).

use crate::config::{Config, MountMode};
use crate::module::Module;
use crate::modules;
use crate::runtime;
use axum::Router;
use axum::routing::get;
use jc_router::{JcRouterBuilder, Mount, MountSpec, RouterError};
use std::future::Future;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Why `jc` could not start or stopped with an error.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error("mount table refused: {0}")]
    Mount(#[from] RouterError),
    #[error("{module}: {message}")]
    Module { module: Module, message: String },
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("{0}")]
    Metrics(String),
    #[error("server: {0}")]
    Serve(std::io::Error),
}

impl StartError {
    pub(crate) fn module(module: Module, message: impl std::fmt::Display) -> Self {
        Self::Module {
            module,
            message: message.to_string(),
        }
    }
}

/// Where one module is mounted: the router-table entry and the mount context its router is built
/// with (public base, cookie path, issuer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPlan {
    pub module: Module,
    pub spec: MountSpec,
    pub mount: Mount,
}

/// Places every configured module: an explicit `[mounts.<module>]` entry wins, otherwise the mode
/// decides (slug: any host, path `/<slug>`; host: `<label>.<base_host>`, path `/`). Overlapping
/// placements are refused here, before any module starts.
pub fn plan_mounts(cfg: &Config) -> Result<Vec<MountPlan>, StartError> {
    let mut plans = Vec::with_capacity(cfg.roles.len());
    for &module in &cfg.roles {
        let (host, prefix) = match cfg.mounts.get(&module) {
            Some(o) => (
                o.host.clone().map(|h| h.trim().to_ascii_lowercase()),
                o.path_prefix.clone().unwrap_or_default(),
            ),
            None => match cfg.mode {
                MountMode::Slug => (None, format!("/{}", module.slug())),
                MountMode::Host => (
                    Some(format!("{}.{}", module.host_label(), cfg.base_host)),
                    String::new(),
                ),
            },
        };
        let mount = Mount::new(
            module.name(),
            cfg.scheme.clone(),
            host.clone().unwrap_or_else(|| cfg.base_host.clone()),
            prefix.clone(),
        )?;
        let spec = MountSpec {
            module: module.name().to_owned(),
            host,
            path_prefix: (!mount.path_prefix.is_empty()).then(|| mount.path_prefix.clone()),
        };
        plans.push(MountPlan {
            module,
            spec,
            mount,
        });
    }
    // Dry run of the router table: overlap is refused before any module state exists.
    let mut dry = JcRouterBuilder::new(cfg.scheme.clone(), cfg.base_host.clone());
    for plan in &plans {
        dry = dry.mount(plan.spec.clone(), Router::new());
    }
    dry.build()?;
    Ok(plans)
}

/// Builds every module's router and the one `jc-router` in front of them. Background work of
/// the modules is spawned on `tasks` and stops when `shutdown` is cancelled.
pub async fn build_router(
    cfg: &Config,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<Router, StartError> {
    let plans = plan_mounts(cfg)?;
    let mut builder = JcRouterBuilder::new(cfg.scheme.clone(), cfg.base_host.clone());
    for net in &cfg.trusted_proxies {
        builder = builder.trust_proxy(*net);
    }
    for plan in plans {
        let router = modules::build(plan.module, cfg, plan.mount, shutdown, tasks).await?;
        tracing::info!(
            module = plan.module.name(),
            host = plan.spec.host.as_deref().unwrap_or("*"),
            prefix = plan.spec.path_prefix.as_deref().unwrap_or("/"),
            "mounted"
        );
        builder = builder.mount(plan.spec, router);
    }
    Ok(builder.build()?.into_axum_router())
}

/// The internal admin router: Prometheus text of the one recorder, and liveness.
fn admin_router() -> Result<Router, StartError> {
    let handle = runtime::metrics_handle().map_err(StartError::Metrics)?;
    Ok(Router::new()
        .route("/metrics", get(move || std::future::ready(handle.render())))
        .route("/healthz", get(|| std::future::ready("ok"))))
}

async fn bind(addr: SocketAddr) -> Result<TcpListener, StartError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| StartError::Bind { addr, source })
}

fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
) -> JoinHandle<std::io::Result<()>> {
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
    })
}

/// A started `jc`: its bound addresses and the handles to stop it.
pub struct Running {
    pub addr: SocketAddr,
    pub admin_addr: Option<SocketAddr>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    servers: Vec<JoinHandle<std::io::Result<()>>>,
}

impl Running {
    /// The token that stops everything when cancelled.
    #[must_use]
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Waits until the servers end (after the shutdown token is cancelled) and every background
    /// task has stopped.
    pub async fn wait(self) -> Result<(), StartError> {
        let mut result = Ok(());
        for server in self.servers {
            match server.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => result = Err(StartError::Serve(e)),
                Err(e) => result = Err(StartError::Serve(std::io::Error::other(e))),
            }
        }
        self.tasks.close();
        self.tasks.wait().await;
        result
    }

    /// Cancels the shutdown token and waits for everything to stop.
    pub async fn stop(self) -> Result<(), StartError> {
        self.shutdown.cancel();
        self.wait().await
    }
}

/// Starts `jc`: installs the crypto provider and metrics recorder (once per process), builds the
/// modules, binds the public listener (and the admin listener when configured) and serves until
/// `shutdown` is cancelled.
pub async fn start(cfg: &Config, shutdown: CancellationToken) -> Result<Running, StartError> {
    runtime::install_crypto_provider();
    let tasks = TaskTracker::new();
    let router = build_router(cfg, &shutdown, &tasks).await?;
    let listener = bind(cfg.listen).await?;
    let addr = listener.local_addr().map_err(StartError::Serve)?;
    let mut servers = vec![serve(listener, router, shutdown.clone())];
    let mut admin_addr = None;
    if let Some(admin) = cfg.admin_listen {
        let admin_router = admin_router()?;
        let listener = bind(admin).await?;
        admin_addr = Some(listener.local_addr().map_err(StartError::Serve)?);
        servers.push(serve(listener, admin_router, shutdown.clone()));
    }
    tracing::info!(%addr, admin = ?admin_addr, "jc listening");
    Ok(Running {
        addr,
        admin_addr,
        shutdown,
        tasks,
        servers,
    })
}

/// Runs `jc` until `stop` resolves (the binary passes SIGTERM/Ctrl-C), then shuts down cleanly.
pub async fn run_until(cfg: Config, stop: impl Future<Output = ()>) -> Result<(), StartError> {
    let running = start(&cfg, CancellationToken::new()).await?;
    stop.await;
    tracing::info!("shutting down");
    running.stop().await
}
