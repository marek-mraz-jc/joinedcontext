//! dataroom (dataset catalogue) inside `jc`: the default catalogue on Postgres when
//! `[dataroom.database]` is set (`dataroom_store_pg::PgStore::connect_and_verify`, the boot check
//! refusing a role with SUPERUSER or BYPASSRLS and any `dataroom` table without forced RLS; with
//! a migration role, `jc` first migrates under dataroom's advisory lock and grants the request
//! role its table rights), else in memory; the policy authorizer of the standalone `dataroom
//! serve`, URLs under the mount prefix. The Portal publishes with a short-lived service token
//! (S11-23): kaisatsu's, verified against `[door]`, in a role split; jc's in-process issuer's
//! beside the Portal. Its subject is `portal-ckan`, Read and Propose on the default catalogue
//! (`dataroom_api::portal_publisher`) and `portal-ckan:{id}` on each Organization's own.

use crate::app::StartError;
use crate::config::{DataroomSection, OrganizationSection};
use crate::module::Module;
use crate::siblings::Siblings;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::branding::{BrandingAddresses, BrandingBlock, PublisherBranding};
use dataroom_core::{
    CatalogueId, DataroomError, LiveAuthorizer, OrgId, Publisher, RecordState, RequestCtx, Store,
    Subject,
};
use dataroom_harvest::{HarvestEngine, HarvestScheduler, HttpFetcher};
use dataroom_store_memory::MemoryStore;
use dataroom_store_pg::PgStore;
use jc_router::Mount;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The configured store: Postgres (refused unless the boot check passes) or memory.
async fn open_store(
    cfg: &DataroomSection,
    moves: &[(OrgId, OrgId, CatalogueId)],
) -> Result<Arc<dyn Store>, StartError> {
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
        // ODP-12: an Organization's catalogue made under the installation's OrgId moves to the
        // Organization's own, as the owner, once (a later start finds nothing to move).
        let owner = PgStore::from_pool_unchecked(pool.clone());
        for &(from, to, catalogue) in moves {
            let moved = owner
                .move_catalogue(from, to, catalogue)
                .await
                .map_err(|e| err(&e))?;
            if moved > 0 {
                tracing::info!(%catalogue, %to, moved, "dataroom catalogue moved to its Organization's OrgId");
            }
        }
        pool.close().await;
    }
    let pool = crate::pg::connect(db.app_options().map_err(|e| err(&e))?, db.max_connections())
        .await
        .map_err(|e| err(&e))?;
    let store = PgStore::connect_and_verify(pool)
        .await
        .map_err(|e| err(&e))?;
    // Without the migration owner nothing moved: a catalogue still under the installation's
    // OrgId would be served to no Organization, so the start stops and names the fix.
    for &(from, _, catalogue) in moves {
        if store
            .get_catalogue(from, catalogue)
            .await
            .map_err(|e| err(&e))?
            .is_some()
        {
            return Err(err(&format!(
                "catalogue {catalogue} still sits under [dataroom] org_id: start once with the \
                 migration owner ([dataroom.database] migrate) so it moves to its Organization (ODP-12)"
            )));
        }
    }
    Ok(Arc::new(store))
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

/// The installation branding `jc` hands dataroom: the host's own logo and publisher badges
/// (`own`, from [`configured_branding`]), the live parts' addresses (G1), named in
/// `[dataroom.addresses]` or taken from the mounted modules, and the assistant widget of jc's own
/// assistant module. A catalogue's own branding still wins per catalogue; one that nobody serves
/// stays empty, so no page points at it.
fn installation_branding(
    own: &BrandingBlock,
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
        ..own.clone()
    }
}

/// `[dataroom.branding]` and `[dataroom.publishers]` as a branding block, the way prod's CKAN
/// chart read its `branding` and `organizations` values. Refused at start: a publisher whose
/// initials are empty or whose colour is no hex value (the badge would not draw), a logo name
/// no route serves, a logo SVG the logo upload would refuse.
fn configured_branding(cfg: &DataroomSection) -> Result<BrandingBlock, String> {
    let mut block = BrandingBlock::default();
    for (name, seed) in &cfg.publishers {
        let initials = seed.initials.trim();
        if initials.is_empty() || initials.chars().count() > 4 {
            return Err(format!(
                "[dataroom.publishers.{name}] initials: one to four letters, e.g. \"PHA\""
            ));
        }
        if !dataroom_core::branding::is_valid_hex_colour(&seed.colour) {
            return Err(format!(
                "[dataroom.publishers.{name}] colour: #rrggbb, e.g. \"#b4232a\""
            ));
        }
        block.publishers.insert(
            name.clone(),
            PublisherBranding {
                initials: Some(initials.to_owned()),
                colour: Some(seed.colour.clone()),
                description: seed.description.clone(),
                ..PublisherBranding::default()
            },
        );
    }
    let own = &cfg.branding;
    if let Some(svg) = &own.logo_svg {
        if !dataroom_core::branding::is_safe_logo_svg(svg) {
            return Err("[dataroom.branding] logo_svg: an SVG without script, event handlers or external references".into());
        }
        block.logo_svg = Some(svg.clone());
    }
    for (key, value, slot) in [
        ("logo", &own.logo, &mut block.logo),
        ("favicon", &own.favicon, &mut block.favicon),
    ] {
        match value.as_deref() {
            None => {}
            Some("jc-logo.svg") => *slot = "jc-logo.svg".into(),
            Some("logo.svg") if own.logo_svg.is_some() => *slot = "logo.svg".into(),
            Some(other) => {
                return Err(format!(
                    "[dataroom.branding] {key} = {other:?}: \"logo.svg\" (with logo_svg) or \"jc-logo.svg\""
                ));
            }
        }
    }
    Ok(block)
}

pub async fn build(
    cfg: &DataroomSection,
    organizations: &[OrganizationSection],
    ctx: super::BuildCtx<'_>,
    mount: Mount,
    assistant_widget: Option<String>,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> Result<Router, StartError> {
    let (router, _writer) = build_with_writer(
        cfg,
        organizations,
        ctx,
        mount,
        assistant_widget,
        shutdown,
        tasks,
    )
    .await?;
    Ok(router)
}

/// [`build`], and the in-process write path of its catalogues (ODP-11): the reconciler creates,
/// updates and deletes a Portal's catalogue through it while the router serves.
pub async fn build_with_writer(
    cfg: &DataroomSection,
    organizations: &[OrganizationSection],
    ctx: super::BuildCtx<'_>,
    mount: Mount,
    assistant_widget: Option<String>,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> Result<(Router, dataroom_server::CatalogueWriter), StartError> {
    let (setup, siblings) = (ctx.setup, ctx.siblings);
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    dataroom_api::check_aliases(&cfg.aliases)
        .map_err(|e| err(&format!("[dataroom.aliases] {e}")))?;
    dataroom_api::check_profiles().map_err(|e| err(&e))?;
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    // ODP-12: each Organization's catalogue lives under its own OrgId, so row level security and
    // the cross-organization harvest check keep Organizations apart; the deployment-wide
    // catalogue stays under `[dataroom] org_id`, the installation's own.
    // Each Organization's tenant and catalogue, by the name jc's host registry decides.
    let mut by_organization = BTreeMap::new();
    let mut moves = Vec::new();
    for org in organizations {
        let own = OrgId::of_organization(org_id, &org.name);
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?;
        // A shared id would move the deployment-wide catalogue (or another Organization's)
        // with this one; the store holds one catalogue per id.
        if id == catalogue_id || moves.iter().any(|&(_, _, c)| c == id) {
            return Err(err(&format!(
                "[[organizations]] {}: catalogue_id {id} is already [dataroom] catalogue_id or \
                 another Organization's; give each Organization a catalogue id of its own",
                org.name
            )));
        }
        by_organization.insert(org.name.clone(), (own, id));
        moves.push((org_id, own, id));
    }
    let store = open_store(cfg, &moves).await?;
    hold_default_host(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    if let Some(hidden) = &cfg.hidden {
        seed_hidden(store.as_ref(), org_id, catalogue_id, hidden)
            .await
            .map_err(|e| err(&format!("[dataroom.hidden] {e}")))?;
    }
    let verifier = publisher_verifier(ctx.door, siblings).map_err(|e| err(&e))?;
    let mut grants = Vec::new();
    let mut publishers = dataroom_api::Publishers::new();
    if verifier.is_some() {
        let (subject, grant) = dataroom_api::portal_publisher(org_id, catalogue_id);
        publishers.insert((org_id, catalogue_id), subject);
        grants.push(grant);
    }
    let own = configured_branding(cfg).map_err(|e| err(&e))?;
    let live = crate::siblings::live_addresses(&cfg.addresses, siblings);
    let mut by_catalogue = BTreeMap::new();
    for org in organizations {
        // Distinct from the default catalogue: checked in the loop above.
        let (tenant, id) = by_organization[&org.name];
        let host = bare_host(org.hosts.opendata_host());
        dataroom_server::ensure_catalogue(store.as_ref(), tenant, id, &org.name, host)
            .await
            .map_err(|e| err(&e))?;
        ensure_publisher(store.as_ref(), tenant, id, org, &mount.scheme)
            .await
            .map_err(|e| err(&e))?;
        if verifier.is_some() {
            let (subject, grant) = dataroom_api::portal_publisher_on(tenant, id, &org.name);
            publishers.insert((tenant, id), subject);
            grants.push(grant);
        }
        by_catalogue.insert(
            id,
            organization_branding(&own, &live, org, &mount.scheme, assistant_widget.clone()),
        );
    }
    let mut catalogues = vec![(org_id, catalogue_id)];
    catalogues.extend(organizations.iter().map(|org| by_organization[&org.name]));
    for &(o, c) in &catalogues {
        grants.push(dataroom_harvest::harvest_principal(o, c).1);
    }
    let own_gateway = own_gateway(&live, siblings);
    let authorizer = Arc::new(LiveAuthorizer::new(grants));
    let mut engine = HarvestEngine::new(Arc::new(HttpFetcher::new()));
    if let Some((public, Some(internal))) = &own_gateway {
        engine = engine.with_own_gateway(public, *internal);
    }
    // One engine for the schedule and for the runs and tests the Portal's sources routes start.
    let engine = Arc::new(engine);
    let scheduler = HarvestScheduler::new(
        engine.clone(),
        store.clone(),
        authorizer.clone(),
        catalogues,
    );
    let harvested = scheduler.catalogues();
    scheduler.spawn();
    #[cfg(feature = "portal")]
    let configured = by_catalogue.clone();
    // Each catalogue's canonical address: the Organization's open data host, whichever
    // registered host reads it.
    let bases: BTreeMap<_, _> = organizations
        .iter()
        .map(|org| {
            let (_, id) = by_organization[&org.name];
            (id, format!("{}://{}", mount.scheme, org.opendata_base()))
        })
        .collect();
    #[cfg(feature = "portal")]
    let by_configured = by_organization.clone();
    #[cfg(feature = "portal")]
    let followed = portal_orgs(
        cfg,
        (org_id, catalogue_id),
        organizations,
        &by_organization,
        &mount,
        own_gateway.as_ref().map(|(public, _)| public.clone()),
    );
    let mut state = DataroomState::new(store, authorizer.clone(), org_id, catalogue_id);
    if let Some(verifier) = verifier.clone() {
        state = state.with_verifier(verifier);
    }
    let state = state
        .with_harvester(engine)
        .with_organization_catalogues(by_organization)
        .with_catalogue_bases(bases)
        .with_publishers(publishers)
        .with_installation_branding(installation_branding(&own, &live, assistant_widget.clone()))
        .with_catalogue_branding(by_catalogue)
        // Several catalogues by host: a loopback name names none of them.
        .with_loopback_default(organizations.is_empty())
        .with_sources_by_portal(cfg!(feature = "portal"))
        .with_public_base(mount.public_base())
        // The portals' files, each Organization under its own OrgId (T-7417).
        .with_files(Arc::new(dataroom_core::files::FileStore::new(
            setup.dir.join("opendata-files"),
        )))
        .with_mount_prefix(mount.path_prefix);
    #[cfg(feature = "portal")]
    let state = with_dashboards(&by_configured, state);
    #[cfg(feature = "portal")]
    follow_appearances(
        &by_configured,
        followed.clone(),
        configured,
        &state,
        shutdown,
        tasks,
    );
    #[cfg(not(feature = "portal"))]
    let _ = (shutdown, tasks);
    let writer = dataroom_server::CatalogueWriter::new(state.clone(), authorizer, harvested);
    #[cfg(feature = "portal")]
    super::portals::follow(writer.clone(), followed, shutdown, tasks);
    #[cfg(feature = "portal")]
    follow_organizations(
        RunTime {
            writer: writer.clone(),
            branding: state.catalogue_branding.clone(),
            installation: org_id,
            default_catalogue: catalogue_id,
            configured: by_configured,
            own,
            live,
            scheme: mount.scheme.clone(),
            assistant_widget,
            publishes: verifier.is_some(),
            applied: BTreeMap::new(),
        },
        shutdown,
        tasks,
    );
    Ok((
        dataroom_api::with_dataset_aliases(dataroom_api::router(state), cfg.aliases.clone()),
        writer,
    ))
}

/// Who verifies the service tokens the Portal publishes with (S11-23): `[door]`'s issuer in a
/// role split, jc's in-process issuer beside the Portal (all in one). Neither: no catalogue here
/// admits a publisher (logged), and a static bearer is refused everywhere.
fn publisher_verifier(
    door: Option<&Arc<crate::door::Door>>,
    siblings: &crate::siblings::Siblings,
) -> Result<Option<Arc<jc_door::Verifier>>, String> {
    if let Some(door) = door {
        return Ok(Some(door.verifier()));
    }
    if siblings.get(Module::Portal).is_some() {
        return crate::door::local_verifier().map(|v| Some(Arc::new(v)));
    }
    tracing::warn!("dataroom: no issuer of service tokens ([door] or a Portal): nothing publishes");
    Ok(None)
}

/// What the Portal reconciler serves: each `[[organizations]]` entry, or without any the
/// installation's Organization, whose `default` portal is the `[dataroom]` catalogue at the
/// dataroom's own address (ODP-09).
#[cfg(feature = "portal")]
fn portal_orgs(
    cfg: &DataroomSection,
    (org_id, default_catalogue): (OrgId, CatalogueId),
    organizations: &[OrganizationSection],
    by_organization: &BTreeMap<String, (OrgId, CatalogueId)>,
    mount: &Mount,
    own_gateway: Option<String>,
) -> Vec<super::portals::OrganizationPortals> {
    let entry = |name: &str, org_id, catalogue_id, opendata: String, listed| {
        super::portals::OrganizationPortals {
            name: name.to_owned(),
            org_id,
            catalogue_id,
            scheme: mount.scheme.clone(),
            opendata,
            listed,
            harvest_hosts: cfg.harvest_hosts(),
            config_dir: cfg.config_dir.clone(),
            own_gateway: own_gateway.clone(),
        }
    };
    if organizations.is_empty() {
        let base = mount.public_base();
        let opendata = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest);
        return vec![super::portals::OrganizationPortals::installation(entry(
            "",
            org_id,
            default_catalogue,
            opendata.trim_end_matches('/').to_owned(),
            Vec::new(),
        ))];
    }
    organizations
        .iter()
        .map(|org| {
            let (tenant, catalogue_id) = by_organization[&org.name];
            entry(
                &org.name,
                tenant,
                catalogue_id,
                org.opendata_base(),
                org.hosts.portals.clone(),
            )
        })
        .collect()
}

/// The origin of this instance's gateway as the catalogue names it (`[dataroom.addresses]
/// api` or the gateway's mount), and the loopback listener that serves it when jc mounts the
/// gateway on its own listener (no host of its own): a harvest reads the Endpoints there, as
/// prod's harvest read them in-cluster. `None` without a gateway.
fn own_gateway(
    live: &crate::siblings::LiveAddresses,
    siblings: &Siblings,
) -> Option<(String, Option<std::net::SocketAddr>)> {
    if live.api.is_empty() {
        return None;
    }
    let loopback_of = |url: &str| {
        crate::siblings::origin(url)
            .strip_prefix("http://")
            .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
            .filter(|a| a.ip().is_loopback())
    };
    let Some(gateway) = siblings.get(Module::Gateway) else {
        // The gateway runs in another container (role split): the api address the operator
        // named is its, read directly only when it is a loopback literal (a kit's front).
        return Some((live.api.clone(), loopback_of(&live.api)));
    };
    let public = crate::siblings::origin(&gateway.public);
    let loopback = loopback_of(&gateway.internal);
    // A named api address that is not the gateway's mount is another host: no shortcut. A
    // public origin that is the loopback listener itself (a kit on http://127.0.0.1:<port>) is
    // still jc serving it there.
    let reach = loopback.filter(|_| live.api == public);
    Some((live.api.clone(), reach))
}

/// Holds the default catalogue on the mount's host. A real (not loopback) host moves the stored
/// row to it, port dropped as the host lookup drops it: an import window or a move from another
/// instance leaves the row on `localhost:8080`, and the catalogue then answered 404 'unknown
/// catalogue host' on its own name (jc-prod 2026-11-10). Decided (supervisor, owner may
/// override): the deployment's configured host wins over a host `catalogue.propose` set on the
/// default catalogue, because the files are the source of truth. A slug-mode or loopback start
/// only creates a missing row and never writes a loopback host over a stored one.
async fn hold_default_host(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    mount_host: &str,
) -> dataroom_core::Result<()> {
    let host = bare_host(mount_host);
    if is_loopback(host) {
        return dataroom_server::ensure_default_catalogue(store, org_id, catalogue_id, mount_host)
            .await;
    }
    let stored = store.get_catalogue(org_id, catalogue_id).await?;
    let name = stored.as_ref().map_or("default", |c| c.name.as_str());
    let held = dataroom_server::ensure_catalogue(store, org_id, catalogue_id, name, host).await?;
    if let Some(old) = stored.filter(|c| c.host != held.host) {
        tracing::info!(from = %old.host, to = %held.host, "dataroom: default catalogue moved to the configured host (the deployment's host wins over one catalogue.propose set)");
    }
    Ok(())
}

/// `localhost`, `*.localhost` or a loopback address (the host as [`bare_host`] leaves it).
fn is_loopback(host: &str) -> bool {
    host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn bare_host(host: &str) -> &str {
    match host.find(']') {
        Some(end) if host.starts_with('[') => &host[..=end],
        _ => host.split(':').next().unwrap_or(host),
    }
}

/// What dataroom keeps on the registry's run-time Organizations (T-7186).
#[cfg(feature = "portal")]
struct RunTime {
    writer: dataroom_server::CatalogueWriter,
    branding: Arc<std::sync::RwLock<BTreeMap<CatalogueId, BrandingBlock>>>,
    installation: OrgId,
    default_catalogue: CatalogueId,
    /// jc.toml's Organizations and their catalogues, set up at start; one the registry removes
    /// leaves here.
    configured: BTreeMap<String, (OrgId, CatalogueId)>,
    own: BrandingBlock,
    live: crate::siblings::LiveAddresses,
    scheme: String,
    assistant_widget: Option<String>,
    /// Whether a service token issuer is known, so the Portal's publisher writes here.
    publishes: bool,
    /// The run-time Organizations served now, as they were set up.
    applied: BTreeMap<String, (OrgId, CatalogueId, OrganizationSection)>,
}

/// Follows `follow::ORGANIZATIONS`: one task applies the newest set ([`RunTime::apply`]), so
/// a set published while a catalogue is written waits and never interleaves with it.
#[cfg(feature = "portal")]
fn follow_organizations(
    run: RunTime,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) {
    follow_organizations_of(&super::follow::ORGANIZATIONS, run, shutdown, tasks);
}

/// [`follow_organizations`] on the sets `published` hands over.
#[cfg(feature = "portal")]
fn follow_organizations_of(
    published: &super::follow::Published<super::follow::Organizations>,
    mut run: RunTime,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) {
    let (tx, mut rx) = tokio::sync::watch::channel(super::follow::Organizations::default());
    published.follow("dataroom", move |set| {
        tx.send_replace(set.clone());
    });
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                changed = rx.changed() => if changed.is_err() { return },
            }
            let set = rx.borrow_and_update().clone();
            run.apply(&set).await;
        }
    });
}

