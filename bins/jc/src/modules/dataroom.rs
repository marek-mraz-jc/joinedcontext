//! dataroom (dataset catalogue) inside `jc`: the default catalogue on Postgres when
//! `[dataroom.database]` is set (`dataroom_store_pg::PgStore::connect_and_verify`, the boot check
//! refusing a role with SUPERUSER or BYPASSRLS and any `dataroom` table without forced RLS; with
//! a migration role, `jc` first migrates under dataroom's advisory lock and grants the request
//! role its table rights), else in memory; the policy authorizer of the standalone `dataroom
//! serve`, URLs under the mount prefix. When the
//! Portal runs beside it, first-start setup holds a CKAN API token for the Portal: the subject
//! `portal-ckan`, Read and Propose on the default catalogue only
//! (`dataroom_api::portal_publisher`).

use crate::app::StartError;
use crate::config::DataroomSection;
use crate::module::Module;
use crate::setup::{Secret, Setup};
use crate::siblings::Siblings;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::branding::BrandingBlock;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use jc_router::Mount;
use std::sync::Arc;

/// The configured store: Postgres (refused unless the boot check passes) or memory.
async fn open_store(cfg: &DataroomSection) -> Result<Arc<dyn Store>, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let Some(db) = &cfg.database else {
        return Ok(Arc::new(MemoryStore::new()));
    };
    if let Some(opts) = db.migrate_options().map_err(|e| err(&e))? {
        let pool = crate::pg::connect(opts, 1).await.map_err(|e| err(&e))?;
        PgStore::from_pool_unchecked(pool.clone())
            .migrate()
            .await
            .map_err(|e| err(&e))?;
        // dataroom's migration names no request role: the owner grants the configured one.
        let role = db.app_role().map_err(|e| err(&e))?;
        let quoted = format!("\"{}\"", role.replace('"', "\"\""));
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA dataroom TO {quoted};
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO {quoted};"
        )))
        .execute(&pool)
        .await
        .map_err(|e| err(&format!("grant to {role}: {e}")))?;
        pool.close().await;
    }
    let pool = crate::pg::connect(db.app_options().map_err(|e| err(&e))?, db.max_connections())
        .await
        .map_err(|e| err(&e))?;
    Ok(Arc::new(
        PgStore::connect_and_verify(pool)
            .await
            .map_err(|e| err(&e))?,
    ))
}

/// The assistant widget the catalogue docks (G2): the assistant's page of `deployment` when the
/// module runs in this `jc`, else empty, so no launcher points at an assistant that is off.
#[must_use]
pub fn assistant_widget(siblings: &Siblings, deployment: &str) -> String {
    siblings
        .get(Module::Assistant)
        .map(|a| format!("{}/d/{deployment}/widget", a.public))
        .unwrap_or_default()
}

pub async fn build(
    cfg: &DataroomSection,
    setup: &Setup,
    mount: Mount,
    assistant_widget: String,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store = open_store(cfg).await?;
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
        .with_public_base(mount.public_base())
        .with_mount_prefix(mount.path_prefix)
        .with_installation_branding(BrandingBlock {
            assistant_widget: Some(assistant_widget),
            ..BrandingBlock::default()
        });
    Ok(dataroom_api::router(state))
}
