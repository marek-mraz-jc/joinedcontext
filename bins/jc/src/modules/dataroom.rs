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
use crate::config::{DataroomSection, HarvestSeed, OrganizationSection};
use crate::module::Module;
use crate::setup::Secret;
use crate::siblings::Siblings;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::branding::{BrandingAddresses, BrandingBlock, PublisherBranding};
use dataroom_core::{
    CatalogueId, DataroomError, HarvestSource, HarvestSourceKind, OrgId, PolicyAuthorizer,
    Publisher, RecordState, RequestCtx, Store, Subject,
};
use dataroom_harvest::{HarvestEngine, HarvestScheduler, HttpFetcher};
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
    let (tokens, setup, siblings) = (&ctx.secrets.organization_tokens, ctx.setup, ctx.siblings);
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    dataroom_api::check_aliases(&cfg.aliases)
        .map_err(|e| err(&format!("[dataroom.aliases] {e}")))?;
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store = open_store(cfg).await?;
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
            organization_branding(&own, &live, org, &mount.scheme, assistant_widget.clone()),
        );
    }
    let mut catalogues = vec![(org_id, catalogue_id)];
    for org in organizations {
        catalogues.push((
            org_id,
            CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?,
        ));
    }
    for &(o, c) in &catalogues {
        grants.push(dataroom_harvest::harvest_principal(o, c).1);
    }
    let own_gateway = own_gateway(&live, siblings);
    for seed in &cfg.harvest {
        let (cat, site) = match &seed.organization {
            None => (catalogue_id, mount.public_base()),
            Some(name) => {
                let org = organizations
                    .iter()
                    .find(|o| &o.name == name)
                    .ok_or_else(|| {
                        err(&format!(
                            "[[dataroom.harvest]] {}: no [[organizations]] {name}",
                            seed.name
                        ))
                    })?;
                (
                    CatalogueId::parse_verified(&org.catalogue_id).map_err(|e| err(&e))?,
                    format!("{}://{}", mount.scheme, org.hosts.opendata),
                )
            }
        };
        seed_harvest(
            store.as_ref(),
            org_id,
            cat,
            seed,
            &site,
            own_gateway.as_ref().map(|(public, _)| public.as_str()),
        )
        .await
        .map_err(|e| err(&format!("[[dataroom.harvest]] {}: {e}", seed.name)))?;
    }
    let authorizer = Arc::new(PolicyAuthorizer::new(grants));
    let mut engine = HarvestEngine::new(Arc::new(HttpFetcher::new()));
    if let Some((public, Some(internal))) = &own_gateway {
        engine = engine.with_own_gateway(public, *internal);
    }
    HarvestScheduler::new(
        Arc::new(engine),
        store.clone(),
        authorizer.clone(),
        catalogues,
    )
    .spawn();
    #[cfg(feature = "portal")]
    let configured = by_catalogue.clone();
    let state = DataroomState::new(store, authorizer, org_id, catalogue_id)
        .with_tokens(bindings)
        .with_installation_branding(installation_branding(&own, &live, assistant_widget))
        .with_catalogue_branding(by_catalogue)
        // Several catalogues by host: a loopback name names none of them.
        .with_loopback_default(organizations.is_empty())
        .with_public_base(mount.public_base())
        .with_mount_prefix(mount.path_prefix);
    #[cfg(feature = "portal")]
    let state = with_dashboards(organizations, state).map_err(|e| err(&e))?;
    #[cfg(feature = "portal")]
    follow_appearances(organizations, configured, &state, shutdown, tasks).map_err(|e| err(&e))?;
    #[cfg(not(feature = "portal"))]
    let _ = (shutdown, tasks);
    Ok(dataroom_api::with_dataset_aliases(
        dataroom_api::router(state),
        cfg.aliases.clone(),
    ))
}

/// The origin of this instance's gateway as the catalogue names it (`[dataroom.addresses]
/// api` or the gateway's mount), and the loopback listener that serves it when jc mounts the
/// gateway on its own listener (no host of its own): a harvest reads the Endpoints there, as
/// prod's harvest read them in-cluster. `None` without a gateway.
fn own_gateway(
    live: &crate::siblings::LiveAddresses,
    siblings: &Siblings,
) -> Option<(String, Option<std::net::SocketAddr>)> {
    let gateway = siblings.get(Module::Gateway)?;
    let public = crate::siblings::origin(&gateway.public);
    if live.api.is_empty() {
        return None;
    }
    let internal = crate::siblings::origin(&gateway.internal);
    let loopback = internal
        .strip_prefix("http://")
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
        .filter(|a| a.ip().is_loopback());
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

/// Keeps each Organization's catalogue branding its `Organization.spec.appearance` over jc's
/// config (UIX-16, T-6777): a member the Organization leaves out, or a removed appearance,
/// shows `configured` again.
#[cfg(feature = "portal")]
fn follow_appearances(
    organizations: &[OrganizationSection],
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
    let shared = state.catalogue_branding.clone();
    super::appearance::follow(
        catalogue.keys().cloned().collect(),
        shutdown,
        tasks,
        move |name, look| {
            let block = catalogue
                .get(&name)
                .and_then(|id| Some((*id, configured.get(id)?.clone())));
            if let Some((id, block)) = block {
                let block = with_appearance(block, look.as_ref());
                shared
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(id, block);
                tracing::info!(organization = %name, appearance = look.is_some(), "dataroom: catalogue branding follows the Organization");
            }
            std::future::ready(())
        },
    );
    Ok(())
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
        match (organization.get(catalogue), super::appearance::PORTAL_MIRROR.get()) {
            (Some(name), Some(mirror)) => super::appearance::dashboard_cards(mirror, name, lang),
            _ => Vec::new(),
        }
    }))
}

