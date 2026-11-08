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

pub fn build_state(
    cfg: &ShakoSection,
    setup: &crate::setup::Setup,
    secrets: &crate::config::Secrets,
) -> Result<ShakoState, StartError> {
    let root = repo_root(cfg.repo_root.as_deref())?;
    let org_id = OrgId::from_u128(1);
    let store = Arc::new(MemoryStore::new());
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

    let key = match &secrets.shako_signing_key {
        Some(given) => ShakoSigningKey::from_text(given.secret.expose().as_bytes()).map_err(err)?,
        None => ShakoSigningKey::generate().map_err(err)?,
    };

    let mut static_tokens = Vec::new();
    if let Some(token) = setup.value(crate::setup::Secret::ShakoToken) {
        static_tokens.push(StaticTokenBinding::new(
            token,
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
