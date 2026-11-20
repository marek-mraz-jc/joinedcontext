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
use crate::config::{DataroomSection, OrganizationSection};
use crate::module::Module;
use crate::setup::Secret;
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
    let (tokens, setup, siblings) = (&ctx.secrets.organization_tokens, ctx.setup, ctx.siblings);
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    dataroom_api::check_aliases(&cfg.aliases)
        .map_err(|e| err(&format!("[dataroom.aliases] {e}")))?;
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    // ODP-12: each Organization's catalogue lives under its own OrgId, so row level security and
    // the cross-organization harvest check keep Organizations apart; the deployment-wide
    // catalogue stays under `[dataroom] org_id`, the installation's own.
    let mut own_org = BTreeMap::new();
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
        own_org.insert(org.name.clone(), own);
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
    let (mut grants, mut bindings) = match setup.value(Secret::CatalogueToken) {
        None => (Vec::new(), Vec::new()),
        Some(token) => {
            let (binding, grant) =
                dataroom_api::portal_publisher(token, org_id, catalogue_id).map_err(|e| err(&e))?;
            (vec![grant], vec![binding])
        }
    };
    let own = configured_branding(cfg).map_err(|e| err(&e))?;
    let live = crate::siblings::live_addresses(&cfg.addresses, siblings);
    let mut by_catalogue = BTreeMap::new();
    for org in organizations {
        // Distinct from the default catalogue: checked in the loop above.
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?;
        let tenant = own_org[&org.name];
        let host = bare_host(org.hosts.opendata_host());
        dataroom_server::ensure_catalogue(store.as_ref(), tenant, id, &org.name, host)
            .await
            .map_err(|e| err(&e))?;
        ensure_publisher(store.as_ref(), tenant, id, org, &mount.scheme)
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
            let (binding, grant) = dataroom_api::portal_publisher_on(token, tenant, id, &org.name)
                .map_err(|e| err(&e))?;
            bindings.push(binding);
            grants.push(grant);
        }
        by_catalogue.insert(
            id,
            organization_branding(&own, &live, org, &mount.scheme, assistant_widget.clone()),
        );
    }
    let mut catalogues = vec![(org_id, catalogue_id)];
    for org in organizations {
        catalogues.push((
            own_org[&org.name],
            CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?,
        ));
    }
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
    // Each Organization's catalogue, by the name jc's host registry decides, and its canonical
    // address: the Organization's open data host, whichever registered host reads it.
    let mut by_organization = BTreeMap::new();
    let mut bases = BTreeMap::new();
    for org in organizations {
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?;
        by_organization.insert(org.name.clone(), (own_org[&org.name], id));
        bases.insert(id, format!("{}://{}", mount.scheme, org.opendata_base()));
    }
    #[cfg(feature = "portal")]
    let by_configured = by_organization.clone();
    #[cfg(feature = "portal")]
    let followed = portal_orgs(
        cfg,
        organizations,
        &own_org,
        &mount,
        own_gateway.as_ref().map(|(public, _)| public.clone()),
    );
    let state = DataroomState::new(store, authorizer.clone(), org_id, catalogue_id)
        .with_harvester(engine)
        .with_organization_catalogues(by_organization)
        .with_catalogue_bases(bases)
        .with_tokens(bindings)
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
    let state = with_dashboards(organizations, state).map_err(|e| err(&e))?;
    #[cfg(feature = "portal")]
    follow_appearances(
        organizations,
        followed.clone(),
        configured,
        &state,
        shutdown,
        tasks,
    )
    .map_err(|e| err(&e))?;
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

