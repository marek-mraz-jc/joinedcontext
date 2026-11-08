//! Assembles the mount table from the configuration, builds every module's router, and serves them
//! all on one listener through `jc-router` (plus an optional internal admin listener).

use crate::config::{Config, MountMode};
use crate::module::Module;
use crate::modules;
use crate::runtime;
use crate::setup::Setup;
use crate::siblings::Siblings;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use jc_router::{JcRouterBuilder, Mount, MountSpec, RouterError};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
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
    #[error("admin listener: {0}")]
    Admin(String),
    #[error("server: {0}")]
    Serve(std::io::Error),
    #[error("first-start setup: {0}")]
    Setup(String),
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

/// The Portal's mount, answered with 503 until the Portal is built. The Portal discovers its
/// realm and reads its forge over HTTP while it is built, so it is built after `jc` serves its
/// siblings, and only then put in place.
#[derive(Clone, Default)]
pub struct LateRouter(Arc<std::sync::OnceLock<Router>>);

impl LateRouter {
    fn router(&self) -> Router {
        let slot = self.clone();
        Router::new().fallback(move |req: Request| {
            let slot = slot.clone();
            async move {
                match slot.0.get() {
                    Some(router) => {
                        use tower::ServiceExt;
                        match router.clone().oneshot(req).await {
                            Ok(resp) => resp,
                            Err(never) => match never {},
                        }
                    }
                    None => (StatusCode::SERVICE_UNAVAILABLE, "starting").into_response(),
                }
            }
        })
    }

    fn set(&self, router: Router) {
        let _ = self.0.set(router);
    }
}

/// The routers of every module and the `jc-router` in front of them, plus the Portal's late
/// slot when the Portal is in the role set.
pub struct Built {
    pub router: Router,
    portal: Option<(LateRouter, Mount)>,
}

/// Builds every module's router (the Portal's as a late slot) and the one `jc-router` in front
/// of them. Background work of the modules is spawned on `tasks` and stops when `shutdown` is
/// cancelled.
pub async fn build_router(
    cfg: &Config,
    ctx: modules::BuildCtx<'_>,
    plans: Vec<MountPlan>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<Built, StartError> {
    let mut builder = JcRouterBuilder::new(cfg.scheme.clone(), cfg.base_host.clone());
    for net in &cfg.trusted_proxies {
        builder = builder.trust_proxy(*net);
    }
    let mut portal = None;
    for plan in plans {
        tracing::info!(
            module = plan.module.name(),
            host = plan.spec.host.as_deref().unwrap_or("*"),
            prefix = plan.spec.path_prefix.as_deref().unwrap_or("/"),
            "mounted"
        );
        #[cfg(feature = "shako")]
        if plan.module == Module::Shako {
            let state = modules::shako::build_state(&cfg.shako, ctx.setup, ctx.secrets)?;
            let (main_router, v2_router) = modules::shako::build(state, plan.mount.clone());
            builder = builder.mount(plan.spec.clone(), main_router);
            // S1: mount `/v2/` at the host root in both one-domain and host modes
            let v2_spec = MountSpec {
                module: "shako-registry".into(),
                host: None,
                path_prefix: Some("/v2".into()),
            };
            builder = builder.mount(v2_spec, v2_router);
            continue;
        }
        if plan.module == Module::Portal {
            let late = LateRouter::default();
            builder = builder.mount(plan.spec, late.router());
            portal = Some((late, plan.mount));
            continue;
        }
        let router = modules::build(plan.module, cfg, ctx, plan.mount, shutdown, tasks).await?;
        builder = builder.mount(plan.spec, router);
    }
    Ok(Built {
        router: builder.build()?.into_axum_router(),
        portal,
    })
}

/// Whether `headers` carry `Authorization: Bearer <expected>`.
fn bearer_matches(headers: &HeaderMap, expected: &jc_secret::Secret) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|got| expected.matches(got.trim().as_bytes()))
}

/// The start report: one line per secret with its name and where it came from (`env`, `file`,
/// `generated`), never its value. With the production profile a secret given as an environment
/// value is a warning: it shows in `docker inspect` and the pod spec.
pub fn report_secrets(
    profile: crate::config::Profile,
    secrets: &crate::config::Secrets,
    setup: &Setup,
) -> Vec<(&'static str, jc_secret::Source)> {
    let lines: Vec<_> = secrets.report().into_iter().chain(setup.report()).collect();
    for (name, source) in &lines {
        if *source == jc_secret::Source::Env && profile == crate::config::Profile::Production {
            tracing::warn!(
                secret = name,
                source = source.as_str(),
                "a secret comes from the environment: in production give it as a file \
                 (NAME_FILE, a compose secret or a mounted Kubernetes Secret)"
            );
        } else {
            tracing::info!(secret = name, source = source.as_str(), "secret");
        }
    }
    lines
}