/// `block` with an Organization's appearance over it, member by member; a logo that fails the
/// logo upload's SVG check keeps jc's.
#[cfg(feature = "portal")]
fn with_appearance(
    mut block: BrandingBlock,
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
    block.apply_override_json(&over);
    block
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
        cat.modified = chrono::Utc::now();
        store.upsert_catalogue(cat).await?;
    }
    Ok(())
}

/// Holds one `[[dataroom.harvest]]` source in catalogue `catalogue_id`, whose public base is
/// `site`: its publisher created when missing, the source written over the stored one. A
/// source naming a scheme other than https, a URL or gateway host outside its own
/// `allowed_hosts`, a schedule that is not cron or a converted file that does not read is
/// refused. A source with converted datasets and no gateway reads `own_gateway`, the origin of
/// this instance's gateway.
pub async fn seed_harvest(
    store: &dyn Store,
    org_id: OrgId,
    catalogue_id: CatalogueId,
    seed: &HarvestSeed,
    site: &str,
    own_gateway: Option<&str>,
) -> dataroom_core::Result<HarvestSource> {
    let kind: HarvestSourceKind =
        serde_json::from_value(serde_json::Value::from(seed.kind.as_str())).map_err(|_| {
            DataroomError::field_error("kind", format!("unknown kind '{}'", seed.kind))
        })?;
    dataroom_harvest::validate_outbound_url(&seed.url, &seed.allowed_hosts, &[], false)?;
    dataroom_harvest::cron_matches(&seed.schedule, chrono::Utc::now())?;
    // A source that names no gateway reads its converted datasets from this instance's own.
    let gateway = match (&seed.gateway, own_gateway) {
        (Some(gateway), _) => {
            dataroom_harvest::validate_outbound_url(gateway, &seed.allowed_hosts, &[], false)?;
            Some(gateway.clone())
        }
        (None, Some(own)) if seed.converted_file.is_some() => Some(own.to_owned()),
        (None, _) => None,
    };
    let converted = match &seed.converted_file {
        None => BTreeMap::new(),
        Some(path) => converted_datasets(path, seed.datasets_base.as_deref(), site)?,
    };
    let ctx = RequestCtx::new(org_id, catalogue_id, Subject::anonymous());
    let now = chrono::Utc::now();
    if store.get_publisher(&ctx, &seed.publisher).await?.is_none() {
        store
            .upsert_publisher(
                &ctx,
                Publisher {
                    org_id,
                    catalogue_id,
                    id: String::new(),
                    name: seed.publisher.clone(),
                    title: BTreeMap::from([("sk".to_owned(), seed.publisher_title.clone())]),
                    description: seed
                        .publisher_description
                        .iter()
                        .map(|d| ("sk".to_owned(), d.clone()))
                        .collect(),
                    uri: seed.publisher_uri.clone(),
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
    }
    store
        .upsert_harvest_source(
            &ctx,
            HarvestSource {
                org_id,
                catalogue_id,
                id: format!("hs-{}", seed.name),
                name: seed.name.clone(),
                kind,
                url: seed.url.clone(),
                publisher_id: seed.publisher.clone(),
                target_publisher_iri: Some(seed.publisher_uri.clone()),
                spatial_uri: seed.spatial_uri.clone(),
                name_prefix: seed.name_prefix.clone(),
                allowed_hosts: seed.allowed_hosts.clone(),
                rewrite_hosts: seed.rewrite_hosts.clone(),
                skip_items: seed.skip.clone(),
                title_overrides: seed.titles.clone(),
                spelling_fixes: seed.spelling.clone(),
                require_column: seed.require_column.clone(),
                schedule: seed.schedule.clone(),
                enabled: seed.enabled,
                converted,
                gateway,
                modified: now,
            },
        )
        .await
}

/// The converted datasets of `path` (prod's `od-datasets.json`), each `dataset` URL moved from
/// `base` to this catalogue's `site`, as prod's harvest CronJob replaces `datasetsBase`.
fn converted_datasets(
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
    use dataroom_core::HarvestRunState;
    use dataroom_core::ops::HarvestRunner;
    use dataroom_harvest::{MapFetcher, validate_outbound_url};
    use std::collections::BTreeSet;

    /// prod's open data catalogue: the base its datasets' `isPartOf` names.
    const SITE: &str = "https://opendata.smartcity.joinedcontext.com";
    const GATEWAY: &str = "https://api.smartcity.joinedcontext.com";

    /// The `[[dataroom.harvest]]` block of prod's two sources, read as jc reads jc.toml (its
    /// `od-datasets.json` beside it).
    fn prod_seeds() -> Vec<HarvestSeed> {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        crate::config::Config::read_file(&docs.join("harvest-sources.toml"))
            .expect("the block parses")
            .dataroom
            .expect("[dataroom]")
            .harvest
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
        assert_eq!(own_gateway(&live("x"), &Siblings::new(&[], listener)), None);
    }

    /// Without a gateway of their own, prod's sources keep prod's converted datasets and read
    /// their Endpoints on this instance's gateway, which no outbound check refuses for being
    /// jc's own loopback listener (gate 4: no converted filter on the rehearsal).
    #[tokio::test]
    async fn prods_sources_read_their_converted_datasets_on_the_own_gateway() {
        let own = "http://localhost:42102";
        let (store, ctx) = catalogue().await;
        for seed in prod_seeds() {
            assert_eq!(seed.gateway, None, "{}", seed.name);
            let src = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seed, SITE, Some(own))
                .await
                .expect("seeded");
            assert_eq!(src.gateway.as_deref(), Some(own), "{}", seed.name);
            assert_eq!(src.converted.len(), 139, "{}", seed.name);
            let entry = &src.converted["bbsk-kontaktne-miesta-2128a0"];
            assert_eq!(
                entry.dataset,
                format!("{SITE}/dataset/bbsk-kontaktne-miesta-2128a0")
            );
            // Without converted datasets the own gateway is not taken.
            let mut plain = seed.clone();
            plain.converted_file = None;
            let src = seed_harvest(
                &store,
                ctx.org_id,
                ctx.catalogue_id,
                &plain,
                SITE,
                Some(own),
            )
            .await
            .expect("seeded");
            assert_eq!(src.gateway, None);
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

    #[tokio::test]
    async fn prods_two_sources_seed_with_their_publishers_and_only_their_hosts() {
        let seeds = prod_seeds();
        assert_eq!(
            seeds
                .iter()
                .map(|s| (s.name.as_str(), s.kind.as_str(), s.schedule.as_str()))
                .collect::<Vec<_>>(),
            [
                ("nkod-banskabystrica", "nkod-sparql", "23 3 * * *"),
                ("hub-bbsk", "arcgis-hub", "23 3 * * *")
            ]
        );
        let (store, ctx) = catalogue().await;
        for seed in &seeds {
            let src = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, seed, SITE, None)
                .await
                .expect("seeded");
            assert!(src.enabled);
            let publisher = store
                .get_publisher(&ctx, &seed.publisher)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(publisher.uri, seed.publisher_uri);
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
                    seed.name
                );
            }
        }
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 2);
        // Seeding again on the next start writes over the stored source, nothing doubles.
        seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[0], SITE, None)
            .await
            .unwrap();
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_seed_outside_its_hosts_or_off_cron_is_refused() {
        let (store, ctx) = catalogue().await;
        let mut off_list = prod_seeds()[0].clone();
        off_list.allowed_hosts.retain(|h| h != "data.slovensko.sk");
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &off_list, SITE, None)
                .await
                .is_err()
        );
        let mut bad_cron = prod_seeds()[1].clone();
        bad_cron.schedule = "nightly".into();
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &bad_cron, SITE, None)
                .await
                .is_err()
        );
        let mut bad_kind = prod_seeds()[1].clone();
        bad_kind.kind = "ftp".into();
        assert!(
            seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &bad_kind, SITE, None)
                .await
                .is_err()
        );
        assert!(store.list_harvest_sources(&ctx).await.unwrap().is_empty());
    }

    /// Both sources run once against what their hosts answered on 2026-11-09 (recorded fixtures,
    /// `tests/fixtures/harvest/`: NKOD's SPARQL answer, the Hub's two search pages, prod's
    /// `od-datasets.json` and its gateway's `schema/index.json` of each Endpoint): every fetch
    /// goes to an allow-listed host, and the datasets prod's CKAN serves for the two publishers
    /// are among those listed, each with prod's distributions.
    #[tokio::test]
    async fn prods_sources_harvest_from_recorded_answers() {
        let (store, ctx) = catalogue().await;
        let fetcher = MapFetcher::new();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/harvest");
        let mut seeds = prod_seeds();
        for seed in &mut seeds {
            seed.converted_file = Some(fixtures.join("od-datasets.json"));
            seed.datasets_base = Some("https://opendata.dev.joinedcontext.com/".into());
            seed.gateway = Some(GATEWAY.into());
            seed.allowed_hosts
                .push("api.smartcity.joinedcontext.com".into());
        }
        let nkod = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[0], SITE, None)
            .await
            .unwrap();
        let hub = seed_harvest(&store, ctx.org_id, ctx.catalogue_id, &seeds[1], SITE, None)
            .await
            .unwrap();
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
