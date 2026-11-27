//! The reconciler of `kind: Portal` (ODP-08, ODP-09, ODP-11): each Portal manifest of an
//! `[[organizations]]` entry, as the Portal of this process mirrors `main`, becomes a dataroom
//! catalogue through the in-process [`CatalogueWriter`], made, changed and deleted while the
//! dataroom serves.
//!
//! Every Organization has the portal `default` whether or not a manifest names it: its
//! catalogue is the `[[organizations]]` entry's `catalogue_id` at the open data host, as before,
//! so no dataset URL or IRI moves; a `default` manifest only describes it. Further portals get a
//! catalogue id derived from the Organization and the portal's path
//! ([`CatalogueId::of_portal`]) and answer at `{opendata}/-/{path}/` or on a host the operator
//! listed in `hosts.portals`.
//!
//! Each catalogue's publisher and contact are the ones the Organization's manifest declares
//! (`spec.catalog`, T-7520) and its profiles the ones the Portal's `spec.profiles` chooses
//! (T-7521), written every round and only on a difference.
//!
//! ponytail: polled every [`super::appearance::FOLLOW_EVERY`] from the one Portal of this
//! process, as the appearance is; upgrade path: the Portal's reconciler calls the writer.

use dataroom_core::files::FileEntry;
use dataroom_core::{
    CatalogueDeclaration, CatalogueId, Dataset, HarvestBudget, HarvestSource, OrgId, Publisher,
    RecordState,
};
use dataroom_server::{CatalogueSpec, CatalogueWriter, FileWrite, PortalRoute};
use jc_core::kinds::{OrganizationSpec, PortalSource, PortalSourceType, PortalSpec, http_url_host};
use joinedcontext_portal::api::portal_files::{
    FileDesk, FileFuture, FileLimits, FileRefusal, PortalFile,
};
use joinedcontext_portal::api::portal_sources::{
    DeskCatalogue, DeskDataset, DeskError, DeskFuture, FeedGap, SourceDesk, SourceTestAnswer,
};
use joinedcontext_portal::store::{ListOptions, Mirror};
use std::collections::{BTreeMap, BTreeSet};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// What jc's configuration says of one Organization's portals.
#[derive(Debug, Clone)]
pub(crate) struct OrganizationPortals {
    pub name: String,
    pub org_id: OrgId,
    /// The `default` portal's catalogue: the entry's `catalogue_id`.
    pub catalogue_id: CatalogueId,
    pub scheme: String,
    /// The open data host, without a port when it has none.
    pub opendata: String,
    /// `hosts.portals`: the only hosts a further portal may take.
    pub listed: Vec<String>,
    /// The deployment's harvest allow-list: every host a source may read (ODP-10).
    pub harvest_hosts: Vec<String>,
    /// jc.toml's directory, where a source's `convertedFile` is read.
    pub config_dir: Option<std::path::PathBuf>,
    /// This instance's gateway origin: a converted source naming no `gateway` reads it.
    pub own_gateway: Option<String>,
}

impl OrganizationPortals {
    /// The entry of an installation without `[[organizations]]`: its one Organization, named
    /// in the mirror at each round ([`named`]), whose `default` portal is the `[dataroom]`
    /// catalogue (ODP-09).
    pub(crate) fn installation(template: Self) -> Self {
        Self {
            name: String::new(),
            ..template
        }
    }
}

/// `orgs` with the installation's entry named after the mirror's installation Organization;
/// unnamed while the mirror names none (two claim it, or none is loaded yet).
pub(crate) fn named(
    orgs: &[OrganizationPortals],
    installation: Option<String>,
) -> Vec<OrganizationPortals> {
    orgs.iter()
        .filter_map(|org| {
            if !org.name.is_empty() {
                return Some(org.clone());
            }
            installation.clone().map(|name| OrganizationPortals {
                name,
                ..org.clone()
            })
        })
        .collect()
}

/// `orgs` with each Organization's own `served` portal hosts of the host registry (T-7415)
/// listed beside jc.toml's `hosts.portals`, so a portal takes such a host with no restart.
pub(crate) fn with_sites(
    orgs: Vec<OrganizationPortals>,
    sites: &joinedcontext_portal::portals::Sites,
) -> Vec<OrganizationPortals> {
    orgs.into_iter()
        .map(|mut org| {
            for host in sites.served(&org.name) {
                if !org.listed.contains(&host) {
                    org.listed.push(host);
                }
            }
            org
        })
        .collect()
}

/// Each Portal manifest of `mirror` that passes jc-core's check, by `metadata.name`.
pub(crate) fn in_mirror(mirror: &Mirror) -> BTreeMap<String, PortalSpec> {
    let items = mirror
        .list(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Portal",
            &ListOptions::default(),
        )
        .items;
    let mut out = BTreeMap::new();
    for envelope in items {
        let name = envelope.metadata.name.clone();
        let checked = serde_json::from_value::<PortalSpec>(envelope.spec)
            .map_err(|e| e.to_string())
            .and_then(|spec| spec.validate().map(|()| spec).map_err(|e| e.to_string()));
        match checked {
            Ok(spec) => {
                out.insert(name, spec);
            }
            Err(e) => tracing::warn!(portal = %name, error = %e, "Portal refused; not served"),
        }
    }
    out
}

/// What each Organization's manifest declares of its catalogues. An Organization the mirror
/// does not hold (nothing is synced yet) or whose manifest does not parse is left out, so its
/// stored values stay: unknown is never read as "declared empty".
pub(crate) fn declarations_in(
    mirror: &Mirror,
    orgs: &[OrganizationPortals],
) -> BTreeMap<String, CatalogueDeclaration> {
    let mut out = BTreeMap::new();
    for org in orgs {
        let Some(held) = mirror.get(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Organization",
            &org.name,
        ) else {
            continue;
        };
        let title = serde_json::to_value(&held.metadata.title).unwrap_or_default();
        match serde_json::from_value::<OrganizationSpec>(held.spec) {
            Ok(spec) => {
                out.insert(org.name.clone(), declaration_of(&spec, title));
            }
            Err(e) => {
                tracing::warn!(organization = %org.name, error = %e, "Organization not read; its catalogues keep their publisher");
            }
        }
    }
    out
}

/// The publisher is `spec.catalog.publisher`, named by the Organization's own title where none
/// is declared; the IRI only when one is declared.
///
/// The Organization owns these members while its Portal is declared: each round writes its
/// publisher IRI (none clears it), and its publisher name and contact desk when it has them, over
/// what an administrator typed in the catalogue.
fn declaration_of(spec: &OrganizationSpec, title: serde_json::Value) -> CatalogueDeclaration {
    let lang = spec.default_locale.to_string();
    let titled = || match title {
        serde_json::Value::String(text) => BTreeMap::from([(lang.clone(), text)]),
        other => serde_json::from_value(other).unwrap_or_default(),
    };
    let catalog = spec.catalog.as_ref();
    let names: dataroom_core::Texts = catalog
        .map(|c| c.publisher.name.clone())
        .filter(|names| !names.is_empty())
        .unwrap_or_else(titled);
    CatalogueDeclaration {
        publisher_uri: Some(
            catalog
                .and_then(|c| c.publisher.uri.clone())
                .filter(|uri| dataroom_core::is_declared_iri(uri)),
        ),
        publisher_name: (!names.is_empty()).then_some(names),
        contact_point: catalog.and_then(|c| c.contact_point.as_ref()).map(|desk| {
            dataroom_core::ContactPoint {
                uri: None,
                kind: dataroom_core::ContactKind::Organization,
                name: BTreeMap::from([(lang.clone(), desk.name.clone())]),
                email: desk.email.clone(),
                url: None,
                identifier: None,
            }
        }),
        ..CatalogueDeclaration::default()
    }
}

/// Writes `declared` into each served catalogue of its Organization, `default` included, with
/// the profiles its Portal chooses (`spec.profiles`, the one declaration of them).
pub(crate) async fn declare(
    writer: &CatalogueWriter,
    orgs: &[OrganizationPortals],
    served: &BTreeMap<String, PortalSpec>,
    declared: &BTreeMap<String, CatalogueDeclaration>,
) {
    for org in orgs {
        let Some(declaration) = declared.get(&org.name) else {
            continue;
        };
        let mut catalogues = vec![((org.org_id, org.catalogue_id), None)];
        for portal in served.values().filter(|p| p.organization_ref == org.name) {
            let chosen = portal.profile_choice();
            match further_catalogue(portal, orgs) {
                Some(key) => catalogues.push((key, chosen)),
                None => catalogues[0].1 = chosen,
            }
        }
        for ((org_id, id), chosen) in catalogues {
            match writer.declare(org_id, id, declaration, chosen).await {
                Ok(true) => tracing::info!(catalogue = %id, "catalogue declaration written"),
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(catalogue = %id, error = %e, "declaration not written; next round");
                }
            }
        }
    }
}