#[cfg(feature = "portal")]
impl RunTime {
    /// Serves each run-time Organization's catalogue under its own OrgId (ODP-12) at its open
    /// data host, the Portal's publisher admitted when a service token issuer is (`publishes`), and stops
    /// serving the catalogue of a removed Organization or of one that left the set; the store
    /// keeps its datasets. A row whose catalogue is missing, not a UUID, the default catalogue
    /// or another Organization's gets none (logged), as jc.toml's check refuses it at start.
    async fn apply(&mut self, set: &super::follow::Organizations) {
        let kept: Vec<&str> = set.run_time.iter().map(|o| o.name.as_str()).collect();
        let gone: Vec<String> = self
            .applied
            .keys()
            .filter(|name| !kept.contains(&name.as_str()))
            .cloned()
            .collect();
        for name in gone.iter().chain(&set.removed) {
            let held = match self.applied.remove(name) {
                Some((org, id, _)) => Some((org, id)),
                None => self.configured.remove(name),
            };
            if let Some((org, id)) = held {
                self.writer.withdraw(org, id).await;
                self.write_branding().remove(&id);
                tracing::info!(organization = %name, "dataroom: catalogue no longer served");
            }
        }
        for org in &set.run_time {
            if self
                .applied
                .get(&org.name)
                .is_some_and(|(_, _, held)| held == org)
            {
                continue;
            }
            match self.serve(org).await {
                Ok((tenant, id)) => {
                    self.applied
                        .insert(org.name.clone(), (tenant, id, org.clone()));
                    tracing::info!(organization = %org.name, catalogue = %id, "dataroom: run-time catalogue served");
                }
                Err(e) => {
                    tracing::warn!(organization = %org.name, error = %e, "dataroom: no catalogue for this Organization");
                }
            }
        }
    }

