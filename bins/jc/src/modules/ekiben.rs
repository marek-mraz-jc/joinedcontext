//! ekiben (stream processor) inside `jc`, hardened for a shared process:
//! - the API refuses every request without the configured bearer token (`require_auth`), even
//!   the read-only endpoints; only `/ping` is open;
//! - a posted config and Bloblang `env()` read no process environment (`EnvAccess::None`): `jc`'s
//!   config loader reads the `EKIBEN_VAR_*` values and hands them over as the default
//!   organization's variable map, so a stream never reads another module's secret;
//! - `file` inputs and outputs are confined to one directory (`file_root`), so a stream cannot
//!   read another module's key file or `/proc/self/environ`;
//! - with the Portal in the same process, the pipeline credentials its reconciler resolves from
//!   each project's own SOPS files reach that same map ([`HostRunner`]), in place of the
//!   Kubernetes Secret the Portal writes for a runner in a cluster;
//! - private and loopback HTTP targets stay refused (ekiben's default quotas), so a stream cannot
//!   call the other modules' listeners from inside the process.
//!
//! With `[ekiben.database]` every continuous stream posted through the API is kept in Postgres
//! (`ekiben_pg`) and started again at the next start; without it streams live in memory.

use crate::app::StartError;
use crate::config::EkibenSection;
use crate::module::Module;
use axum::Router;
use ekiben_core::{EnvAccess, OrgContext, OrgId, ResourceManager};
use ekiben_server::{
    ServerAuthConfig, ServerState, StreamManager, build_full_registry, build_router,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

pub async fn build(
    cfg: &EkibenSection,
    setup: &crate::setup::Setup,
    vars: &BTreeMap<String, jc_secret::Secret>,
    portal_beside: bool,
) -> Result<Router, StartError> {
    let token = setup
        .value(crate::setup::Secret::EkibenToken)
        .ok_or_else(|| err("no API token"))?
        .to_owned();
    let root = file_root(cfg.file_root.as_deref())?;
    tracing::info!(file_root = %root.display(), "ekiben file components confined");
    let resources = ResourceManager::new();
    let runner = HostRunner {
        org: resources.get_or_create_org(&OrgId::default()),
        given: variables(vars),
    };
    runner.org.replace_secrets(runner.given.clone());
    if portal_beside {
        hand_to_portal(runner);
    }
    let mut manager = StreamManager::new(
        build_full_registry(),
        resources,
        Some(root),
        EnvAccess::None,
    );
    if let Some(db) = &cfg.database {
        if let Some(opts) = db.migrate_options().map_err(err)? {
            let pool = crate::pg::connect(opts, 1).await.map_err(err)?;
            super::ekiben_pg::migrate(&pool, &db.app_role().map_err(err)?)
                .await
                .map_err(err)?;
            pool.close().await;
        }
        let pool = crate::pg::connect(db.app_options().map_err(err)?, db.max_connections())
            .await
            .map_err(err)?;
        let streams =
            std::sync::Arc::new(super::ekiben_pg::PgStreams::open(pool).await.map_err(err)?);
        manager = manager.with_persistence(streams.clone());
        let started = streams.restore(&manager).await.map_err(err)?;
        tracing::info!(started, "ekiben: saved streams started");
    }
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

/// The default organization's variable map, as the Portal's reconciler sees the runner: what
/// `jc` was given (`EKIBEN_VAR_*`) plus the pipeline credentials of the last sync. A given name
/// wins over a pipeline's, so a project cannot replace a variable the operator set.
pub struct HostRunner {
    org: Arc<OrgContext>,
    given: HashMap<String, String>,
}

impl HostRunner {
    /// The variables after a sync handed `resolved`.
    fn merged(&self, resolved: BTreeMap<String, String>) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = resolved.into_iter().collect();
        for (name, value) in &self.given {
            if out.insert(name.clone(), value.clone()).is_some() {
                tracing::warn!(variable = %name, "a pipeline credential names a variable jc was given; jc's value is kept");
            }
        }
        out
    }
}

#[cfg(feature = "portal")]
impl joinedcontext_portal::pipeline_secrets::RunnerSecrets for HostRunner {
    fn replace(&self, values: BTreeMap<String, String>) {
        self.org.replace_secrets(self.merged(values));
    }
}

#[cfg(feature = "portal")]
fn hand_to_portal(runner: HostRunner) {
    joinedcontext_portal::pipeline_secrets::hand_runner_secrets_to(Arc::new(runner));
}

#[cfg(not(feature = "portal"))]
fn hand_to_portal(_: HostRunner) {}

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
    fn a_given_variable_wins_over_a_pipeline_credential_of_the_same_name() {
        let resources = ResourceManager::new();
        let runner = HostRunner {
            org: resources.get_or_create_org(&OrgId::default()),
            given: HashMap::from([("SHARED".to_owned(), "operator".to_owned())]),
        };
        let merged = runner.merged(BTreeMap::from([
            ("SHARED".to_owned(), "pipeline".to_owned()),
            ("GOLEMIO_KEY".to_owned(), "k".to_owned()),
        ]));
        assert_eq!(merged["SHARED"], "operator");
        assert_eq!(merged["GOLEMIO_KEY"], "k");
        // A later sync without the credential drops it again.
        assert_eq!(runner.merged(BTreeMap::new()).len(), 1);
    }

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
