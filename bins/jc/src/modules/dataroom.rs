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
use crate::config::{DataroomSection, Given, OrganizationSection};
use crate::module::Module;
use crate::setup::{Secret, Setup};
use crate::siblings::Siblings;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::branding::{BrandingAddresses, BrandingBlock};
use dataroom_core::{
    CatalogueId, OrgId, PolicyAuthorizer, Publisher, RecordState, RequestCtx, Store, Subject,
};
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use jc_router::Mount;
use std::collections::BTreeMap;
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
/// module runs in this `jc`; `None` otherwise, so only an assistant address named in
/// `[dataroom.addresses]` (or none) points a launcher anywhere.
#[must_use]
pub fn assistant_widget(siblings: &Siblings, deployment: &str) -> Option<String> {
    siblings
        .get(Module::Assistant)
        .map(|a| format!("{}/d/{deployment}/widget", a.public))
}

/// The installation branding `jc` hands dataroom: only the live parts' addresses (G1), named in
/// `[dataroom.addresses]` or taken from the mounted modules, and the assistant widget of jc's own
/// assistant module. A catalogue's own branding still wins per catalogue; one that nobody serves
/// stays empty, so no page points at it.
fn installation_branding(
    live: &crate::siblings::LiveAddresses,
    assistant_widget: Option<String>,
) -> BrandingBlock {
    BrandingBlock {
        addresses: BrandingAddresses {
            viz: live.viz.clone(),
            api: live.api.clone(),
            tiles: live.tiles.clone(),
            assistant: live.assistant.clone(),
            portal: live.portal.clone(),
        },
        assistant_widget,
        ..BrandingBlock::default()
    }
}

pub async fn build(
    cfg: &DataroomSection,
    organizations: &[OrganizationSection],
    tokens: &BTreeMap<String, Given>,
    setup: &Setup,
    siblings: &Siblings,
    mount: Mount,
    assistant_widget: Option<String>,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store = open_store(cfg).await?;
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let (mut grants, mut bindings) = match setup.value(Secret::CatalogueToken) {
        None => (Vec::new(), Vec::new()),
        Some(token) => {
            let (binding, grant) =
                dataroom_api::portal_publisher(token, org_id, catalogue_id).map_err(|e| err(&e))?;
            (vec![grant], vec![binding])
        }
    };
    let live = crate::siblings::live_addresses(&cfg.addresses, siblings);
    let mut by_catalogue = BTreeMap::new();
    for org in organizations {
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?;
        if id == catalogue_id {
            return Err(err(&format!(
                "[[organizations]] {}: catalogue {id} is the [dataroom] default catalogue",
                org.name
            )));
        }
        let host = bare_host(&org.hosts.opendata);
        dataroom_server::ensure_catalogue(store.as_ref(), org_id, id, &org.name, host)
            .await
            .map_err(|e| err(&e))?;
        ensure_publisher(store.as_ref(), org_id, id, org, &mount.scheme)
            .await
            .map_err(|e| err(&e))?;
        if let Some(token) = tokens.get(&org.name) {
            let token = token.secret.expose();
            if bindings
                .iter()
                .any(|b: &dataroom_api::TokenBinding| b.token == token.trim())
            {
                return Err(err(&format!(
                    "[[organizations]] {}: its Portal token is another catalogue's",
                    org.name
                )));
            }
            let (binding, grant) = dataroom_api::portal_publisher_on(token, org_id, id, &org.name)
                .map_err(|e| err(&e))?;
            bindings.push(binding);
            grants.push(grant);
        }
        by_catalogue.insert(
            id,
            organization_branding(&live, org, &mount.scheme, assistant_widget.clone()),
        );
    }
    let state = DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::new(grants)),
        org_id,
        catalogue_id,
    )
    .with_tokens(bindings)
    .with_installation_branding(installation_branding(&live, assistant_widget))
    .with_catalogue_branding(by_catalogue)
    // Several catalogues by host: a loopback name names none of them.
    .with_loopback_default(organizations.is_empty())
    .with_public_base(mount.public_base())
    .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}

fn bare_host(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

/// An Organization's catalogue frames its own Portal and API: the installation's other live
/// parts stay as they are.
fn organization_branding(
    live: &crate::siblings::LiveAddresses,
    org: &OrganizationSection,
    scheme: &str,
    assistant_widget: Option<String>,
) -> BrandingBlock {
    let mut block = installation_branding(live, assistant_widget);
    block.addresses.api = format!("{scheme}://{}", org.hosts.api);
    block.addresses.portal = format!("{scheme}://{}", org.hosts.portal);
    block
}

/// The CKAN organisation of the JC Organization in its catalogue (ADR-N-058 2.6), held by its
/// name: the one publisher the Portal's grant there may publish under.
async fn ensure_publisher(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    org: &OrganizationSection,
    scheme: &str,
) -> dataroom_core::Result<()> {
    let ctx = RequestCtx::new(org_id, catalogue_id, Subject::anonymous());
    if store.get_publisher(&ctx, &org.name).await?.is_some() {
        return Ok(());
    }
    let now = chrono::Utc::now();
    store
        .upsert_publisher(
            &ctx,
            Publisher {
                org_id,
                catalogue_id,
                id: String::new(),
                name: org.name.clone(),
                title: BTreeMap::from([("en".to_owned(), org.name.clone())]),
                description: BTreeMap::new(),
                uri: format!("{scheme}://{}/publisher/{}", org.hosts.opendata, org.name),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: Vec::new(),
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
                created: now,
                modified: now,
            },
        )
        .await?;
    Ok(())
}