/// What the Portal reconciler serves: each `[[organizations]]` entry, or without any the
/// installation's Organization, whose `default` portal is the `[dataroom]` catalogue at the
/// dataroom's own address (ODP-09).
#[cfg(feature = "portal")]
fn portal_orgs(
    cfg: &DataroomSection,
    organizations: &[OrganizationSection],
    own_org: &BTreeMap<String, OrgId>,
    mount: &Mount,
    own_gateway: Option<String>,
) -> Vec<super::portals::OrganizationPortals> {
    let default_catalogue = CatalogueId::parse_verified(&cfg.catalogue_id).ok();
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
        let (Ok(org_id), Some(catalogue_id)) =
            (OrgId::parse_verified(&cfg.org_id), default_catalogue)
        else {
            return Vec::new();
        };
        let base = mount.public_base();
        let opendata = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest);
        return vec![super::portals::OrganizationPortals::installation(entry(
            "",
            org_id,
            catalogue_id,
            opendata.trim_end_matches('/').to_owned(),
            Vec::new(),
        ))];
    }
    organizations
        .iter()
        .filter_map(|org| {
            let catalogue_id = CatalogueId::parse_verified(&org.catalogue_id)
                .ok()
                .or(default_catalogue)?;
            Some(entry(
                &org.name,
                own_org[&org.name],
                catalogue_id,
                org.opendata_base(),
                org.hosts.portals.clone(),
            ))
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
        tracing::info!(
            from = %old.host,
            to = %held.host,
            "dataroom: default catalogue moved to the configured host (the deployment's host wins over one catalogue.propose set)"
        );
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
    /// The run-time Organizations served now, as they were set up.
    applied: BTreeMap<String, (OrgId, CatalogueId, OrganizationSection)>,
}

/// Follows `follow::ORGANIZATIONS`: one task applies the newest set ([`RunTime::apply`]), so
/// a set published while a catalogue is written waits and never interleaves with it.
#[cfg(feature = "portal")]
fn follow_organizations(
    mut run: RunTime,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) {
    let (tx, mut rx) = tokio::sync::watch::channel(None);
    super::follow::ORGANIZATIONS.follow("dataroom", move |set| {
        tx.send_replace(Some(set.clone()));
    });
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                changed = rx.changed() => if changed.is_err() { return },
            }
            let set = rx.borrow_and_update().clone();
            if let Some(set) = set {
                run.apply(&set).await;
            }
        }
    });
}

#[cfg(feature = "portal")]
impl RunTime {
    /// Serves each run-time Organization's catalogue under its own OrgId (ODP-12) at its open
    /// data host, its Portal granted in process ([`super::follow::catalogue_token`]), and stops
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
        let token = super::follow::catalogue_token(&org.name)?;
        self.writer
            .upsert(dataroom_server::CatalogueSpec {
                org_id: tenant,
                id,
                name: org.name.clone(),
                host: bare_host(org.hosts.opendata_host()).to_owned(),
                base: Some(format!("{}://{}", self.scheme, org.opendata_base())),
                organization: Some(org.name.clone()),
                publisher: publisher_of(tenant, id, org, &self.scheme),
                publish_token: Some(token.expose().to_owned()),
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
    organizations: &[OrganizationSection],
    portals: Vec<super::portals::OrganizationPortals>,
    configured: BTreeMap<CatalogueId, BrandingBlock>,
    state: &DataroomState,
    shutdown: &tokio_util::sync::CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> Result<(), String> {
    let mut catalogue = BTreeMap::new();
    for org in organizations {
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| e.to_string())?;
        catalogue.insert(org.name.clone(), id);
    }
    if catalogue.is_empty() {
        return Ok(());
    }
    let shared = state.catalogue_branding.clone();
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut seen: BTreeMap<CatalogueId, BrandingBlock> = BTreeMap::new();
        loop {
            if let Some(portal) = super::PORTAL_STATE.get() {
                let wanted = catalogue_looks(&portal.mirror, &catalogue, &portals, &configured);
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
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(super::appearance::FOLLOW_LOOK_EVERY) => {}
            }
        }
    });
    Ok(())
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
    out
}

