//! One builder per module. Each returns the module's own axum router built over the module's own
//! state (store, keys, authorizer); no state is shared between modules, and the mount router hands
//! a request to exactly one of them.

use crate::app::StartError;
use crate::config::Config;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[cfg(feature = "antares")]
mod antares;
#[cfg(feature = "controlroom")]
mod controlroom;
#[cfg(feature = "dataroom")]
mod dataroom;
#[cfg(feature = "ekiben")]
mod ekiben;
#[cfg(feature = "portal")]
mod portal;
#[cfg(feature = "wardkeep")]
mod wardkeep;

/// Builds the router of `module` mounted at `mount`.
pub async fn build(
    module: Module,
    cfg: &Config,
    mount: Mount,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<Router, StartError> {
    let _ = (shutdown, tasks);
    match module {
        #[cfg(feature = "wardkeep")]
        Module::Wardkeep => wardkeep::build(&cfg.wardkeep, mount).await,
        #[cfg(feature = "dataroom")]
        Module::Dataroom => dataroom::build(&cfg.dataroom, mount).await,
        #[cfg(feature = "controlroom")]
        Module::Controlroom => Ok(controlroom::build(mount)),
        #[cfg(feature = "ekiben")]
        Module::Ekiben => ekiben::build(&cfg.ekiben),
        #[cfg(feature = "antares")]
        Module::Antares => antares::build(&cfg.antares).await,
        #[cfg(feature = "portal")]
        Module::Portal => portal::build(mount, shutdown).await,
        #[allow(unreachable_patterns)]
        other => Err(StartError::module(
            other,
            "not compiled into this jc binary",
        )),
    }
}
