//! ekiben (stream processor) inside `jc`, hardened for a shared process:
//! - the API refuses every request without the configured bearer token (`require_auth`), even
//!   the read-only endpoints; only `/ping` is open;
//! - a posted config and Bloblang `env()` read only `EKIBEN_VAR_*` from the process environment,
//!   never another module's secret (`EnvAccess::AllowListed`);
//! - `file` inputs and outputs are confined to one directory (`file_root`), so a stream cannot
//!   read another module's key file or `/proc/self/environ`;
//! - private and loopback HTTP targets stay refused (ekiben's default quotas), so a stream cannot
//!   call the other modules' listeners from inside the process.
//!
//! With `[ekiben.database]` every continuous stream posted through the API is kept in Postgres
//! (`ekiben_pg`) and started again at the next start; without it streams live in memory.

use crate::app::StartError;
use crate::config::EkibenSection;
use crate::module::Module;
use axum::Router;
use ekiben_core::{EnvAccess, ResourceManager};
use ekiben_server::{
    ServerAuthConfig, ServerState, StreamManager, build_full_registry, build_router,
};
use std::path::{Path, PathBuf};

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Ekiben, e)
}

/// Reads the bearer token: the whole file, trimmed; an empty file is refused. The token is never
/// logged or put into an error.
fn read_token(path: &Path) -> Result<String, StartError> {
    crate::runtime::read_secret_file(path).map_err(err)
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
) -> Result<Router, StartError> {
    let token_file = cfg
        .token_file
        .as_deref()
        .or_else(|| setup.path(crate::setup::Secret::EkibenToken))
        .ok_or_else(|| err("no token_file"))?;
    let token = read_token(token_file)?;
    let root = file_root(cfg.file_root.as_deref())?;
    tracing::info!(file_root = %root.display(), "ekiben file components confined");
    let mut manager = StreamManager::new(
        build_full_registry(),
        ResourceManager::new(),
        Some(root),
        EnvAccess::AllowListed,
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
        let streams = std::sync::Arc::new(
            super::ekiben_pg::PgStreams::open(pool)
                .await
                .map_err(err)?,
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_file_must_exist_and_hold_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(read_token(&missing).is_err());
        let empty = dir.path().join("empty");
        std::fs::write(&empty, " \n").unwrap();
        let e = read_token(&empty).unwrap_err().to_string();
        assert!(e.contains("is empty"), "{e}");
        let ok = dir.path().join("tok");
        std::fs::write(&ok, "s3cret\n").unwrap();
        assert_eq!(read_token(&ok).unwrap(), "s3cret");
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
