//! shako (Git forge) inside `jc`: mounts both the forge router (`/{slug}` or `git.{base_host}`)
//! and the host-root `/v2` container registry router (`shako_server::registry_router`).

use crate::app::StartError;
use crate::config::ShakoSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use shako_core::{
    Action, LocalAuthorizer, OrgId, RoleBinding, ShakoSigningKey, StaticTokenBinding, Subject,
    TokenVerifier,
};
use shako_server::{ShakoState, registry_router, router};
use shako_store_memory::MemoryStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Shako, e)
}

fn repo_root(configured: Option<&Path>) -> Result<PathBuf, StartError> {
    let dir = match configured {
        Some(dir) => dir.to_path_buf(),
        None => std::env::temp_dir().join(format!("jc-shako-{}", std::process::id())),
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
        .map_err(|e| err(format!("cannot create repo root {}: {e}", dir.display())))?;
    Ok(dir)
}

/// shako's store: PostgreSQL when `[shako.database]` is set (migrations first when it names a
/// migration role; `connect_with` refuses an app role that bypasses RLS), else memory.
async fn store(cfg: &ShakoSection) -> Result<Arc<dyn shako_core::Store>, StartError> {
    let Some(db) = &cfg.database else {
        return Ok(Arc::new(MemoryStore::new()));
    };
    if let Some(pool) = db.migrate_pool().await.map_err(err)? {
        shako_store_pg::PgStore::run_migrations(&pool)
            .await
            .map_err(err)?;
        pool.close().await;
    }
    let opts = crate::pg::options(&db.url_file, db.password_file.as_deref()).map_err(err)?;
    let pg = shako_store_pg::PgStore::connect_with(opts, db.max_connections.unwrap_or(5))
        .await
        .map_err(err)?;
    Ok(Arc::new(pg))
}

pub async fn build_state(
    cfg: &ShakoSection,
    setup: &crate::setup::Setup,
) -> Result<ShakoState, StartError> {
    let root = repo_root(cfg.repo_root.as_deref())?;
    let org_id = OrgId::from_u128(1);
    let store = store(cfg).await?;
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
        org_id,
        principal: "admin".into(),
        actions: vec![
            Action::Admin,
            Action::Write,
            Action::Approve,
            Action::Publish,
        ],
        project_id: None,
        repo: None,
    }]));

    let key = match &cfg.signing_key_file {
        Some(path) => ShakoSigningKey::from_file(path).map_err(err)?,
        None => ShakoSigningKey::generate().map_err(err)?,
    };

    let mut static_tokens = Vec::new();
    let token_file = cfg
        .token_file
        .as_deref()
        .or_else(|| setup.path(crate::setup::Secret::ShakoToken));
    if let Some(tf) = token_file {
        let token = crate::runtime::read_secret_file(tf).map_err(err)?;
        static_tokens.push(StaticTokenBinding::new(
            &token,
            Subject::authenticated("admin", "admin", vec![org_id], vec![]),
        ));
    }

    let verifier = TokenVerifier::new(key).with_static_tokens(static_tokens);
    Ok(ShakoState::new(
        shako_core::InstanceId::from_u128(1),
        org_id,
        root,
        store,
        authz,
        verifier,
        shako_core::OutboundGuard::default(),
    ))
}

pub fn build(state: ShakoState, mount: Mount) -> (Router, Router) {
    let main_router = router(state.clone(), mount.clone());
    let v2_router = registry_router(state, mount);
    (main_router, v2_router)
}
