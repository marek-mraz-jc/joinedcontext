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
#[cfg(feature = "daiya")]
mod daiya;
#[cfg(feature = "dataroom")]
mod dataroom;
#[cfg(feature = "ekiben")]
mod ekiben;
#[cfg(feature = "kaisatsu")]
mod kaisatsu;
#[cfg(feature = "portal")]
mod portal;
#[cfg(feature = "shako")]
pub mod shako;

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
        #[cfg(feature = "kaisatsu")]
        Module::Kaisatsu => kaisatsu::build(&cfg.kaisatsu, mount).await,
        #[cfg(feature = "dataroom")]
        Module::Dataroom => dataroom::build(&cfg.dataroom, mount).await,
        #[cfg(feature = "daiya")]
        Module::Daiya => Ok(daiya::build(mount)),
        #[cfg(feature = "ekiben")]
        Module::Ekiben => ekiben::build(&cfg.ekiben),
        #[cfg(feature = "antares")]
        Module::Antares => antares::build(&cfg.antares).await,
        #[cfg(feature = "portal")]
        Module::Portal => portal::build(mount, shutdown).await,
        #[cfg(feature = "shako")]
        Module::Shako => {
            let state = shako::build_state(&cfg.shako)?;
            Ok(shako::build(state, mount).0)
        }
        #[allow(unreachable_patterns)]
        other => Err(StartError::module(
            other,
            "not compiled into this jc binary",
        )),
    }
}