/// Each Organization's catalogue home shows the public Dashboards its
/// `spec.appearance.opendataWidgets` names, read from the Portal's mirror on every render
/// (UIX-16, T-6780): a catalogue without an Organization shows none.
#[cfg(feature = "portal")]
fn with_dashboards(
    organizations: &[OrganizationSection],
    state: DataroomState,
) -> Result<DataroomState, String> {
    let mut organization = BTreeMap::new();
    for org in organizations {
        let id = CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| e.to_string())?;
        organization.insert(id, org.name.clone());
    }
    Ok(state.with_dashboard_resolver(move |catalogue, lang| {
        match (organization.get(catalogue), super::PORTAL_STATE.get()) {
            (Some(name), Some(state)) => {
                super::appearance::dashboard_cards(&state.mirror, name, lang)
            }
            _ => Vec::new(),
        }
    }))
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
    if let Some(font) = look.font {
        over["fonts"] = serde_json::json!({ "heading": font.stack(), "body": font.stack() });
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
        tracing::warn!(
            "dataroom: an appearance colour below WCAG AA on the background; jc's colours show"
        );
        if let Some(members) = over.as_object_mut() {
            members.remove("colours");
        }
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
    };
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
        let mut urls = Vec::new();
        for role in ["split_app", "split_owner"] {
            let pw = format!("pw-{role}-{}", std::process::id());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
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

    /// A memory-store dataroom as jc builds it with no `[[organizations]]`, and its writer.
    async fn runtime_dataroom() -> (Router, dataroom_server::CatalogueWriter) {
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

    /// The catalogue of a Portal `name` of one Organization, with its own publish token.
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
            publish_token: Some(format!("publish-token-of-{name}-0123456789abcdef")),
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
        let token = spec.publish_token.as_deref();
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
        let token = super::super::follow::catalogue_token("rt7186").unwrap();
        let token = token.expose().to_owned();
        assert_eq!(
            decided("rt7186", "package_create", Some(&token)).await,
            axum::http::StatusCode::OK,
            "the Portal publishes with its in-process token"
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
        let (host, token) = (default.host.clone(), default.publish_token.clone());
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
        let (host, token) = (spec.host.clone(), spec.publish_token.clone());
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
        // Its principal went with it: the token binds to no catalogue any more.
        let mut next = portal_spec(0xA2, 0xC9, "schools");
        next.publish_token = token.clone();
        writer.upsert(next).await.expect("the token is free again");
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
                spec.publish_token.as_deref(),
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
        if let Some(route) = spec.portal.as_mut() {
            route.host = Some(host.to_owned());
            route.redirect = redirect;
        }
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
        let token = mobility.publish_token.clone();
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
    async fn a_publish_principal_of_one_catalogue_cannot_write_another() {
        let (router, writer) = runtime_dataroom().await;
        let (x, y) = (
            portal_spec(0xA3, 0xC3, "mobility"),
            portal_spec(0xA3, 0xC4, "schools"),
        );
        writer.upsert(x.clone()).await.unwrap();
        writer.upsert(y.clone()).await.unwrap();
        let foreign = action(
            &router,
            &y.host,
            x.publish_token.as_deref(),
            "package_create",
            Some(package("evil")),
        )
        .await;
        assert!(
            matches!(foreign.as_u16(), 403 | 404),
            "X's principal on Y's catalogue: {foreign}"
        );
        let shown = action(&router, &y.host, None, "package_show?id=evil", None).await;
        assert_eq!(shown, axum::http::StatusCode::NOT_FOUND);
        let own = action(
            &router,
            &y.host,
            y.publish_token.as_deref(),
            "package_create",
            Some(package("air")),
        )
        .await;
        assert!(own.is_success(), "{own}");
        // One token cannot become a second catalogue's principal.
        let mut z = portal_spec(0xA3, 0xC5, "culture");
        z.publish_token = x.publish_token.clone();
        assert!(matches!(
            writer.upsert(z).await,
            Err(DataroomError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn a_catalogue_id_stays_with_its_organization_after_a_delete_and_under_a_race() {
        let (router, writer) = runtime_dataroom().await;
        let a = portal_spec(0xA6, 0xC6, "mobility");
        let mut b = portal_spec(0xB6, 0xC6, "schools");
        b.publish_token = Some("publish-token-of-b-0123456789abcdef".into());
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

        let (c, mut d) = (
            portal_spec(0xA7, 0xC7, "culture"),
            portal_spec(0xB7, 0xC7, "sport"),
        );
        d.publish_token = Some("publish-token-of-d-0123456789abcdef".into());
        let (rc, rd) = tokio::join!(writer.upsert(c.clone()), writer.upsert(d.clone()));
        assert!(
            rc.is_ok() != rd.is_ok(),
            "exactly one Organization holds the id"
        );
        let (won, lost) = if rc.is_ok() { (&c, &d) } else { (&d, &c) };
        let made = action(
            &router,
            &won.host,
            won.publish_token.as_deref(),
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
            let entry = nkod
                .converted
                .get(name)
                .or_else(|| hub.converted.get(name))
                .unwrap_or_else(|| panic!("{name} is converted"));
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
                .unwrap_or_else(|| panic!("{name} is not listed"));
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
        let shown = with_appearance(configured.clone(), Some(&pale));
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
        const { assert!(super::super::appearance::FOLLOW_LOOK_EVERY.as_secs() < 10) };
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
        let (store, org, cat, _) = stored_default("opendata.city.example").await;
        let mut proposed = store.get_catalogue(org, cat).await.unwrap().unwrap();
        proposed.host = "proposed.example".into();
        store.upsert_catalogue(proposed).await.unwrap();
        hold_default_host(&store, org, cat, "opendata.city.example")
            .await
            .unwrap();
        assert_eq!(host_of(&store, org, cat).await, "opendata.city.example");
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
        seed_hidden(&store, ctx.org_id, ctx.catalogue_id, &BTreeMap::new())
            .await
            .unwrap();
        assert!(get().await.is_empty());
        assert_eq!(prod_hidden().len(), 100);
    }
}
