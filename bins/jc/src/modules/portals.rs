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
//! ponytail: polled every [`super::appearance::FOLLOW_EVERY`] from the one Portal of this
//! process, as the appearance is; upgrade path: the Portal's reconciler calls the writer.

use dataroom_core::files::FileEntry;
use dataroom_core::{
    CatalogueId, Dataset, HarvestBudget, HarvestSource, OrgId, Publisher, RecordState,
};
use dataroom_server::{CatalogueSpec, CatalogueWriter, FileWrite, PortalRoute};
use jc_core::kinds::{PortalSource, PortalSourceType, PortalSpec, http_url_host};
use joinedcontext_portal::api::portal_files::{
    FileDesk, FileFuture, FileLimits, FileRefusal, PortalFile,
};
use joinedcontext_portal::api::portal_sources::{
    DeskDataset, DeskError, DeskFuture, SourceDesk, SourceTestAnswer,
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
        // The Portal publishes Endpoints into an Organization's `default` with the token jc
        // bound at start; a further portal's datasets come from its sources (ODP-10).
        publish_token: None,
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
                let orgs = named(&orgs, installation);
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
            Ok(match answer["runs"].take() {
                serde_json::Value::Array(runs) => runs,
                _ => Vec::new(),
            })
        })
    }

    fn datasets<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, Vec<DeskDataset>> {
        Box::pin(async move {
            let (org, id) = self.key(portal)?;
            let all = self.writer.datasets(org, id).await.map_err(desk_error)?;
            Ok(all
                .into_iter()
                .filter(|d| d.state == RecordState::Active)
                .map(|d| DeskDataset {
                    title: dataroom_core::model::text_in_lang(&d.title, None)
                        .unwrap_or(&d.name)
                        .to_owned(),
                    source: d.harvest_source_id,
                    name: d.name,
                })
                .collect())
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
        dataroom_core::DataroomError::Validation { message, .. } => FileRefusal::Invalid(message),
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

    #[test]
    fn the_default_portal_keeps_its_catalogue_id() {
        let orgs = city();
        let spec = catalogue_of(&portal("default", None), &orgs).unwrap();
        assert_eq!(spec.id, orgs[0].catalogue_id, "the configured catalogue_id");
        assert_eq!(spec.host, "data.city.test");
        assert_eq!(spec.base.as_deref(), Some("https://data.city.test"));
        assert_eq!(spec.organization.as_deref(), Some("city"));
        assert!(spec.portal.is_none() && spec.publish_token.is_none());
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
}
