//! One builder per module. Each returns the module's own axum router built over the module's own
//! state (store, keys, authorizer); no state is shared between modules, and the mount router hands
//! a request to exactly one of them.

use crate::app::StartError;
use crate::config::Config;
use crate::module::Module;
use crate::setup::Setup;
use crate::siblings::Siblings;
use axum::Router;
use jc_router::Mount;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The Portal's internal listener once its state is built (`portal::build`): jc's ekiben lets a
/// stream's report sinks post to its pipeline routes there (`ekiben::host_target`), as prod's
/// runner reaches the Portal's internal port. ponytail: one Portal per process, as the Portal's
/// own capture fallback assumes.
#[cfg(any(feature = "ekiben", feature = "portal"))]
pub(crate) static PORTAL_INTERNAL: std::sync::LazyLock<Arc<std::sync::OnceLock<SocketAddr>>> =
    std::sync::LazyLock::new(Default::default);

/// The Portal's state once `portal::build` made it: the gateway asks it for a signed-in person's
/// Endpoint token (`gateway::session_reads`, T-6783), daiya's dashboard page whom a browser's
/// Portal cookie names (`daiya::page_reader`). ponytail: one Portal per process, as
/// [`PORTAL_INTERNAL`].
#[cfg(feature = "portal")]
pub(crate) static PORTAL_STATE: std::sync::OnceLock<joinedcontext_portal::state::AppState> =
    std::sync::OnceLock::new();

#[cfg(feature = "antares")]
pub mod antares;
#[cfg(feature = "portal")]
pub(crate) mod appearance;
#[cfg(feature = "assistant")]
pub mod assistant;
#[cfg(feature = "daiya")]
pub(crate) mod daiya;
#[cfg(all(feature = "portal", feature = "dataroom"))]
pub(crate) mod portals;
#[cfg(feature = "dataroom")]
pub mod dataroom;
#[cfg(feature = "ekiben")]
mod ekiben;
#[cfg(feature = "ekiben")]
mod ekiben_pg;
#[cfg(feature = "gateway")]
pub mod gateway;
#[cfg(feature = "kaisatsu")]
mod kaisatsu;
#[cfg(feature = "model-tools")]
pub mod model_tools;
#[cfg(feature = "portal")]
pub mod portal;
#[cfg(feature = "shako")]
pub mod shako;
#[cfg(feature = "viz")]
pub mod viz;

/// The gateway's identity at the broker, what Linkerd names it on dev: the only caller whose
/// subject and provenance headers Antares believes inside `jc`.
#[cfg_attr(not(feature = "antares"), allow(dead_code))]
pub const GATEWAY_IDENTITY: &str = "context-gateway.jc";
/// The header the gateway's hop to Antares carries its secret in.
#[cfg_attr(not(feature = "gateway"), allow(dead_code))]
pub const GATEWAY_HOP_HEADER: &str = "x-jc-gateway-hop";

/// The gateway's private way into the broker: a loopback listener whose every request carries
/// [`GATEWAY_HOP_HEADER`] with this secret, and which stamps the gateway's identity.
#[derive(Clone)]
pub struct Hop {
    pub addr: SocketAddr,
    #[cfg_attr(not(feature = "gateway"), allow(dead_code))]
    pub secret: Arc<str>,
}

impl std::fmt::Debug for Hop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hop")
            .field("addr", &self.addr)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// What a module builder gets from `jc` beside its own section: the secrets first-start setup
/// holds and where its siblings are mounted.
#[derive(Debug, Clone, Copy)]
pub struct BuildCtx<'a> {
    pub setup: &'a Setup,
    pub siblings: &'a Siblings,
    pub secrets: &'a crate::config::Secrets,
    /// `[door]`: verifies kaisatsu tokens for the modules this container serves to others.
    pub door: Option<&'a std::sync::Arc<crate::door::Door>>,
}

/// Builds the router of `module` mounted at `mount`.
pub async fn build(
    module: Module,
    cfg: &Config,
    ctx: BuildCtx<'_>,
    mount: Mount,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<Router, StartError> {
    let _ = (shutdown, tasks, &ctx, cfg, &mount);
    match module {
        #[cfg(feature = "kaisatsu")]
        Module::Kaisatsu => {
            kaisatsu::build(
                &cfg.kaisatsu,
                &cfg.organizations,
                ctx,
                mount,
                shutdown,
                tasks,
            )
            .await
        }
        #[cfg(feature = "dataroom")]
        Module::Dataroom => {
            let widget =
                dataroom::assistant_widget(ctx.siblings, &cfg.assistant.catalogue_deployment);
            dataroom::build(
                &cfg.dataroom,
                &cfg.organizations,
                ctx,
                mount,
                widget,
                shutdown,
                tasks,
            )
            .await
        }
        #[cfg(feature = "daiya")]
        Module::Daiya => daiya::build(&cfg.daiya, ctx.setup, ctx.siblings, mount)
            .await
            .map_err(|e| StartError::module(Module::Daiya, e)),
        #[cfg(feature = "ekiben")]
        Module::Ekiben => {
            ekiben::build(
                &cfg.ekiben,
                ctx.setup,
                ctx.secrets,
                ctx.siblings,
                cfg.roles.contains(&Module::Portal),
            )
            .await
        }
        #[cfg(feature = "antares")]
        Module::Antares => antares::build(&cfg.antares, ctx.setup, None, shutdown)
            .await
            .map(|built| built.router),
        #[cfg(feature = "gateway")]
        Module::Gateway => gateway::build(ctx, cfg.portal_siblings, mount, None, None)
            .await
            .map(gateway::at_root),
        #[cfg(feature = "portal")]
        Module::Portal => {
            let mut organizations = portal::organization_vars(cfg, ctx.siblings.listener);
            organizations.extend(portal::apps_vars(cfg));
            organizations.extend(portal::limits_vars(cfg));
            portal::build(ctx, cfg.portal_siblings, organizations, mount, shutdown)
                .await
                .map(|(router, _)| router)
        }
        #[cfg(feature = "shako")]
        Module::Shako => {
            let state = shako::build_state(&cfg.shako, ctx.setup, ctx.secrets).await?;
            Ok(shako::build(
                state,
                mount,
                &ctx.siblings.portal_base().unwrap_or_default(),
            )
            .0)
        }
        #[cfg(feature = "viz")]
        Module::Viz => Ok(viz::build(
            &crate::siblings::live_addresses(&cfg.dataroom.addresses, ctx.siblings),
            &mount,
        )),
        #[allow(unreachable_patterns)]
        other => Err(StartError::module(
            other,
            "not compiled into this jc binary",
        )),
    }
}
