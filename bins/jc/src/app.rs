//! Assembles the mount table from the configuration, builds every module's router, and serves them
//! all on one listener through `jc-router` (plus an optional internal admin listener).

use crate::access;
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
    /// More router-table entries of the same router: the other root paths of a module that
    /// answers at whole paths (`Module::root_paths`), whose router then sees the path unstripped.
    pub also: Vec<MountSpec>,
}

impl MountPlan {
    /// Every router-table entry of this module.
    pub fn specs(&self) -> impl Iterator<Item = &MountSpec> {
        std::iter::once(&self.spec).chain(&self.also)
    }

    /// Whether the module answers at whole root paths rather than under a prefix of its own.
    #[must_use]
    pub fn at_root(&self) -> bool {
        !self.also.is_empty()
    }
}

/// Places every configured module: an explicit `[mounts.<module>]` entry wins, otherwise the mode
/// decides (slug: any host, path `/<slug>`; host: `<label>.<base_host>`, path `/`). Overlapping
/// placements are refused here, before any module starts.
pub fn plan_mounts(cfg: &Config) -> Result<Vec<MountPlan>, StartError> {
    let mut plans = Vec::with_capacity(cfg.roles.len());
    for &module in &cfg.roles {
        // Model Tools has no public mount: it listens on loopback only (`start`).
        if module == Module::ModelTools {
            continue;
        }
        let roots = module.root_paths();
        if cfg.mode == MountMode::Slug && !roots.is_empty() && !cfg.mounts.contains_key(&module) {
            let mount = Mount::new(module.name(), cfg.scheme.clone(), cfg.base_host.clone(), "")?;
            let mut specs = roots.iter().map(|prefix| MountSpec {
                module: module.name().to_owned(),
                host: None,
                path_prefix: Some((*prefix).to_owned()),
            });
            let spec = specs
                .next()
                .ok_or(RouterError::InvalidPrefix(String::new()))?;
            plans.push(MountPlan {
                module,
                spec,
                mount,
                also: specs.collect(),
            });
            continue;
        }
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
            also: Vec::new(),
        });
    }
    // Dry run of the router table: overlap is refused before any module state exists.
    let mut dry = JcRouterBuilder::new(cfg.scheme.clone(), cfg.base_host.clone());
    for spec in plans.iter().flat_map(MountPlan::specs) {
        dry = dry.mount(spec.clone(), Router::new());
    }
    dry.build()?;
    Ok(plans)
}

/// The Portal's mount, answered with 503 until the Portal is built. The Portal discovers its
/// realm and reads its forge over HTTP while it is built, so it is built after `jc` serves its
/// siblings, and only then put in place.
#[derive(Clone, Default)]
pub struct LateRouter(Arc<(std::sync::OnceLock<Router>, tokio::sync::Notify)>);

