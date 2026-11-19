//! The open-data catalogue as an NGSI-LD Endpoint (T-4500, ADR-N-025, EP-78).
//!
//! An agent's first call goes to the Endpoint named `catalog` on the space named `catalog`, where
//! it finds the dataset it needs with the ordinary NGSI-LD queries; its second call goes to that
//! dataset's own Endpoint. This wave writes that catalogue (EP-140, T-4746): one SDM DCAT-AP
//! `Dataset` per public CKAN dataset, named as CKAN names it, one `Organization` per CKAN
//! organization that publishes one, one `DataService` per public Endpoint, and a `Dataset` of its
//! own for a public Endpoint no CKAN dataset names.
//!
//! Three rules, each from what goes wrong without it:
//!
//! - **Read, never listed.** The Endpoints come from the gateway's DCAT-AP feed (EP-84), which
//!   holds the record of every public Endpoint as the anonymous caller reads it, and the datasets
//!   from CKAN's own `package_search`, read anonymously, so only public ones: a dataset added,
//!   renamed or withdrawn in CKAN is here at the next run, and no private title, contact or count
//!   can reach the catalogue. A CKAN that does not answer fails the run, which then writes
//!   nothing, rather than turn every CKAN dataset back into its Endpoint's.
//! - **Every write goes through an Endpoint and its Policy**, as the Portal's own client, the way
//!   the drift wave puts a seed entity back. The `catalog` Endpoint's Policy is what admits it.
//! - **Only a change is written.** A run compares with what the last run wrote and sends only the
//!   difference; after a restart, and once an hour, it lists what the catalogue holds instead of
//!   trusting its memory, so an entity a half-finished run left behind is still removed.

use super::drift::Watch;
use crate::resource::reference_name;
use futures_util::StreamExt as _;
pub use jc_core::ngsi_catalog::{entities, fold, link, Ckan, Package, Place};
use jcctl::loader::Repository;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The name of the catalogue's Endpoint and of its space.
pub const CATALOG: &str = "catalog";
/// The catalogue's write door: an organization Endpoint of the same space. The public `catalog`
/// Endpoint is open data, and open data is read-only (EP-187), so the wave writes here when the
/// project declares it.
pub const CATALOG_WRITE: &str = "catalog-write";
/// Entities per batch request.
const BATCH: usize = 100;
/// Datasets per page of CKAN's `package_search`, its own ceiling, and the pages read at most.
const CKAN_PAGE: usize = 1000;
const CKAN_PAGES: usize = 20;
/// How long one page of CKAN's `package_search` may take.
pub const CKAN_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the memory of the last write is trusted before the catalogue is listed again.
const RELIST: Duration = Duration::from_secs(3600);

/// An Endpoint's entity types, each with how many entities it holds, most first.
type Counts = Vec<(String, u64)>;
/// What a run left in one catalogue, by id, and when that catalogue was last listed.
type Memory = (Instant, BTreeMap<String, Value>);

/// What one run wrote.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Entities created or replaced.
    pub upserted: usize,
    /// Entities removed.
    pub deleted: usize,
}

/// The catalogue wave of one Portal.
pub struct CatalogSync {
    watch: Arc<Watch>,
    /// The client of the CKAN reads: a `CkanInstance` names its URL, so the read is an outside
    /// one, guarded as the catalogue page's is (T-5335).
    ckan: reqwest::Client,
    /// Per catalogue Endpoint slug, what the last run left in it, by id, and when it was last
    /// listed.
    written: Mutex<HashMap<String, Memory>>,
    /// Per public Endpoint slug, its entity types and their counts as an anonymous caller reads
    /// them, and when they were read (T-4526).
    counts: Mutex<HashMap<String, (Instant, Counts)>>,
}

