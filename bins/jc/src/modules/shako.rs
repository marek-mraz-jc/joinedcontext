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

const INSTANCE: shako_core::InstanceId = shako_core::InstanceId::from_u128(1);
const ORG: OrgId = OrgId::from_u128(1);

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
    // Forced RLS shows rows of one tenant per transaction: load jc's one tenant into the store.
    pg.hydrate_tenant(INSTANCE, ORG).await.map_err(err)?;
    Ok(Arc::new(pg))
}

pub async fn build_state(
    cfg: &ShakoSection,
    setup: &crate::setup::Setup,
) -> Result<ShakoState, StartError> {
    let root = repo_root(cfg.repo_root.as_deref())?;
    let org_id = ORG;
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
        INSTANCE,
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

#[cfg(test)]
mod tests {
    use super::*;
    use shako_core::{InstanceRecord, RepoId, RepoRecord};

    /// An instance and a repository written before a restart read back after it; a role that
    /// bypasses RLS is refused at start.
    #[tokio::test]
    async fn shako_on_postgres_keeps_state_over_a_restart_and_refuses_a_bypassing_role() {
        let Some(db) = crate::pg::testdb::create("shako").await else {
            return;
        };
        let cfg = ShakoSection {
            database: Some(db.database.clone()),
            ..ShakoSection::default()
        };
        let first = store(&cfg).await.unwrap();
        first
            .upsert_instance(InstanceRecord::default_for(INSTANCE, ORG, "git.jc.example"))
            .await
            .unwrap();
        let now = chrono::Utc::now();
        first
            .create_repo(RepoRecord {
                id: RepoId::new(),
                instance_id: INSTANCE,
                org_id: ORG,
                project_id: "org".into(),
                owner: "jc".into(),
                name: "config".into(),
                description: String::new(),
                default_branch: "main".into(),
                private: true,
                archived: false,
                size_bytes: 0,
                next_pr_number: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        drop(first);
        let again = store(&cfg).await.unwrap();
        let read = again
            .get_repo_by_name(INSTANCE, ORG, "jc", "config")
            .await
            .unwrap();
        assert_eq!(
            read.map(|r| r.name),
            Some("config".to_owned()),
            "state survives a restart"
        );
        let bypassing = ShakoSection {
            database: Some(db.admin.clone()),
            ..ShakoSection::default()
        };
        let refused = store(&bypassing)
            .await
            .err()
            .expect("a superuser app role is refused");
        assert!(
            refused.to_string().to_lowercase().contains("rls"),
            "{refused}"
        );
        drop(db.dir);
    }
}
