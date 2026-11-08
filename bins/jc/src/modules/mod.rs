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
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[cfg(feature = "antares")]
mod antares;
#[cfg(feature = "daiya")]
mod daiya;
#[cfg(feature = "dataroom")]
mod dataroom;
#[cfg(feature = "ekiben")]
mod ekiben;
#[cfg(feature = "ekiben")]
mod ekiben_pg;
#[cfg(feature = "kaisatsu")]
mod kaisatsu;
#[cfg(feature = "portal")]
pub mod portal;
#[cfg(feature = "shako")]
pub mod shako;

/// What a module builder gets from `jc` beside its own section: the secrets first-start setup
/// holds and where its siblings are mounted.
#[derive(Debug, Clone, Copy)]
pub struct BuildCtx<'a> {
    pub setup: &'a Setup,
    pub siblings: &'a Siblings,
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
    let _ = (shutdown, tasks, &ctx);
    match module {
        #[cfg(feature = "kaisatsu")]
        Module::Kaisatsu => kaisatsu::build(&cfg.kaisatsu, ctx, mount).await,
        #[cfg(feature = "dataroom")]
        Module::Dataroom => dataroom::build(&cfg.dataroom, ctx.setup, mount).await,
        #[cfg(feature = "daiya")]
        Module::Daiya => daiya::build(&cfg.daiya, ctx.setup, mount)
            .await
            .map_err(|e| StartError::module(Module::Daiya, e)),
        #[cfg(feature = "ekiben")]
        Module::Ekiben => ekiben::build(&cfg.ekiben, ctx.setup).await,
        #[cfg(feature = "antares")]
        Module::Antares => antares::build(&cfg.antares, ctx.setup).await,
        #[cfg(feature = "portal")]
        Module::Portal => portal::build(ctx, cfg.portal_siblings, mount, shutdown).await,
        #[cfg(feature = "shako")]
        Module::Shako => {
            let state = shako::build_state(&cfg.shako, ctx.setup).await?;
            Ok(shako::build(state, mount).0)
        }
        #[allow(unreachable_patterns)]
        other => Err(StartError::module(
            other,
            "not compiled into this jc binary",
        )),
    }
}