/// The internal admin router: Prometheus text of the one recorder behind the admin bearer token
/// (it carries every module's series; Antares serves its own only behind its admin token), and
/// open liveness.
fn admin_router(token: jc_secret::Secret) -> Result<Router, StartError> {
    let handle = runtime::metrics_handle().map_err(StartError::Metrics)?;
    let expected = Arc::new(token);
    let guard = move |req: Request, next: Next| {
        let expected = Arc::clone(&expected);
        async move {
            if bearer_matches(req.headers(), &expected) {
                next.run(req).await
            } else {
                unauthorized()
            }
        }
    };
    Ok(Router::new()
        .route("/metrics", get(move || std::future::ready(handle.render())))
        .route_layer(axum::middleware::from_fn(guard))
        .route("/healthz", get(|| std::future::ready("ok"))))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
    )
        .into_response()
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

/// Starts `jc`: installs the crypto provider and the metrics recorder (once per process, before
/// any module), runs first-start setup, binds the public listener (and the admin listener when
/// configured), serves the modules, provisions the forge the Portal writes to, then builds the
/// Portal against its siblings and serves it too. Serves until `shutdown` is cancelled.
pub async fn start(cfg: &Config, shutdown: CancellationToken) -> Result<Running, StartError> {
    runtime::install_crypto_provider();
    // The one recorder goes in before any module is built: a module that installs its own when
    // none is there (the Portal's `telemetry::install`) then falls back to recording into this one.
    runtime::metrics_handle().map_err(StartError::Metrics)?;
    let secrets = crate::config::Secrets::load(cfg)?;
    let admin_token = match cfg.admin_listen {
        Some(_) => Some(
            secrets
                .admin_token
                .as_ref()
                .ok_or(crate::config::ConfigError::AdminToken)?
                .secret
                .clone(),
        ),
        None => None,
    };
    let plans = plan_mounts(cfg)?;
    let setup = Setup::prepare(&cfg.state_dir, &cfg.roles, &secrets.module)?;
    report_secrets(cfg.profile, &secrets, &setup);
    // Bound before the modules are built: the Portal's sibling addresses name this listener.
    let listener = bind(cfg.listen).await?;
    let addr = listener.local_addr().map_err(StartError::Serve)?;
    let siblings = Siblings::new(&plans, addr);
    let ctx = modules::BuildCtx {
        setup: &setup,
        siblings: &siblings,
        secrets: &secrets,
    };
    let tasks = TaskTracker::new();
    let built = build_router(cfg, ctx, plans, &shutdown, &tasks).await?;
    let mut servers = vec![serve(listener, built.router, shutdown.clone())];
    let mut admin_addr = None;
    if let (Some(admin), Some(token)) = (cfg.admin_listen, admin_token) {
        let admin_router = admin_router(token)?;
        let listener = bind(admin).await?;
        admin_addr = Some(listener.local_addr().map_err(StartError::Serve)?);
        servers.push(serve(listener, admin_router, shutdown.clone()));
    }
    let running = Running {
        addr,
        admin_addr,
        shutdown,
        tasks,
        servers,
    };
    if let Some((late, mount)) = built.portal {
        let portal = async {
            if cfg.portal_siblings {
                crate::provision::forge(&siblings, &setup).await?;
            }
            modules::build(
                Module::Portal,
                cfg,
                ctx,
                mount,
                &running.shutdown,
                &running.tasks,
            )
            .await
        };
        match portal.await {
            Ok(router) => late.set(router),
            Err(e) => {
                let _ = running.stop().await;
                return Err(e);
            }
        }
    }
    tracing::info!(%addr, admin = ?admin_addr, "jc listening");
    Ok(running)
}

/// Runs `jc` until `stop` resolves (the binary passes SIGTERM/Ctrl-C), then shuts down cleanly.
pub async fn run_until(cfg: Config, stop: impl Future<Output = ()>) -> Result<(), StartError> {
    let running = start(&cfg, CancellationToken::new()).await?;
    stop.await;
    tracing::info!("shutting down");
    running.stop().await
}