    async fn serve(&self, org: &OrganizationSection) -> Result<(OrgId, CatalogueId), String> {
        let id = CatalogueId::parse_verified(&org.catalogue_id)
            .map_err(|_| "its row names no catalogue id (a UUID) yet".to_owned())?;
        let taken = id == self.default_catalogue
            || self
                .configured
                .values()
                .map(|&(_, c)| c)
                .chain(
                    self.applied
                        .iter()
                        .filter(|(name, _)| **name != org.name)
                        .map(|(_, &(_, c, _))| c),
                )
                .any(|c| c == id);
        if taken {
            return Err(format!(
                "catalogue {id} is the default catalogue or another Organization's"
            ));
        }
        let tenant = OrgId::of_organization(self.installation, &org.name);
        self.writer
            .upsert(dataroom_server::CatalogueSpec {
                org_id: tenant,
                id,
                name: org.name.clone(),
                host: bare_host(org.hosts.opendata_host()).to_owned(),
                base: Some(format!("{}://{}", self.scheme, org.opendata_base())),
                organization: Some(org.name.clone()),
                publisher: publisher_of(tenant, id, org, &self.scheme),
                publishes: self.publishes,
                portal: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        let block = organization_branding(
            &self.own,
            &self.live,
            org,
            &self.scheme,
            self.assistant_widget.clone(),
        );
        self.write_branding().insert(id, block);
        Ok((tenant, id))
    }

    fn write_branding(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, BTreeMap<CatalogueId, BrandingBlock>> {
        self.branding
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Keeps each Organization's catalogue branding its `Organization.spec.appearance` over jc's
/// config (UIX-16, T-6777), and each further portal's its `kind: Portal` `spec.appearance` over
/// that (T-7410): a member the Organization leaves out, or a removed appearance, shows
/// `configured` again; a removed portal look falls back to the installation's.
#[cfg(feature = "portal")]
fn follow_appearances(
    by_organization: &BTreeMap<String, (OrgId, CatalogueId)>,
    portals: Vec<super::portals::OrganizationPortals>,
    configured: BTreeMap<CatalogueId, BrandingBlock>,
    state: &DataroomState,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) {
    let catalogue: BTreeMap<String, CatalogueId> = by_organization
        .iter()
        .map(|(name, &(_, id))| (name.clone(), id))
        .collect();
    if catalogue.is_empty() {
        return;
    }
    let shared = state.catalogue_branding.clone();
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut seen: BTreeMap<CatalogueId, BrandingBlock> = BTreeMap::new();
        loop {
            apply_looks(
                super::PORTAL_STATE.get(),
                (&catalogue, &portals, &configured),
                &shared,
                &mut seen,
            );
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(super::appearance::FOLLOW_LOOK_EVERY) => {}
            }
        }
    });
}

/// One round of [`follow_appearances`]: each catalogue's branding as `portal`'s mirror has it
/// now ([`catalogue_looks`] of `wanted`'s catalogues, portals and configured blocks) written
/// over `shared` where it changed since `seen`; a catalogue `seen` holds that no longer has a
/// look leaves `shared`. Nothing while no Portal runs in this process.
#[cfg(feature = "portal")]
fn apply_looks(
    portal: Option<&joinedcontext_portal::state::AppState>,
    wanted: (
        &BTreeMap<String, CatalogueId>,
        &[super::portals::OrganizationPortals],
        &BTreeMap<CatalogueId, BrandingBlock>,
    ),
    shared: &std::sync::RwLock<BTreeMap<CatalogueId, BrandingBlock>>,
    seen: &mut BTreeMap<CatalogueId, BrandingBlock>,
) {
    let Some(portal) = portal else {
        return;
    };
    let (catalogue, portals, configured) = wanted;
    let wanted = catalogue_looks(&portal.mirror, catalogue, portals, configured);
    let mut branding = shared
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    seen.retain(|id, _| {
        let kept = wanted.contains_key(id);
        if !kept {
            branding.remove(id);
        }
        kept
    });
    for (id, block) in wanted {
        if seen.get(&id) != Some(&block) {
            tracing::info!(catalogue = %id, "dataroom: catalogue branding follows its appearance");
            branding.insert(id, block.clone());
            seen.insert(id, block);
        }
    }
}

/// Each catalogue's branding as `mirror` has it now: every Organization's catalogue its
/// configured block under its appearance, and each further portal with a `spec.appearance` its
/// Organization's block under that look over the Organization's. A portal's look lands only in
/// a catalogue of its own `organizationRef` (T-7410).
#[cfg(feature = "portal")]
fn catalogue_looks(
    mirror: &joinedcontext_portal::store::Mirror,
    catalogue: &BTreeMap<String, CatalogueId>,
    portals: &[super::portals::OrganizationPortals],
    configured: &BTreeMap<CatalogueId, BrandingBlock>,
) -> BTreeMap<CatalogueId, BrandingBlock> {
    let mut out = BTreeMap::new();
    let mut looks = BTreeMap::new();
    for (name, id) in catalogue {
        let look = super::appearance::in_mirror(mirror, name);
        if let Some(block) = configured.get(id) {
            out.insert(*id, with_appearance(block.clone(), look.as_ref()));
        }
        looks.insert(name.clone(), (*id, look));
    }
    let installation = joinedcontext_portal::permissions::installation_organization(mirror);
    let orgs = super::portals::named(portals, installation);
    for portal in super::portals::in_mirror(mirror).values() {
        let Some(top) = &portal.appearance else {
            continue;
        };
        let Some((own, under)) = looks.get(&portal.organization_ref) else {
            continue;
        };
        let (Some((_, id)), Some(block)) = (
            super::portals::catalogue_key(portal, &orgs),
            configured.get(own),
        ) else {
            continue;
        };
        let look = top.over(&under.clone().unwrap_or_default());
        out.insert(id, with_appearance(block.clone(), Some(&look)));
    }
    // T-7433: each portal's pages built from blocks, over whatever look its catalogue has; the
    // mirror's refresh carries a published change to the page within seconds.
    for portal in super::portals::in_mirror(mirror).values() {
        if portal.pages.is_empty() {
            continue;
        }
        let Some((own, under)) = looks.get(&portal.organization_ref) else {
            continue;
        };
        let Some((_, id)) = super::portals::catalogue_key(portal, &orgs) else {
            continue;
        };
        let block = match (out.get(&id), configured.get(own)) {
            (Some(shown), _) => shown.clone(),
            (None, Some(base)) => with_appearance(base.clone(), under.as_ref()),
            (None, None) => continue,
        };
        let mut block = block;
        block.layout.pages.clone_from(&portal.pages);
        out.insert(id, block);
    }
    // T-7746: each catalogue carries its own Organization's dataset fields, never another's;
    // a portal's catalogue shows them under whatever look it has.
    let domain = joinedcontext_portal::permissions::installation_organization_manifest(mirror)
        .and_then(|org| org.spec.get("domain")?.as_str().map(str::to_owned));
    let mut owners: Vec<(CatalogueId, String)> = catalogue
        .iter()
        .map(|(name, id)| (*id, name.clone()))
        .collect();
    for portal in super::portals::in_mirror(mirror).values() {
        if let Some((_, id)) = super::portals::catalogue_key(portal, &orgs) {
            owners.push((id, portal.organization_ref.clone()));
        }
    }
    for (id, org) in owners {
        let fields = dataset_fields_in(mirror, &org, domain.as_deref());
        if fields.fields.is_empty() {
            continue;
        }
        let Some((own, under)) = looks.get(&org) else {
            continue;
        };
        let block = match (out.remove(&id), configured.get(own)) {
            (Some(shown), _) => shown,
            (None, Some(base)) => with_appearance(base.clone(), under.as_ref()),
            (None, None) => continue,
        };
        out.insert(
            id,
            BrandingBlock {
                dataset_fields: fields,
                ..block
            },
        );
    }
    // T-7745: a portal's own stylesheet, checked and scoped to its catalogue again where it is
    // handed over; one jc-core refuses (a manifest older than a rule) is linked nowhere.
    for portal in super::portals::in_mirror(mirror).values() {
        let Some(css) = portal.appearance.as_ref().and_then(|a| a.css.as_deref()) else {
            continue;
        };
        let Some((_, id)) = super::portals::catalogue_key(portal, &orgs) else {
            continue;
        };
        let Some(block) = out.get_mut(&id) else {
            continue;
        };
        match dataroom_core::branding::PortalStylesheet::checked(&id.to_string(), css) {
            Ok(sheet) => block.layout.stylesheet = Some(sheet),
            Err(e) => {
                tracing::warn!(catalogue = %id, error = %e, "dataroom: a portal stylesheet jc-core refuses is not linked");
            }
        }
    }
    out
}

/// The Organization `name`'s own dataset fields (`spec.datasetFields`, T-7746) with their DCAT
/// namespace `https://<installation domain>/ns/dataset-fields/<name>/`: none when it defines none
/// or its list fails jc-core's check, and no namespace (the page still shows them, the feed not)
/// when the installation names no domain.
#[cfg(feature = "portal")]
fn dataset_fields_in(
    mirror: &joinedcontext_portal::store::Mirror,
    name: &str,
    domain: Option<&str>,
) -> dataroom_core::branding::BrandingDatasetFields {
    let fields: Vec<jc_core::kinds::DatasetField> = mirror
        .get(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Organization",
            name,
        )
        .and_then(|org| org.spec.get("datasetFields").cloned())
        .and_then(|defs| serde_json::from_value(defs).ok())
        .filter(|defs: &Vec<_>| jc_core::kinds::dataset_fields::validate(defs).is_ok())
        .unwrap_or_default();
    let namespace = domain
        .filter(|d| jc_core::names::validate_org_domain(d).is_ok())
        .map(|d| format!("https://{d}/ns/dataset-fields/{name}/"))
        .unwrap_or_default();
    dataroom_core::branding::BrandingDatasetFields {
        organization: name.to_owned(),
        namespace,
        fields,
    }
}

/// Each Organization's catalogue home shows the public Dashboards its
/// `spec.appearance.opendataWidgets` names, read from the Portal's mirror on every render
/// (UIX-16, T-6780): a catalogue without an Organization shows none.
#[cfg(feature = "portal")]
fn with_dashboards(
    by_organization: &BTreeMap<String, (OrgId, CatalogueId)>,
    state: DataroomState,
) -> DataroomState {
    let organization: BTreeMap<CatalogueId, String> = by_organization
        .iter()
        .map(|(name, &(_, id))| (id, name.clone()))
        .collect();
    state.with_dashboard_resolver(move |catalogue, lang| {
        dashboard_cards_of(&organization, super::PORTAL_STATE.get(), catalogue, lang)
    })
}

/// The dashboard cards of `catalogue`'s Organization in `portal`'s mirror; none for a catalogue
/// of no Organization or while no Portal runs here.
#[cfg(feature = "portal")]
fn dashboard_cards_of(
    organization: &BTreeMap<CatalogueId, String>,
    portal: Option<&joinedcontext_portal::state::AppState>,
    catalogue: &CatalogueId,
    lang: &str,
) -> Vec<dataroom_api::DashboardCard> {
    match (organization.get(catalogue), portal) {
        (Some(name), Some(state)) => super::appearance::dashboard_cards(&state.mirror, name, lang),
        _ => Vec::new(),
    }
}

/// `block` with an Organization's appearance over it, member by member; a logo that fails the
/// logo upload's SVG check keeps jc's.
#[cfg(feature = "portal")]
fn with_appearance(
    block: BrandingBlock,
    look: Option<&jc_core::kinds::OrganizationAppearance>,
) -> BrandingBlock {
    let Some(look) = look else {
        return block;
    };
    let mut over = serde_json::json!({
        "instanceName": look.instance_name,
        "shortName": super::appearance::short_name(look),
        "city": look.city,
        "organisation": look.publisher,
        "contactEmail": look.contact_email,
        "licenseDefault": look.license_default,
        "colours": {
            "primary": look.colours.primary,
            "secondary": look.colours.secondary,
            "accent": look.colours.accent,
            "background": look.colours.background,
            "text": look.colours.text,
        },
    });
    if let Some(svg) = look
        .logo_svg
        .as_deref()
        .filter(|svg| dataroom_core::branding::is_safe_logo_svg(svg))
    {
        over["logoSvg"] = svg.into();
        over["logo"] = "logo.svg".into();
    }
    // T-7744: a heading or body font set alone goes over `font`, which sets both.
    let (heading, body) = (
        look.heading_font.or(look.font),
        look.body_font.or(look.font),
    );
    if heading.is_some() || body.is_some() {
        let mut fonts = serde_json::Map::new();
        if let Some(font) = heading {
            fonts.insert("heading".into(), font.stack().into());
        }
        if let Some(font) = body {
            fonts.insert("body".into(), font.stack().into());
        }
        over["fonts"] = fonts.into();
    }
    let mut shown = block.clone();
    shown.apply_override_json(&over);
    // T-7410: a colour that is readable alone can meet the installation's other colour of its
    // pair; below WCAG AA the Organization's colours give way to jc's, the rest still shows.
    let readable = |b: &BrandingBlock| {
        [&b.colours.text, &b.colours.primary].iter().all(|c| {
            jc_core::kinds::contrast_ratio(c, &b.colours.background)
                .is_some_and(|r| r >= jc_core::kinds::appearance::MIN_CONTRAST)
        })
    };
    if readable(&block) && !readable(&shown) {
        tracing::warn!("dataroom: a colour below WCAG AA on the background; jc's colours show");
        over["colours"] = serde_json::Value::Null;
        shown = block;
        shown.apply_override_json(&over);
    }
    let mut block = shown;
    block.layout = dataroom_core::branding::BrandingLayout {
        radius_px: look.radius,
        hero_title: look.hero.title.clone(),
        hero_intro_html: look.hero.intro.as_deref().map(hero_intro),
        dataset_table: look.dataset_list == Some(jc_core::kinds::DatasetListLayout::Table),
        nav_links: look
            .nav_links
            .iter()
            .filter(|l| jc_core::kinds::is_link_url(&l.url))
            .map(|l| (l.url.clone(), l.label.clone()))
            .collect(),
        footer_links: look
            .footer_links
            .iter()
            .filter(|l| jc_core::kinds::is_link_url(&l.url))
            .map(|l| (l.url.clone(), l.label.clone()))
            .collect(),
        pages: Vec::new(),
        tokens: readable_tokens(&block, look),
        // T-7745: a portal's own stylesheet is set by `catalogue_looks`, for its catalogue alone.
        stylesheet: None,
    };
    // T-7743: the look's display over the installation's, list by list.
    block.display = look.display.over(&block.display);
    // T-7031: the Organization's own page texts, each page and language over the built-in one.
    // A text the manifest's rules refuse never reaches here (jc-core validates every load), and
    // the page renders it with jc-core's one safe renderer.
    block.pages = look
        .pages
        .iter()
        .map(|text| {
            (
                (text.page.clone(), text.lang.clone()),
                text.markdown.clone(),
            )
        })
        .collect();
    block
}

/// The look's style tokens (T-7744), each colour pair kept only where it reads at WCAG AA
/// against what the page draws it on: a token the look leaves out is the theme's (the header
/// on the page background in the text colour, the footer in the text colour with the
/// background's), as T-7410 does for the text and primary colours.
#[cfg(feature = "portal")]
fn readable_tokens(
    block: &BrandingBlock,
    look: &jc_core::kinds::OrganizationAppearance,
) -> dataroom_core::branding::BrandingTokens {
    let c = &look.colours;
    let theme = &block.colours;
    let readable = |fore: &str, back: &str| {
        jc_core::kinds::contrast_ratio(fore, back)
            .is_some_and(|r| r >= jc_core::kinds::appearance::MIN_CONTRAST)
    };
    let pair =
        |fore: &Option<String>, back: &Option<String>, fore_theme: &str, back_theme: &str| {
            let (f, b) = (
                fore.as_deref().unwrap_or(fore_theme),
                back.as_deref().unwrap_or(back_theme),
            );
            if readable(f, b) {
                (fore.clone(), back.clone())
            } else {
                if fore.is_some() || back.is_some() {
                    tracing::warn!("dataroom: a colour pair below WCAG AA; the theme's shows");
                }
                (None, None)
            }
        };
    let (header_text, header_background) = pair(
        &c.header_text,
        &c.header_background,
        &theme.text,
        &theme.background,
    );
    let (footer_text, footer_background) = pair(
        &c.footer_text,
        &c.footer_background,
        &theme.background,
        &theme.text,
    );
    let (link, _) = pair(&c.link, &None, &theme.primary, &theme.background);
    dataroom_core::branding::BrandingTokens {
        link,
        header_background,
        header_text,
        footer_background,
        footer_text,
        font_size_px: look.font_size,
        width: look.width,
        button_outline: look.button_style == Some(jc_core::kinds::ButtonStyle::Outline),
        card_flat: look.card_shadow == Some(jc_core::kinds::CardShadow::None),
    }
}

/// The hero introduction as safe HTML through jc-core's page renderer: raw HTML shows as text,
/// links keep to http(s) and the portal; a leading `# heading` stays, as a level-two one.
#[cfg(feature = "portal")]
fn hero_intro(markdown: &str) -> String {
    let page = jc_core::opendata_pages::render(markdown, &[]);
    match page.title {
        Some(title) => {
            let title = title
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            format!("<h2>{title}</h2>{}", page.html)
        }
        None => page.html,
    }
}

/// An Organization's catalogue frames its own Portal and API: the installation's other live
/// parts stay as they are.
fn organization_branding(
    own: &BrandingBlock,
    live: &crate::siblings::LiveAddresses,
    org: &OrganizationSection,
    scheme: &str,
    assistant_widget: Option<String>,
) -> BrandingBlock {
    let mut block = installation_branding(own, live, assistant_widget);
    block.addresses.api = format!("{scheme}://{}", org.api_base());
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
    store
        .upsert_publisher(&ctx, publisher_of(org_id, catalogue_id, org, scheme))
        .await?;
    Ok(())
}

/// The CKAN organisation of `org` in its catalogue, as [`ensure_publisher`] makes it.
fn publisher_of(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    org: &OrganizationSection,
    scheme: &str,
) -> Publisher {
    let now = jc_time::TimestampTz::now();
    Publisher {
        org_id,
        catalogue_id,
        id: String::new(),
        name: org.name.clone(),
        title: BTreeMap::from([("en".to_owned(), org.name.clone())]),
        description: BTreeMap::new(),
        uri: format!("{scheme}://{}/publisher/{}", org.opendata_base(), org.name),
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
    }
}

/// Writes `hidden` over the publish gate's list of catalogue `catalogue_id` (prod's `hidden`
/// chart value, `jc_hide`): each named dataset answers its editors only, as on prod.
pub async fn seed_hidden(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    hidden: &BTreeMap<String, String>,
) -> dataroom_core::Result<()> {
    let Some(mut cat) = store.get_catalogue(org_id, catalogue_id).await? else {
        return Err(DataroomError::NotFound(format!("catalogue {catalogue_id}")));
    };
    if cat.hidden_datasets != *hidden {
        cat.hidden_datasets = hidden.clone();
        cat.modified = jc_time::TimestampTz::now();
        store.upsert_catalogue(cat).await?;
    }
    Ok(())
}

/// The converted datasets of `path` (prod's `od-datasets.json`), each `dataset` URL moved from
/// `base` to this catalogue's `site`, as prod's harvest CronJob replaces `datasetsBase`.
#[cfg(feature = "portal")]
pub(crate) fn converted_datasets(
    path: &std::path::Path,
    base: Option<&str>,
    site: &str,
) -> dataroom_core::Result<BTreeMap<String, dataroom_core::ConvertedDataset>> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        DataroomError::field_error(
            "converted_file",
            format!("{}: {}", path.display(), e.kind()),
        )
    })?;
    let mut all: BTreeMap<String, dataroom_core::ConvertedDataset> = serde_json::from_str(&raw)
        .map_err(|e| {
            DataroomError::field_error("converted_file", format!("{}: {e}", path.display()))
        })?;
    if let Some(base) = base.filter(|b| !b.is_empty()) {
        let to = format!("{}/", site.trim_end_matches('/'));
        for entry in all.values_mut() {
            entry.dataset = entry.dataset.replace(base, &to);
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::ops::HarvestRunner;
    use dataroom_core::{HarvestRunState, HarvestSource};
    use dataroom_harvest::{MapFetcher, validate_outbound_url};
    use std::collections::BTreeSet;

    /// Each failing step of a Postgres start refuses it with a module error, on real Postgres
    /// (`DATAROOM_TEST_DATABASE_URL`, a superuser that only prepares): an owner that may not
    /// create the schema, a request role no login holds (the grant), a request URL that does not
    /// parse, a request login with BYPASSRLS, and a catalogue to check that the request login may
    /// not read. No password reaches a message.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // The test reads its database URL, as the split test does.
    async fn each_failing_step_of_a_postgres_start_refuses_it() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        let Ok(admin_url) = std::env::var("DATAROOM_TEST_DATABASE_URL") else {
            println!(
                "SKIP each_failing_step_of_a_postgres_start: DATAROOM_TEST_DATABASE_URL is unset"
            );
            return;
        };
        let base = PgConnectOptions::from_str(&admin_url).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let db = format!("jc_drfail_{}", std::process::id());
        let dir = tempfile::tempdir().unwrap();
        let mut statements = vec![
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
            "DO $$ BEGIN CREATE ROLE dataroom_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
             NOBYPASSRLS; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;"
                .to_owned(),
        ];
        for (role, create) in [
            ("jc_drf_owner", true),
            ("jc_drf_app", false),
            ("jc_drf_guest", false),
        ] {
            let create = if create { "CREATE, " } else { "" };
            statements.push(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD 'pw-{role}-7782';
                 GRANT {create}CONNECT ON DATABASE {db} TO {role};"
            ));
        }
        for statement in statements {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&admin)
                .await
                .unwrap();
        }
        let url = |role: &str| {
            format!(
                "postgres://{role}@{}:{}/{db}",
                base.get_host(),
                base.get_port()
            )
        };
        let file = |role: &str| {
            let file = dir.path().join(role);
            std::fs::write(&file, format!("pw-{role}-7782")).unwrap();
            file
        };
        let section = |request: &str, owner: Option<&str>| DataroomSection {
            database: Some(crate::pg::DatabaseSection {
                url: url(request),
                password_file: file(request),
                max_connections: Some(1),
                migrate_url: owner.map(url),
                migrate_password_file: owner.map(file),
                maintenance_url: None,
                maintenance_password_file: None,
            }),
            ..DataroomSection::default()
        };
        let refused = |cfg: DataroomSection, moves: Vec<(OrgId, OrgId, CatalogueId)>| async move {
            open_store(&cfg, &moves)
                .await
                .err()
                .expect("the start is refused")
                .to_string()
        };
        let quiet = |e: &str| !e.contains("-7782");
        let e = refused(section("jc_drf_app", Some("jc_drf_guest")), Vec::new()).await;
        assert!(
            e.contains("dataroom") && quiet(&e),
            "an owner without CREATE: {e}"
        );
        let e = refused(section("jc_drf_nobody", Some("jc_drf_owner")), Vec::new()).await;
        assert!(e.contains("grant to jc_drf_nobody") && quiet(&e), "{e}");
        let mut unparsable = section("jc_drf_app", Some("jc_drf_owner"));
        unparsable.database.as_mut().expect("a section").url = "not a url".into();
        let e = refused(unparsable, Vec::new()).await;
        assert!(e.contains("url") && quiet(&e), "{e}");
        let installation = OrgId::parse_verified(&DataroomSection::default().org_id).unwrap();
        let catalogue =
            CatalogueId::parse_verified("00000000-0000-7000-8000-000000007782").unwrap();
        let moves = vec![(
            installation,
            OrgId::of_organization(installation, "city"),
            catalogue,
        )];
        let e = refused(section("jc_drf_guest", None), moves).await;
        assert!(
            e.contains("dataroom") && quiet(&e),
            "a catalogue the login may not read: {e}"
        );
        sqlx::raw_sql("ALTER ROLE jc_drf_app BYPASSRLS")
            .execute(&admin)
            .await
            .unwrap();
        let e = refused(section("jc_drf_app", None), Vec::new()).await;
        assert!(e.contains("dataroom") && quiet(&e), "BYPASSRLS: {e}");
        for statement in [
            "ALTER ROLE jc_drf_app NOBYPASSRLS".to_owned(),
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&admin)
                .await
                .unwrap();
        }
    }

    /// Each store write of a Postgres start that the server refuses stops it with a module
    /// error, on real Postgres (`DATAROOM_TEST_DATABASE_URL`, a superuser that only prepares):
    /// after one clean start a trigger refuses the catalogue table's writes for the hidden list,
    /// then for an Organization's catalogue, then the publisher table's for its publisher, and
    /// last the default catalogue once it is gone. Each refusal leaves the next start whole.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // The test reads its database URL, as the split test does.
    async fn each_refused_store_write_of_a_postgres_start_refuses_it() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        let Ok(admin_url) = std::env::var("DATAROOM_TEST_DATABASE_URL") else {
            println!("SKIP each_refused_store_write: DATAROOM_TEST_DATABASE_URL is unset");
            return;
        };
        let base = PgConnectOptions::from_str(&admin_url).unwrap();
        let server = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let db = format!("jc_drwrite_{}", std::process::id());
        let dir = tempfile::tempdir().unwrap();
        let mut statements = vec![
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
            "DO $$ BEGIN CREATE ROLE dataroom_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
             NOBYPASSRLS; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;"
                .to_owned(),
        ];
        for role in ["jc_drw_owner", "jc_drw_app"] {
            statements.push(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD 'pw-{role}-7903';
                 GRANT CREATE, CONNECT ON DATABASE {db} TO {role};"
            ));
        }
        for statement in statements {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&server)
                .await
                .unwrap();
        }
        let url = |role: &str| {
            format!(
                "postgres://{role}@{}:{}/{db}",
                base.get_host(),
                base.get_port()
            )
        };
        let file = |role: &str| {
            let file = dir.path().join(role);
            std::fs::write(&file, format!("pw-{role}-7903")).unwrap();
            file
        };
        let database = crate::pg::DatabaseSection {
            url: url("jc_drw_app"),
            password_file: file("jc_drw_app"),
            max_connections: Some(2),
            migrate_url: Some(url("jc_drw_owner")),
            migrate_password_file: Some(file("jc_drw_owner")),
            maintenance_url: None,
            maintenance_password_file: None,
        };
        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let started = |hidden: bool, organizations: Vec<OrganizationSection>| {
            let cfg = DataroomSection {
                database: Some(database.clone()),
                hidden: hidden.then(|| BTreeMap::from([("air".to_owned(), "gate".to_owned())])),
                ..DataroomSection::default()
            };
            let ctx = crate::modules::BuildCtx {
                setup: &setup,
                siblings: &siblings,
                secrets: &secrets,
                door: None,
                portal_internal: None,
            };
            async move {
                let (shutdown, tasks) = (
                    tokio_util::sync::CancellationToken::new(),
                    tokio_util::task::TaskTracker::new(),
                );
                let built = build_with_writer(
                    &cfg,
                    &organizations,
                    ctx,
                    Mount::default(),
                    None,
                    &shutdown,
                    &tasks,
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
                shutdown.cancel();
                built
            }
        };
        let city = || vec![organization("city", "00000000-0000-7000-8000-000000007903")];
        started(false, Vec::new())
            .await
            .expect("a clean first start");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone().database(&db))
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE public.refuse (tbl text NOT NULL);
             CREATE FUNCTION public.refuse() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER AS $$
             BEGIN
                 IF EXISTS (SELECT 1 FROM public.refuse WHERE tbl = TG_TABLE_NAME) THEN
                     RAISE EXCEPTION 'the test refuses %', TG_TABLE_NAME;
                 END IF;
                 RETURN NEW;
             END $$;
             CREATE TRIGGER refuse BEFORE INSERT OR UPDATE ON dataroom.cat_catalogue
                 FOR EACH ROW EXECUTE FUNCTION public.refuse();
             CREATE TRIGGER refuse BEFORE INSERT OR UPDATE ON dataroom.cat_publisher
                 FOR EACH ROW EXECUTE FUNCTION public.refuse();",
        )
        .execute(&admin)
        .await
        .unwrap();
        let refuse = |table: &'static str| {
            let admin = admin.clone();
            async move {
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM public.refuse; INSERT INTO public.refuse VALUES ('{table}')"
                )))
                .execute(&admin)
                .await
                .unwrap();
            }
        };
        let quiet = |e: &str| !e.contains("-7903");
        refuse("cat_catalogue").await;
        // The default catalogue is there: only the hidden list writes it.
        started(false, Vec::new())
            .await
            .expect("a start that writes nothing");
        let e = started(true, Vec::new())
            .await
            .expect_err("the hidden list");
        assert!(e.contains("[dataroom.hidden]") && quiet(&e), "{e}");
        let e = started(false, city())
            .await
            .expect_err("the city's catalogue");
        assert!(e.contains("dataroom") && quiet(&e), "{e}");
        refuse("cat_publisher").await;
        let e = started(false, city())
            .await
            .expect_err("the city's publisher");
        assert!(e.contains("dataroom") && quiet(&e), "{e}");
        refuse("cat_catalogue").await;
        sqlx::raw_sql("TRUNCATE dataroom.cat_catalogue CASCADE")
            .execute(&admin)
            .await
            .unwrap();
        let e = started(false, Vec::new())
            .await
            .expect_err("the default catalogue");
        assert!(e.contains("dataroom") && quiet(&e), "{e}");
        sqlx::raw_sql("DELETE FROM public.refuse")
            .execute(&admin)
            .await
            .unwrap();
        started(true, city()).await.expect("a whole start again");
        drop(admin);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
        )))
        .execute(&server)
        .await
        .unwrap();
    }

    /// ODP-12 on real Postgres (`DATAROOM_TEST_DATABASE_URL`, a superuser that only prepares):
    /// two Organizations' catalogues written as before the split, under the installation's one
    /// OrgId, then a start of the module as its NOSUPERUSER migration and request roles. Every
    /// dataset URL answers as before, each catalogue now sits under its Organization's own
    /// OrgId, and Organization A's tenant sees none of B's rows.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // The test reads its database URL, as tests/restart_pg.rs does.
    async fn every_dataset_url_answers_after_the_org_split() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        use std::str::FromStr;
        use tower::ServiceExt;
        let Ok(admin_url) = std::env::var("DATAROOM_TEST_DATABASE_URL") else {
            println!(
                "SKIP every_dataset_url_answers_after_the_org_split: DATAROOM_TEST_DATABASE_URL is unset"
            );
            return;
        };
        let (log, _guard) = super::super::capture_log();
        let base = PgConnectOptions::from_str(&admin_url).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await
            .unwrap();
        let db = format!("jc_split_{}", std::process::id());
        let dir = tempfile::tempdir().unwrap();
        for statement in [
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&admin)
                .await
                .unwrap();
        }
        // The cluster-wide role dataroom's first migration creates when it is missing, made
        // by the superuser as a deployment does: the NOCREATEROLE owner cannot, and on a fresh
        // server no earlier test has made it.
        sqlx::raw_sql(
            "DO $$ BEGIN CREATE ROLE dataroom_app NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
             NOBYPASSRLS; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;",
        )
        .execute(&admin)
        .await
        .unwrap();
        let mut urls = Vec::new();
        for role in ["split_app", "split_owner"] {
            let pw = format!("pw-{role}-{}", std::process::id());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{pw}';
                 GRANT CREATE, CONNECT ON DATABASE {db} TO {role};"
            )))
            .execute(&admin)
            .await
            .unwrap();
            let file = dir.path().join(role);
            std::fs::write(&file, &pw).unwrap();
            std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .unwrap();
            let url = format!(
                "postgres://{role}@{}:{}/{db}",
                base.get_host(),
                base.get_port()
            );
            urls.push((url, file));
        }
        let database = crate::pg::DatabaseSection {
            url: urls[0].0.clone(),
            password_file: urls[0].1.clone(),
            max_connections: Some(2),
            migrate_url: Some(urls[1].0.clone()),
            migrate_password_file: Some(urls[1].1.clone()),
            maintenance_url: None,
            maintenance_password_file: None,
        };
        let cfg = DataroomSection {
            database: Some(database),
            ..DataroomSection::default()
        };
        let installation = OrgId::parse_verified(&cfg.org_id).unwrap();
        let organizations: Vec<OrganizationSection> = ["city-a", "city-b"]
            .iter()
            .enumerate()
            .map(|(n, name)| OrganizationSection {
                name: (*name).into(),
                id: None,
                hosts: crate::config::OrganizationHosts {
                    portal: format!("portal.{name}.test"),
                    opendata: Some(format!("data.{name}.test")),
                    api: Some(format!("api.{name}.test")),
                    mcp: Some(format!("mcp.{name}.test")),
                    ..Default::default()
                },
                host: None,
                same_site_apps: false,
                catalogue_id: format!("6f1c0b2e-5d3a-4c1e-9a8b-7e2d4f6a1b{n:02}"),
                portal_token_file: None,
                first_admin: None,
            })
            .collect();

        // Before ODP-12: the schema, and both catalogues with a public dataset under the
        // installation's OrgId, as every jc wrote them.
        let owner = crate::pg::connect(
            cfg.database
                .as_ref()
                .unwrap()
                .migrate_options()
                .unwrap()
                .unwrap(),
            1,
        )
        .await
        .unwrap();
        PgStore::from_pool_unchecked(owner.clone())
            .migrate()
            .await
            .unwrap();
        sqlx::raw_sql(
            "GRANT USAGE ON SCHEMA dataroom TO split_app;
             GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA dataroom TO split_app;",
        )
        .execute(&owner)
        .await
        .unwrap();
        owner.close().await;
        let app = crate::pg::connect(cfg.database.as_ref().unwrap().app_options().unwrap(), 1)
            .await
            .unwrap();
        let before = PgStore::connect_and_verify(app.clone()).await.unwrap();
        for org in &organizations {
            let cat = CatalogueId::parse_verified(&org.catalogue_id).unwrap();
            dataroom_server::ensure_catalogue(
                &before,
                installation,
                cat,
                &org.name,
                org.hosts.opendata_host(),
            )
            .await
            .unwrap();
            let ctx = RequestCtx::new(installation, cat, Subject::anonymous());
            before
                .upsert_publisher(&ctx, serde_json::from_value(serde_json::json!({
                    "orgId": installation, "catalogueId": cat, "id": "", "name": "city",
                    "title": {"sk": "Mesto"}, "description": {}, "uri": "https://example.org/city",
                    "state": "active", "editorGrants": [],
                    "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
                })).unwrap())
                .await
                .unwrap();
            before
                .upsert_dataset(
                    &ctx,
                    serde_json::from_value(serde_json::json!({
                        "orgId": installation, "catalogueId": cat, "id": "",
                        "name": format!("air-{}", org.name), "title": {"sk": "Ovzdušie"},
                        "description": {"sk": "Popis"}, "publisherId": "city",
                        "visibility": "public", "state": "active", "tags": [], "distributions": [],
                        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
        }

        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let ctx = crate::modules::BuildCtx {
            setup: &setup,
            siblings: &siblings,
            secrets: &secrets,
            door: None,
            portal_internal: None,
        };
        let (shutdown, tasks) = (
            tokio_util::sync::CancellationToken::new(),
            tokio_util::task::TaskTracker::new(),
        );
        // Without the migration owner nothing moves: the start stops and names the fix.
        let mut no_owner = cfg.clone();
        let section = no_owner.database.as_mut().expect("a database section");
        section.migrate_url = None;
        section.migrate_password_file = None;
        let moves: Vec<_> = organizations
            .iter()
            .map(|o| {
                (
                    installation,
                    OrgId::of_organization(installation, &o.name),
                    CatalogueId::parse_verified(&o.catalogue_id).unwrap(),
                )
            })
            .collect();
        let e = open_store(&no_owner, &moves)
            .await
            .err()
            .expect("a catalogue still under the installation's OrgId")
            .to_string();
        assert!(e.contains("still sits under [dataroom] org_id"), "{e}");
        let router = build(
            &cfg,
            &organizations,
            ctx,
            Mount::default(),
            None,
            &shutdown,
            &tasks,
        )
        .await
        .expect("the module starts and moves both catalogues");
        shutdown.cancel();
        open_store(&cfg, &moves)
            .await
            .expect("a second start with the owner finds nothing to move");
        let logged = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            logged.contains("moved to its Organization's OrgId") && logged.contains("moved="),
            "{logged}"
        );

        for org in &organizations {
            let request =
                axum::http::Request::get(format!("/api/3/action/package_show?id=air-{}", org.name))
                    .header("host", org.hosts.opendata_host())
                    .body(axum::body::Body::empty())
                    .unwrap();
            let answer = router.clone().oneshot(request).await.unwrap();
            assert_eq!(answer.status(), axum::http::StatusCode::OK, "{}", org.name);
        }
        let own = |n: usize| OrgId::of_organization(installation, &organizations[n].name);
        let cat = |n: usize| CatalogueId::parse_verified(&organizations[n].catalogue_id).unwrap();
        assert_ne!(own(0), own(1));
        for n in 0..2 {
            assert!(
                before
                    .get_catalogue(installation, cat(n))
                    .await
                    .unwrap()
                    .is_none(),
                "moved"
            );
            assert!(
                before
                    .get_catalogue(own(n), cat(n))
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        let a_on_b = RequestCtx::new(own(0), cat(1), Subject::anonymous());
        assert!(
            before
                .get_dataset(&a_on_b, "air-city-b")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            before
                .get_catalogue(own(0), cat(1))
                .await
                .unwrap()
                .is_none()
        );
        app.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }

    /// The Portal's publisher token as jc's in-process issuer signs it, as a bearer.
    fn bearer() -> String {
        let token = crate::door::local_publisher_token().expect("signed");
        format!("Bearer {}", token.expose())
    }

    /// A memory-store dataroom as jc builds it with no `[[organizations]]` beside the Portal
    /// (all in one: its in-process issuer verifies the publisher), and its writer.
    async fn runtime_dataroom() -> (Router, dataroom_server::CatalogueWriter) {
        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let mut siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        siblings.insert_loopback(Module::Portal, "127.0.0.1:1".parse().unwrap());
        let ctx = crate::modules::BuildCtx {
            setup: &setup,
            siblings: &siblings,
            secrets: &secrets,
            door: None,
            portal_internal: None,
        };
        let shutdown = tokio_util::sync::CancellationToken::new();
        let tasks = tokio_util::task::TaskTracker::new();
        let built = build_with_writer(
            &DataroomSection::default(),
            &[],
            ctx,
            Mount::default(),
            None,
            &shutdown,
            &tasks,
        )
        .await
        .expect("the module starts");
        shutdown.cancel();
        built
    }

    /// A set the host registry publishes after the follower started reaches it: the row's
    /// catalogue is served (its branding held) without a restart, and a shutdown ends it.
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn a_published_organization_set_reaches_the_follower() {
        static PUBLISHED: super::super::follow::Published<super::super::follow::Organizations> =
            super::super::follow::Published::new();
        let (_router, writer) = runtime_dataroom().await;
        let installation = OrgId::parse_verified(&DataroomSection::default().org_id).unwrap();
        let run = RunTime {
            writer,
            branding: Arc::default(),
            installation,
            default_catalogue: CatalogueId::parse_verified(
                &DataroomSection::default().catalogue_id,
            )
            .unwrap(),
            configured: BTreeMap::new(),
            own: BrandingBlock::default(),
            live: crate::siblings::LiveAddresses::default(),
            scheme: "https".into(),
            assistant_widget: None,
            publishes: false,
            applied: BTreeMap::new(),
        };
        let branding = run.branding.clone();
        let (shutdown, tasks) = (
            tokio_util::sync::CancellationToken::new(),
            tokio_util::task::TaskTracker::new(),
        );
        follow_organizations_of(&PUBLISHED, run, &shutdown, &tasks);
        let id = "00000000-0000-7000-8000-000000007782";
        assert!(PUBLISHED.publish(super::super::follow::Organizations {
            run_time: vec![organization("rt7782", id)],
            removed: Vec::new(),
        }));
        let id = CatalogueId::parse_verified(id).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !branding.read().unwrap().contains_key(&id) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the published row is served");
        shutdown.cancel();
        tasks.close();
        tokio::time::timeout(std::time::Duration::from_secs(10), tasks.wait())
            .await
            .expect("the follower ends with the shutdown");
    }

    /// A `[dataroom]` org_id or catalogue_id that is no UUID, or a publisher badge that cannot
    /// draw, refuses the start and names the field.
    #[tokio::test]
    async fn a_bad_id_or_badge_refuses_the_start() {
        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let mut badge = DataroomSection::default();
        badge.publishers.insert(
            "praha".into(),
            crate::config::PublisherSeed {
                initials: String::new(),
                colour: "#b4232a".into(),
                description: BTreeMap::new(),
            },
        );
        let cases = [
            (
                DataroomSection {
                    org_id: "not-a-uuid".into(),
                    ..DataroomSection::default()
                },
                "",
            ),
            (
                DataroomSection {
                    catalogue_id: "not-a-uuid".into(),
                    ..DataroomSection::default()
                },
                "",
            ),
            (badge, "[dataroom.publishers.praha] initials"),
        ];
        for (cfg, names) in cases {
            let ctx = crate::modules::BuildCtx {
                setup: &setup,
                siblings: &siblings,
                secrets: &secrets,
                door: None,
                portal_internal: None,
            };
            let (shutdown, tasks) = (
                tokio_util::sync::CancellationToken::new(),
                tokio_util::task::TaskTracker::new(),
            );
            let e = build_with_writer(&cfg, &[], ctx, Mount::default(), None, &shutdown, &tasks)
                .await
                .err()
                .expect("refused")
                .to_string();
            assert!(e.contains("dataroom") && e.contains(names), "{e}");
        }
    }

    /// `[dataroom.hidden]` is written over the default catalogue's list at the start.
    #[tokio::test]
    async fn a_configured_hidden_list_starts_the_module() {
        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let ctx = crate::modules::BuildCtx {
            setup: &setup,
            siblings: &siblings,
            secrets: &secrets,
            door: None,
            portal_internal: None,
        };
        let cfg = DataroomSection {
            hidden: Some(BTreeMap::from([("air".to_owned(), "gate".to_owned())])),
            ..DataroomSection::default()
        };
        let (shutdown, tasks) = (
            tokio_util::sync::CancellationToken::new(),
            tokio_util::task::TaskTracker::new(),
        );
        let built =
            build_with_writer(&cfg, &[], ctx, Mount::default(), None, &shutdown, &tasks).await;
        shutdown.cancel();
        assert!(built.is_ok(), "the hidden list is seeded");
    }

    fn organization(name: &str, catalogue_id: &str) -> OrganizationSection {
        OrganizationSection {
            name: name.into(),
            id: None,
            hosts: crate::config::OrganizationHosts {
                portal: format!("smartcity.{name}.sk"),
                opendata: Some(format!("data.{name}.sk")),
                ..Default::default()
            },
            host: None,
            same_site_apps: false,
            catalogue_id: catalogue_id.into(),
            portal_token_file: None,
            first_admin: None,
        }
    }

    /// The start of a memory-store dataroom with `organizations`, beside the Portal when
    /// `portal`: its refusal, if any.
    async fn start(
        organizations: &[OrganizationSection],
        portal: bool,
    ) -> Result<(Router, dataroom_server::CatalogueWriter), String> {
        let (setup, secrets) = (
            crate::setup::Setup::default(),
            crate::config::Secrets::default(),
        );
        let mut siblings = crate::siblings::Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        if portal {
            siblings.insert_loopback(Module::Portal, "127.0.0.1:1".parse().unwrap());
        }
        let ctx = crate::modules::BuildCtx {
            setup: &setup,
            siblings: &siblings,
            secrets: &secrets,
            door: None,
            portal_internal: None,
        };
        let shutdown = tokio_util::sync::CancellationToken::new();
        let tasks = tokio_util::task::TaskTracker::new();
        let built = build_with_writer(
            &DataroomSection::default(),
            organizations,
            ctx,
            Mount::default(),
            None,
            &shutdown,
            &tasks,
        )
        .await
        .map_err(|e| e.to_string());
        shutdown.cancel();
        built
    }

    /// Two Organizations on one catalogue id would move each other's catalogue: the start
    /// stops and names the Organization; so does an id that is no UUID.
    #[tokio::test]
    async fn a_catalogue_id_held_twice_or_not_a_uuid_refuses_the_start() {
        let default = DataroomSection::default().catalogue_id;
        let e = start(&[organization("bbsk", &default)], true)
            .await
            .err()
            .expect("the default catalogue's id");
        assert!(e.contains("[[organizations]] bbsk"), "{e}");
        assert!(e.contains("already [dataroom] catalogue_id"), "{e}");
        let shared = "00000000-0000-7000-8000-00000000c0de";
        let e = start(
            &[organization("one", shared), organization("two", shared)],
            true,
        )
        .await
        .err()
        .expect("another Organization's id");
        assert!(e.contains("[[organizations]] two"), "{e}");
        let e = start(&[organization("bad", "not-a-uuid")], true)
            .await
            .err()
            .expect("no UUID");
        assert!(e.contains("dataroom"), "{e}");
        let (router, _) = start(
            &[
                organization("one", "00000000-0000-7000-8000-000000000a01"),
                organization("two", "00000000-0000-7000-8000-000000000a02"),
            ],
            true,
        )
        .await
        .expect("distinct ids start");
        for host in ["data.one.sk", "data.two.sk"] {
            assert_eq!(
                action(&router, host, None, "package_list", None).await,
                axum::http::StatusCode::OK,
                "{host}"
            );
        }
    }

    /// Without the Portal beside it and without `[door]` nobody issues service tokens: the
    /// module starts, and the Portal's publisher token is refused on every catalogue.
    #[tokio::test]
    async fn without_a_token_issuer_no_catalogue_admits_the_publisher() {
        let (log, _guard) = super::super::capture_log();
        let (router, _) = start(&[], false).await.expect("starts without an issuer");
        let logged = std::fs::read_to_string(log.path()).unwrap();
        assert!(logged.contains("no issuer of service tokens"), "{logged}");
        let status = action(
            &router,
            "localhost",
            Some(&bearer()),
            "package_create",
            Some(package("refused")),
        )
        .await;
        assert!(
            matches!(
                status,
                axum::http::StatusCode::UNAUTHORIZED | axum::http::StatusCode::FORBIDDEN
            ),
            "{status}"
        );
    }

    fn unreachable_database(dir: &std::path::Path, migrate: bool) -> crate::pg::DatabaseSection {
        let password = dir.join("password");
        std::fs::write(&password, "pw").unwrap();
        crate::pg::DatabaseSection {
            url: "postgres://dataroom_app@127.0.0.1:1/dataroom".into(),
            password_file: password.clone(),
            max_connections: None,
            migrate_url: migrate.then(|| "postgres://dataroom_owner@127.0.0.1:1/dataroom".into()),
            migrate_password_file: migrate.then_some(password),
            maintenance_url: None,
            maintenance_password_file: None,
        }
    }

    /// A refused server or an unreadable password file stops the start with a module error;
    /// no memory store stands in for the configured database.
    #[tokio::test]
    async fn a_database_that_cannot_be_reached_or_read_refuses_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        for migrate in [false, true] {
            let cfg = DataroomSection {
                database: Some(unreachable_database(tmp.path(), migrate)),
                ..DataroomSection::default()
            };
            let e = open_store(&cfg, &[])
                .await
                .err()
                .expect("refused")
                .to_string();
            assert!(
                e.contains("cannot connect to Postgres"),
                "migrate {migrate}: {e}"
            );
        }
        for migrate in [false, true] {
            let mut db = unreachable_database(tmp.path(), migrate);
            if migrate {
                db.migrate_password_file = Some(tmp.path().join("missing"));
            } else {
                db.password_file = tmp.path().join("missing");
            }
            let cfg = DataroomSection {
                database: Some(db),
                ..DataroomSection::default()
            };
            let e = open_store(&cfg, &[])
                .await
                .err()
                .expect("unreadable")
                .to_string();
            assert!(e.contains("dataroom"), "migrate {migrate}: {e}");
            assert!(!e.contains("cannot connect"), "nothing dialed: {e}");
        }
    }

    /// The catalogue of a Portal `name` of one Organization, its publisher admitted.
    fn portal_spec(org: u128, cat: u128, name: &str) -> dataroom_server::CatalogueSpec {
        let uuid = |n: u128| format!("00000000-0000-7000-8000-{n:012x}");
        let (org_id, id) = (
            OrgId::parse_verified(&uuid(org)).unwrap(),
            CatalogueId::parse_verified(&uuid(cat)).unwrap(),
        );
        dataroom_server::CatalogueSpec {
            org_id,
            id,
            name: name.into(),
            host: format!("{name}.city.portal.invalid"),
            base: Some(format!("https://data.city.test/city/opendata/-/{name}")),
            organization: None,
            publisher: serde_json::from_value(serde_json::json!({
                "orgId": org_id, "catalogueId": id, "id": "", "name": "city",
                "title": {"sk": "Mesto"}, "description": {}, "uri": "https://data.city.test/publisher/city",
                "state": "active", "editorGrants": [],
                "created": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
            }))
            .unwrap(),
            publishes: true,
            portal: None,
        }
    }

    /// One CKAN action on the catalogue at `host`, with `token` when given: its status.
    async fn action(
        router: &Router,
        host: &str,
        token: Option<&str>,
        call: &str,
        body: Option<serde_json::Value>,
    ) -> axum::http::StatusCode {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .uri(format!("/api/3/action/{call}"))
            .header("host", host);
        if let Some(token) = token {
            request = request.header("authorization", token);
        }
        let request = match body {
            Some(json) => request
                .method("POST")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(json.to_string())),
            None => request.body(axum::body::Body::empty()),
        }
        .unwrap();
        router.clone().oneshot(request).await.unwrap().status()
    }

    fn package(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name, "owner_org": "city", "title": "Air quality", "notes": "Hourly.",
            "private": false,
            "resources": [{"name": "CSV", "url": "https://data.city.test/air.csv", "format": "CSV"}]
        })
    }

    #[tokio::test]
    async fn a_catalogue_created_at_runtime_is_served_without_restart() {
        let (router, writer) = runtime_dataroom().await;
        let spec = portal_spec(0xA1, 0xC1, "mobility");
        let host = spec.host.clone();
        let before = action(&router, &host, None, "package_list", None).await;
        assert_eq!(before, axum::http::StatusCode::NOT_FOUND);
        writer.upsert(spec.clone()).await.unwrap();
        let after = action(&router, &host, None, "package_list", None).await;
        assert_eq!(
            after,
            axum::http::StatusCode::OK,
            "served by the same router"
        );
        let token = bearer();
        let token = Some(token.as_str());
        let made = action(
            &router,
            &host,
            token,
            "package_create",
            Some(package("air")),
        )
        .await;
        assert!(made.is_success(), "{made}");
        let shown = action(&router, &host, None, "package_show?id=air", None).await;
        assert_eq!(shown, axum::http::StatusCode::OK);
        writer
            .upsert(spec)
            .await
            .expect("an update of the same catalogue");
    }

    /// T-7186: a run-time Organization (a row jc.toml does not name) gets its catalogue under its
    /// own OrgId at its open data host without a restart, with the Portal's in-process grant; once
    /// the registry removes it the catalogue answers nothing, its datasets kept in the store.
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn a_run_time_organizations_catalogue_answers_and_a_removed_one_refuses() {
        use tower::ServiceExt;
        let (router, writer) = runtime_dataroom().await;
        let installation = OrgId::parse_verified(&DataroomSection::default().org_id).unwrap();
        let mut run = RunTime {
            writer,
            branding: Arc::default(),
            installation,
            default_catalogue: CatalogueId::parse_verified(
                &DataroomSection::default().catalogue_id,
            )
            .unwrap(),
            configured: BTreeMap::new(),
            own: BrandingBlock::default(),
            live: crate::siblings::LiveAddresses::default(),
            scheme: "https".into(),
            assistant_widget: None,
            publishes: true,
            applied: BTreeMap::new(),
        };
        let org = OrganizationSection {
            name: "rt7186".into(),
            id: None,
            hosts: crate::config::OrganizationHosts {
                portal: "smartcity.rt7186.sk".into(),
                opendata: Some("data.rt7186.sk".into()),
                ..Default::default()
            },
            host: None,
            same_site_apps: false,
            catalogue_id: "00000000-0000-7000-8000-000000007186".into(),
            portal_token_file: None,
            first_admin: None,
        };
        let decided = |name: &str, call: &str, token: Option<&str>| {
            let mut request = axum::http::Request::builder()
                .uri(format!("/api/3/action/{call}"))
                .header("host", "data.rt7186.sk");
            if let Some(token) = token {
                request = request
                    .method("POST")
                    .header("authorization", token)
                    .header("content-type", "application/json");
            }
            let body = match token {
                Some(_) => {
                    let mut air = package("air");
                    air["owner_org"] = "rt7186".into();
                    axum::body::Body::from(air.to_string())
                }
                None => axum::body::Body::empty(),
            };
            let mut request = request.body(body).unwrap();
            request.extensions_mut().insert(Mount {
                module: "dataroom".into(),
                scheme: "https".into(),
                host: "data.rt7186.sk".into(),
                path_prefix: String::new(),
            });
            request
                .extensions_mut()
                .insert(jc_router::Organization(name.into()));
            let router = router.clone();
            async move { router.oneshot(request).await.unwrap().status() }
        };
        let not_found = axum::http::StatusCode::NOT_FOUND;
        assert_eq!(
            decided("rt7186", "package_list", None).await,
            not_found,
            "no catalogue yet"
        );
        let set = |run_time: Vec<OrganizationSection>, removed: Vec<String>| {
            super::super::follow::Organizations { run_time, removed }
        };
        run.apply(&set(vec![org.clone()], Vec::new())).await;
        assert_eq!(
            decided("rt7186", "package_list", None).await,
            axum::http::StatusCode::OK
        );
        let token = bearer();
        assert_eq!(
            decided("rt7186", "package_create", Some(&token)).await,
            axum::http::StatusCode::OK,
            "the Portal publishes with a token of jc's in-process issuer"
        );
        let stranger = decided(
            "rt7186",
            "package_create",
            Some("not-the-token-0123456789abcdef"),
        )
        .await;
        assert!(
            matches!(
                stranger,
                axum::http::StatusCode::UNAUTHORIZED | axum::http::StatusCode::FORBIDDEN
            ),
            "another token publishes nothing: {stranger}"
        );
        // A row naming the default catalogue or none gets nothing.
        let mut twin = org.clone();
        twin.name = "twin".into();
        twin.catalogue_id = DataroomSection::default().catalogue_id;
        let mut bare = org.clone();
        bare.name = "bare".into();
        bare.catalogue_id = String::new();
        run.apply(&set(vec![org.clone(), twin, bare], Vec::new()))
            .await;
        assert_eq!(decided("twin", "package_list", None).await, not_found);
        assert_eq!(decided("bare", "package_list", None).await, not_found);
        // While rt7186 is served its catalogue id is taken for any other row; a fresh id is
        // served beside it.
        let mut thief = organization("thief7186", &org.catalogue_id);
        thief.hosts.opendata = Some("data.thief7186.sk".into());
        let other = organization("other7186", "00000000-0000-7000-8000-000000007187");
        run.apply(&set(vec![org.clone(), thief, other], Vec::new()))
            .await;
        assert!(run.applied.contains_key("other7186"), "a distinct id");
        assert!(!run.applied.contains_key("thief7186"), "another's id");
        assert!(
            run.applied.contains_key("rt7186"),
            "its own id stays its own"
        );
        let mut squatter = organization("squat7186", "00000000-0000-7000-8000-000000007188");
        squatter.hosts.opendata = Some("data.other7186.sk".into());
        run.apply(&set(vec![org.clone(), squatter], Vec::new()))
            .await;
        assert!(
            !run.applied.contains_key("squat7186"),
            "a host another catalogue holds"
        );
        run.apply(&set(Vec::new(), vec!["rt7186".into()])).await;
        assert_eq!(
            decided("rt7186", "package_list", None).await,
            not_found,
            "removed"
        );
        assert_eq!(
            decided("rt7186", "package_create", Some(&token)).await,
            not_found,
            "its grant went with it"
        );
        // The store kept it: served again (an import), the dataset is there.
        run.apply(&set(vec![org], Vec::new())).await;
        assert_eq!(
            decided("rt7186", "package_show?id=air", None).await,
            axum::http::StatusCode::OK
        );
    }

    /// A package as the Portal's publisher sends it for the Endpoint `slug` (`published`), or
    /// as a hand-made or harvested one (no `generated_by`), tagged `tag`.
    fn endpoint_package(name: &str, slug: &str, tag: &str, published: bool) -> serde_json::Value {
        let mut extras = vec![serde_json::json!({
            "key": "endpoint", "value": format!("https://api.city.test/api/endpoint/{slug}/")
        })];
        if published {
            extras
                .push(serde_json::json!({"key": "generated_by", "value": "jcctl/ckan-publisher"}));
        }
        let mut package = package(name);
        package["tags"] = serde_json::json!([{ "name": tag }]);
        package["extras"] = serde_json::Value::Array(extras);
        package
    }

    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn an_endpoint_without_publish_never_appears_through_a_tag_selection() {
        use crate::modules::portals::{self, EndpointRow, OrganizationPortals};
        let (router, writer) = runtime_dataroom().await;
        let default = portal_spec(0xA3, 0xC3, "city");
        let (host, token) = (default.host.clone(), Some(bearer()));
        let orgs = vec![OrganizationPortals {
            name: "city".into(),
            org_id: default.org_id,
            catalogue_id: default.id,
            scheme: "https".into(),
            opendata: host.clone(),
            listed: Vec::new(),
            harvest_hosts: Vec::new(),
            config_dir: None,
            own_gateway: None,
        }];
        writer.upsert(default).await.unwrap();
        for (name, tag, published) in [
            ("air", "mobility", true),
            ("bikes", "mobility", false),
            ("noise", "noise", true),
            // marked published by its extras, but its Endpoint manifest has no publish block
            ("forged", "mobility", true),
        ] {
            let package = endpoint_package(name, &format!("{name}slug"), tag, published);
            let made = action(
                &router,
                &host,
                token.as_deref(),
                "package_create",
                Some(package),
            );
            assert!(made.await.is_success(), "{name}");
        }
        let endpoints: Vec<EndpointRow> = ["air", "bikes", "noise", "parking", "forged"]
            .map(|n| EndpointRow {
                project: "doprava".into(),
                space: "mesto".into(),
                name: n.into(),
                slug: format!("{n}slug"),
                published: n != "forged",
            })
            .into();
        let mut mobility: jc_core::kinds::PortalSpec = serde_json::from_value(serde_json::json!({
            "organizationRef": "city", "path": "mobility",
            "sources": [{ "name": "tagged", "type": "endpoints", "tags": ["mobility"] }],
        }))
        .unwrap();
        let spec = portals::catalogue_of(&mobility, &orgs).unwrap();
        let further = spec.host.clone();
        writer.upsert(spec).await.unwrap();
        let mut served = BTreeMap::from([("city-mobility".to_owned(), mobility.clone())]);
        let mut listing = std::collections::BTreeSet::new();
        portals::list_endpoints(&writer, &orgs, &served, &endpoints, &mut listing).await;
        let shown = async |name: &str| {
            let call = format!("package_show?id={name}");
            action(&router, &further, None, &call, None).await
        };
        assert_eq!(
            shown("air").await,
            axum::http::StatusCode::OK,
            "published and tagged"
        );
        assert_eq!(
            shown("bikes").await,
            axum::http::StatusCode::NOT_FOUND,
            "tagged, but its Endpoint never published it"
        );
        assert_eq!(
            shown("noise").await,
            axum::http::StatusCode::NOT_FOUND,
            "another tag"
        );
        assert_eq!(shown("parking").await, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            shown("forged").await,
            axum::http::StatusCode::NOT_FOUND,
            "generated_by written by the caller, no publish block on the Endpoint"
        );
        mobility.sources.clear();
        served.insert("city-mobility".into(), mobility);
        portals::list_endpoints(&writer, &orgs, &served, &endpoints, &mut listing).await;
        assert_eq!(
            shown("air").await,
            axum::http::StatusCode::NOT_FOUND,
            "a source taken away withdraws its copies"
        );
        assert!(listing.is_empty());
    }

    #[tokio::test]
    async fn a_deleted_catalogues_dataset_answers_410() {
        let (router, writer) = runtime_dataroom().await;
        let spec = portal_spec(0xA2, 0xC2, "mobility");
        let (host, token) = (spec.host.clone(), Some(bearer()));
        let (org, id) = (spec.org_id, spec.id);
        writer.upsert(spec).await.unwrap();
        let made = action(
            &router,
            &host,
            token.as_deref(),
            "package_create",
            Some(package("air")),
        )
        .await;
        assert!(made.is_success(), "{made}");
        assert!(writer.delete(org, id).await.unwrap());
        let shown = action(&router, &host, None, "package_show?id=air", None).await;
        assert_eq!(shown, axum::http::StatusCode::GONE);
        let listed = action(&router, &host, None, "package_list", None).await;
        assert_eq!(listed, axum::http::StatusCode::GONE);
        let again = action(
            &router,
            &host,
            token.as_deref(),
            "package_create",
            Some(package("new")),
        )
        .await;
        assert_eq!(
            again,
            axum::http::StatusCode::GONE,
            "nothing is written into it"
        );
        // Another catalogue still admits the publisher.
        let next = portal_spec(0xA2, 0xC9, "schools");
        writer.upsert(next).await.expect("another catalogue");
        assert!(
            !writer
                .delete(
                    org,
                    CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000ee").unwrap()
                )
                .await
                .unwrap()
        );
    }

    /// One GET on `path` as jc's host router hands it on: `host`'s mount, Organization `org`.
    async fn routed(
        router: &Router,
        org: &str,
        host: &str,
        path: &str,
    ) -> axum::response::Response {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .uri(path)
            .header("host", host)
            .body(axum::body::Body::empty())
            .unwrap();
        let mount = jc_router::Mount::new("dataroom", "https", host, "").unwrap();
        request.extensions_mut().insert(mount);
        request
            .extensions_mut()
            .insert(jc_router::Organization(org.to_owned()));
        router.clone().oneshot(request).await.unwrap()
    }

    async fn text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Organization `city`'s `default` at `data.city.test` with dataset `air`, and its further
    /// portal `path` at its slug address (path mode) with dataset `air` of its own: the writer.
    async fn city_with_portal(
        path: &str,
    ) -> (
        Router,
        dataroom_server::CatalogueWriter,
        dataroom_server::CatalogueSpec,
    ) {
        let (router, writer) = runtime_dataroom().await;
        let mut default = portal_spec(0xB1, 0xD1, "city");
        default.host = "data.city.test".into();
        default.base = Some("https://data.city.test".into());
        default.organization = Some("city".into());
        writer.upsert(default.clone()).await.unwrap();
        let mut further = portal_spec(0xB1, 0xD2, path);
        further.host = dataroom_api::synthetic_host("city", path);
        further.base = Some(format!("https://data.city.test/-/{path}"));
        further.portal = Some(dataroom_server::PortalRoute {
            organization: "city".into(),
            path: path.into(),
            host: None,
            redirect: true,
        });
        writer.upsert(further.clone()).await.unwrap();
        for spec in [&default, &further] {
            let made = action(
                &router,
                &spec.host,
                Some(bearer().as_str()),
                "package_create",
                Some(package("air")),
            )
            .await;
            assert!(made.is_success(), "{made}");
        }
        (router, writer, further)
    }

    /// `spec` on its own host `host`, with the slug address redirecting or kept.
    fn on_host(
        mut spec: dataroom_server::CatalogueSpec,
        host: &str,
        redirect: bool,
    ) -> dataroom_server::CatalogueSpec {
        spec.base = Some(format!("https://{host}"));
        spec.host = host.to_owned();
        let route = spec.portal.as_mut().expect("a further portal's route");
        route.host = Some(host.to_owned());
        route.redirect = redirect;
        spec
    }

    #[tokio::test]
    async fn the_slug_address_of_a_domain_portal_redirects_308_with_path_and_query() {
        let (router, writer, mobility) = city_with_portal("mobility").await;
        writer
            .upsert(on_host(mobility, "mobility.city.test", true))
            .await
            .unwrap();
        let moved = routed(
            &router,
            "city",
            "data.city.test",
            "/-/mobility/dataset/air?lang=sk&x=1",
        )
        .await;
        assert_eq!(moved.status(), axum::http::StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            moved.headers()["location"],
            "https://mobility.city.test/dataset/air?lang=sk&x=1"
        );
        let home = routed(&router, "city", "data.city.test", "/-/mobility").await;
        assert_eq!(home.headers()["location"], "https://mobility.city.test/");
        let served = routed(
            &router,
            "city",
            "mobility.city.test",
            "/api/3/action/package_show?id=air",
        )
        .await;
        assert_eq!(
            served.status(),
            axum::http::StatusCode::OK,
            "the domain serves it"
        );
    }

    #[tokio::test]
    async fn slug_keep_serves_both_and_names_the_domain_canonical() {
        let (router, writer, mobility) = city_with_portal("mobility").await;
        let token = Some(bearer());
        writer
            .upsert(on_host(mobility, "mobility.city.test", false))
            .await
            .unwrap();
        // A dataset published on the domain is named by the domain, on both addresses.
        let made = action(
            &router,
            "mobility.city.test",
            token.as_deref(),
            "package_create",
            Some(package("bus")),
        )
        .await;
        assert!(made.is_success(), "{made}");
        for (host, path) in [
            ("data.city.test", "/-/mobility/catalog.ttl"),
            ("mobility.city.test", "/catalog.ttl"),
        ] {
            let feed = routed(&router, "city", host, path).await;
            assert_eq!(feed.status(), axum::http::StatusCode::OK, "{host}{path}");
            let feed = text(feed).await;
            assert!(
                feed.contains("<https://mobility.city.test/dataset/bus>"),
                "{host}{path}: {feed}"
            );
            assert!(
                !feed.contains("data.city.test/-/mobility/dataset/bus"),
                "{host}{path}"
            );
        }
    }

    #[tokio::test]
    async fn an_address_change_keeps_every_dataset_iri() {
        let (router, writer, mobility) = city_with_portal("mobility").await;
        let iri = "<https://data.city.test/-/mobility/dataset/air>";
        let before =
            text(routed(&router, "city", "data.city.test", "/-/mobility/catalog.ttl").await).await;
        assert!(before.contains(iri), "{before}");
        writer
            .upsert(on_host(mobility.clone(), "mobility.city.test", false))
            .await
            .unwrap();
        let after = text(routed(&router, "city", "mobility.city.test", "/catalog.ttl").await).await;
        assert!(after.contains(iri), "the dataset keeps its IRI: {after}");
        // And back to the slug address: the IRI still does not move, the old host still answers.
        writer.upsert(mobility).await.unwrap();
        let back =
            text(routed(&router, "city", "data.city.test", "/-/mobility/catalog.ttl").await).await;
        assert!(back.contains(iri), "{back}");
        let old = routed(
            &router,
            "city",
            "mobility.city.test",
            "/api/3/action/package_show?id=air",
        )
        .await;
        assert_eq!(old.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn a_portal_named_like_a_default_path_still_cannot_shadow_it() {
        let (router, _writer, _) = city_with_portal("dataset").await;
        let own = text(routed(&router, "city", "data.city.test", "/catalog.ttl").await).await;
        assert!(
            own.contains("<https://data.city.test/dataset/air>"),
            "{own}"
        );
        assert!(!own.contains("/-/dataset/"), "default lists its own: {own}");
        let portal =
            text(routed(&router, "city", "data.city.test", "/-/dataset/catalog.ttl").await).await;
        assert!(
            portal.contains("<https://data.city.test/-/dataset/dataset/air>"),
            "{portal}"
        );
        let none = routed(&router, "city", "data.city.test", "/-/nosuch/catalog.ttl").await;
        assert_eq!(none.status(), axum::http::StatusCode::NOT_FOUND);
        let other = routed(&router, "town", "data.city.test", "/-/dataset/catalog.ttl").await;
        assert_eq!(
            other.status(),
            axum::http::StatusCode::NOT_FOUND,
            "another Organization's"
        );
    }

    #[tokio::test]
    async fn a_catalogue_that_admits_no_publisher_refuses_its_token() {
        let (router, writer) = runtime_dataroom().await;
        let (x, mut y) = (
            portal_spec(0xA3, 0xC3, "mobility"),
            portal_spec(0xA3, 0xC4, "schools"),
        );
        y.publishes = false;
        writer.upsert(x.clone()).await.unwrap();
        writer.upsert(y.clone()).await.unwrap();
        let foreign = action(
            &router,
            &y.host,
            Some(bearer().as_str()),
            "package_create",
            Some(package("evil")),
        )
        .await;
        assert_eq!(foreign.as_u16(), 401, "Y admits no publisher: {foreign}");
        let shown = action(&router, &y.host, None, "package_show?id=evil", None).await;
        assert_eq!(shown, axum::http::StatusCode::NOT_FOUND);
        let own = action(
            &router,
            &x.host,
            Some(bearer().as_str()),
            "package_create",
            Some(package("air")),
        )
        .await;
        assert!(own.is_success(), "{own}");
    }

    #[tokio::test]
    async fn a_catalogue_id_stays_with_its_organization_after_a_delete_and_under_a_race() {
        let (router, writer) = runtime_dataroom().await;
        let a = portal_spec(0xA6, 0xC6, "mobility");
        let b = portal_spec(0xB6, 0xC6, "schools");
        writer.upsert(a.clone()).await.unwrap();
        assert!(writer.delete(a.org_id, a.id).await.unwrap());
        assert!(
            matches!(
                writer.upsert(b.clone()).await,
                Err(DataroomError::Conflict(_))
            ),
            "B cannot take A's deleted catalogue id"
        );
        let shown = action(&router, &b.host, None, "package_list", None).await;
        assert_eq!(
            shown,
            axum::http::StatusCode::NOT_FOUND,
            "B's host serves nothing"
        );

        let (c, d) = (
            portal_spec(0xA7, 0xC7, "culture"),
            portal_spec(0xB7, 0xC7, "sport"),
        );
        let (rc, rd) = tokio::join!(writer.upsert(c.clone()), writer.upsert(d.clone()));
        assert!(
            rc.is_ok() != rd.is_ok(),
            "exactly one Organization holds the id"
        );
        let (won, lost) = if rc.is_ok() { (&c, &d) } else { (&d, &c) };
        let made = action(
            &router,
            &won.host,
            Some(bearer().as_str()),
            "package_create",
            Some(package("air")),
        )
        .await;
        assert!(
            made.is_success(),
            "the winner's principal still writes: {made}"
        );
        let lost_list = action(&router, &lost.host, None, "package_list", None).await;
        assert_eq!(lost_list, axum::http::StatusCode::NOT_FOUND);
    }

    /// prod's open data catalogue: the base its datasets' `isPartOf` names.
    const SITE: &str = "https://opendata.smartcity.joinedcontext.com";
    const GATEWAY: &str = "https://api.smartcity.joinedcontext.com";

    /// prod's two sources as the installation's `default` Portal holds them (docs/
    /// default-portal.yaml: what T-6880's migration wrote from the `[[dataroom.harvest]]` block).
    #[cfg(feature = "portal")]
    fn prod_portal() -> jc_core::kinds::PortalSpec {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let text = std::fs::read_to_string(docs.join("default-portal.yaml")).unwrap();
        let read =
            jc_core::envelope::ResourceEnvelope::<jc_core::kinds::PortalSpec>::from_yaml(&text)
                .expect("the manifest parses");
        read.validate().expect("the manifest is valid");
        read.spec
    }

    /// The installation Organization `ops` of catalogue `ctx`, at prod's open data host, whose
    /// sources read `convertedFile` in `config_dir` and may fetch every host `portal` names.
    #[cfg(feature = "portal")]
    fn ops(
        ctx: &RequestCtx,
        portal: &jc_core::kinds::PortalSpec,
        config_dir: std::path::PathBuf,
        own_gateway: Option<&str>,
    ) -> Vec<super::super::portals::OrganizationPortals> {
        use super::super::portals::{OrganizationPortals, named};
        let hosts = portal.sources.iter().flat_map(|s| s.allowed_hosts.clone());
        named(
            &[OrganizationPortals::installation(OrganizationPortals {
                name: String::new(),
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                scheme: "https".into(),
                opendata: SITE.trim_start_matches("https://").into(),
                listed: Vec::new(),
                harvest_hosts: hosts.collect(),
                config_dir: Some(config_dir),
                own_gateway: own_gateway.map(str::to_owned),
            })],
            Some("ops".into()),
        )
    }

    /// The harvest sources of `portal` written as the Portal's reconciler writes them, each
    /// with its publisher.
    #[cfg(feature = "portal")]
    async fn hold(
        store: &MemoryStore,
        ctx: &RequestCtx,
        portal: &jc_core::kinds::PortalSpec,
        orgs: &[super::super::portals::OrganizationPortals],
    ) -> Vec<HarvestSource> {
        let mut held = Vec::new();
        for (source, publisher) in super::super::portals::harvest_sources_of(portal, orgs).unwrap()
        {
            store.upsert_publisher(ctx, publisher).await.unwrap();
            held.push(store.upsert_harvest_source(ctx, source).await.unwrap());
        }
        held
    }

    /// The gateway on jc's own listener (slug mode, the rehearsal) is read there; one on a host
    /// of its own, or an api address naming another host, is read through the guard.
    #[test]
    fn the_own_gateway_is_reached_on_loopback_only_when_jc_serves_it_there() {
        use crate::siblings::{LiveAddresses, Sibling};
        let listener: std::net::SocketAddr = "127.0.0.1:42102".parse().unwrap();
        let with = |public: &str, internal: &str| {
            let mut s = Siblings::new(&[], listener);
            s.insert_remote(
                Module::Gateway,
                Sibling::direct(public.into(), internal.into()),
            );
            s
        };
        let live = |api: &str| LiveAddresses {
            api: api.into(),
            ..LiveAddresses::default()
        };
        let slug = with(
            "http://localhost:42102/gateway",
            "http://127.0.0.1:42102/gateway",
        );
        assert_eq!(
            own_gateway(&live("http://localhost:42102"), &slug),
            Some(("http://localhost:42102".into(), Some(listener)))
        );
        let literal = with(
            "http://127.0.0.1:42102/gateway",
            "http://127.0.0.1:42102/gateway",
        );
        assert_eq!(
            own_gateway(&live("http://127.0.0.1:42102"), &literal),
            Some(("http://127.0.0.1:42102".into(), Some(listener)))
        );
        let host = with("https://api.city.example", "https://api.city.example");
        assert_eq!(
            own_gateway(&live("https://api.city.example"), &host),
            Some(("https://api.city.example".into(), None))
        );
        assert_eq!(
            own_gateway(&live("https://api.other.example"), &slug),
            Some(("https://api.other.example".into(), None))
        );
        let elsewhere = with("http://localhost:42102", "http://10.0.0.5:8080");
        assert_eq!(
            own_gateway(&live("http://localhost:42102"), &elsewhere),
            Some(("http://localhost:42102".into(), None))
        );
        assert_eq!(own_gateway(&live(""), &slug), None);
        // No gateway in this jc (role split): the named api address, on loopback only as a literal.
        let apart = Siblings::new(&[], listener);
        assert_eq!(own_gateway(&live(""), &apart), None);
        assert_eq!(
            own_gateway(&live("https://api.city.example"), &apart),
            Some(("https://api.city.example".into(), None))
        );
        assert_eq!(
            own_gateway(&live("http://127.0.0.1:42262"), &apart),
            Some((
                "http://127.0.0.1:42262".into(),
                Some("127.0.0.1:42262".parse().unwrap())
            ))
        );
        assert_eq!(
            own_gateway(&live("http://10.0.0.5:8080"), &apart),
            Some(("http://10.0.0.5:8080".into(), None))
        );
    }

    /// Without a gateway of their own, prod's sources keep prod's converted datasets and read
    /// their Endpoints on this instance's gateway, which no outbound check refuses for being
    /// jc's own loopback listener (gate 4: no converted filter on the rehearsal).
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn prods_sources_read_their_converted_datasets_on_the_own_gateway() {
        let own = "http://localhost:42102";
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let (_, ctx) = catalogue().await;
        let mut portal = prod_portal();
        let orgs = ops(&ctx, &portal, docs, Some(own));
        let sources = super::super::portals::harvest_sources_of(&portal, &orgs).unwrap();
        assert_eq!(sources.len(), 2);
        for (src, _) in sources {
            assert_eq!(src.gateway.as_deref(), Some(own), "{}", src.name);
            assert_eq!(src.converted.len(), 139, "{}", src.name);
            let entry = &src.converted["bbsk-kontaktne-miesta-2128a0"];
            assert_eq!(
                entry.dataset,
                format!("{SITE}/dataset/bbsk-kontaktne-miesta-2128a0")
            );
        }
        // Without converted datasets the own gateway is not taken.
        for source in &mut portal.sources {
            source.converted_file = None;
        }
        for (src, _) in super::super::portals::harvest_sources_of(&portal, &orgs).unwrap() {
            assert_eq!(src.gateway, None, "{}", src.name);
        }
    }

    async fn catalogue() -> (MemoryStore, RequestCtx) {
        let d = DataroomSection::default();
        let org = OrgId::parse_verified(&d.org_id).unwrap();
        let cat = CatalogueId::parse_verified(&d.catalogue_id).unwrap();
        let store = MemoryStore::new();
        dataroom_server::ensure_default_catalogue(&store, org, cat, "opendata.example")
            .await
            .expect("catalogue");
        let (subject, _) = dataroom_harvest::harvest_principal(org, cat);
        (store, RequestCtx::new(org, cat, subject))
    }

    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn prods_two_sources_hold_their_publishers_and_only_their_hosts() {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let portal = prod_portal();
        let (store, ctx) = catalogue().await;
        let orgs = ops(&ctx, &portal, docs, None);
        let held = hold(&store, &ctx, &portal, &orgs).await;
        assert_eq!(
            held.iter()
                .map(|s| (
                    s.name.as_str(),
                    serde_json::to_value(s.kind).unwrap(),
                    s.schedule.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("nkod-banskabystrica", "nkod-sparql".into(), "23 3 * * *"),
                ("hub-bbsk", "arcgis-hub".into(), "23 3 * * *")
            ]
        );
        for src in &held {
            assert!(src.enabled);
            let publisher = store
                .get_publisher(&ctx, &src.publisher_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(Some(&publisher.uri), src.target_publisher_iri.as_ref());
            assert!(
                publisher.title.contains_key("sk"),
                "{}: the publisher keeps the seeds' Slovak",
                src.name
            );
            // The source's own host is admitted; any other host, an internal one included, is not.
            validate_outbound_url(&src.url, &src.allowed_hosts, &[], false).expect("own host");
            for refused in [
                "https://data.gov.sk/api/sparql",
                "https://evil.example/x",
                "https://127.0.0.1/x",
                "https://opendata.smartcity.joinedcontext.com/api/3/action/package_list",
                "http://data.slovensko.sk/api/sparql",
            ] {
                assert!(
                    validate_outbound_url(refused, &src.allowed_hosts, &[], false).is_err(),
                    "{}: {refused}",
                    src.name
                );
            }
        }
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 2);
    }

    /// A source reading a further host off the deployment's allow-list (ODP-10), a schedule that is not
    /// cron and a kind dataroom does not run are refused before anything is written.
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn a_source_off_the_allow_list_or_off_cron_is_refused() {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let portal = prod_portal();
        let (_, ctx) = catalogue().await;
        let mut orgs = ops(&ctx, &portal, docs.clone(), None);
        // The url's own host needs no operator listing; a further host still does.
        orgs[0].harvest_hosts.retain(|h| h != "data.slovensko.sk");
        super::super::portals::harvest_sources_of(&portal, &orgs).expect("own host admitted");
        orgs[0]
            .harvest_hosts
            .retain(|h| h != "egov.banskabystrica.sk");
        let off_list = super::super::portals::harvest_sources_of(&portal, &orgs).unwrap_err();
        assert!(off_list.contains("egov.banskabystrica.sk"), "{off_list}");
        let mut bad_cron = portal.clone();
        bad_cron.sources[1].schedule = Some("nightly".into());
        assert!(bad_cron.validate().is_err());
        let text = std::fs::read_to_string(docs.join("default-portal.yaml")).unwrap();
        let bad_kind = text.replace("type: arcgis-hub", "type: ftp");
        assert!(
            jc_core::envelope::ResourceEnvelope::<jc_core::kinds::PortalSpec>::from_yaml(&bad_kind)
                .is_err()
        );
    }

    /// Both sources run once against what their hosts answered on 2026-11-09 (recorded fixtures,
    /// `tests/fixtures/harvest/`: NKOD's SPARQL answer, the Hub's two search pages, prod's
    /// `od-datasets.json` and its gateway's `schema/index.json` of each Endpoint): every fetch
    /// goes to an allow-listed host, and the datasets prod's CKAN serves for the two publishers
    /// are among those listed, each with prod's distributions.
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn prods_sources_harvest_from_recorded_answers() {
        let (store, ctx) = catalogue().await;
        let fetcher = MapFetcher::new();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harvest");
        let mut portal = prod_portal();
        for source in &mut portal.sources {
            assert_eq!(source.converted_file.as_deref(), Some("od-datasets.json"));
            source.gateway = Some(GATEWAY.into());
            source
                .allowed_hosts
                .push("api.smartcity.joinedcontext.com".into());
        }
        let orgs = ops(&ctx, &portal, fixtures, None);
        let [nkod, hub]: [HarvestSource; 2] =
            hold(&store, &ctx, &portal, &orgs).await.try_into().unwrap();
        fetcher.insert(
            dataroom_harvest::runner::nkod_query_url(&nkod).unwrap(),
            Some("application/sparql-results+json"),
            include_str!("../../tests/fixtures/harvest/nkod-banskabystrica.json"),
        );
        let pages: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/harvest/hub-bbsk.json"))
                .unwrap();
        for page in pages {
            fetcher.insert(
                page["url"].as_str().unwrap(),
                Some("application/json"),
                page["body"].to_string(),
            );
        }
        let indexes: BTreeMap<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/harvest/model-index.json"
        ))
        .unwrap();
        for (endpoint, index) in indexes {
            fetcher.insert(
                format!("{GATEWAY}/api/endpoint/{endpoint}/schema/index.json"),
                Some("application/json"),
                index.to_string(),
            );
        }
        // prod's package_show of each dataset it serves (2026-11-09T18:5xZ): the resource count
        // and the sheet, a CSV named by the entity type that its harvest writes when the
        // Endpoint answers the dataset's `file.csv`. Each file answers here for a dataset whose
        // sheet prod holds; for the other two (their file is the whole Endpoint) none answers.
        // Every file answers 503 once first, as the gateway does while its download slots are
        // full (jc's start, the Portal probing each Endpoint): prod's read asks again.
        let prod_resources: BTreeMap<String, serde_json::Value> = serde_json::from_str(
            include_str!("../../tests/fixtures/harvest/prod-resources.json"),
        )
        .unwrap();
        let mut files = BTreeSet::new();
        for (name, shown) in &prod_resources {
            let entry = [&nkod, &hub]
                .into_iter()
                .find_map(|source| source.converted.get(name))
                .expect("every prod resource is converted");
            let file = dataroom_harvest::converted::sheet_address(
                &format!("{GATEWAY}/api/endpoint/{}/", entry.endpoint),
                entry,
            );
            if shown["sheet"].as_array().is_some_and(|s| !s.is_empty()) {
                fetcher.insert(file.clone(), Some("text/csv"), "id,type\n");
            }
            if files.insert(file.clone()) {
                fetcher.insert_status(file, 503);
            }
        }
        let engine = HarvestEngine::new(Arc::new(fetcher.clone()))
            .with_custom_dns(|_, port| {
                Ok(vec![std::net::SocketAddr::from(([93, 184, 216, 34], port))])
            })
            .with_retry_waits(vec![std::time::Duration::ZERO]);
        for src in [&nkod, &hub] {
            let run = engine.run_source(&ctx, &store, src).await.expect("run");
            assert_eq!(run.state, HarvestRunState::Succeeded);
            eprintln!(
                "{}: created {} skipped {}",
                src.name,
                run.created_count,
                run.skipped.len()
            );
        }
        let hosts: BTreeSet<String> = fetcher.pinned_seen().into_iter().map(|(h, _)| h).collect();
        assert_eq!(
            hosts,
            BTreeSet::from([
                "api.smartcity.joinedcontext.com".to_owned(),
                "data.slovensko.sk".to_owned(),
                "opendata.bbsk.sk".to_owned()
            ])
        );
        let datasets = store.list_datasets(&ctx).await.unwrap();
        let listed: BTreeMap<&str, &dataroom_core::Dataset> =
            datasets.iter().map(|d| (d.name.as_str(), d)).collect();
        // What prod's CKAN answered for every converted name of the two publishers on 2026-11-09:
        // its organisation (served), 403 (the publish gate hides it) or 404.
        let prod: BTreeMap<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/harvest/prod-presence.json"
        ))
        .unwrap();
        let served: Vec<(&String, &str)> = prod
            .iter()
            .filter_map(|(name, org)| org.as_str().map(|o| (name, o)))
            .collect();
        assert_eq!(served.len(), 17);
        for (name, org) in &served {
            let ds = listed
                .get(name.as_str())
                .expect("every served dataset is listed");
            assert_eq!(&ds.publisher_id, org, "{name}");
            assert!(
                ds.endpoint_url
                    .as_deref()
                    .is_some_and(|u| u.starts_with(GATEWAY)),
                "{name}"
            );
            // prod's resources, the sheet last; the dataset is the catalogue's own (its DCAT
            // IRI), the source its landing page.
            let shown = &prod_resources[name.as_str()];
            assert_eq!(
                ds.distributions.len() as u64,
                shown["resources"].as_u64().unwrap(),
                "{name}"
            );
            let last = ds.distributions.last().unwrap();
            let sheet = shown["sheet"].get(0).and_then(serde_json::Value::as_str);
            assert_eq!(
                sheet.is_some(),
                last.title.get("sk").map(String::as_str) == sheet,
                "{name}"
            );
            assert_eq!(ds.canonical_uri(SITE), format!("{SITE}/dataset/{name}"));
            assert_eq!(
                ds.extras.get("harvest_source").map(String::as_str),
                Some(if *org == "bbsk" {
                    "hub-bbsk"
                } else {
                    "nkod-banskabystrica"
                })
            );
        }
        let more: Vec<&&str> = listed
            .keys()
            .filter(|n| !prod.get(**n).is_some_and(|o| o.is_string()))
            .collect();
        eprintln!(
            "listed {} of which prod serves {}; listed beyond prod's served: {more:?}",
            listed.len(),
            served.len()
        );
        // Nothing but converted datasets: each listed name is one prod's mapping converted.
        assert!(listed.keys().all(|n| prod.contains_key(*n)), "{more:?}");
        // prod's publish gate hides the rest (`[dataroom.hidden]`, prod's chart value): what a
        // visitor sees is what prod serves, no more.
        seed_hidden(&store, ctx.org_id, ctx.catalogue_id, &prod_hidden())
            .await
            .unwrap();
        let cat = store
            .get_catalogue(ctx.org_id, ctx.catalogue_id)
            .await
            .unwrap()
            .unwrap();
        let shown: BTreeSet<&str> = listed
            .keys()
            .copied()
            .filter(|n| !cat.hidden_datasets.contains_key(*n))
            .collect();
        assert_eq!(
            shown,
            served
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<BTreeSet<_>>()
        );
    }

    /// prod's publisher badges and words, as jc.toml carries them (`[dataroom.publishers]`).
    fn prod_publishers() -> DataroomSection {
        #[derive(serde::Deserialize)]
        struct File {
            dataroom: DataroomSection,
        }
        let raw = include_str!("../../docs/dataroom-publishers.toml");
        toml::from_str::<File>(raw)
            .expect("the block parses")
            .dataroom
    }

    #[test]
    fn prods_publisher_block_reaches_every_catalogue_as_badges_and_words() {
        let cfg = prod_publishers();
        let own = configured_branding(&cfg).expect("prod's block is accepted");
        assert_eq!(own.publishers.len(), 8);
        let praha = &own.publishers["praha"];
        assert_eq!(praha.initials.as_deref(), Some("PHA"));
        assert_eq!(praha.colour.as_deref(), Some("#b4232a"));
        assert!(praha.description["sk"].starts_with("Otvorené údaje hlavného mesta Prahy"));
        // Old prod gave no words for banskabystrica and bbsk: six publishers have them.
        let worded = own
            .publishers
            .values()
            .filter(|p| !p.description.is_empty())
            .count();
        assert_eq!(worded, 6);
        let live = crate::siblings::LiveAddresses::default();
        assert_eq!(
            installation_branding(&own, &live, None).publishers,
            own.publishers
        );
    }

    #[test]
    fn a_publisher_or_logo_entry_that_would_not_draw_is_refused_at_start() {
        let seed = |initials: &str, colour: &str| {
            let mut cfg = DataroomSection::default();
            cfg.publishers.insert(
                "praha".into(),
                crate::config::PublisherSeed {
                    initials: initials.into(),
                    colour: colour.into(),
                    description: BTreeMap::new(),
                },
            );
            configured_branding(&cfg)
        };
        assert!(seed("PHA", "#b4232a").is_ok());
        for (initials, colour) in [
            ("", "#b4232a"),
            ("PRAHA", "#b4232a"),
            ("PHA", "red"),
            ("PHA", "#b4232a\"/>"),
        ] {
            let e = seed(initials, colour).expect_err(initials);
            assert!(e.starts_with("[dataroom.publishers.praha]"), "{e}");
        }
        let logo = |logo: Option<&str>, svg: Option<&str>| {
            let mut cfg = DataroomSection::default();
            cfg.branding.logo = logo.map(str::to_owned);
            cfg.branding.favicon = logo.map(str::to_owned);
            cfg.branding.logo_svg = svg.map(str::to_owned);
            configured_branding(&cfg)
        };
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><path d="M0 0h1v1z"/></svg>"#;
        let ok = logo(Some("logo.svg"), Some(svg)).expect("logo.svg with its SVG");
        assert_eq!(
            (ok.logo.as_str(), ok.favicon.as_str()),
            ("logo.svg", "logo.svg")
        );
        assert_eq!(ok.logo_svg.as_deref(), Some(svg));
        assert!(logo(Some("jc-logo.svg"), None).is_ok());
        assert!(
            logo(Some("logo.svg"), None).is_err(),
            "no file serves logo.svg"
        );
        assert!(
            logo(Some("other.png"), Some(svg)).is_err(),
            "no route serves other.png"
        );
        let e = logo(None, Some(r#"<svg onload="alert(1)"></svg>"#)).unwrap_err();
        assert!(e.contains("logo_svg"), "{e}");
    }

    /// prod's publish-gate hidden list, as jc.toml carries it (`[dataroom.hidden]`).
    /// An Organization's catalogue (UIX-16, T-6777): its appearance over jc's config member by
    /// member; without one, or with a logo the upload would refuse, jc's config shows.
    #[cfg(feature = "portal")]
    #[test]
    fn an_organizations_catalogue_shows_its_appearance_over_jcs_config() {
        let cfg = prod_publishers();
        let configured = configured_branding(&cfg).expect("prod's block is accepted");
        assert_eq!(with_appearance(configured.clone(), None), configured);
        let logo = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1"/></svg>"##;
        let look: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "instanceName": "Banskobystrický kraj",
                "publisher": "BBSK",
                "colours": { "primary": "#0b5394" },
                "logoSvg": logo,
            }))
            .expect("an appearance");
        let shown = with_appearance(configured.clone(), Some(&look));
        assert_eq!(shown.instance_name, "Banskobystrický kraj");
        assert_eq!(
            shown.short_name, "Banskobystrický kraj",
            "the full name stands in"
        );
        assert_eq!(shown.organisation, "BBSK");
        assert_eq!(shown.colours.primary, "#0b5394");
        assert_eq!(
            shown.primary_rgb, "11, 83, 148",
            "derived values follow the colour"
        );
        assert_eq!(
            shown.colours.secondary, configured.colours.secondary,
            "left out: jc's"
        );
        assert_eq!(shown.city, configured.city);
        assert_eq!(shown.logo_svg.as_deref(), Some(logo));
        assert_eq!(shown.logo, "logo.svg");
        let scripted = jc_core::kinds::OrganizationAppearance {
            logo_svg: Some("<svg><script>alert(1)</script></svg>".into()),
            ..look
        };
        assert_eq!(
            with_appearance(configured.clone(), Some(&scripted)).logo_svg,
            configured.logo_svg,
            "a logo the upload refuses keeps jc's"
        );
        // T-7031: its page texts by page and language; none keeps every built-in text.
        assert!(shown.pages.is_empty());
        let worded: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "pages": [{ "page": "connector", "lang": "sk", "markdown": "# Konektor mesta" }],
            }))
            .expect("an appearance with a page");
        let shown = with_appearance(configured.clone(), Some(&worded));
        assert_eq!(
            shown
                .pages
                .get(&("connector".to_owned(), "sk".to_owned()))
                .map(String::as_str),
            Some("# Konektor mesta")
        );
        assert_eq!(shown.pages.len(), 1);
    }

    /// T-7410: the theme and layout members reach the catalogue's block; the introduction is
    /// rendered safe, and a colour below WCAG AA on jc's background gives way to jc's colours.
    /// T-7744: a portal's tokens reach its catalogue over the Organization's; a pair that
    /// meets the theme's other colour below WCAG AA gives way, the rest still shows.
    #[cfg(feature = "portal")]
    #[test]
    fn style_tokens_reach_the_catalogue_and_unreadable_pairs_give_way() {
        let (log, _guard) = super::super::capture_log();
        let look = |v: serde_json::Value| -> jc_core::kinds::OrganizationAppearance {
            serde_json::from_value(v).expect("an appearance")
        };
        let org = look(
            serde_json::json!({ "colours": { "link": "#1d4ed8" }, "width": "wide", "font": "serif" }),
        );
        let portal = look(serde_json::json!({
            "colours": { "headerBackground": "#0b5394", "headerText": "#ffffff", "footerText": "#ffffff" },
            "fontSize": 17, "buttonStyle": "outline", "cardShadow": "none", "bodyFont": "mono",
        }));
        let shown = with_appearance(BrandingBlock::default(), Some(&portal.over(&org)));
        let t = &shown.layout.tokens;
        assert_eq!(t.link.as_deref(), Some("#1d4ed8"));
        assert_eq!(t.header_background.as_deref(), Some("#0b5394"));
        assert_eq!(t.header_text.as_deref(), Some("#ffffff"));
        assert_eq!(
            t.footer_text.as_deref(),
            Some("#ffffff"),
            "white on the theme's dark footer"
        );
        assert_eq!(
            (t.font_size_px, t.width),
            (Some(17), Some(jc_core::kinds::ContentWidth::Wide))
        );
        assert!(t.button_outline && t.card_flat);
        assert!(
            shown.fonts.heading.starts_with("Charter"),
            "the Organization's font for headings"
        );
        assert!(
            shown.fonts.body.starts_with("ui-monospace"),
            "the portal's body font"
        );
        // White header text on the theme's white page: the pair gives way, the rest shows.
        let pale =
            look(serde_json::json!({ "colours": { "headerText": "#ffffff" }, "fontSize": 15 }));
        let shown = with_appearance(BrandingBlock::default(), Some(&pale));
        assert_eq!(shown.layout.tokens.header_text, None);
        assert_eq!(shown.layout.tokens.font_size_px, Some(15));
        let logged = std::fs::read_to_string(log.path()).unwrap();
        assert!(logged.contains("colour pair below WCAG AA"), "{logged}");
        // A theme whose own header pair is unreadable and a look setting no colour of it: the
        // theme's shows and nothing is logged, the look is not at fault.
        let mut glaring = BrandingBlock::default();
        glaring.colours.text = "#ffffff".into();
        let before = std::fs::read_to_string(log.path()).unwrap();
        let shown = with_appearance(glaring, Some(&look(serde_json::json!({ "fontSize": 16 }))));
        assert_eq!(shown.layout.tokens.header_text, None);
        assert_eq!(shown.layout.tokens.font_size_px, Some(16));
        assert_eq!(std::fs::read_to_string(log.path()).unwrap(), before);
        // Reset: no tokens on the portal or its Organization shows the theme's.
        let reset = with_appearance(
            BrandingBlock::default(),
            Some(&jc_core::kinds::OrganizationAppearance::default()),
        );
        assert_eq!(
            reset.layout.tokens,
            dataroom_core::branding::BrandingTokens::default()
        );
    }

    /// T-7743: a display resolves Portal > Organization > installation > built-in list by
    /// list, and a Portal's display removed again (reset) shows the page as before.
    #[cfg(feature = "portal")]
    #[test]
    fn a_display_cascades_over_four_levels_and_resets() {
        let display = |v: serde_json::Value| -> jc_core::kinds::OrganizationAppearance {
            serde_json::from_value(serde_json::json!({ "display": v })).expect("a display")
        };
        assert!(
            with_appearance(BrandingBlock::default(), None)
                .display
                .is_unset(),
            "built-in"
        );
        let installation = BrandingBlock::from_json_lossy(&serde_json::json!({
            "display": { "dataset": { "about": ["license", "publisher"] }, "datasetList": { "fields": ["format"] } }
        }));
        let org = display(
            serde_json::json!({ "dataset": { "sections": [{ "name": "about" }, { "name": "map" }] } }),
        );
        let portal = display(
            serde_json::json!({ "dataset": { "sections": [{ "name": "resources" }] }, "datasetList": { "fields": ["modified"] } }),
        );
        let at_org = with_appearance(installation.clone(), Some(&org));
        assert_eq!(
            at_org.display.dataset.sections,
            org.display.dataset.sections
        );
        assert_eq!(at_org.display.dataset.about, ["license", "publisher"]);
        let at_portal = with_appearance(installation.clone(), Some(&portal.over(&org)));
        assert_eq!(
            at_portal.display.dataset.sections,
            portal.display.dataset.sections
        );
        assert_eq!(at_portal.display.dataset.about, ["license", "publisher"]);
        assert_eq!(at_portal.display.dataset_list.fields, ["modified"]);
        let reset = jc_core::kinds::OrganizationAppearance::default();
        assert_eq!(
            with_appearance(installation, Some(&reset.over(&org))),
            at_org
        );
    }

    #[cfg(feature = "portal")]
    #[test]
    fn an_organizations_theme_and_layout_reach_its_catalogue_safely() {
        let configured = BrandingBlock::default();
        let look: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "font": "serif",
                "radius": 4,
                "colours": { "primary": "#7a1f5c" },
                "hero": { "title": "Dáta mesta", "intro": "# Vitajte\n\nDáta <script>alert(1)</script> [x](javascript:alert(1)) [y](https://mesto.sk)" },
                "datasetList": "table",
                "navLinks": [{ "label": "Mesto", "url": "https://mesto.sk" }],
                "footerLinks": [{ "label": "API", "url": "/about#api" }],
            }))
            .expect("an appearance");
        look.validate().expect("valid");
        let shown = with_appearance(configured.clone(), Some(&look));
        assert!(
            shown.fonts.body.starts_with("Charter"),
            "{}",
            shown.fonts.body
        );
        assert_eq!(shown.fonts.heading, shown.fonts.body);
        assert_eq!(shown.colours.primary, "#7a1f5c");
        assert_eq!(shown.layout.radius_px, Some(4));
        assert_eq!(shown.layout.hero_title.as_deref(), Some("Dáta mesta"));
        let intro = shown.layout.hero_intro_html.as_deref().expect("rendered");
        assert!(intro.starts_with("<h2>Vitajte</h2>"), "{intro}");
        assert!(!intro.contains("<script"), "{intro}");
        assert!(!intro.contains("javascript:"), "{intro}");
        assert!(intro.contains("https://mesto.sk"), "{intro}");
        assert!(shown.layout.dataset_table);
        assert_eq!(
            shown.layout.nav_links,
            vec![("https://mesto.sk".to_owned(), "Mesto".to_owned())]
        );
        assert_eq!(
            shown.layout.footer_links,
            vec![("/about#api".to_owned(), "API".to_owned())]
        );
        // Defence in depth: a link that skipped validate() still never reaches the template.
        let unchecked: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "navLinks": [{ "label": "x", "url": "javascript:alert(1)" }, { "label": "Mesto", "url": "https://mesto.sk" }],
                "footerLinks": [{ "label": "y", "url": "//evil.example" }],
            }))
            .expect("an appearance");
        let shown_unchecked = with_appearance(configured.clone(), Some(&unchecked));
        assert_eq!(
            shown_unchecked.layout.nav_links,
            vec![("https://mesto.sk".to_owned(), "Mesto".to_owned())]
        );
        assert!(shown_unchecked.layout.footer_links.is_empty());
        // Text alone passes jc-core (no background of its own) yet is unreadable on jc's white.
        let pale: jc_core::kinds::OrganizationAppearance =
            serde_json::from_value(serde_json::json!({
                "instanceName": "Mesto",
                "colours": { "text": "#eeeeee" },
            }))
            .expect("an appearance");
        pale.validate()
            .expect("one colour of a pair passes jc-core");
        let (log, _guard) = super::super::capture_log();
        let shown = with_appearance(configured.clone(), Some(&pale));
        let logged = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            logged.contains("below WCAG AA on the background"),
            "{logged}"
        );
        assert_eq!(
            shown.colours, configured.colours,
            "jc's colours keep it readable"
        );
        assert_eq!(shown.instance_name, "Mesto", "the rest still shows");
    }

    /// T-7410: each catalogue follows its own Organization's look; a `kind: Portal` look goes
    /// over its Organization's in that portal's catalogue only, never another Organization's.
    #[cfg(feature = "portal")]
    #[test]
    fn a_look_reaches_only_its_own_organizations_catalogues() {
        use joinedcontext_portal::resource::ResourceEnvelope;
        let ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
        let envelope = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: jc_core::ObjectMeta::new(name, ns),
            spec,
            status: None,
        };
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        let org = |name: &str, cat: &str| super::super::portals::OrganizationPortals {
            name: name.into(),
            org_id: OrgId::of_organization(installation, name),
            catalogue_id: CatalogueId::parse_verified(cat).unwrap(),
            scheme: "https".into(),
            opendata: format!("data.{name}.test"),
            listed: Vec::new(),
            harvest_hosts: Vec::new(),
            config_dir: None,
            own_gateway: None,
        };
        let portals = vec![
            org("city", "00000000-0000-7000-8000-0000000000c1"),
            org("town", "00000000-0000-7000-8000-0000000000c2"),
        ];
        let catalogue: BTreeMap<String, CatalogueId> = portals
            .iter()
            .map(|o| (o.name.clone(), o.catalogue_id))
            .collect();
        let configured: BTreeMap<CatalogueId, BrandingBlock> = portals
            .iter()
            .map(|o| (o.catalogue_id, BrandingBlock::default()))
            .collect();
        let mirror = joinedcontext_portal::store::Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            "city",
            serde_json::json!({ "domain": "city.test", "appearance": {
                "colours": { "primary": "#0b5394" }, "hero": { "title": "City data" },
            }}),
        ));
        mirror.upsert(envelope(
            "Organization",
            "town",
            serde_json::json!({ "domain": "town.test" }),
        ));
        mirror.upsert(envelope(
            "Portal",
            "city-mobility",
            serde_json::json!({ "organizationRef": "city", "path": "mobility",
                "appearance": { "colours": { "primary": "#7a1f5c" } } }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        let (city, town) = (portals[0].catalogue_id, portals[1].catalogue_id);
        let mobility = CatalogueId::of_portal(portals[0].org_id, "mobility");
        assert_eq!(looks.len(), 3, "{:?}", looks.keys());
        assert_eq!(looks[&city].colours.primary, "#0b5394");
        assert_eq!(looks[&town], BrandingBlock::default(), "town sets nothing");
        assert_eq!(
            looks[&mobility].colours.primary, "#7a1f5c",
            "the portal's own"
        );
        assert_eq!(
            looks[&mobility].layout.hero_title.as_deref(),
            Some("City data"),
            "left out: its Organization's"
        );
        mirror.upsert(envelope(
            "Organization",
            "city",
            serde_json::json!({ "domain": "city.test" }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        assert_eq!(
            looks[&city],
            BrandingBlock::default(),
            "a removed look is jc's again"
        );
        // T-7433: a portal's pages reach its own catalogue alone, over its look, and leave with
        // them; town's catalogue never shows city's pages.
        let home = serde_json::json!([{ "slug": "home", "rows": [{ "blocks": [
            { "type": "numbers", "items": ["datasets"] }] }] }]);
        mirror.upsert(envelope(
            "Portal",
            "city-default",
            serde_json::json!({ "organizationRef": "city", "path": "default", "pages": home }),
        ));
        mirror.upsert(envelope(
            "Portal",
            "city-mobility",
            serde_json::json!({ "organizationRef": "city", "path": "mobility", "pages": home,
                "appearance": { "colours": { "primary": "#7a1f5c" } } }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        assert!(looks[&city].layout.page("home").is_some());
        assert!(looks[&mobility].layout.page("home").is_some());
        assert_eq!(
            looks[&mobility].colours.primary, "#7a1f5c",
            "pages keep the look"
        );
        assert!(
            looks[&town].layout.pages.is_empty(),
            "never another Organization's"
        );
        mirror.upsert(envelope(
            "Portal",
            "city-default",
            serde_json::json!({ "organizationRef": "city", "path": "default" }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        assert!(looks[&city].layout.pages.is_empty(), "removed pages leave");
        // T-7745: a portal's stylesheet reaches its own catalogue alone, scoped to its class; the
        // Organization's default portal and town's catalogue link none.
        mirror.upsert(envelope(
            "Portal",
            "city-mobility",
            serde_json::json!({ "organizationRef": "city", "path": "mobility",
                "appearance": { "css": ".site-header { background: #7a1f5c }" } }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        let sheet = looks[&mobility]
            .layout
            .stylesheet
            .as_ref()
            .expect("its own");
        assert_eq!(
            sheet.class,
            jc_core::kinds::stylesheet::portal_class(&mobility.to_string())
        );
        assert!(
            sheet
                .css
                .starts_with(&format!("body.{} .site-header", sheet.class))
        );
        assert!(
            looks[&city].layout.stylesheet.is_none(),
            "not the default portal's"
        );
        assert!(
            looks[&town].layout.stylesheet.is_none(),
            "not another Organization's"
        );
        const { assert!(super::super::appearance::FOLLOW_LOOK_EVERY.as_secs() < 10) };
    }

    /// What no catalogue holds gets no look: a portal of an Organization jc serves no catalogue
    /// for, an Organization no portal row names, one without a configured block; a portal's
    /// pages without a look of its own go over its Organization's.
    #[cfg(feature = "portal")]
    #[test]
    fn looks_of_what_no_catalogue_holds_go_nowhere() {
        use joinedcontext_portal::resource::ResourceEnvelope;
        let ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
        let envelope = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: jc_core::ObjectMeta::new(name, ns),
            spec,
            status: None,
        };
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        let org = |name: &str, cat: &str| super::super::portals::OrganizationPortals {
            name: name.into(),
            org_id: OrgId::of_organization(installation, name),
            catalogue_id: CatalogueId::parse_verified(cat).unwrap(),
            scheme: "https".into(),
            opendata: format!("data.{name}.test"),
            listed: Vec::new(),
            harvest_hosts: Vec::new(),
            config_dir: None,
            own_gateway: None,
        };
        // ghost has portals but no catalogue here; lonely a catalogue but no portal row; bare a
        // catalogue and portals but no configured block.
        let portals = vec![
            org("city", "00000000-0000-7000-8000-0000000000c1"),
            org("ghost", "00000000-0000-7000-8000-0000000000c3"),
            org("bare", "00000000-0000-7000-8000-0000000000c5"),
        ];
        let id = |cat: &str| CatalogueId::parse_verified(cat).unwrap();
        let (city, lonely, bare) = (
            id("00000000-0000-7000-8000-0000000000c1"),
            id("00000000-0000-7000-8000-0000000000c4"),
            id("00000000-0000-7000-8000-0000000000c5"),
        );
        let catalogue = BTreeMap::from([
            ("city".to_owned(), city),
            ("lonely".to_owned(), lonely),
            ("bare".to_owned(), bare),
        ]);
        let configured = BTreeMap::from([
            (city, BrandingBlock::default()),
            (lonely, BrandingBlock::default()),
        ]);
        let mirror = joinedcontext_portal::store::Mirror::new();
        let fields = serde_json::json!([{ "name": "funding", "label": { "sk": "Financovanie" },
            "type": "enum", "values": ["eu", "city"] }]);
        mirror.upsert(envelope(
            "Organization",
            "city",
            serde_json::json!({ "domain": "city.test", "installation": true,
                "appearance": { "colours": { "primary": "#0b5394" } } }),
        ));
        for name in ["ghost", "bare", "lonely"] {
            mirror.upsert(envelope(
                "Organization",
                name,
                serde_json::json!({ "domain": format!("{name}.test"), "datasetFields": fields }),
            ));
        }
        let home = serde_json::json!([{ "slug": "home", "rows": [{ "blocks": [
            { "type": "numbers", "items": ["datasets"] }] }] }]);
        let everything = |org: &str, path: &str| {
            serde_json::json!({ "organizationRef": org, "path": path, "pages": home,
                "appearance": { "colours": { "primary": "#7a1f5c" },
                    "css": ".site-header { background: #7a1f5c }" } })
        };
        mirror.upsert(envelope("Portal", "ghost-x", everything("ghost", "x")));
        mirror.upsert(envelope("Portal", "lonely-y", everything("lonely", "y")));
        mirror.upsert(envelope(
            "Portal",
            "bare-z",
            serde_json::json!({ "organizationRef": "bare", "path": "z", "pages": home }),
        ));
        mirror.upsert(envelope(
            "Portal",
            "city-plain",
            serde_json::json!({ "organizationRef": "city", "path": "plain", "pages": home }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        let city_org = OrgId::of_organization(installation, "city");
        let plain = CatalogueId::of_portal(city_org, "plain");
        assert_eq!(
            looks.keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([city, lonely, plain]),
            "ghost's, bare's and their portals' catalogues get nothing"
        );
        assert_eq!(looks[&city].colours.primary, "#0b5394");
        let theme = BrandingBlock::default();
        assert_eq!(
            looks[&lonely].colours, theme.colours,
            "no portal look reaches it"
        );
        assert!(
            looks[&lonely].layout.pages.is_empty() && looks[&lonely].layout.stylesheet.is_none()
        );
        assert_eq!(
            looks[&lonely].dataset_fields.organization, "lonely",
            "its own fields"
        );
        assert!(
            looks[&plain].layout.page("home").is_some(),
            "pages over the look"
        );
        assert_eq!(
            looks[&plain].colours.primary, "#0b5394",
            "its Organization's look"
        );
    }

    /// One round of the appearance follower writes what changed into the shared branding and
    /// takes out a catalogue whose look went; an unchanged round writes nothing.
    #[cfg(feature = "portal")]
    #[test]
    fn the_follower_writes_changed_looks_and_takes_out_gone_ones() {
        use joinedcontext_portal::resource::ResourceEnvelope;
        let ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
        let envelope = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: jc_core::ObjectMeta::new(name, ns),
            spec,
            status: None,
        };
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        let city = CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c1").unwrap();
        let portals = vec![super::super::portals::OrganizationPortals {
            name: "city".into(),
            org_id: OrgId::of_organization(installation, "city"),
            catalogue_id: city,
            scheme: "https".into(),
            opendata: "data.city.test".into(),
            listed: Vec::new(),
            harvest_hosts: Vec::new(),
            config_dir: None,
            own_gateway: None,
        }];
        let catalogue = BTreeMap::from([("city".to_owned(), city)]);
        let configured = BTreeMap::from([(city, BrandingBlock::default())]);
        let state = joinedcontext_portal::state::AppState::new(
            joinedcontext_portal::config::Config::for_tests(),
            None,
        );
        let mirror = &state.mirror;
        mirror.upsert(envelope(
            "Organization",
            "city",
            serde_json::json!({ "domain": "city.test" }),
        ));
        let portal = |look: bool| {
            let mut spec = serde_json::json!({ "organizationRef": "city", "path": "mobility" });
            if look {
                spec["appearance"] = serde_json::json!({ "colours": { "primary": "#7a1f5c" } });
            }
            envelope("Portal", "city-mobility", spec)
        };
        mirror.upsert(portal(true));
        let shared = std::sync::RwLock::new(BTreeMap::new());
        let mut seen = BTreeMap::new();
        let mut seen_alone = BTreeMap::new();
        apply_looks(
            None,
            (&catalogue, &portals, &configured),
            &shared,
            &mut seen_alone,
        );
        assert!(
            shared.read().unwrap().is_empty() && seen_alone.is_empty(),
            "no Portal, no look"
        );
        let round = |seen: &mut BTreeMap<CatalogueId, BrandingBlock>| {
            apply_looks(
                Some(&state),
                (&catalogue, &portals, &configured),
                &shared,
                seen,
            );
        };
        let mobility = CatalogueId::of_portal(portals[0].org_id, "mobility");
        round(&mut seen);
        assert_eq!(shared.read().unwrap()[&mobility].colours.primary, "#7a1f5c");
        assert_eq!(seen.len(), 2);
        shared.write().unwrap().clear();
        round(&mut seen);
        assert!(
            shared.read().unwrap().is_empty(),
            "an unchanged round writes nothing"
        );
        mirror.upsert(portal(false));
        shared
            .write()
            .unwrap()
            .insert(mobility, BrandingBlock::default());
        round(&mut seen);
        assert!(
            !shared.read().unwrap().contains_key(&mobility),
            "a gone look leaves"
        );
        assert!(!seen.contains_key(&mobility));
    }

    /// A catalogue's dashboard cards come from its Organization in the Portal's mirror; a
    /// catalogue of no Organization, or no Portal in the process, has none.
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn dashboard_cards_need_an_organization_and_a_portal() {
        let city = CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c1").unwrap();
        let other = CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c2").unwrap();
        let organization = BTreeMap::from([(city, "city".to_owned())]);
        let portal = joinedcontext_portal::state::AppState::new(
            joinedcontext_portal::config::Config::for_tests(),
            None,
        );
        assert!(dashboard_cards_of(&organization, None, &city, "sk").is_empty());
        assert!(dashboard_cards_of(&organization, Some(&portal), &other, "sk").is_empty());
        assert!(
            dashboard_cards_of(&organization, Some(&portal), &city, "sk").is_empty(),
            "an Organization with no dashboard in the mirror shows none"
        );
    }

    /// The catalogue's assistant widget is the page of the assistant module beside it, for the
    /// deployment named; none without that module.
    #[test]
    fn the_assistant_widget_is_the_page_of_the_assistant_beside_it() {
        let mut siblings = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        assert_eq!(assistant_widget(&siblings, "city"), None);
        siblings.insert_remote(
            Module::Assistant,
            crate::siblings::Sibling::direct(
                "https://assistant.city.test".into(),
                "http://assistant:8080".into(),
            ),
        );
        assert_eq!(
            assistant_widget(&siblings, "city").as_deref(),
            Some("https://assistant.city.test/d/city/widget")
        );
    }

    /// A heading or a body font set alone goes over that one, the other keeps the theme's.
    #[cfg(feature = "portal")]
    #[test]
    fn a_heading_or_body_font_alone_sets_only_its_own() {
        let look = |v: serde_json::Value| -> jc_core::kinds::OrganizationAppearance {
            serde_json::from_value(v).expect("an appearance")
        };
        let theme = BrandingBlock::default();
        let heading = with_appearance(
            theme.clone(),
            Some(&look(serde_json::json!({ "headingFont": "serif" }))),
        );
        assert_ne!(heading.fonts.heading, theme.fonts.heading);
        assert_eq!(heading.fonts.body, theme.fonts.body);
        let body = with_appearance(
            theme.clone(),
            Some(&look(serde_json::json!({ "bodyFont": "mono" }))),
        );
        assert_eq!(body.fonts.heading, theme.fonts.heading);
        assert_ne!(body.fonts.body, theme.fonts.body);
    }

    /// T-7746: each catalogue carries its own Organization's dataset fields under the
    /// installation's namespace, a portal's catalogue too; another Organization's never.
    #[cfg(feature = "portal")]
    #[test]
    fn dataset_fields_reach_only_their_own_organizations_catalogues() {
        use joinedcontext_portal::resource::ResourceEnvelope;
        let ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
        let envelope = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
            api_version: jc_core::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: jc_core::ObjectMeta::new(name, ns),
            spec,
            status: None,
        };
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        let org = |name: &str, cat: &str| super::super::portals::OrganizationPortals {
            name: name.into(),
            org_id: OrgId::of_organization(installation, name),
            catalogue_id: CatalogueId::parse_verified(cat).unwrap(),
            scheme: "https".into(),
            opendata: format!("data.{name}.test"),
            listed: Vec::new(),
            harvest_hosts: Vec::new(),
            config_dir: None,
            own_gateway: None,
        };
        let portals = vec![
            org("bbsk", "00000000-0000-7000-8000-0000000000c1"),
            org("town", "00000000-0000-7000-8000-0000000000c2"),
        ];
        let catalogue: BTreeMap<String, CatalogueId> = portals
            .iter()
            .map(|o| (o.name.clone(), o.catalogue_id))
            .collect();
        let configured: BTreeMap<CatalogueId, BrandingBlock> = portals
            .iter()
            .map(|o| (o.catalogue_id, BrandingBlock::default()))
            .collect();
        let mirror = joinedcontext_portal::store::Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            "bbsk",
            serde_json::json!({ "domain": "bbsk.sk", "installation": true, "datasetFields": [
                { "name": "funding", "label": { "sk": "Financovanie" }, "type": "enum",
                  "values": ["eu", "city"] },
                { "name": "reviewed", "label": { "sk": "Revízia" }, "type": "date" }] }),
        ));
        mirror.upsert(envelope(
            "Organization",
            "town",
            serde_json::json!({ "domain": "town.test" }),
        ));
        mirror.upsert(envelope(
            "Portal",
            "bbsk-doprava",
            serde_json::json!({ "organizationRef": "bbsk", "path": "doprava" }),
        ));
        let looks = catalogue_looks(&mirror, &catalogue, &portals, &configured);
        let (bbsk, town) = (portals[0].catalogue_id, portals[1].catalogue_id);
        let doprava = CatalogueId::of_portal(portals[0].org_id, "doprava");
        for id in [bbsk, doprava] {
            let fields = &looks[&id].dataset_fields;
            assert_eq!(fields.namespace, "https://bbsk.sk/ns/dataset-fields/bbsk/");
            assert_eq!(
                fields
                    .fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>(),
                ["funding", "reviewed"]
            );
        }
        assert!(
            looks
                .get(&town)
                .is_none_or(|b| b.dataset_fields.fields.is_empty()),
            "never another Organization's"
        );
    }

    fn prod_hidden() -> BTreeMap<String, String> {
        #[derive(serde::Deserialize)]
        struct File {
            dataroom: DataroomSection,
        }
        let raw = include_str!("../../docs/catalogue-hidden.toml");
        toml::from_str::<File>(raw)
            .expect("the block parses")
            .dataroom
            .hidden
            .expect("[dataroom.hidden]")
    }

    /// The default catalogue of `DataroomSection::default()` stored on `host`, and an
    /// Organization's catalogue on `other.city.example` beside it.
    async fn stored_default(host: &str) -> (MemoryStore, OrgId, CatalogueId, CatalogueId) {
        let d = DataroomSection::default();
        let org = OrgId::parse_verified(&d.org_id).unwrap();
        let cat = CatalogueId::parse_verified(&d.catalogue_id).unwrap();
        let other = CatalogueId::parse_verified("6f1c0b2e-5d3a-4c1e-9a8b-7e2d4f6a1b3c").unwrap();
        let store = MemoryStore::new();
        dataroom_server::ensure_default_catalogue(&store, org, cat, host)
            .await
            .unwrap();
        dataroom_server::ensure_catalogue(&store, org, other, "bbsk", "other.city.example")
            .await
            .unwrap();
        (store, org, cat, other)
    }

    async fn host_of(store: &MemoryStore, org: OrgId, cat: CatalogueId) -> String {
        store.get_catalogue(org, cat).await.unwrap().unwrap().host
    }

    /// jc-prod 2026-11-10: the import window left the default catalogue on `localhost:8080`, so
    /// its own host answered 404. A host-mode start moves it to the bare mount host, keeps its
    /// name and leaves every other catalogue as it was.
    #[tokio::test]
    async fn a_host_mode_start_moves_the_default_catalogue_off_localhost() {
        let (store, org, cat, other) = stored_default("localhost:8080").await;
        hold_default_host(&store, org, cat, "opendata.city.example")
            .await
            .unwrap();
        assert_eq!(host_of(&store, org, cat).await, "opendata.city.example");
        let held = store.resolve_host("opendata.city.example").await.unwrap();
        assert_eq!(held.map(|c| (c.id, c.name)), Some((cat, "default".into())));
        assert_eq!(host_of(&store, org, other).await, "other.city.example");
    }

    /// A slug-mode or loopback start never writes a loopback host over a stored real one, and
    /// still creates the default catalogue when the store has none.
    #[tokio::test]
    async fn a_loopback_start_leaves_a_stored_real_host() {
        for mount in [
            "localhost:8080",
            "localhost",
            "dataroom.localhost:42102",
            "127.0.0.1:42102",
            "[::1]:8080",
        ] {
            let (store, org, cat, _) = stored_default("opendata.city.example").await;
            hold_default_host(&store, org, cat, mount).await.unwrap();
            assert_eq!(
                host_of(&store, org, cat).await,
                "opendata.city.example",
                "{mount}"
            );
            let empty = MemoryStore::new();
            hold_default_host(&empty, org, cat, mount).await.unwrap();
            assert_eq!(host_of(&empty, org, cat).await, mount);
        }
    }

    /// A stored host with a port never resolves on Postgres (its lookup compares the stored host
    /// with the request's, port dropped): it is moved to the mount host, port dropped.
    #[tokio::test]
    async fn a_stored_host_with_a_port_is_moved_to_the_bare_mount_host() {
        let (store, org, cat, _) = stored_default("opendata.city.example:8080").await;
        hold_default_host(&store, org, cat, "opendata.city.example:443")
            .await
            .unwrap();
        assert_eq!(host_of(&store, org, cat).await, "opendata.city.example");
        assert!(
            store
                .resolve_host("opendata.city.example")
                .await
                .unwrap()
                .is_some()
        );
    }

    /// Decided (supervisor, owner may override): the configured host wins over the one
    /// `catalogue.propose` set on the default catalogue.
    #[tokio::test]
    async fn the_configured_host_replaces_a_proposed_one() {
        let (log, _guard) = super::super::capture_log();
        let (store, org, cat, _) = stored_default("opendata.city.example").await;
        let mut proposed = store.get_catalogue(org, cat).await.unwrap().unwrap();
        proposed.host = "proposed.example".into();
        store.upsert_catalogue(proposed).await.unwrap();
        hold_default_host(&store, org, cat, "opendata.city.example")
            .await
            .unwrap();
        assert_eq!(host_of(&store, org, cat).await, "opendata.city.example");
        hold_default_host(&store, org, cat, "opendata.city.example")
            .await
            .unwrap();
        let logged = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(
            logged.matches("default catalogue moved").count(),
            1,
            "the same host again moves nothing: {logged}"
        );
        assert!(
            logged.contains("default catalogue moved")
                && logged.contains("from=proposed.example")
                && logged.contains("to=opendata.city.example"),
            "{logged}"
        );
    }

    #[tokio::test]
    async fn a_hidden_list_for_a_catalogue_that_is_not_there_is_refused() {
        let (store, ctx) = catalogue().await;
        let other = CatalogueId::parse_verified("00000000-0000-7000-8000-00000000dead").unwrap();
        let hidden = BTreeMap::from([("a".to_owned(), "gate".to_owned())]);
        let e = seed_hidden(&store, ctx.org_id, other, &hidden)
            .await
            .unwrap_err();
        assert!(matches!(e, DataroomError::NotFound(_)), "{e}");
    }

    #[cfg(feature = "portal")]
    #[test]
    fn converted_datasets_move_to_the_site_and_a_bad_file_names_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("od-datasets.json");
        let e = converted_datasets(&path, None, "https://data.city.test")
            .unwrap_err()
            .to_string();
        assert!(e.contains("converted_file"), "{e}");
        std::fs::write(&path, "{not json").unwrap();
        let e = converted_datasets(&path, None, "https://data.city.test")
            .unwrap_err()
            .to_string();
        assert!(e.contains("converted_file"), "{e}");
        std::fs::write(
            &path,
            r#"{"air": {"endpoint": "air", "type": "AirQualityObserved",
                "dataset": "https://old.example/dataset/air"}}"#,
        )
        .unwrap();
        let moved = converted_datasets(
            &path,
            Some("https://old.example/"),
            "https://data.city.test/",
        )
        .unwrap();
        assert_eq!(moved["air"].dataset, "https://data.city.test/dataset/air");
        assert!(moved["air"].filtered, "the default holds");
        for base in [None, Some("")] {
            let kept = converted_datasets(&path, base, "https://data.city.test").unwrap();
            assert_eq!(
                kept["air"].dataset, "https://old.example/dataset/air",
                "{base:?}"
            );
        }
    }

    #[cfg(feature = "portal")]
    #[test]
    fn the_hero_intro_keeps_a_heading_as_level_two_and_shows_raw_html_as_text() {
        let with = hero_intro("# Data <b>&</b> city\n\nOpen *data*.");
        assert!(with.starts_with("<h2>"), "{with}");
        assert!(!with.contains("<b>"), "{with}");
        assert!(with.contains("<em>data</em>"), "{with}");
        let without = hero_intro("Just <script>x</script> text.");
        assert!(!without.contains("<h2>"), "{without}");
        assert!(!without.contains("<script>"), "{without}");
    }

    /// The Organization's CKAN publisher is made once: a second start finds it and keeps it.
    #[tokio::test]
    async fn the_organizations_publisher_is_made_once() {
        let (store, ctx) = catalogue().await;
        let org = organization("city7782", "00000000-0000-7000-8000-000000007782");
        ensure_publisher(&store, ctx.org_id, ctx.catalogue_id, &org, "https")
            .await
            .unwrap();
        let get = || async {
            store
                .get_publisher(&ctx, "city7782")
                .await
                .unwrap()
                .expect("the publisher")
        };
        let made = get().await;
        ensure_publisher(&store, ctx.org_id, ctx.catalogue_id, &org, "http")
            .await
            .unwrap();
        assert_eq!(get().await, made, "the stored one is kept");
    }

    #[tokio::test]
    async fn the_hidden_list_is_written_over_the_stored_one() {
        let (store, ctx) = catalogue().await;
        let one = BTreeMap::from([("a".to_owned(), "gate".to_owned())]);
        seed_hidden(&store, ctx.org_id, ctx.catalogue_id, &one)
            .await
            .unwrap();
        let get = || async {
            store
                .get_catalogue(ctx.org_id, ctx.catalogue_id)
                .await
                .unwrap()
                .unwrap()
                .hidden_datasets
        };
        assert_eq!(get().await, one);
        let modified = || async {
            store
                .get_catalogue(ctx.org_id, ctx.catalogue_id)
                .await
                .unwrap()
                .unwrap()
                .modified
        };
        let before = modified().await;
        seed_hidden(&store, ctx.org_id, ctx.catalogue_id, &one)
            .await
            .unwrap();
        assert_eq!(modified().await, before, "the same list writes nothing");
        seed_hidden(&store, ctx.org_id, ctx.catalogue_id, &BTreeMap::new())
            .await
            .unwrap();
        assert!(get().await.is_empty());
        assert_eq!(prod_hidden().len(), 100);
    }
}