impl CatalogSync {
    /// A wave writing through the Portal's own client on the gateway `watch` speaks to, reading
    /// CKAN through `ckan`: [`crate::api::catalogue::ckan_client`] in the Portal.
    pub fn new(watch: Arc<Watch>, ckan: reqwest::Client) -> Self {
        Self {
            watch,
            ckan,
            written: Mutex::new(HashMap::new()),
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// Brings every catalogue Endpoint of `repo` to its public Endpoints: the installation's to
    /// all of them, a project's to its own. `Ok(None)` when the repository declares no
    /// catalogue Endpoint, which leaves this wave off.
    pub async fn converge(&self, repo: &Repository) -> Result<Option<Run>, String> {
        let targets = catalogues(repo)?;
        if targets.is_empty() {
            return Ok(None);
        }
        let (http, base) = self.watch.surface();
        let feed: Value = http
            .get(format!("{base}/catalog.jsonld"))
            .header("Accept", "application/ld+json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| format!("the DCAT-AP feed did not read: {err}"))?
            .json()
            .await
            .map_err(|err| format!("the DCAT-AP feed is not JSON: {err}"))?;
        let nodes = feed
            .get("@included")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let places = places(repo);
        let sites = ckan_sites(repo);
        let mut wanted = BTreeMap::new();
        if sites.is_empty() {
            wanted = entities(nodes, &places, None);
        }
        for site in &sites {
            let packages = packages(&self.ckan, site).await?;
            wanted.extend(entities(
                nodes,
                &places,
                Some(Ckan {
                    site,
                    packages: &packages,
                }),
            ));
        }
        // Two catalogues name the same Endpoints: tied once more over both.
        link(&mut wanted);
        let counted = self.counted(http, base, &wanted).await;
        with_types(&mut wanted, &counted);

        let owners = owners(repo);
        let mut run = Run::default();
        for target in &targets {
            // A token per catalogue: one minted for the whole run expires partway through a
            // run of many catalogues and fails the rest of them.
            let token = self.watch.scan_token().await?;
            let slice = match &target.project {
                None => wanted.clone(),
                Some(project) => slice(&wanted, project, &owners),
            };
            let wrote = self.write(http, base, &token, &target.slug, slice).await?;
            run.upserted += wrote.upserted;
            run.deleted += wrote.deleted;
        }
        if let Ok(mut written) = self.written.lock() {
            written.retain(|slug, _| targets.iter().any(|target| &target.slug == slug));
        }
        Ok(Some(run))
    }

    /// Writes what changed in one catalogue Endpoint since the last run.
    async fn write(
        &self,
        http: &reqwest::Client,
        base: &str,
        token: &str,
        slug: &str,
        wanted: BTreeMap<String, Value>,
    ) -> Result<Run, String> {
        let surface = format!("{base}/api/endpoint/{slug}/ngsi-ld/v1");
        let remembered = self
            .written
            .lock()
            .map_err(|_| "the catalogue's memory is poisoned".to_owned())?
            .remove(slug)
            .filter(|(listed, _)| listed.elapsed() < RELIST);
        let (listed, known) = match remembered {
            Some(memory) => memory,
            None => (Instant::now(), listing(http, &surface, token).await?),
        };

        let changed: Vec<&Value> = wanted
            .iter()
            .filter(|(id, entity)| known.get(*id) != Some(entity))
            .map(|(_, entity)| entity)
            .collect();
        let gone: Vec<&String> = known
            .keys()
            .filter(|id| !wanted.contains_key(*id))
            .collect();
        for batch in changed.chunks(BATCH) {
            post(http, &surface, "upsert", token, &json!(batch), None).await?;
        }
        // The gateway refuses a delete that does not name why (DM-113).
        let reason = format!(
            "catalogue (EP-140): no longer a public Endpoint or CKAN dataset, catalogue Endpoint {slug}"
        );
        for batch in gone.chunks(BATCH) {
            post(
                http,
                &surface,
                "delete",
                token,
                &json!(batch),
                Some(&reason),
            )
            .await?;
        }
        let run = Run {
            upserted: changed.len(),
            deleted: gone.len(),
        };
        // Remembered only once every write landed: a failed run lists the catalogue again.
        if let Ok(mut written) = self.written.lock() {
            written.insert(slug.to_owned(), (listed, wanted));
        }
        Ok(run)
    }
}

impl CatalogSync {
    /// Per DataService of `wanted`, its types and counts: from memory while younger than
    /// [`RELIST`], else read anonymously from the Endpoint itself, eight Endpoints at a time. A
    /// read that fails keeps what was read before, or leaves the Endpoint without types.
    async fn counted(
        &self,
        http: &reqwest::Client,
        base: &str,
        wanted: &BTreeMap<String, Value>,
    ) -> HashMap<String, Counts> {
        let slugs: Vec<String> = wanted
            .keys()
            .filter_map(|id| id.strip_prefix("urn:ngsi-ld:DataService:"))
            .map(str::to_owned)
            .collect();
        let mut held = match self.counts.lock() {
            Ok(mut counts) => std::mem::take(&mut *counts),
            Err(_) => HashMap::new(),
        };
        held.retain(|slug, _| slugs.contains(slug));
        let stale: Vec<String> = slugs
            .iter()
            .filter(|slug| {
                held.get(*slug)
                    .is_none_or(|(read, _)| read.elapsed() >= RELIST)
            })
            .cloned()
            .collect();
        let read: Vec<(String, Result<Counts, String>)> = futures_util::stream::iter(stale)
            .map(|slug| async move {
                let types = types_of(http, base, &slug).await;
                (slug, types)
            })
            .buffer_unordered(8)
            .collect()
            .await;
        for (slug, types) in read {
            match types {
                Ok(types) => {
                    held.insert(slug, (Instant::now(), types));
                }
                Err(why) => tracing::warn!(endpoint = %slug, %why, "catalogue counts not read"),
            }
        }
        let out = held
            .iter()
            .map(|(slug, (_, types))| (slug.clone(), types.clone()))
            .collect();
        if let Ok(mut counts) = self.counts.lock() {
            *counts = held;
        }
        out
    }
}

/// An Endpoint's entity types and their counts, read as the anonymous caller: its `/types`, then
/// per type `count=true&limit=0`.
async fn types_of(http: &reqwest::Client, base: &str, slug: &str) -> Result<Counts, String> {
    let surface = format!("{base}/api/endpoint/{slug}/ngsi-ld/v1");
    let list: Value = http
        .get(format!("{surface}/types"))
        .header("Accept", "application/json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| format!("/types did not read: {err}"))?
        .json()
        .await
        .map_err(|err| format!("/types is not JSON: {err}"))?;
    let names: Vec<String> = list
        .get("typeList")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let answer = http
            .get(format!("{surface}/entities"))
            .query(&[("type", name.as_str()), ("count", "true"), ("limit", "0")])
            .header("Accept", "application/json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| format!("the count of {name} did not read: {err}"))?;
        let count = answer
            .headers()
            .get("NGSILD-Results-Count")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| format!("the count of {name} came without NGSILD-Results-Count"))?;
        // A type outside the core @context comes back as its IRI; the catalogue names its term.
        let term = name.rsplit(['/', '#']).next().unwrap_or(&name).to_owned();
        out.push((term, count));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

/// Puts each DataService's `entityTypes` and `nextQuery` on it, and the type names in the
/// `searchText` of every Dataset it lists in `refDataset` (T-4526).
pub fn with_types(
    wanted: &mut BTreeMap<String, Value>,
    counted: &HashMap<String, Vec<(String, u64)>>,
) {
    let mut words: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, entity) in wanted.iter_mut() {
        let Some(slug) = id.strip_prefix("urn:ngsi-ld:DataService:") else {
            continue;
        };
        let Some(types) = counted.get(slug).filter(|types| !types.is_empty()) else {
            continue;
        };
        entity["entityTypes"] = json!({ "type": "Property", "value": types
            .iter()
            .map(|(name, count)| json!({ "type": name, "count": count }))
            .collect::<Vec<_>>() });
        entity["nextQuery"] = json!({ "type": "Property", "value": {
            "tool": "query_entities",
            "arguments": { "endpoint": slug, "type": types[0].0, "limit": 10 },
        }});
        let datasets = entity["refDataset"]["objectList"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["object"].as_str());
        for dataset in datasets {
            words
                .entry(dataset.to_owned())
                .or_default()
                .extend(types.iter().map(|(name, _)| name.clone()));
        }
    }
    for (dataset, names) in words {
        if let Some(entity) = wanted.get_mut(&dataset) {
            let text = entity["searchText"]["value"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            entity["searchText"]["value"] = json!(format!("{text} {}", fold(&names.join(" "))));
        }
    }
}

/// One catalogue Endpoint the wave writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalogue {
    /// The Endpoint the wave writes through: the project's `catalog-write` door when it has one,
    /// else the `catalog` Endpoint itself.
    pub slug: String,
    /// The project it is the slice of; `None` for the installation's, which holds every project.
    pub project: Option<String>,
}

/// The repository's catalogue Endpoints, the installation's first: each Endpoint named `catalog`
/// on a space named `catalog`. The one whose space segment is `catalog` is the installation's
/// (EP-140); one on `{project}-catalog` is its project's slice. Two installation catalogues are
/// a repository that does not say where to write. Each is written through its project's
/// `catalog-write` door on the same space when one is declared (EP-187).
pub fn catalogues(repo: &Repository) -> Result<Vec<Catalogue>, String> {
    let on_catalog = |name: &'static str| {
        repo.iter()
            .filter(move |(id, _)| id.kind == "Endpoint" && id.name == name)
            .filter_map(|(id, resource)| {
                let spec = &resource.manifest.spec;
                if spec.get("contextSpaceRef").and_then(reference_name) != Some(CATALOG) {
                    return None;
                }
                let slug = spec.get("slug")?.as_str()?.to_owned();
                Some((id.namespace.clone().unwrap_or_default(), slug))
            })
    };
    let doors: HashMap<String, String> = on_catalog(CATALOG_WRITE).collect();
    let mut found: Vec<Catalogue> = on_catalog(CATALOG)
        .map(|(project, slug)| {
            let whole = repo.space_segment(&project, CATALOG) == CATALOG;
            Catalogue {
                slug: doors.get(&project).cloned().unwrap_or(slug),
                project: (!whole).then_some(project),
            }
        })
        .collect();
    let whole = found.iter().filter(|c| c.project.is_none()).count();
    if whole > 1 {
        return Err(format!(
            "{whole} Endpoints are named `{CATALOG}` on a space with the segment `{CATALOG}`; the catalogue is written to none"
        ));
    }
    found.sort_by_key(|c| c.project.is_some());
    Ok(found)
}

/// Every public Endpoint's project by slug.
fn owners(repo: &Repository) -> HashMap<String, String> {
    repo.iter()
        .filter(|(id, _)| id.kind == "Endpoint")
        .filter_map(|(id, resource)| {
            let slug = resource.manifest.spec.get("slug")?.as_str()?;
            Some((slug.to_owned(), id.namespace.clone().unwrap_or_default()))
        })
        .collect()
}

/// A project's slice of the catalogue: its own Endpoints' DataServices, the Datasets that
/// name one, and their Organizations, counted over the slice.
pub fn slice(
    wanted: &BTreeMap<String, Value>,
    project: &str,
    owners: &HashMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = wanted
        .iter()
        .filter(|(id, _)| {
            id.strip_prefix("urn:ngsi-ld:DataService:")
                .is_none_or(|slug| owners.get(slug).map(String::as_str) == Some(project))
        })
        .map(|(id, entity)| (id.clone(), entity.clone()))
        .collect();
    // Datasets whose DataService left and Organizations with no Dataset left go here.
    link(&mut out);
    out
}

/// The address of every CKAN instance of the repository, once each.
pub fn ckan_sites(repo: &Repository) -> std::collections::BTreeSet<String> {
    repo.iter()
        .filter(|(id, _)| id.kind == "CkanInstance")
        .filter_map(|(_, resource)| resource.manifest.spec.get("url")?.as_str())
        .map(|url| url.trim_end_matches('/').to_owned())
        .collect()
}

/// Every public dataset of the CKAN at `site`, read anonymously a page at a time through `http`,
/// a [`crate::api::catalogue::ckan_client`]; each page up to its cap.
async fn packages(http: &reqwest::Client, site: &str) -> Result<Vec<Package>, String> {
    // An address written into the URL is never resolved, so the resolver never judges it.
    if let Some(ip) = crate::sync::guard::internal_host(site) {
        return Err(format!(
            "CKAN at {site} names the internal address {ip}; a catalogue is read on the public \
             internet only"
        ));
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    for _ in 0..CKAN_PAGES {
        let response = http
            .get(format!("{site}/api/3/action/package_search"))
            .query(&[
                ("rows", CKAN_PAGE.to_string()),
                ("start", start.to_string()),
                ("sort", "name asc".to_owned()),
            ])
            .header("Accept", "application/json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| format!("CKAN at {site} did not list its datasets: {err}"))?;
        let page: Value = crate::capped::json(response, crate::api::catalogue::MAX_CKAN_ANSWER)
            .await
            .map_err(|err| format!("CKAN at {site} did not answer JSON: {err}"))?;
        let (results, count) = ckan_page(&page)
            .ok_or_else(|| format!("CKAN at {site} answered no package_search result"))?;
        start += results.len();
        out.extend(results.iter().filter_map(Package::from_ckan));
        if results.is_empty() || start as u64 >= count {
            return Ok(out);
        }
    }
    Err(format!(
        "CKAN at {site} holds more than {} datasets; the catalogue reads no further",
        CKAN_PAGE * CKAN_PAGES
    ))
}

/// The datasets of one `package_search` answer and the count of all of them.
fn ckan_page(page: &Value) -> Option<(&Vec<Value>, u64)> {
    if page.get("success").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let result = page.get("result")?;
    Some((
        result.get("results")?.as_array()?,
        result.get("count")?.as_u64()?,
    ))
}

/// Every public Endpoint's place by slug. The catalogue's own Endpoint is not a dataset.
pub fn places(repo: &Repository) -> BTreeMap<String, Place> {
    let mut areas: BTreeMap<(String, String), [f64; 4]> = BTreeMap::new();
    for (id, resource) in repo.iter() {
        if id.kind != "ContextSpace" {
            continue;
        }
        let rules = resource
            .manifest
            .spec
            .get("quality")
            .and_then(Value::as_array);
        let bbox = rules.into_iter().flatten().find_map(|rule| {
            if rule.get("kind").and_then(Value::as_str) != Some("area") {
                return None;
            }
            let corners: Vec<f64> = rule
                .get("bbox")?
                .as_array()?
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            <[f64; 4]>::try_from(corners).ok()
        });
        if let Some(bbox) = bbox {
            let project = id.namespace.clone().unwrap_or_default();
            areas.insert((project, id.name.clone()), bbox);
        }
    }
    let mut places = BTreeMap::new();
    for (id, resource) in repo.iter() {
        let spec = &resource.manifest.spec;
        if id.kind != "Endpoint" || spec.get("audience").and_then(Value::as_str) != Some("public") {
            continue;
        }
        let (Some(slug), Some(space)) = (
            spec.get("slug").and_then(Value::as_str),
            spec.get("contextSpaceRef").and_then(reference_name),
        ) else {
            continue;
        };
        if id.name == CATALOG && space == CATALOG {
            continue;
        }
        let project = id.namespace.clone().unwrap_or_default();
        let bbox = areas.get(&(project.clone(), space.to_owned())).copied();
        // The `{space}` segment, which is what the gateway knows a space by: the hub's
        // catalogue names the same Dataset (T-4503).
        let segment = repo.space_segment(&project, space);
        places.insert(slug.to_owned(), Place::public(segment, bbox));
    }
    places
}

/// What the catalogue holds now, by id; the values are unknown, so every entity is written once.
async fn listing(
    http: &reqwest::Client,
    surface: &str,
    token: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let mut held = BTreeMap::new();
    let mut offset = 0usize;
    loop {
        let response = http
            .get(format!("{surface}/entities"))
            .query(&[
                ("type", "Dataset,DataService,Organization"),
                ("options", "keyValues"),
                ("limit", "1000"),
                ("offset", &offset.to_string()),
            ])
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|err| format!("the catalogue did not list: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            let problem: Value = response.json().await.unwrap_or_default();
            // CIM 009 clause 5.5.10: the broker creates a Tenant on its first create, so a
            // catalogue nobody has written yet has none and holds nothing; the upsert that
            // follows creates it through the Endpoint and its Policy (T-7387). Any other 404,
            // such as an unknown Endpoint, still fails the run.
            if status == reqwest::StatusCode::NOT_FOUND && names_no_tenant(&problem) {
                return Ok(held);
            }
            return Err(format!(
                "the catalogue did not list: {status}{}",
                explained(&problem)
            ));
        }
        let page: Vec<Value> = response
            .json()
            .await
            .map_err(|err| format!("the catalogue's listing is not JSON: {err}"))?;
        if page.is_empty() {
            return Ok(held);
        }
        offset += page.len();
        for entity in page {
            if let Some(id) = entity.get("id").and_then(Value::as_str) {
                held.insert(id.to_owned(), Value::Null);
            }
        }
    }
}

/// Whether a problem document is CIM 009's `NonexistentTenant` (clause 5.5.2), by the last
/// segment of its `type`, as the gateway reads it.
fn names_no_tenant(problem: &Value) -> bool {
    problem["type"]
        .as_str()
        .is_some_and(|kind| kind.rsplit('/').next() == Some("NonexistentTenant"))
}

/// A refusal's problem `type` and `detail`, cut to [`REASON_CHARS`], so the run's error says
/// which 404 or 403 it was; empty when the answer was no problem document.
fn explained(problem: &Value) -> String {
    let said: Vec<&str> = ["type", "detail"]
        .into_iter()
        .filter_map(|member| problem[member].as_str())
        .collect();
    match said.is_empty() {
        true => String::new(),
        false => format!(
            " ({})",
            said.join(": ")
                .chars()
                .take(REASON_CHARS)
                .collect::<String>()
        ),
    }
}

/// The broker's reasons in a multi-status answer's `errors`, most frequent first, each with how
/// many members it refused and the first of them: "failed for 100 entities" alone named no cause
/// an operator could act on (T-6071). At most [`REASONS`], each cut to [`REASON_CHARS`].
fn refusals(errors: &[Value]) -> String {
    let mut reasons: Vec<(String, usize, &str)> = Vec::new();
    for member in errors {
        let error = &member["error"];
        let reason: String = error["detail"]
            .as_str()
            .or_else(|| error["title"].as_str())
            .unwrap_or("no reason given")
            .chars()
            .take(REASON_CHARS)
            .collect();
        match reasons.iter_mut().find(|(seen, ..)| *seen == reason) {
            Some((_, count, _)) => *count += 1,
            None => reasons.push((reason, 1, member["entityId"].as_str().unwrap_or("?"))),
        }
    }
    reasons.sort_by_key(|(_, count, _)| std::cmp::Reverse(*count));
    let shown: Vec<String> = reasons
        .iter()
        .take(REASONS)
        .map(|(reason, count, first)| format!("{count} x {reason} ({first})"))
        .collect();
    match shown.is_empty() {
        true => "the answer named no member".to_owned(),
        false => shown.join("; "),
    }
}

/// How many distinct reasons a failed batch reports.
const REASONS: usize = 3;
/// How long one reported reason may be.
const REASON_CHARS: usize = 200;

/// One batch operation, with `JC-Reason` when it names one; a multi-status answer is a failure,
/// with what failed and why.
async fn post(
    http: &reqwest::Client,
    surface: &str,
    operation: &str,
    token: &str,
    body: &Value,
    reason: Option<&str>,
) -> Result<(), String> {
    let mut request = http
        .post(format!("{surface}/entityOperations/{operation}"))
        .bearer_auth(token)
        .json(body);
    if let Some(reason) = reason {
        request = request.header(
            "JC-Reason",
            crate::pipeline_source_deletes::reason_header(reason),
        );
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("the catalogue {operation} did not reach the gateway: {err}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::MULTI_STATUS {
        let answer: Value = response.json().await.unwrap_or_default();
        let errors = answer
            .get("errors")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        return Err(format!(
            "the catalogue {operation} failed for {} entities: {}",
            errors.len(),
            refusals(errors)
        ));
    }
    if !status.is_success() {
        return Err(format!("the catalogue refused the {operation}: {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcctl::loader::{RawManifest, RawMetadata};

    fn endpoint(project: &str, name: &str, space: &str, slug: &str) -> RawManifest {
        RawManifest {
            api_version: "joinedcontext.com/v1alpha1".to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: RawMetadata {
                name: name.to_owned(),
                namespace: Some(project.to_owned()),
                rest: serde_json::Map::new(),
            },
            spec: json!({ "contextSpaceRef": space, "slug": slug }),
            status: None,
        }
    }

    /// A wave on a gateway at `server`, and one Dataset it wants written.
    fn wave(server: &wiremock::MockServer) -> (CatalogSync, BTreeMap<String, Value>) {
        let watch = Watch::new(
            server.uri(),
            &format!("{}/realms/city", server.uri()),
            "portal".to_owned(),
            crate::config::ClientAuth::Secret("s".into()),
        );
        let id = "urn:ngsi-ld:Dataset:air";
        let wanted = BTreeMap::from([(id.to_owned(), json!({ "id": id, "type": "Dataset" }))]);
        (
            CatalogSync::new(Arc::new(watch), reqwest::Client::new()),
            wanted,
        )
    }

    /// T-7387, CIM 009 clause 5.5.10: a catalogue nobody has written has no Tenant, so its list
    /// answers `NonexistentTenant`; that is an empty catalogue, and the upsert that creates the
    /// Tenant goes out through the write door.
    #[tokio::test]
    async fn a_catalogue_without_a_tenant_is_empty_and_gets_its_first_write() {
        use wiremock::matchers::{method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .and(path("/api/endpoint/door/ngsi-ld/v1/entities"))
            .respond_with(wiremock::ResponseTemplate::new(404).set_body_json(json!({
                "type": "https://uri.etsi.org/ngsi-ld/errors/NonexistentTenant",
                "title": "NonexistentTenant",
                "detail": "tenant catalog does not exist",
            })))
            .mount(&server)
            .await;
        wiremock::Mock::given(method("POST"))
            .and(path(
                "/api/endpoint/door/ngsi-ld/v1/entityOperations/upsert",
            ))
            .respond_with(wiremock::ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let (sync, wanted) = wave(&server);
        let run = sync
            .write(&reqwest::Client::new(), &server.uri(), "t", "door", wanted)
            .await
            .expect("an empty catalogue is written");
        assert_eq!(
            run,
            Run {
                upserted: 1,
                deleted: 0
            }
        );
    }

    /// Any other 404 (an unknown Endpoint, a view the broker does not hold) still fails the run,
    /// writes nothing and names what the gateway said.
    #[tokio::test]
    async fn a_catalogue_list_answering_another_404_fails_the_run() {
        use wiremock::matchers::method;
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404).set_body_json(json!({
                "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound",
                "title": "Not Found",
                "detail": "no Endpoint of that name",
            })))
            .mount(&server)
            .await;
        wiremock::Mock::given(method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        let (sync, wanted) = wave(&server);
        let failed = sync
            .write(&reqwest::Client::new(), &server.uri(), "t", "door", wanted)
            .await
            .expect_err("a plain 404 fails the run");
        assert!(failed.contains("404"), "{failed}");
        assert!(failed.contains("no Endpoint of that name"), "{failed}");
    }

    /// EP-187, T-6980: the public `catalog` Endpoint is open data and refuses writes, so the wave
    /// writes through the project's `catalog-write` door; a project without one keeps writing
    /// through `catalog`, and a door on another space or of another project is not its door.
    #[test]
    fn a_catalogue_is_written_through_its_projects_write_door() {
        let repo = Repository::from_manifests(
            vec![
                endpoint("city", CATALOG, CATALOG, "public"),
                endpoint("city", CATALOG_WRITE, CATALOG, "door"),
                endpoint("town", CATALOG, CATALOG, "town-public"),
                endpoint("town", CATALOG_WRITE, "air", "not-the-door"),
                endpoint("village", CATALOG, CATALOG, "village-public"),
                endpoint("other", CATALOG_WRITE, CATALOG, "other-door"),
            ],
            None,
        )
        .expect("a repository");
        let slugs: BTreeMap<_, _> = catalogues(&repo)
            .expect("catalogues")
            .into_iter()
            .map(|c| (c.project.unwrap_or_default(), c.slug))
            .collect();
        assert_eq!(slugs.get("city").map(String::as_str), Some("door"));
        assert_eq!(slugs.get("town").map(String::as_str), Some("town-public"));
        assert_eq!(
            slugs.get("village").map(String::as_str),
            Some("village-public")
        );
        assert!(
            !slugs.contains_key("other"),
            "a door without a catalogue is none"
        );
    }
}