impl LateRouter {
    fn router(&self) -> Router {
        let slot = self.clone();
        Router::new().fallback(move |req: Request| {
            let slot = slot.clone();
            async move {
                match slot.0.0.get() {
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
        let _ = self.0.0.set(router);
        self.0.1.notify_waiters();
    }

    /// Resolves once a router is in place.
    async fn ready(&self) {
        loop {
            let notified = self.0.1.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.0.0.get().is_some() {
                return;
            }
            notified.await;
        }
    }
}

/// The first wait between two failed Portal builds; it doubles up to [`PORTAL_RETRY_MAX`].
const PORTAL_RETRY_FIRST: std::time::Duration = std::time::Duration::from_secs(1);
const PORTAL_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs `attempt` until it builds a router, then puts it in `late`. A failed attempt (the realm's
/// discovery or the forge not answering yet) is logged and retried with backoff, so `jc` keeps
/// serving its other modules and the Portal's mount answers 503 meanwhile. Returns without a
/// router when `shutdown` is cancelled; answers what the successful attempt built beside the
/// router.
async fn build_until_ready<F, Fut, T>(
    mut attempt: F,
    late: LateRouter,
    shutdown: CancellationToken,
    first: std::time::Duration,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(Router, T), StartError>>,
{
    let mut delay = first;
    loop {
        let result = tokio::select! {
            () = shutdown.cancelled() => return None,
            result = attempt() => result,
        };
        match result {
            Ok((router, built)) => {
                late.set(router);
                tracing::info!("portal: ready");
                return Some(built);
            }
            Err(e) => tracing::warn!(error = %e, retry_in = ?delay, "portal: not built, retrying"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return None,
            () = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(PORTAL_RETRY_MAX);
    }
}

/// The routers of every module and the `jc-router` in front of them, plus the Portal's late
/// slot when the Portal is in the role set.
pub struct Built {
    pub router: Router,
    portal: Option<(LateRouter, Mount)>,
    gateway: Option<LateGateway>,
}

/// The gateway's late slot: built after the forge holds the organization repository it reads,
/// over Antares' hop when Antares runs beside it.
struct LateGateway {
    slot: LateRouter,
    mount: Mount,
    hop: Option<modules::Hop>,
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
    let mut gateway = None;
    let mut hop = None;
    for plan in plans {
        tracing::info!(
            module = plan.module.name(),
            host = plan.spec.host.as_deref().unwrap_or("*"),
            prefix = plan.spec.path_prefix.as_deref().unwrap_or("/"),
            "mounted"
        );
        #[cfg(feature = "shako")]
        if plan.module == Module::Shako {
            let state = modules::shako::build_state(&cfg.shako, ctx.setup, ctx.secrets).await?;
            let (main_router, v2_router) = modules::shako::build(state, plan.mount.clone());
            let (main_router, v2_router) = match ctx.door {
                Some(door) => (
                    door.guard(Module::Shako, main_router, ctx.setup),
                    door.guard(Module::Shako, v2_router, ctx.setup),
                ),
                None => (main_router, v2_router),
            };
            builder = builder.mount(plan.spec.clone(), access::layer("shako", main_router));
            // S1: mount `/v2/` at the host root in both one-domain and host modes
            let v2_spec = MountSpec {
                module: "shako-registry".into(),
                host: None,
                path_prefix: Some("/v2".into()),
            };
            builder = builder.mount(v2_spec, access::layer("shako", v2_router));
            continue;
        }
        if plan.module == Module::Portal {
            let late = LateRouter::default();
            builder = builder.mount(plan.spec, access::layer("portal", late.router()));
            portal = Some((late, plan.mount));
            continue;
        }
        #[cfg(feature = "antares")]
        if plan.module == Module::Antares {
            let hop_secret = cfg
                .roles
                .contains(&Module::Gateway)
                .then(|| ctx.setup.value(crate::setup::Secret::GatewayHopToken))
                .flatten();
            let built =
                modules::antares::build(&cfg.antares, ctx.setup, hop_secret, shutdown).await?;
            let router = match ctx.door {
                Some(door) => door.guard(Module::Antares, built.router, ctx.setup),
                None => built.router,
            };
            builder = builder.mount(plan.spec, access::layer("antares", router));
            hop = built.hop;
            continue;
        }
        if plan.module == Module::Gateway {
            let late = LateRouter::default();
            for spec in plan.specs() {
                let router = late.router();
                #[cfg(feature = "gateway")]
                let router = if plan.at_root() {
                    modules::gateway::at_root(router)
                } else {
                    router
                };
                builder = builder.mount(spec.clone(), access::layer("gateway", router));
            }
            gateway = Some(LateGateway {
                slot: late,
                mount: plan.mount,
                hop: None,
            });
            continue;
        }
        let mut router = modules::build(plan.module, cfg, ctx, plan.mount, shutdown, tasks).await?;
        if let Some(door) = ctx.door {
            router = door.guard(plan.module, router, ctx.setup);
        }
        builder = builder.mount(plan.spec, access::layer(plan.module.name(), router));
    }
    if let Some(late) = gateway.as_mut() {
        late.hop = hop;
    }
    Ok(Built {
        router: builder.build()?.into_axum_router(),
        portal,
        gateway,
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

/// The service credentials a role split needs held although the Portal and the module do not
/// run together: the modules this container serves through its door, and the remote modules its
/// Portal calls through a forwarder.
fn split_credentials(cfg: &Config) -> Vec<crate::setup::Secret> {
    let served = crate::config::SPLIT_MODULES
        .into_iter()
        .filter(|m| cfg.door.is_some() && cfg.roles.contains(m));
    served
        .chain(cfg.remote_modules.keys().copied())
        .filter_map(crate::setup::Secret::service_credential)
        .collect()
}

/// One loopback forwarder per `[remote.modules]` entry, each added to `siblings` so the Portal
/// calls the module there with the per-boot credential setup holds for it.
async fn remote_forwarders(
    cfg: &Config,
    secrets: &crate::config::Secrets,
    setup: &Setup,
    siblings: &mut Siblings,
) -> Result<Vec<(TcpListener, Router)>, StartError> {
    let (Some(section), Some(secret)) = (&cfg.remote, &secrets.remote_client_secret) else {
        return Ok(Vec::new());
    };
    let remote = Arc::new(
        crate::door::Remote::new(section, secret.secret.clone()).map_err(StartError::Setup)?,
    );
    let mut out = Vec::new();
    for (module, base) in &cfg.remote_modules {
        let local = crate::setup::Secret::service_credential(*module)
            .and_then(|s| setup.get(s))
            .map(|held| held.value.clone())
            .ok_or_else(|| StartError::Setup(format!("no credential held for {module}")))?;
        let (listener, router) = remote
            .forward(*module, base.clone(), local)
            .await
            .map_err(StartError::Serve)?;
        let at = listener.local_addr().map_err(StartError::Serve)?;
        tracing::info!(module = module.name(), remote = %base, forwarder = %at, "remote module");
        siblings.insert_remote(
            *module,
            crate::siblings::Sibling {
                public: base.clone(),
                internal: format!("http://{at}"),
            },
        );
        out.push((listener, router));
    }
    Ok(out)
}

/// The Portal's age identity, from `--portal-age-key-file` or the Portal's own
/// `JC_PORTAL_SOPS_AGE_KEY_FILE`, read once here so a missing or unusable file refuses the start
/// instead of failing every project's secrets at the first sync. The error names the file only.
fn portal_age_key(
    cfg: &Config,
    secrets: &crate::config::Secrets,
) -> Result<Option<std::path::PathBuf>, StartError> {
    if !cfg.roles.contains(&Module::Portal) {
        return Ok(None);
    }
    let Some(path) = secrets.portal_age_key_file.clone().or_else(|| {
        crate::config::portal_env("JC_PORTAL_SOPS_AGE_KEY_FILE").map(std::path::PathBuf::from)
    }) else {
        return Ok(None);
    };
    check_age_key(&path).map_err(|e| {
        StartError::module(
            Module::Portal,
            format!("the age key file {}: {e}", path.display()),
        )
    })?;
    Ok(Some(path))
}

#[cfg(feature = "portal")]
fn check_age_key(path: &std::path::Path) -> Result<(), String> {
    joinedcontext_portal::pipeline_secrets::check_age_key_file(path)
}

#[cfg(not(feature = "portal"))]
fn check_age_key(_: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// The start report: one line per secret with its name and where it came from (`env`, `file`,
/// `generated`), never its value. With the production profile a secret given as an environment
/// value is a warning: it shows in `docker inspect` and the pod spec.
pub fn report_secrets(
    cfg: &Config,
    secrets: &crate::config::Secrets,
    setup: &Setup,
) -> Vec<(&'static str, jc_secret::Source)> {
    let profile = cfg.profile;
    let lines: Vec<_> = secrets
        .report()
        .into_iter()
        .chain(setup.report())
        .chain(cfg.database_report())
        .collect();
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
    portal: Option<LateRouter>,
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

    /// Resolves once the Portal serves (at once when the Portal is not in the role set). jc
    /// serves its other modules before that; the Portal's mount answers 503 meanwhile.
    pub async fn portal_ready(&self) {
        if let Some(late) = &self.portal {
            late.ready().await;
        }
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
    let mut setup = Setup::prepare_with(
        &cfg.state_dir,
        &cfg.roles,
        &secrets.module,
        &split_credentials(cfg),
    )?;
    setup.age_key_file = portal_age_key(cfg, &secrets)?;
    report_secrets(cfg, &secrets, &setup);
    // Bound before the modules are built: the Portal's sibling addresses name this listener.
    let listener = bind(cfg.listen).await?;
    let addr = listener.local_addr().map_err(StartError::Serve)?;
    let mut siblings = Siblings::new(&plans, addr);
    let tasks = TaskTracker::new();
    #[cfg(feature = "model-tools")]
    if cfg.roles.contains(&Module::ModelTools) {
        let at = modules::model_tools::spawn(shutdown.clone(), &tasks)
            .await
            .map_err(|e| StartError::module(Module::ModelTools, e))?;
        tracing::info!(%at, "model-tools: listening on loopback");
        siblings.insert_loopback(Module::ModelTools, at);
    }
    let forwarders = remote_forwarders(cfg, &secrets, &setup, &mut siblings).await?;
    let door = match &cfg.door {
        Some(section) => Some(Arc::new(
            crate::door::Door::new(section).map_err(StartError::Setup)?,
        )),
        None => None,
    };
    let ctx = modules::BuildCtx {
        setup: &setup,
        siblings: &siblings,
        secrets: &secrets,
        door: door.as_ref(),
    };
    let built = build_router(cfg, ctx, plans, &shutdown, &tasks).await?;
    // A Portal setting that cannot parse refuses the start here; only what needs a sibling to
    // answer (discovery, the forge) is retried after jc serves.
    #[cfg(feature = "portal")]
    if let Some((_, mount)) = &built.portal {
        let env = crate::config::portal_env;
        let vars = if cfg.portal_siblings {
            crate::siblings::portal_vars(&siblings, &setup, env)
        } else {
            std::collections::BTreeMap::new()
        };
        modules::portal::config(mount, env, vars)?;
    }
    let mut servers = vec![serve(listener, built.router, shutdown.clone())];
    for (listener, router) in forwarders {
        servers.push(serve(listener, router, shutdown.clone()));
    }
    let mut admin_addr = None;
    if let (Some(admin), Some(token)) = (cfg.admin_listen, admin_token) {
        let admin_router = admin_router(token)?;
        let listener = bind(admin).await?;
        admin_addr = Some(listener.local_addr().map_err(StartError::Serve)?);
        servers.push(serve(listener, admin_router, shutdown.clone()));
    }
    let portal = built.portal.as_ref().map(|(late, _)| late.clone());
    let running = Running {
        addr,
        admin_addr,
        shutdown,
        tasks,
        servers,
        portal,
    };
    if let Some((late, mount)) = built.portal {
        // Built in the background: a sibling that does not answer yet (the realm's discovery
        // through the edge, the forge) delays the Portal, never stops jc. The gateway beside it
        // follows the Portal, since it reads revocations from the address the Portal's internal
        // listener took.
        let (cfg, setup, siblings) = (Arc::new(cfg.clone()), Arc::new(setup), Arc::new(siblings));
        let (secrets, door) = (Arc::new(secrets), door);
        let shutdown = running.shutdown.clone();
        let gateway = built.gateway;
        let attempt = {
            let (cfg, setup, siblings, secrets, door, shutdown) = (
                cfg.clone(),
                setup.clone(),
                siblings.clone(),
                secrets.clone(),
                door.clone(),
                shutdown.clone(),
            );
            move || {
                portal_attempt(
                    cfg.clone(),
                    Shared {
                        setup: setup.clone(),
                        siblings: siblings.clone(),
                        secrets: secrets.clone(),
                        door: door.clone(),
                    },
                    mount.clone(),
                    shutdown.clone(),
                )
            }
        };
        running.tasks.spawn(async move {
            let Some(internal) =
                build_until_ready(attempt, late, shutdown.clone(), PORTAL_RETRY_FIRST).await
            else {
                return;
            };
            if let Some(late) = gateway {
                let shared = Shared {
                    setup,
                    siblings,
                    secrets,
                    door,
                };
                let built = gateway_attempt(cfg, shared, late.mount, late.hop, internal);
                match built.await {
                    Ok(router) => late.slot.set(router),
                    Err(e) => tracing::error!(error = %e, "gateway: not built"),
                }
            }
        });
    } else if let Some(late) = built.gateway {
        if cfg.portal_siblings
            && let Err(e) = crate::provision::forge(&siblings, &setup).await
        {
            let _ = running.stop().await;
            return Err(e);
        }
        match build_gateway(ctx, cfg.portal_siblings, late.mount, late.hop, None).await {
            Ok(router) => late.slot.set(router),
            Err(e) => {
                let _ = running.stop().await;
                return Err(e);
            }
        }
    }
    tracing::info!(%addr, admin = ?admin_addr, "jc listening");
    Ok(running)
}

/// What a background build of the Portal or the gateway borrows its [`modules::BuildCtx`] from.
struct Shared {
    setup: Arc<Setup>,
    siblings: Arc<Siblings>,
    secrets: Arc<crate::config::Secrets>,
    door: Option<Arc<crate::door::Door>>,
}

impl Shared {
    fn ctx(&self) -> modules::BuildCtx<'_> {
        modules::BuildCtx {
            setup: &self.setup,
            siblings: &self.siblings,
            secrets: &self.secrets,
            door: self.door.as_ref(),
        }
    }
}

/// One attempt at the Portal: the forge it writes to, then its router.
async fn portal_attempt(
    cfg: Arc<Config>,
    shared: Shared,
    mount: Mount,
    shutdown: CancellationToken,
) -> Result<(Router, Option<std::net::SocketAddr>), StartError> {
    // Boxed as `Send` where the borrows are concrete: rustc cannot prove these futures `Send`
    // for every lifetime once they sit inside a spawned task (rust-lang/rust#102211).
    type Step<'a, T> = std::pin::Pin<Box<dyn Future<Output = Result<T, StartError>> + Send + 'a>>;
    if cfg.portal_siblings {
        let forge: Step<'_, ()> =
            Box::pin(crate::provision::forge(&shared.siblings, &shared.setup));
        forge.await?;
    }
    let ctx = shared.ctx();
    #[cfg(feature = "portal")]
    {
        let build: Step<'_, (Router, Option<std::net::SocketAddr>)> = Box::pin(
            modules::portal::build(ctx, cfg.portal_siblings, mount, &shutdown),
        );
        build.await
    }
    #[cfg(not(feature = "portal"))]
    {
        let _ = (ctx, mount, &shutdown);
        Err(StartError::module(
            Module::Portal,
            "not compiled into this jc binary",
        ))
    }
}

/// Builds the gateway, over Antares' hop when Antares runs beside it, reading the Portal's
/// internal listener at `portal` when the Portal runs beside it.
async fn build_gateway(
    ctx: modules::BuildCtx<'_>,
    wire: bool,
    mount: Mount,
    hop: Option<modules::Hop>,
    portal: Option<std::net::SocketAddr>,
) -> Result<Router, StartError> {
    #[cfg(feature = "gateway")]
    {
        modules::gateway::build(ctx, wire, mount, hop, portal).await
    }
    #[cfg(not(feature = "gateway"))]
    {
        let _ = (ctx, wire, mount, hop, portal);
        Err(StartError::module(
            Module::Gateway,
            "not compiled into this jc binary",
        ))
    }
}

/// The gateway built in the Portal's background task, once the Portal's internal listener has
/// its address.
async fn gateway_attempt(
    cfg: Arc<Config>,
    shared: Shared,
    mount: Mount,
    hop: Option<modules::Hop>,
    portal: Option<std::net::SocketAddr>,
) -> Result<Router, StartError> {
    let ctx = shared.ctx();
    // Boxed as `Send`, as in `portal_attempt` (rust-lang/rust#102211).
    let build: std::pin::Pin<Box<dyn Future<Output = Result<Router, StartError>> + Send + '_>> =
        Box::pin(build_gateway(ctx, cfg.portal_siblings, mount, hop, portal));
    build.await
}

/// Runs `jc` until `stop` resolves (the binary passes SIGTERM/Ctrl-C), then shuts down cleanly.
pub async fn run_until(cfg: Config, stop: impl Future<Output = ()>) -> Result<(), StartError> {
    let running = start(&cfg, CancellationToken::new()).await?;
    stop.await;
    tracing::info!("shutting down");
    running.stop().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tower::ServiceExt;

    async fn status(late: &LateRouter) -> StatusCode {
        let req = Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        late.router().oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn a_failed_portal_build_is_retried_and_the_mount_answers_503_until_it_builds() {
        let late = LateRouter::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt = {
            let calls = calls.clone();
            move || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err(StartError::module(
                            Module::Portal,
                            "discovery: connection refused",
                        ))
                    } else {
                        Ok((Router::new().route("/", get(|| async { "portal" })), ()))
                    }
                }
            }
        };
        assert_eq!(status(&late).await, StatusCode::SERVICE_UNAVAILABLE);
        let task = tokio::spawn(build_until_ready(
            attempt,
            late.clone(),
            CancellationToken::new(),
            Duration::from_millis(10),
        ));
        tokio::time::timeout(Duration::from_secs(10), late.ready())
            .await
            .expect("the Portal is built after two failed attempts");
        task.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(status(&late).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn shutdown_ends_the_retries_of_a_portal_that_never_builds() {
        let late = LateRouter::default();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(build_until_ready(
            || async {
                Err::<(Router, ()), _>(StartError::module(Module::Portal, "discovery: timed out"))
            },
            late.clone(),
            shutdown.clone(),
            Duration::from_millis(5),
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "a failed build keeps retrying");
        assert_eq!(status(&late).await, StatusCode::SERVICE_UNAVAILABLE);
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the retry loop stops on shutdown")
            .unwrap();
    }
}