/// The catalogue `portal` asks for, or why it is not served: an Organization this jc does not
/// serve, or a host the operator did not list for it (the Portal's judge refuses both first).
pub(crate) fn catalogue_of(
    portal: &PortalSpec,
    orgs: &[OrganizationPortals],
) -> Result<CatalogueSpec, String> {
    let org = orgs
        .iter()
        .find(|o| o.name == portal.organization_ref)
        .ok_or_else(|| format!("no [[organizations]] {}", portal.organization_ref))?;
    let at = |host: &str| format!("{}://{host}", org.scheme);
    let (id, host, base, route) = if portal.is_default() {
        (
            org.catalogue_id,
            org.opendata.clone(),
            at(&org.opendata),
            None,
        )
    } else {
        let id = CatalogueId::of_portal(org.org_id, &portal.path);
        let route = PortalRoute {
            organization: org.name.clone(),
            path: portal.path.clone(),
            host: portal.host.clone(),
            redirect: portal.slug == jc_core::kinds::SlugAddress::Redirect,
        };
        match &portal.host {
            Some(host) if org.listed.contains(host) => (id, host.clone(), at(host), Some(route)),
            Some(host) => {
                return Err(format!(
                    "spec.host {host} is not listed for the Organization {}",
                    org.name
                ));
            }
            None => (
                id,
                dataroom_api::synthetic_host(&org.name, &portal.path),
                format!("{}/-/{}", at(&org.opendata), portal.path),
                Some(route),
            ),
        }
    };
    let now = jc_time::TimestampTz::now();
    Ok(CatalogueSpec {
        org_id: org.org_id,
        id,
        name: portal.path.clone(),
        host,
        base: Some(base),
        organization: portal.is_default().then(|| org.name.clone()),
        publisher: Publisher {
            org_id: org.org_id,
            catalogue_id: id,
            id: String::new(),
            name: org.name.clone(),
            title: BTreeMap::from([("en".to_owned(), org.name.clone())]),
            description: BTreeMap::new(),
            uri: format!("{}/publisher/{}", at(&org.opendata), org.name),
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
        // The Portal publishes Endpoints into an Organization's `default`, admitted at start; a
        // further portal's datasets come from its sources (ODP-10).
        publishes: false,
        portal: route,
    })
}

/// The catalogue a further portal holds, whatever its host: what a delete removes.
fn further_catalogue(
    portal: &PortalSpec,
    orgs: &[OrganizationPortals],
) -> Option<(OrgId, CatalogueId)> {
    let org = orgs.iter().find(|o| o.name == portal.organization_ref)?;
    (!portal.is_default()).then(|| (org.org_id, CatalogueId::of_portal(org.org_id, &portal.path)))
}

/// The catalogue `portal` writes its sources into: `default`'s is the configured one.
pub(crate) fn catalogue_key(
    portal: &PortalSpec,
    orgs: &[OrganizationPortals],
) -> Option<(OrgId, CatalogueId)> {
    let org = orgs.iter().find(|o| o.name == portal.organization_ref)?;
    Some(match further_catalogue(portal, orgs) {
        Some(key) => key,
        None => (org.org_id, org.catalogue_id),
    })
}

/// The harvest source (and the publisher its records land in) of each harvested source of
/// `portal`; an `endpoints` source lists published Endpoints and harvests nothing. Refused: a
/// source reading a host outside the deployment's harvest allow-list (ODP-10).
pub(crate) fn harvest_sources_of(
    portal: &PortalSpec,
    orgs: &[OrganizationPortals],
) -> Result<Vec<(HarvestSource, Publisher)>, String> {
    let org = orgs
        .iter()
        .find(|o| o.name == portal.organization_ref)
        .ok_or_else(|| format!("no [[organizations]] {}", portal.organization_ref))?;
    let (org_id, catalogue_id) = catalogue_key(portal, orgs).ok_or("no catalogue")?;
    let now = jc_time::TimestampTz::now();
    let mut out = Vec::new();
    for (position, source) in portal.sources.iter().enumerate() {
        if source.kind == PortalSourceType::Endpoints {
            continue;
        }
        // The url's own host is the admin's (the Portal's propose door refused an address or
        // an internal name); the harvest guard carries it. A further host is the operator's.
        let own = source.url.as_deref().and_then(http_url_host);
        if let Some(host) = source.allowed_hosts.iter().find(|h| {
            own.as_deref().is_none_or(|o| !o.eq_ignore_ascii_case(h))
                && !org.harvest_hosts.iter().any(|a| a.eq_ignore_ascii_case(h))
        }) {
            return Err(format!(
                "source {}: {host} is not in the deployment's harvest allow-list",
                source.name
            ));
        }
        let kind = serde_json::to_value(source.kind)
            .and_then(serde_json::from_value)
            .map_err(|e| format!("source {}: {e}", source.name))?;
        let gateway = match (&source.gateway, &org.own_gateway) {
            (Some(gateway), _) => Some(gateway.clone()),
            (None, Some(own)) if source.converted_file.is_some() => Some(own.clone()),
            (None, _) => None,
        };
        let converted = match &source.converted_file {
            None => BTreeMap::new(),
            Some(file) => {
                let dir = org.config_dir.as_deref().ok_or_else(|| {
                    format!(
                        "source {}: convertedFile, but jc.toml has no [dataroom]",
                        source.name
                    )
                })?;
                let site = catalogue_of(portal, orgs)?.base.unwrap_or_default();
                super::dataroom::converted_datasets(
                    &dir.join(file),
                    source.datasets_base.as_deref(),
                    &site,
                )
                .map_err(|e| format!("source {}: {e}", source.name))?
            }
        };
        let publisher = source.publisher.clone().unwrap_or_else(|| org.name.clone());
        let lang = portal.languages.first().map_or("en", String::as_str);
        let source_publisher =
            publisher_of(source, org, (org_id, catalogue_id), lang, &publisher, now);
        out.push((
            HarvestSource {
                org_id,
                catalogue_id,
                id: format!("hs-{}", source.name),
                name: source.name.clone(),
                kind,
                url: source.url.clone().unwrap_or_default(),
                publisher_id: publisher,
                target_publisher_iri: source.publisher_uri.clone(),
                spatial_uri: source.spatial_uri.clone(),
                name_prefix: source.name_prefix.clone().unwrap_or_default(),
                allowed_hosts: source.allowed_hosts.clone(),
                rewrite_hosts: source.rewrite_hosts.clone(),
                skip_items: source.skip.clone(),
                title_overrides: source.titles.clone(),
                spelling_fixes: source.spelling.clone(),
                require_column: source.require_column.clone(),
                schedule: source
                    .schedule
                    .clone()
                    .unwrap_or_else(|| "23 3 * * *".into()),
                enabled: source.enabled,
                converted,
                gateway,
                budget: source.budget.map(|b| HarvestBudget {
                    records: b.records,
                    bytes: b.bytes,
                }),
                position: u32::try_from(position).unwrap_or(u32::MAX),
                modified: now,
            },
            source_publisher,
        ));
    }
    Ok(out)
}

fn publisher_of(
    source: &PortalSource,
    org: &OrganizationPortals,
    (org_id, catalogue_id): (OrgId, CatalogueId),
    lang: &str,
    name: &str,
    now: jc_time::TimestampTz,
) -> Publisher {
    let title = source
        .publisher_title
        .clone()
        .unwrap_or_else(|| name.to_owned());
    Publisher {
        org_id,
        catalogue_id,
        id: String::new(),
        name: name.to_owned(),
        title: BTreeMap::from([(lang.to_owned(), title)]),
        description: source
            .publisher_description
            .iter()
            .map(|d| (lang.to_owned(), d.clone()))
            .collect(),
        uri: source
            .publisher_uri
            .clone()
            .unwrap_or_else(|| format!("{}://{}/publisher/{name}", org.scheme, org.opendata)),
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

/// Writes `portal`'s harvest sources and deletes the ones `before` had that it no longer
/// lists; `false` when a write failed (the next round tries again).
async fn sync_sources(
    writer: &CatalogueWriter,
    orgs: &[OrganizationPortals],
    before: Option<&PortalSpec>,
    portal: &PortalSpec,
) -> bool {
    let Some((org, id)) = catalogue_key(portal, orgs) else {
        return false;
    };
    let wanted = match harvest_sources_of(portal, orgs) {
        Ok(wanted) => wanted,
        Err(e) => {
            tracing::warn!(portal = %portal.path, error = %e, "portal sources refused");
            return false;
        }
    };
    let mut ok = true;
    for gone in before.into_iter().flat_map(|b| &b.sources) {
        if wanted.iter().any(|(s, _)| s.name == gone.name)
            || gone.kind == PortalSourceType::Endpoints
        {
            continue;
        }
        if let Err(e) = writer.delete_harvest_source(org, id, &gone.name).await {
            tracing::warn!(source = %gone.name, error = %e, "harvest source not deleted");
            ok = false;
        }
    }
    for (source, publisher) in wanted {
        let name = source.name.clone();
        if let Err(e) = writer
            .upsert_harvest_source(org, id, source, Some(publisher))
            .await
        {
            tracing::warn!(source = %name, error = %e, "harvest source not written");
            ok = false;
        }
    }
    ok
}

/// Follows the Portal manifests until `shutdown`: a new or changed further portal is written,
/// one that left `main` is deleted (its addresses answer 410). `default` is never deleted.
pub(crate) fn follow(
    writer: CatalogueWriter,
    orgs: Vec<OrganizationPortals>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) {
    if orgs.is_empty() {
        return;
    }
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut served: BTreeMap<String, PortalSpec> = BTreeMap::new();
        let mut listing = BTreeSet::new();
        let mut handed: Option<Vec<String>> = None;
        loop {
            if let Some(state) = super::PORTAL_STATE.get() {
                let installation =
                    joinedcontext_portal::permissions::installation_organization(&state.mirror);
                let orgs = with_sites(
                    named(&orgs, installation),
                    &state.config.portal_settings.sites,
                );
                let names: Vec<String> = orgs.iter().map(|o| o.name.clone()).collect();
                if handed.as_ref() != Some(&names) {
                    let desk = std::sync::Arc::new(Desk {
                        writer: writer.clone(),
                        orgs: orgs.clone(),
                        active: std::sync::Arc::default(),
                    });
                    joinedcontext_portal::api::portal_sources::hand_desk_to(desk.clone());
                    joinedcontext_portal::api::portal_files::hand_file_desk_to(desk);
                    handed = Some(names);
                }
                apply(&writer, &orgs, &mut served, in_mirror(&state.mirror)).await;
                let declared = declarations_in(&state.mirror, &orgs);
                declare(&writer, &orgs, &served, &declared).await;
                let endpoints = endpoints_in(&state.mirror);
                list_endpoints(&writer, &orgs, &served, &endpoints, &mut listing).await;
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(super::appearance::FOLLOW_EVERY) => {}
            }
        }
    });
}

/// The Portal's sources routes on this process's catalogues (ODP-15): each call runs as the
/// portal's catalogue's own harvest principal, so it reaches that catalogue's rows alone.
struct Desk {
    writer: CatalogueWriter,
    orgs: Vec<OrganizationPortals>,
    /// The sources a run started from the Portal holds, until it ends.
    active: std::sync::Arc<std::sync::Mutex<BTreeSet<(CatalogueId, String)>>>,
}

impl Desk {
    fn key(&self, portal: &PortalSpec) -> Result<(OrgId, CatalogueId), DeskError> {
        catalogue_key(portal, &self.orgs).ok_or(DeskError::NotFound)
    }

    async fn op(
        &self,
        portal: &PortalSpec,
        op: &str,
        source: &str,
    ) -> Result<serde_json::Value, DeskError> {
        let (org, id) = self.key(portal)?;
        self.writer
            .run_op(org, id, op, serde_json::json!({ "source": source }))
            .await
            .map_err(desk_error)
    }
}

fn desk_error(e: dataroom_core::DataroomError) -> DeskError {
    match e {
        dataroom_core::DataroomError::NotFound(_) => DeskError::NotFound,
        dataroom_core::DataroomError::Conflict(_) => DeskError::Busy,
        other => DeskError::Failed(other.to_string()),
    }
}

impl SourceDesk for Desk {
    fn test<'a>(
        &'a self,
        portal: &'a PortalSpec,
        source: &'a str,
    ) -> DeskFuture<'a, SourceTestAnswer> {
        Box::pin(async move {
            let (org, id) = self.key(portal)?;
            let input = serde_json::json!({ "source": source, "preview": true });
            let answer = self
                .writer
                .run_op(org, id, "harvest_source.test", input)
                .await
                .map_err(desk_error)?;
            serde_json::from_value(answer)
                .map_err(|e| DeskError::Failed(format!("a source test answered no outcome: {e}")))
        })
    }

    fn start<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, ()> {
        Box::pin(async move {
            let (_, id) = self.key(portal)?;
            // The list proves the source is served and shows a scheduled run still going.
            let runs = self.op(portal, "harvest_run.list", source).await?;
            let running = runs["runs"]
                .as_array()
                .is_some_and(|r| r.iter().any(|r| r["state"] == "running"));
            let held = (id, source.to_owned());
            if running
                || !self
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(held.clone())
            {
                return Err(DeskError::Busy);
            }
            let (writer, active, (org, id)) =
                (self.writer.clone(), self.active.clone(), self.key(portal)?);
            let name = source.to_owned();
            tokio::spawn(async move {
                let input = serde_json::json!({ "source": name });
                if let Err(e) = writer.run_op(org, id, "harvest_run.start", input).await {
                    tracing::warn!(source = %name, error = %e, "portal source run failed");
                }
                active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&held);
            });
            Ok(())
        })
    }

    fn runs<'a>(
        &'a self,
        portal: &'a PortalSpec,
        source: &'a str,
    ) -> DeskFuture<'a, Vec<serde_json::Value>> {
        Box::pin(async move {
            let mut answer = self.op(portal, "harvest_run.list", source).await?;
            // `harvest_run.list` answers `{"runs": [...]}`; anything else lists no run.
            Ok(serde_json::from_value(answer["runs"].take()).unwrap_or_default())
        })
    }

    fn datasets<'a>(
        &'a self,
        portal: &'a PortalSpec,
        offset: usize,
        limit: usize,
    ) -> DeskFuture<'a, DeskCatalogue> {
        Box::pin(async move {
            let (org, id) = self.key(portal)?;
            let report = self
                .writer
                .report(org, id, offset, limit)
                .await
                .map_err(desk_error)?;
            Ok(DeskCatalogue {
                total: report.total,
                missing_publisher: report.missing_publisher,
                profiles: report.served,
                datasets: report
                    .datasets
                    .into_iter()
                    .map(|(d, gaps)| DeskDataset {
                        title: dataroom_core::model::text_in_lang(&d.title, None)
                            .unwrap_or(&d.name)
                            .to_owned(),
                        source: d.harvest_source_id,
                        name: d.name,
                        missing: gaps
                            .into_iter()
                            .map(|(profile, fields)| FeedGap {
                                profile,
                                fields: fields.to_vec(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
        })
    }
}

fn portal_file(e: FileEntry) -> PortalFile {
    PortalFile {
        path: e.path(),
        id: e.id,
        name: e.name,
        content_type: e.content_type,
        size: e.size,
        sha256: e.sha256,
        uploaded_by: e.uploaded_by,
        uploaded_at: e.uploaded_at.to_rfc3339(),
        used_by: Vec::new(),
    }
}

fn file_refusal(e: dataroom_core::DataroomError) -> FileRefusal {
    match e {
        // The problem alone, without the `file: ` field prefix a person has no use for.
        dataroom_core::DataroomError::Validation { message, fields } => {
            FileRefusal::Invalid(fields.into_values().flatten().next().unwrap_or(message))
        }
        other => FileRefusal::Desk(desk_error(other)),
    }
}

impl Desk {
    async fn write_file(
        &self,
        portal: &PortalSpec,
        by: &str,
        write: FileWrite,
    ) -> Result<PortalFile, FileRefusal> {
        let (org, id) = self.key(portal)?;
        self.writer
            .write_file(org, id, by, write)
            .await
            .map(portal_file)
            .map_err(file_refusal)
    }
}

/// The Portal's files routes on this process's catalogues (T-7417): each portal's files under
/// its catalogue's key, so one portal's call reaches its own files alone.
impl FileDesk for Desk {
    fn list<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, (Vec<PortalFile>, FileLimits)> {
        Box::pin(async move {
            let (org, id) = self.key(portal)?;
            let (files, usage) = self.writer.files(org, id).await.map_err(desk_error)?;
            Ok((
                files.into_iter().map(portal_file).collect(),
                FileLimits {
                    files: usage.files,
                    bytes: usage.bytes,
                    max_files: usage.max_files,
                    max_bytes: usage.max_bytes,
                    max_file_bytes: usage.max_file_bytes,
                },
            ))
        })
    }

    fn upload<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        name: &'a str,
        bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile> {
        let write = FileWrite::Upload {
            name: name.to_owned(),
            bytes,
        };
        Box::pin(self.write_file(portal, by, write))
    }

    fn rename<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        id: &'a str,
        name: &'a str,
    ) -> FileFuture<'a, PortalFile> {
        let write = FileWrite::Rename {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        Box::pin(self.write_file(portal, by, write))
    }

    fn replace<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        id: &'a str,
        bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile> {
        let write = FileWrite::Replace {
            id: id.to_owned(),
            bytes,
        };
        Box::pin(self.write_file(portal, by, write))
    }

    fn delete<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        id: &'a str,
    ) -> FileFuture<'a, ()> {
        let write = FileWrite::Delete { id: id.to_owned() };
        Box::pin(async move { self.write_file(portal, by, write).await.map(|_| ()) })
    }
}

/// The `generated_by` of a dataset the Portal published from an Endpoint's `publish` block
/// (jcctl's CKAN publisher): only such a dataset opted in to being listed (ODP-02, ODP-10).
const PUBLISHED_BY: &str = "jcctl/ckan-publisher";

/// The `generated_by` of a further portal's copy of a published dataset (ODP-10 `endpoints`).
const LISTED_BY: &str = "jc/portal-endpoints";

/// One Endpoint manifest of the mirror, as an `endpoints` source narrows by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndpointRow {
    pub project: String,
    pub space: String,
    pub name: String,
    pub slug: String,
    /// The manifest carries a `publish` block: the Endpoint opted in (ODP-02). A dataset's
    /// `generated_by` alone is a value any `package_create` caller can write in its extras.
    pub published: bool,
}

/// Every Endpoint manifest of the mirror's projects.
pub(crate) fn endpoints_in(mirror: &Mirror) -> Vec<EndpointRow> {
    let mut out = Vec::new();
    for project in mirror.namespaces() {
        for envelope in mirror
            .list(&project, "Endpoint", &ListOptions::default())
            .items
        {
            let spec = &envelope.spec;
            let space = spec
                .get("contextSpaceRef")
                .and_then(joinedcontext_portal::resource::reference_name);
            if let (Some(space), Some(slug)) = (space, spec["slug"].as_str()) {
                out.push(EndpointRow {
                    project: project.clone(),
                    space: space.to_owned(),
                    name: envelope.metadata.name.clone(),
                    slug: slug.to_owned(),
                    published: !spec["publish"].is_null(),
                });
            }
        }
    }
    out
}

/// Whether the `endpoints` source `source` lists `dataset` of its Organization's `default`
/// catalogue: a dataset the Portal published from an Endpoint (never a harvested or hand-made
/// one, never an Endpoint without `publish`), of an Endpoint of `endpoints` whose manifest
/// publishes it, inside every
/// narrowing the source names (each list one of its values; an empty list narrows nothing).
pub(crate) fn selects(source: &PortalSource, dataset: &Dataset, endpoints: &[EndpointRow]) -> bool {
    if source.kind != PortalSourceType::Endpoints
        || dataset.generated_by.as_deref() != Some(PUBLISHED_BY)
    {
        return false;
    }
    let slug = dataset
        .endpoint_url
        .as_deref()
        .and_then(|url| url.trim_end_matches('/').rsplit('/').next());
    let Some(endpoint) = endpoints
        .iter()
        .find(|e| e.published && Some(e.slug.as_str()) == slug)
    else {
        return false;
    };
    let within = |list: &[String], value: &str| list.is_empty() || list.iter().any(|v| v == value);
    within(&source.projects, &endpoint.project)
        && within(&source.spaces, &endpoint.space)
        && within(&source.names, &endpoint.name)
        && (source.tags.is_empty()
            || dataset.tags.iter().any(|t| source.tags.contains(t))
            || dataset
                .keywords
                .values()
                .flatten()
                .any(|t| source.tags.contains(t)))
}

/// Lists in each served further portal what its `endpoints` sources select of its
/// Organization's `default` catalogue (ODP-10); a portal that had such a source last round
/// (in `listing`) and has none now withdraws its copies.
pub(crate) async fn list_endpoints(
    writer: &CatalogueWriter,
    orgs: &[OrganizationPortals],
    served: &BTreeMap<String, PortalSpec>,
    endpoints: &[EndpointRow],
    listing: &mut BTreeSet<String>,
) {
    let mut now = BTreeSet::new();
    for (name, portal) in served {
        let lists = portal
            .sources
            .iter()
            .any(|s| s.kind == PortalSourceType::Endpoints);
        let (Some(org), Some((org_id, to))) = (
            orgs.iter().find(|o| o.name == portal.organization_ref),
            further_catalogue(portal, orgs),
        ) else {
            continue;
        };
        if !lists && !listing.contains(name) {
            continue;
        }
        let from_base = format!("{}://{}", org.scheme, org.opendata);
        let keep = |ds: &Dataset| portal.sources.iter().any(|s| selects(s, ds, endpoints));
        match writer
            .list_from(org_id, (org.catalogue_id, &from_base), to, LISTED_BY, keep)
            .await
        {
            Ok(_) if !lists => {}
            Ok(_) => {
                now.insert(name.clone());
            }
            Err(e) => {
                tracing::warn!(portal = %name, error = %e, "published Endpoints not listed; next round");
                now.insert(name.clone());
            }
        }
    }
    *listing = now;
}

/// One round of [`follow`]: `served` is what the last round wrote.
pub(crate) async fn apply(
    writer: &CatalogueWriter,
    orgs: &[OrganizationPortals],
    served: &mut BTreeMap<String, PortalSpec>,
    now: BTreeMap<String, PortalSpec>,
) {
    for (name, gone) in served.clone() {
        if now.contains_key(&name) {
            continue;
        }
        if gone.is_default() {
            // `default` stays; the sources its manifest held leave with it.
            let bare = PortalSpec {
                sources: Vec::new(),
                ..gone.clone()
            };
            if sync_sources(writer, orgs, Some(&gone), &bare).await {
                served.remove(&name);
            }
            continue;
        }
        if let Some((org, id)) = further_catalogue(&gone, orgs) {
            match writer.delete(org, id).await {
                Ok(_) => tracing::info!(portal = %name, "portal deleted"),
                Err(e) => {
                    tracing::warn!(portal = %name, error = %e, "portal not deleted; next round");
                    continue;
                }
            }
        }
        served.remove(&name);
    }
    for (name, portal) in now {
        if served.get(&name) == Some(&portal) {
            continue;
        }
        // `default` keeps the catalogue, grants and publish token jc bound at start.
        if portal.is_default() {
            if sync_sources(writer, orgs, served.get(&name), &portal).await {
                served.insert(name, portal);
            }
            continue;
        }
        // A portal moved to another path leaves its old catalogue.
        if let Some(old) = served.get(&name).filter(|old| old.path != portal.path)
            && let Some((org, id)) = further_catalogue(old, orgs)
            && let Err(e) = writer.delete(org, id).await
        {
            tracing::warn!(portal = %name, error = %e, "old path not deleted; next round");
            continue;
        }
        let written = match catalogue_of(&portal, orgs) {
            Ok(spec) => writer.upsert(spec).await.map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        match written {
            Ok(_) => {
                tracing::info!(portal = %name, path = %portal.path, "portal served");
                let before = served.get(&name).filter(|b| b.path == portal.path);
                if sync_sources(writer, orgs, before, &portal).await {
                    served.insert(name, portal);
                }
            }
            Err(e) => tracing::warn!(portal = %name, error = %e, "portal not served"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city() -> Vec<OrganizationPortals> {
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        vec![OrganizationPortals {
            name: "city".into(),
            org_id: OrgId::of_organization(installation, "city"),
            catalogue_id: CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c1")
                .unwrap(),
            scheme: "https".into(),
            opendata: "data.city.test".into(),
            listed: vec!["mobility.city.test".into()],
            harvest_hosts: vec!["data.kosice.sk".into()],
            config_dir: None,
            own_gateway: None,
        }]
    }

    #[test]
    fn a_served_own_portal_host_is_listed_live_and_a_pending_one_is_not() {
        let sites = joinedcontext_portal::portals::Sites::default();
        let row =
            |host: &str, org: &str, state: &str| joinedcontext_portal::host_registry::HostRow {
                host: host.into(),
                organization: org.into(),
                organization_id: None,
                role: "portal-site".into(),
                state: state.into(),
            };
        sites.replace(&[
            row("data2.city.test", "city", "served"),
            row("wait.city.test", "city", "dns-ok"),
            row("theirs.other.test", "other", "served"),
            row("mobility.city.test", "city", "served"),
        ]);
        let orgs = with_sites(city(), &sites);
        assert_eq!(orgs[0].listed, ["mobility.city.test", "data2.city.test"]);
        let portal = serde_json::from_value::<PortalSpec>(serde_json::json!({
            "organizationRef": "city", "path": "two", "host": "data2.city.test",
        }))
        .unwrap();
        assert_eq!(
            catalogue_of(&portal, &orgs).unwrap().host,
            "data2.city.test"
        );
        for host in ["wait.city.test", "theirs.other.test"] {
            let portal = PortalSpec {
                host: Some(host.into()),
                ..portal.clone()
            };
            assert!(catalogue_of(&portal, &orgs).is_err(), "{host}");
        }
    }

    #[test]
    fn the_installation_default_portal_is_served_without_organizations() {
        let template = OrganizationPortals {
            name: "ignored".into(),
            ..city().remove(0)
        };
        let only = [OrganizationPortals::installation(template)];
        assert!(
            named(&only, None).is_empty(),
            "no installation Organization yet: none"
        );
        let orgs = named(&only, Some("city".into()));
        let spec = catalogue_of(&portal("default", None), &orgs).unwrap();
        assert_eq!(spec.id, only[0].catalogue_id, "the [dataroom] catalogue");
        assert_eq!(spec.base.as_deref(), Some("https://data.city.test"));
        let further = catalogue_of(&portal("mobility", None), &orgs).unwrap();
        assert_eq!(
            further.base.as_deref(),
            Some("https://data.city.test/-/mobility")
        );
        let mut other = portal("default", None);
        other.organization_ref = "town".into();
        assert!(
            catalogue_of(&other, &orgs).is_err(),
            "another Organization is not served"
        );
    }

    fn portal(path: &str, host: Option<&str>) -> PortalSpec {
        serde_json::from_value(serde_json::json!({
            "organizationRef": "city", "path": path, "host": host,
        }))
        .unwrap()
    }

    /// Only a Portal manifest that reads and passes jc-core's check is served, and only an
    /// Endpoint with its Context Space and slug is a row to list.
    #[test]
    fn a_portal_that_does_not_read_and_an_endpoint_without_its_slug_are_left_out() {
        let mirror = Mirror::new();
        let put = |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| {
            mirror.upsert(
                serde_json::from_value(serde_json::json!({
                    "apiVersion": "joinedcontext.com/v1alpha1",
                    "kind": kind,
                    "metadata": { "name": name, "namespace": namespace },
                    "spec": spec,
                }))
                .unwrap(),
            );
        };
        let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
        put(
            "Portal",
            "good",
            org,
            serde_json::json!({"organizationRef": "city", "path": "mobility"}),
        );
        put(
            "Portal",
            "bad",
            org,
            serde_json::json!({"organizationRef": "city", "bogus": 1}),
        );
        assert_eq!(
            in_mirror(&mirror).keys().collect::<Vec<_>>(),
            ["good"],
            "a manifest that does not read is not served"
        );
        put(
            "Endpoint",
            "air",
            "doprava",
            serde_json::json!({"contextSpaceRef": "ovzdusie", "slug": "air-quality"}),
        );
        put(
            "Endpoint",
            "noslug",
            "doprava",
            serde_json::json!({"contextSpaceRef": "ovzdusie"}),
        );
        put(
            "Endpoint",
            "nospace",
            "doprava",
            serde_json::json!({"slug": "nospace"}),
        );
        let rows = endpoints_in(&mirror);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["air"],
            "{rows:?}"
        );
        assert!(!rows[0].published);
    }

    /// T-7520 C2: only an Organization the mirror holds and can read declares anything.
    #[test]
    fn a_catalogue_takes_the_publisher_its_organization_declares() {
        let orgs = city();
        let mirror = Mirror::new();
        assert!(declarations_in(&mirror, &orgs).is_empty(), "nothing synced");
        let put_titled = |spec: serde_json::Value, title: serde_json::Value| {
            mirror.upsert(
                serde_json::from_value(serde_json::json!({
                    "apiVersion": "joinedcontext.com/v1alpha1",
                    "kind": "Organization",
                    "metadata": {
                        "name": "city",
                        "namespace": joinedcontext_portal::permissions::ORG_NAMESPACE,
                        "title": title,
                    },
                    "spec": spec,
                }))
                .unwrap(),
            );
        };
        let put = |spec: serde_json::Value| {
            put_titled(spec, serde_json::json!({"sk": "Mesto", "en": "City"}));
        };
        put(serde_json::json!({"locales": "not a list"}));
        assert!(declarations_in(&mirror, &orgs).is_empty(), "not parsed");

        let base = serde_json::json!({
            "domain": "city.test", "locales": ["sk", "en"], "defaultLocale": "sk",
        });
        put(base.clone());
        let bare = &declarations_in(&mirror, &orgs)["city"];
        assert_eq!(bare.publisher_uri, Some(None), "no IRI is made up");
        assert_eq!(bare.publisher_name.as_ref().unwrap()["en"], "City");
        assert_eq!(bare.contact_point, None);

        let with = |uri: &str| {
            let mut spec = base.clone();
            spec["catalog"] = serde_json::json!({
                "publisher": {"name": {"sk": "Mesto Košice"}, "uri": uri},
                "contactPoint": {"name": "Dátová kancelária", "email": "data@city.test"},
            });
            put(spec);
            declarations_in(&mirror, &orgs)["city"].clone()
        };
        let uri = "https://data.gov.sk/id/legal-subject/00691135";
        let declared = with(uri);
        assert_eq!(declared.publisher_uri, Some(Some(uri.to_owned())));
        assert_eq!(declared.publisher_name.unwrap()["sk"], "Mesto Košice");
        assert_eq!(
            declared.contact_point.unwrap().name["sk"],
            "Dátová kancelária"
        );
        let placeholder = with("https://data.gov.sk/id/legal-subject/00000000");
        assert_eq!(
            placeholder.publisher_uri,
            Some(None),
            "a placeholder is no publisher"
        );
        // A title given as one text names the publisher in the default locale.
        put_titled(base, serde_json::json!("Mesto"));
        let one = &declarations_in(&mirror, &orgs)["city"];
        assert_eq!(one.publisher_name.as_ref().unwrap()["sk"], "Mesto");
    }

    #[test]
    fn the_default_portal_keeps_its_catalogue_id() {
        let orgs = city();
        let spec = catalogue_of(&portal("default", None), &orgs).unwrap();
        assert_eq!(spec.id, orgs[0].catalogue_id, "the configured catalogue_id");
        assert_eq!(spec.host, "data.city.test");
        assert_eq!(spec.base.as_deref(), Some("https://data.city.test"));
        assert_eq!(spec.organization.as_deref(), Some("city"));
        assert!(spec.portal.is_none() && !spec.publishes);
    }

    #[test]
    fn a_further_portal_gets_a_stable_catalogue_and_its_slug_address() {
        let orgs = city();
        let spec = catalogue_of(&portal("mobility", None), &orgs).unwrap();
        assert_eq!(spec.id, CatalogueId::of_portal(orgs[0].org_id, "mobility"));
        assert_ne!(spec.id, orgs[0].catalogue_id);
        assert_eq!(spec.host, "mobility.city.portal.invalid");
        assert_eq!(
            spec.base.as_deref(),
            Some("https://data.city.test/-/mobility")
        );
        assert!(
            spec.organization.is_none(),
            "never the Organization's own catalogue"
        );
        let own = catalogue_of(&portal("mobility", Some("mobility.city.test")), &orgs).unwrap();
        assert_eq!(own.id, spec.id, "a new address keeps the catalogue");
        assert_eq!(own.base.as_deref(), Some("https://mobility.city.test"));
    }

    #[test]
    fn each_harvested_source_becomes_a_harvest_source_in_list_order() {
        let orgs = city();
        let mut region = portal("region", None);
        region.sources = serde_json::from_value(serde_json::json!([
            { "name": "own", "type": "endpoints", "projects": ["doprava"] },
            { "name": "kosice", "type": "dcat-ap", "url": "https://data.kosice.sk/catalog.ttl",
              "allowedHosts": ["data.kosice.sk"], "budget": { "records": 10, "bytes": 1000 },
              "publisher": "kosice", "publisherTitle": "Mesto Košice" },
            { "name": "nkod", "type": "nkod-sparql", "url": "https://data.kosice.sk/sparql",
              "allowedHosts": ["data.kosice.sk"] },
        ]))
        .unwrap();
        let wanted = harvest_sources_of(&region, &orgs).unwrap();
        assert_eq!(wanted.len(), 2, "an endpoints source harvests nothing");
        let (kosice, publisher) = &wanted[0];
        assert_eq!(
            kosice.catalogue_id,
            CatalogueId::of_portal(orgs[0].org_id, "region")
        );
        assert_eq!((kosice.id.as_str(), kosice.position), ("hs-kosice", 1));
        assert_eq!(
            kosice.budget,
            Some(HarvestBudget {
                records: 10,
                bytes: 1000
            })
        );
        assert_eq!(
            (publisher.name.as_str(), kosice.publisher_id.as_str()),
            ("kosice", "kosice")
        );
        assert_eq!(publisher.title["en"], "Mesto Košice");
        let (nkod, publisher) = &wanted[1];
        assert_eq!((nkod.position, publisher.name.as_str()), (2, "city"));
        assert_eq!(nkod.schedule, "23 3 * * *");
        let default = portal("default", None);
        assert!(harvest_sources_of(&default, &orgs).unwrap().is_empty());
    }

    #[test]
    fn allowed_hosts_outside_the_deployment_list_are_refused() {
        let orgs = city();
        let mut region = portal("region", None);
        region.sources = serde_json::from_value(serde_json::json!([
            { "name": "far", "type": "dcat-ap", "url": "https://data.praha.eu/catalog.ttl",
              "allowedHosts": ["data.praha.eu", "files.praha.eu"] },
        ]))
        .unwrap();
        let err = harvest_sources_of(&region, &orgs).unwrap_err();
        assert!(
            err.contains("files.praha.eu is not in the deployment's harvest allow-list"),
            "{err}"
        );
        // The url's own host alone is the admin's, no operator listing (T-7416).
        region.sources[0].allowed_hosts = vec!["data.praha.eu".into()];
        let sources = harvest_sources_of(&region, &orgs).expect("own host");
        assert_eq!(sources[0].0.allowed_hosts, ["data.praha.eu"]);
    }

    #[test]
    fn a_host_not_listed_for_the_organization_is_refused() {
        let orgs = city();
        let err = catalogue_of(&portal("mobility", Some("data.town.test")), &orgs).unwrap_err();
        assert!(err.contains("not listed"), "{err}");
        let mut other = portal("mobility", None);
        other.organization_ref = "town".into();
        assert!(
            catalogue_of(&other, &orgs).is_err(),
            "an Organization jc does not serve"
        );
    }

    /// The harvest sources of a portal whose Organization jc does not serve, and of a converted
    /// file with no `[dataroom]` directory or no such file, are refused with the source named.
    #[test]
    fn a_source_jc_cannot_harvest_is_refused_with_its_name() {
        let mut orgs = city();
        let mut stranger = portal("mobility", None);
        stranger.organization_ref = "town".into();
        let err = harvest_sources_of(&stranger, &orgs).unwrap_err();
        assert_eq!(err, "no [[organizations]] town");
        let mut converted = portal("mobility", None);
        converted.sources = serde_json::from_value(serde_json::json!([
            { "name": "parks", "type": "dcat-ap", "url": "https://data.kosice.sk/parks.ttl",
              "convertedFile": "parks.json" },
        ]))
        .unwrap();
        let err = harvest_sources_of(&converted, &orgs).unwrap_err();
        assert_eq!(
            err,
            "source parks: convertedFile, but jc.toml has no [dataroom]"
        );
        let dir = tempfile::tempdir().unwrap();
        orgs[0].config_dir = Some(dir.path().to_path_buf());
        let err = harvest_sources_of(&converted, &orgs).unwrap_err();
        assert!(err.starts_with("source parks: "), "{err}");
    }
}

/// The writer half of the reconciler and the Portal's desks on a memory dataroom (T-7820):
/// each round writes, moves and deletes what it should, and each desk call reaches its own
/// portal's catalogue alone.
#[cfg(test)]
mod desk_tests {
    use super::*;
    use dataroom_core::LiveAuthorizer;
    use dataroom_core::files::FileStore;
    use dataroom_store_memory::MemoryStore;
    use std::sync::Arc;

    fn city() -> Vec<OrganizationPortals> {
        let installation = OrgId::parse_verified("00000000-0000-7000-8000-000000000001").unwrap();
        vec![OrganizationPortals {
            name: "city".into(),
            org_id: OrgId::of_organization(installation, "city"),
            catalogue_id: CatalogueId::parse_verified("00000000-0000-7000-8000-0000000000c1")
                .unwrap(),
            scheme: "https".into(),
            opendata: "data.city.test".into(),
            listed: vec!["mobility.city.test".into()],
            harvest_hosts: vec!["data.kosice.sk".into()],
            config_dir: None,
            own_gateway: None,
        }]
    }

    fn portal(path: &str, sources: serde_json::Value) -> PortalSpec {
        serde_json::from_value(serde_json::json!({
            "organizationRef": "city", "path": path, "sources": sources,
        }))
        .unwrap()
    }

    /// A source the harvest guard refuses before any request (a loopback address).
    fn loopback() -> serde_json::Value {
        serde_json::json!([{ "name": "local", "type": "dcat-ap",
            "url": "http://127.0.0.1:9/catalog.ttl", "allowedHosts": ["127.0.0.1"] }])
    }

    fn writer(store: Arc<dyn dataroom_core::Store>, files: &std::path::Path) -> CatalogueWriter {
        let orgs = city();
        let authorizer = Arc::new(LiveAuthorizer::new(Vec::new()));
        let state = dataroom_api::DataroomState::new(
            store,
            authorizer.clone(),
            orgs[0].org_id,
            orgs[0].catalogue_id,
        )
        .with_files(Arc::new(FileStore::new(files.to_path_buf())));
        CatalogueWriter::new(state, authorizer, Arc::default())
    }

    fn served(names: &[(&str, PortalSpec)]) -> BTreeMap<String, PortalSpec> {
        names
            .iter()
            .map(|(n, p)| ((*n).to_owned(), p.clone()))
            .collect()
    }

    async fn sources(w: &CatalogueWriter, path: &str) -> Vec<String> {
        let (org, id) = (
            city()[0].org_id,
            CatalogueId::of_portal(city()[0].org_id, path),
        );
        let answer = w
            .run_op(org, id, "harvest_source.list", serde_json::json!({}))
            .await
            .expect("the catalogue is served");
        let mut names: Vec<String> = answer["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["name"].as_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    /// Without an Organization there is nothing to follow: no task starts.
    #[tokio::test]
    async fn no_organization_starts_no_follower() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn dataroom_core::Store> =
            Arc::new(dataroom_store_memory::MemoryStore::new());
        let tasks = TaskTracker::new();
        follow(
            writer(store, dir.path()),
            Vec::new(),
            &CancellationToken::new(),
            &tasks,
        );
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn a_round_writes_moves_and_deletes_further_portals_and_their_sources() {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(Arc::new(MemoryStore::new()), dir.path());
        let orgs = city();
        let mut done = BTreeMap::new();
        let two = serde_json::json!([
            { "name": "a", "type": "dcat-ap", "url": "https://data.kosice.sk/a.ttl" },
            { "name": "b", "type": "dcat-ap", "url": "https://data.kosice.sk/b.ttl" },
        ]);
        let mobility = portal("mobility", two);
        apply(&w, &orgs, &mut done, served(&[("m", mobility.clone())])).await;
        assert_eq!(done.get("m"), Some(&mobility));
        assert_eq!(sources(&w, "mobility").await, ["a", "b"]);
        // The same manifest again writes nothing new.
        apply(&w, &orgs, &mut done, served(&[("m", mobility.clone())])).await;
        // A source left out of the manifest is deleted, an endpoints source is never harvested.
        let fewer = portal(
            "mobility",
            serde_json::json!([
                { "name": "a", "type": "dcat-ap", "url": "https://data.kosice.sk/a.ttl" },
                { "name": "own", "type": "endpoints" },
            ]),
        );
        apply(&w, &orgs, &mut done, served(&[("m", fewer.clone())])).await;
        assert_eq!(sources(&w, "mobility").await, ["a"]);
        // A move to another path leaves the old catalogue.
        let moved = portal("transit", serde_json::json!([]));
        apply(&w, &orgs, &mut done, served(&[("m", moved.clone())])).await;
        assert_eq!(done.get("m"), Some(&moved));
        let old = CatalogueId::of_portal(orgs[0].org_id, "mobility");
        assert!(
            w.datasets(orgs[0].org_id, old).await.is_err(),
            "the old path is gone"
        );
        // A portal that left the manifests is deleted.
        apply(&w, &orgs, &mut done, BTreeMap::new()).await;
        assert!(done.is_empty());
        let gone = CatalogueId::of_portal(orgs[0].org_id, "transit");
        assert!(w.datasets(orgs[0].org_id, gone).await.is_err());
    }

    /// A dataroom that refuses each write in turn: the round keeps the portal it last wrote, so
    /// the next round tries again, and a declaration that cannot be read is left for the next.
    #[tokio::test]
    async fn a_store_that_refuses_keeps_the_last_written_portal_for_the_next_round() {
        use crate::modules::faulty_dataroom::FaultyDataroom;
        let dir = tempfile::tempdir().unwrap();
        let faults = Arc::new(FaultyDataroom::new(Arc::new(MemoryStore::new())));
        let w = writer(faults.clone(), dir.path());
        let orgs = city();
        let mut done = BTreeMap::new();
        let mobility = portal(
            "mobility",
            serde_json::json!([
                { "name": "a", "type": "dcat-ap", "url": "https://data.kosice.sk/a.ttl" },
                { "name": "b", "type": "dcat-ap", "url": "https://data.kosice.sk/b.ttl" },
            ]),
        );
        apply(&w, &orgs, &mut done, served(&[("m", mobility.clone())])).await;
        assert_eq!(done.get("m"), Some(&mobility));
        // Declarations: one for `city` (its own and its further catalogue), none for `town`.
        let mut town = city();
        town[0].name = "town".into();
        let both: Vec<OrganizationPortals> = orgs.iter().cloned().chain(town).collect();
        let declaration = CatalogueDeclaration {
            publisher_uri: Some(Some("https://city.test/id/city".into())),
            ..CatalogueDeclaration::default()
        };
        let declared = BTreeMap::from([("city".to_owned(), declaration)]);
        declare(&w, &both, &done, &declared).await;
        let further = CatalogueId::of_portal(orgs[0].org_id, "mobility");
        let held = dataroom_core::Store::get_catalogue(faults.as_ref(), orgs[0].org_id, further)
            .await
            .unwrap()
            .expect("the further catalogue");
        assert_eq!(
            held.publisher_uri.as_deref(),
            Some("https://city.test/id/city")
        );
        // The default portal declares into the Organization's own catalogue.
        dataroom_server::ensure_catalogue(
            faults.as_ref(),
            orgs[0].org_id,
            orgs[0].catalogue_id,
            "city",
            "data.city.test",
        )
        .await
        .expect("the own catalogue is made");
        let mut with_default = done.clone();
        with_default.insert("d".into(), portal("default", serde_json::json!([])));
        declare(&w, &both, &with_default, &declared).await;
        let own = dataroom_core::Store::get_catalogue(
            faults.as_ref(),
            orgs[0].org_id,
            orgs[0].catalogue_id,
        )
        .await
        .unwrap()
        .expect("the own catalogue");
        assert_eq!(
            own.publisher_uri.as_deref(),
            Some("https://city.test/id/city")
        );
        // A source that cannot be deleted keeps the manifest that held it.
        faults.fail("delete_harvest_source");
        let fewer = portal(
            "mobility",
            serde_json::json!([
                { "name": "a", "type": "dcat-ap", "url": "https://data.kosice.sk/a.ttl" },
            ]),
        );
        apply(&w, &orgs, &mut done, served(&[("m", fewer)])).await;
        assert_eq!(done.get("m"), Some(&mobility));
        // A catalogue that cannot be deleted keeps a moved portal and one that left.
        faults.fail("delete_catalogue");
        let moved = portal("transit", serde_json::json!([]));
        apply(&w, &orgs, &mut done, served(&[("m", moved)])).await;
        assert_eq!(done.get("m"), Some(&mobility));
        apply(&w, &orgs, &mut done, BTreeMap::new()).await;
        assert_eq!(done.get("m"), Some(&mobility));
        // A catalogue that cannot be read is not declared; the next round tries again.
        faults.fail("get_catalogue");
        declare(&w, &both, &done, &declared).await;
    }

    #[tokio::test]
    async fn a_refused_portal_or_source_is_left_for_the_next_round() {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(Arc::new(MemoryStore::new()), dir.path());
        let orgs = city();
        let mut done = BTreeMap::new();
        // A host the Organization does not list: never served.
        let mut far: PortalSpec = portal("far", serde_json::json!([]));
        far.host = Some("data.town.test".into());
        // A source outside the harvest allow-list: the catalogue is written, the round retried.
        let wide = portal(
            "wide",
            serde_json::json!([{ "name": "x", "type": "dcat-ap",
                "url": "https://data.praha.eu/c.ttl", "allowedHosts": ["files.praha.eu"] }]),
        );
        // A portal of an Organization jc does not serve.
        let mut stranger = portal("other", serde_json::json!([]));
        stranger.organization_ref = "town".into();
        apply(
            &w,
            &orgs,
            &mut done,
            served(&[
                ("far", far),
                ("wide", wide.clone()),
                ("x", stranger.clone()),
            ]),
        )
        .await;
        assert!(done.is_empty(), "{:?}", done.keys().collect::<Vec<_>>());
        assert!(!sync_sources(&w, &orgs, None, &stranger).await);
        assert!(!sync_sources(&w, &orgs, None, &wide).await);
    }

    #[tokio::test]
    async fn the_default_portal_keeps_its_catalogue_and_drops_only_its_sources() {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(Arc::new(MemoryStore::new()), dir.path());
        let orgs = city();
        let mut done = BTreeMap::new();
        let default = portal("default", serde_json::json!([]));
        apply(&w, &orgs, &mut done, served(&[("d", default.clone())])).await;
        assert!(
            done.contains_key("d"),
            "a default without sources writes nothing"
        );
        assert!(further_catalogue(&default, &orgs).is_none());
        apply(&w, &orgs, &mut done, BTreeMap::new()).await;
        assert!(
            done.is_empty(),
            "default left the manifests: its sources went"
        );
        // A default naming a source before its own catalogue is routed: the source is not
        // written and the next round tries again.
        let with = portal("default", loopback());
        apply(&w, &orgs, &mut done, served(&[("d", with.clone())])).await;
        assert!(done.is_empty(), "retried next round");
    }

    /// With the Portal in this process each round reads its mirror: a Portal manifest there is
    /// written with its sources and one that does not read is left out.
    #[tokio::test(start_paused = true)]
    async fn a_round_writes_the_portals_the_mirror_of_this_processs_portal_holds() {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(Arc::new(MemoryStore::new()), dir.path());
        let _ = crate::modules::PORTAL_STATE.set(joinedcontext_portal::state::AppState::new(
            joinedcontext_portal::config::Config::for_tests(),
            None,
        ));
        let state = crate::modules::PORTAL_STATE
            .get()
            .expect("the Portal state");
        let envelope = |name: &str, spec: serde_json::Value| {
            joinedcontext_portal::resource::ResourceEnvelope {
                api_version: jc_core::API_VERSION.to_owned(),
                kind: "Portal".to_owned(),
                metadata: jc_core::ObjectMeta::new(
                    name,
                    joinedcontext_portal::permissions::ORG_NAMESPACE,
                ),
                spec,
                status: None,
            }
        };
        state.mirror.upsert(envelope(
            "m",
            serde_json::json!({ "organizationRef": "city", "path": "mobility", "sources": [
                { "name": "a", "type": "dcat-ap", "url": "https://data.kosice.sk/a.ttl",
                  "allowedHosts": ["data.kosice.sk"] },
            ] }),
        ));
        state
            .mirror
            .upsert(envelope("broken", serde_json::json!({ "path": 7 })));
        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        let (file, guard) = crate::modules::capture_log();
        follow(w.clone(), city(), &shutdown, &tasks);
        // Two rounds: the second finds the same Organizations and keeps the desks it handed.
        tokio::time::sleep(
            super::super::appearance::FOLLOW_EVERY + std::time::Duration::from_secs(1),
        )
        .await;
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
        drop(guard);
        let log = std::fs::read_to_string(file.path()).unwrap();
        assert!(log.contains("Portal refused; not served"), "{log}");
        assert!(log.contains("portal served"), "{log}");
        assert_eq!(sources(&w, "mobility").await, ["a"]);
    }

    /// A run the store refuses to claim ends at once: the failure is logged and the source is
    /// free for the next start; a source test answers its outcome word.
    #[tokio::test]
    async fn a_run_the_store_refuses_is_logged_and_frees_its_source() {
        use crate::modules::faulty_dataroom::FaultyDataroom;
        let dir = tempfile::tempdir().unwrap();
        let faults = Arc::new(FaultyDataroom::new(Arc::new(MemoryStore::new())));
        let w = writer(faults.clone(), dir.path());
        let orgs = city();
        let mobility = portal("mobility", loopback());
        apply(
            &w,
            &orgs,
            &mut BTreeMap::new(),
            served(&[("m", mobility.clone())]),
        )
        .await;
        let desk = Desk {
            writer: w,
            orgs,
            active: Arc::default(),
        };
        let tested = SourceDesk::test(&desk, &mobility, "local")
            .await
            .expect("a served source answers");
        assert!(!tested.outcome.is_empty(), "{tested:?}");
        faults.fail("claim_harvest_run");
        let (file, guard) = crate::modules::capture_log();
        desk.start(&mobility, "local").await.expect("started");
        for _ in 0..100 {
            if desk.active.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        drop(guard);
        assert!(desk.active.lock().unwrap().is_empty(), "the source is free");
        let log = std::fs::read_to_string(file.path()).unwrap();
        assert!(log.contains("portal source run failed"), "{log}");
        assert!(log.contains("injected dataroom store failure"), "{log}");
    }

    #[tokio::test]
    async fn the_desks_reach_their_own_portals_catalogue_alone() {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(Arc::new(MemoryStore::new()), dir.path());
        let orgs = city();
        let mut done = BTreeMap::new();
        let mobility = portal("mobility", loopback());
        apply(&w, &orgs, &mut done, served(&[("m", mobility.clone())])).await;
        let desk = Desk {
            writer: w.clone(),
            orgs: orgs.clone(),
            active: Arc::default(),
        };
        let mut stranger = portal("mobility", serde_json::json!([]));
        stranger.organization_ref = "town".into();
        let unserved = portal("unserved", serde_json::json!([]));

        // Sources.
        assert_eq!(
            SourceDesk::test(&desk, &stranger, "local")
                .await
                .unwrap_err(),
            DeskError::NotFound
        );
        assert!(SourceDesk::test(&desk, &unserved, "local").await.is_err());
        assert!(SourceDesk::test(&desk, &mobility, "nope").await.is_err());
        let tested = SourceDesk::test(&desk, &mobility, "local").await;
        assert!(
            !matches!(tested, Err(DeskError::NotFound)),
            "a served source answers: {tested:?}"
        );
        assert_eq!(
            desk.runs(&mobility, "local").await.unwrap(),
            Vec::<serde_json::Value>::new()
        );
        assert!(desk.runs(&stranger, "local").await.is_err());
        // A start holds the source until its run ends: a second start meanwhile is busy.
        desk.active.lock().unwrap().insert((
            CatalogueId::of_portal(orgs[0].org_id, "mobility"),
            "local".into(),
        ));
        assert_eq!(
            desk.start(&mobility, "local").await.unwrap_err(),
            DeskError::Busy
        );
        desk.active.lock().unwrap().clear();
        desk.start(&mobility, "local").await.expect("started");
        for _ in 0..100 {
            if desk.active.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(desk.active.lock().unwrap().is_empty(), "the run ended");
        assert!(desk.start(&stranger, "local").await.is_err());

        // Datasets.
        let page = desk.datasets(&mobility, 0, 10).await.unwrap();
        assert_eq!((page.total, page.datasets.len()), (0, 0));
        assert_eq!(
            desk.datasets(&stranger, 0, 10).await.unwrap_err(),
            DeskError::NotFound
        );

        // Files.
        let (files, limits) = FileDesk::list(&desk, &mobility).await.unwrap();
        assert!(files.is_empty() && limits.files == 0);
        let up = desk
            .upload(&mobility, "ana", "plan.csv", b"a,b\n1,2\n".to_vec())
            .await
            .unwrap();
        assert_eq!(
            (up.name.as_str(), up.uploaded_by.as_str()),
            ("plan.csv", "ana")
        );
        let renamed = desk
            .rename(&mobility, "ana", &up.id, "plan-2026.csv")
            .await
            .unwrap();
        assert_eq!(renamed.name, "plan-2026.csv");
        let replaced = desk
            .replace(&mobility, "ana", &up.id, b"a,b\n3,4\n".to_vec())
            .await
            .unwrap();
        assert_ne!(replaced.sha256, up.sha256);
        assert_eq!(FileDesk::list(&desk, &mobility).await.unwrap().0.len(), 1);
        FileDesk::delete(&desk, &mobility, "ana", &up.id)
            .await
            .unwrap();
        assert!(matches!(
            FileDesk::delete(&desk, &mobility, "ana", &up.id).await,
            Err(FileRefusal::Desk(DeskError::NotFound))
        ));
        // A name the store refuses names the problem, without the field prefix.
        assert!(matches!(
            desk.upload(&mobility, "ana", "../x.exe", b"MZ".to_vec())
                .await,
            Err(FileRefusal::Invalid(_))
        ));
        assert!(matches!(
            FileDesk::list(&desk, &stranger).await,
            Err(DeskError::NotFound)
        ));
        assert!(matches!(
            desk.upload(&stranger, "ana", "plan.csv", Vec::new()).await,
            Err(FileRefusal::Desk(DeskError::NotFound))
        ));
    }

    fn dataset(org: OrgId, cat: CatalogueId, name: &str, extra: serde_json::Value) -> Dataset {
        let mut v = serde_json::json!({
            "orgId": org, "catalogueId": cat, "id": "", "name": name,
            "title": {"sk": name}, "description": {}, "publisherId": "city",
            "visibility": "public", "state": "active", "tags": [], "distributions": [],
            "issued": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            v[key] = value.clone();
        }
        serde_json::from_value(v).expect("a dataset")
    }

    /// ODP-10: a further portal's `endpoints` source lists the published Endpoints' datasets of
    /// its Organization's `default` catalogue, withdraws them when the source goes, and a
    /// portal whose catalogue is not served is retried next round.
    #[tokio::test]
    async fn an_endpoints_source_lists_published_datasets_and_withdraws_them() {
        use dataroom_core::{RequestCtx, Store, Subject};
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::new());
        let w = writer(store.clone(), dir.path());
        let orgs = city();
        let (org, own) = (orgs[0].org_id, orgs[0].catalogue_id);
        w.upsert(catalogue_of(&portal("default", serde_json::json!([])), &orgs).unwrap())
            .await
            .expect("the default catalogue is served");
        let ctx = RequestCtx::new(org, own, Subject::anonymous());
        let published = serde_json::json!({
            "generatedBy": PUBLISHED_BY,
            "endpointUrl": "https://api.city.test/endpoint/air-quality/",
            "tags": ["air"],
        });
        store
            .upsert_dataset(&ctx, dataset(org, own, "air", published))
            .await
            .unwrap();
        store
            .upsert_dataset(&ctx, dataset(org, own, "handmade", serde_json::json!({})))
            .await
            .unwrap();
        let listing_portal = portal(
            "mobility",
            serde_json::json!([{ "name": "own", "type": "endpoints", "tags": ["air"] }]),
        );
        let mut done = BTreeMap::new();
        apply(
            &w,
            &orgs,
            &mut done,
            served(&[("m", listing_portal.clone())]),
        )
        .await;
        let endpoints = [EndpointRow {
            project: "doprava".into(),
            space: "ovzdusie".into(),
            name: "air".into(),
            slug: "air-quality".into(),
            published: true,
        }];
        let mut listing = BTreeSet::new();
        list_endpoints(&w, &orgs, &done, &endpoints, &mut listing).await;
        assert!(listing.contains("m"));
        let desk = Desk {
            writer: w.clone(),
            orgs: orgs.clone(),
            active: Arc::default(),
        };
        let page = desk.datasets(&listing_portal, 0, 10).await.unwrap();
        assert_eq!(page.total, 1, "the published dataset alone");
        assert_eq!(page.datasets[0].name, "air");
        assert_eq!(page.datasets[0].title, "air");
        // The source left the manifest: the copies are withdrawn and the portal leaves the list.
        let bare = portal("mobility", serde_json::json!([]));
        apply(&w, &orgs, &mut done, served(&[("m", bare)])).await;
        list_endpoints(&w, &orgs, &done, &endpoints, &mut listing).await;
        assert!(listing.is_empty());
        assert_eq!(
            desk.datasets(&listing_portal, 0, 10).await.unwrap().total,
            0
        );
        // A portal no round served yet: kept in the list so the next round lists it.
        let ghost = served(&[(
            "g",
            portal(
                "ghost",
                serde_json::json!([{ "name": "own", "type": "endpoints" }]),
            ),
        )]);
        list_endpoints(&w, &orgs, &ghost, &endpoints, &mut listing).await;
        assert!(listing.contains("g"));
        // Nothing to list: a portal without an endpoints source that no round listed, and one of
        // an Organization jc does not serve.
        let stranger: PortalSpec = serde_json::from_value(serde_json::json!({
            "organizationRef": "elsewhere", "path": "other",
            "sources": [{ "name": "own", "type": "endpoints" }],
        }))
        .unwrap();
        let quiet = served(&[
            ("m", portal("mobility", serde_json::json!([]))),
            ("s", stranger.clone()),
        ]);
        let mut listing = BTreeSet::new();
        list_endpoints(&w, &orgs, &quiet, &endpoints, &mut listing).await;
        assert!(listing.is_empty(), "{listing:?}");
        // A portal of an Organization jc does not serve leaves the round without a delete.
        let mut done = served(&[("s", stranger)]);
        apply(&w, &orgs, &mut done, BTreeMap::new()).await;
        assert!(done.is_empty());
    }

    #[test]
    fn each_dataroom_error_maps_to_its_desk_answer() {
        use dataroom_core::DataroomError as E;
        assert_eq!(desk_error(E::NotFound("x".into())), DeskError::NotFound);
        assert_eq!(desk_error(E::Conflict("x".into())), DeskError::Busy);
        assert!(matches!(
            desk_error(E::Storage("down".into())),
            DeskError::Failed(why) if why.contains("down")
        ));
        let invalid = E::Validation {
            message: "file: too big".into(),
            fields: BTreeMap::from([("file".into(), vec!["too big".into()])]),
        };
        assert_eq!(
            file_refusal(invalid),
            FileRefusal::Invalid("too big".into())
        );
        let bare = E::Validation {
            message: "refused".into(),
            fields: BTreeMap::new(),
        };
        assert_eq!(file_refusal(bare), FileRefusal::Invalid("refused".into()));
        assert_eq!(
            file_refusal(E::NotFound("f".into())),
            FileRefusal::Desk(DeskError::NotFound)
        );
    }
}
