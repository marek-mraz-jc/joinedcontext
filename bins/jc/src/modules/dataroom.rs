//! dataroom (dataset catalogue) inside `jc`: in-memory store with the default catalogue, the
//! policy authorizer of the standalone `dataroom serve`, URLs under the mount prefix. When the
//! Portal runs beside it, first-start setup holds a CKAN API token for the Portal: the subject
//! `portal-ckan`, Read and Propose on the default catalogue only
//! (`dataroom_api::portal_publisher`).

use crate::app::StartError;
use crate::config::DataroomSection;
use crate::module::Module;
use crate::setup::{Secret, Setup};
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

/// dataroom's store: PostgreSQL when `[dataroom.database]` is set (migrations first when it
/// names a migration role; `connect_and_verify` refuses an app role bypassing RLS), else memory.
async fn store(cfg: &DataroomSection) -> Result<Arc<dyn Store>, String> {
    let Some(db) = &cfg.database else {
        return Ok(Arc::new(MemoryStore::new()));
    };
    if let Some(pool) = db.migrate_pool().await? {
        let migrator = dataroom_store_pg::PgStore::from_pool_unchecked(pool.clone());
        migrator.migrate().await.map_err(|e| e.to_string())?;
        pool.close().await;
    }
    let pg = dataroom_store_pg::PgStore::connect_and_verify(db.app_pool().await?)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(pg))
}

pub async fn build(
    cfg: &DataroomSection,
    setup: &Setup,
    mount: Mount,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store = store(cfg).await.map_err(|e| err(&e))?;
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let (authorizer, tokens) = match setup.value(Secret::CatalogueToken) {
        None => (PolicyAuthorizer::default(), Vec::new()),
        Some(token) => {
            let (binding, grant) =
                dataroom_api::portal_publisher(token, org_id, catalogue_id).map_err(|e| err(&e))?;
            (PolicyAuthorizer::new(vec![grant]), vec![binding])
        }
    };
    let state = DataroomState::new(store, Arc::new(authorizer), org_id, catalogue_id)
        .with_tokens(tokens)
        .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default catalogue written before a restart reads back after it; a role that bypasses
    /// RLS is refused at start.
    #[tokio::test]
    async fn dataroom_on_postgres_keeps_state_over_a_restart_and_refuses_a_bypassing_role() {
        let Some(db) = crate::pg::testdb::create("dataroom").await else {
            return;
        };
        let cfg = DataroomSection {
            database: Some(db.database.clone()),
            ..DataroomSection::default()
        };
        let org = OrgId::parse_verified(&cfg.org_id).unwrap();
        let catalogue = CatalogueId::parse_verified(&cfg.catalogue_id).unwrap();
        let first = store(&cfg).await.unwrap();
        dataroom_server::ensure_default_catalogue(first.as_ref(), org, catalogue, "data.example")
            .await
            .unwrap();
        drop(first);
        let again = store(&cfg).await.unwrap();
        let read = again.get_catalogue(org, catalogue).await.unwrap();
        assert_eq!(
            read.map(|c| c.host),
            Some("data.example".to_owned()),
            "state survives a restart"
        );
        let bypassing = DataroomSection {
            database: Some(db.admin.clone()),
            ..DataroomSection::default()
        };
        let refused = store(&bypassing)
            .await
            .err()
            .expect("a superuser app role is refused");
        assert!(refused.to_lowercase().contains("rls"), "{refused}");
        drop(db.dir);
    }
}
