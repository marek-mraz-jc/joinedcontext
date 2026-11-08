//! dataroom (dataset catalogue) inside `jc`: in-memory store with the default catalogue, the
//! policy authorizer of the standalone `dataroom serve`, URLs under the mount prefix. When the
//! Portal runs beside it, first-start setup holds a CKAN API token for the Portal: the subject
//! `portal`, administrator of the default catalogue only.

use crate::app::StartError;
use crate::config::DataroomSection;
use crate::module::Module;
use crate::setup::{Secret, Setup};
use axum::Router;
use dataroom_api::{DataroomState, TokenBinding};
use dataroom_core::{Action, CatalogueId, GrantRule, OrgId, PolicyAuthorizer, Store, Subject};
use dataroom_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

/// The subject the Portal's CKAN token stands for.
pub(crate) const PORTAL_SUBJECT: &str = "portal";

pub async fn build(
    cfg: &DataroomSection,
    setup: &Setup,
    mount: Mount,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let (authorizer, tokens) = match setup.value(Secret::CatalogueToken) {
        None => (PolicyAuthorizer::default(), Vec::new()),
        Some(token) => (
            PolicyAuthorizer::new(vec![GrantRule {
                org_id,
                catalogue_id: Some(catalogue_id),
                principal: PORTAL_SUBJECT.into(),
                actions: vec![Action::Admin],
                publisher_id: None,
            }]),
            vec![TokenBinding {
                token: token.to_owned(),
                subject: Subject::authenticated(PORTAL_SUBJECT, vec![org_id], Vec::new()),
            }],
        ),
    };
    let state = DataroomState::new(store, Arc::new(authorizer), org_id, catalogue_id)
        .with_tokens(tokens)
        .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}
