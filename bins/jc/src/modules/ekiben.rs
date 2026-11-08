//! ekiben (stream processor) inside `jc`, hardened for a shared process:
//! - the API refuses every request without the configured bearer token (`require_auth`), even
//!   the read-only endpoints; only `/ping` is open;
//! - a posted config and Bloblang `env()` read no process environment (`EnvAccess::None`): `jc`'s
//!   config loader reads the `EKIBEN_VAR_*` values and hands them over as the default
//!   organization's variable map, so a stream never reads another module's secret;
//! - `file` inputs and outputs are confined to one directory (`file_root`), so a stream cannot
//!   read another module's key file or `/proc/self/environ`;
//! - private and loopback HTTP targets stay refused (ekiben's default quotas), so a stream cannot
//!   call the other modules' listeners from inside the process.

use crate::app::StartError;
use crate::config::EkibenSection;
use crate::module::Module;
use axum::Router;
use ekiben_core::{EnvAccess, OrgId, ResourceManager};
use ekiben_server::{
    ServerAuthConfig, ServerState, StreamManager, build_full_registry, build_router,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Ekiben, e)
}

/// The sandbox directory for `file` components: the configured one, or a private (0700)
/// directory of this process under the system temp dir.
fn file_root(configured: Option<&Path>) -> Result<PathBuf, StartError> {
    let dir = match configured {
        Some(dir) => dir.to_path_buf(),
        None => std::env::temp_dir().join(format!("jc-ekiben-{}", std::process::id())),
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|e| err(format!("cannot create file root {}: {e}", dir.display())))?;
    Ok(dir)
}

pub fn build(
    cfg: &EkibenSection,
    setup: &crate::setup::Setup,
    vars: &BTreeMap<String, jc_secret::Secret>,
) -> Result<Router, StartError> {
    let token = setup
        .value(crate::setup::Secret::EkibenToken)
        .ok_or_else(|| err("no API token"))?
        .to_owned();
    let root = file_root(cfg.file_root.as_deref())?;
    tracing::info!(file_root = %root.display(), "ekiben file components confined");
    let resources = ResourceManager::new();
    resources
        .get_or_create_org(&OrgId::default())
        .replace_secrets(variables(vars));
    let manager = StreamManager::new(
        build_full_registry(),
        resources,
        Some(root),
        EnvAccess::None,
    );
    let state = ServerState::new(
        manager,
        ServerAuthConfig {
            admin_bearer_token: Some(token),
            org_tokens: Vec::new(),
            require_auth: true,
            guard_probes: true,
            ..Default::default()
        },
    );
    Ok(build_router(state))
}

/// The default organization's variable map: what `jc` read from `EKIBEN_VAR_*`.
fn variables(vars: &BTreeMap<String, jc_secret::Secret>) -> HashMap<String, String> {
    vars.iter()
        .map(|(k, v)| (k.clone(), v.expose().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_root_is_created_private() {
        let dir = tempfile::tempdir().unwrap();
        let root = file_root(Some(&dir.path().join("a/b"))).unwrap();
        assert!(root.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let default = file_root(None).unwrap();
        assert!(default.ends_with(format!("jc-ekiben-{}", std::process::id())));
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(file_root(Some(&blocker.join("sub"))).is_err());
    }
}
