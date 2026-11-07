//! `GET /api/v1/catalogue`: every public dataset of the installation, searched and faceted
//! (EP-81, EP-82, Architecture/21 §6).
//!
//! The page reads CKAN, never the manifests, so what it lists is what `data.{host}` holds. It
//! reads as an anonymous caller: no API token is ever sent, CKAN answers such a caller with
//! public datasets only, and a dataset still marked `private` is dropped here as well, so a
//! restricted dataset can only be absent (EP-67, EP-69). The routes need no session for the
//! same reason: they carry nothing the public catalogue does not already show.
//!
//! Facets are computed here and not by CKAN's Solr, because the extras the publisher writes
//! (`theme`, `spatial_uri`, `temporal_start`) are free text to Solr and cannot be faceted there.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Datelike;
use jc_core::kinds::ckan::CkanInstanceSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;
use utoipa::ToSchema;

use crate::cache::ByteLru;
use crate::error::ApiError;
use crate::state::AppState;
use crate::store::ListOptions;

/// How long a browser, and this Portal, may keep a catalogue answer.
pub const MAX_AGE_SECONDS: u64 = 60;
/// Datasets on one page of the list.
pub const PAGE_SIZE: usize = 20;
/// The most datasets read from one catalogue per search.
// ponytail: one search of 1000 rows per catalogue, faceted in memory; page through CKAN when a
// catalogue outgrows it.
const ROWS: usize = 1000;
/// The most rows a sample shows (EP-82).
const SAMPLE_ROWS: usize = 10;
/// The most entities the dataset page's map reads (EP-82, T-5628).
// ponytail: one page of 1000; past it the page says "N of M" and the developer page pages on.
pub const MAP_ROWS: usize = 1000;
/// The GeoJSON geometry types a map draws.
const GEOMETRIES: &[&str] = &[
    "Point",
    "MultiPoint",
    "LineString",
    "MultiLineString",
    "Polygon",
    "MultiPolygon",
];

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("joinedcontext-portal catalogue")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

/// The client of the CKAN reads (T-4776). A `CkanInstance` manifest names the catalogue, so its
/// URL is an outside one: a name resolving into the cluster is refused by the resolver, a redirect
/// is followed only to a public `https` address and only a few hops, and no proxy from the
/// environment resolves past the guard. [`HTTP`] keeps the reads of the installation's own gateway.
pub(crate) static CKAN_HTTP: LazyLock<reqwest::Client> =
    LazyLock::new(|| ckan_client(Duration::from_secs(10)));

/// A client for reads of a `CkanInstance`'s URL that take at most `total` (T-4776, T-5335): the
/// platform's outbound guard ([`jc_core::outbound::client`]) with the Portal's `https`-only
/// redirect rule.
pub fn ckan_client(total: Duration) -> reqwest::Client {
    jc_core::outbound::client(Duration::from_secs(5), total)
        .user_agent("joinedcontext-portal catalogue")
        .redirect(crate::sync::guard::https_redirects(CKAN_REDIRECTS))
        .build()
        .unwrap_or_default()
}

/// How many redirects a CKAN read follows.
const CKAN_REDIRECTS: usize = 3;

/// The longest CKAN answer read: a search of [`ROWS`] packages is a few megabytes, so a host that
/// streams more is cut off instead of filling the Portal's memory (T-4776).
pub(crate) const MAX_CKAN_ANSWER: usize = 32 * 1024 * 1024;

/// A catalogue's base URL and the search text.
type SearchKey = (String, String);

/// The bytes the search answers may hold together, as serialized JSON.
const SEARCHES_MAX_BYTES: usize = 8 * 1024 * 1024;

/// One search answer per catalogue and query text, cut to the fields a page reads and kept for
/// [`MAX_AGE_SECONDS`], so a burst of anonymous visitors costs CKAN one search a minute and a
/// burst of distinct queries costs at most [`SEARCHES_MAX_BYTES`] (T-4353).
// ponytail: one replica's memory; share it only when the Portal runs more than one pod.
static SEARCHES: LazyLock<Mutex<ByteLru<SearchKey, Arc<Vec<Value>>>>> = LazyLock::new(|| {
    Mutex::new(ByteLru::new(
        SEARCHES_MAX_BYTES,
        Some(Duration::from_secs(MAX_AGE_SECONDS)),
    ))
});

/// The bytes the map answers may hold together, as serialized JSON.
const MAPS_MAX_BYTES: usize = 16 * 1024 * 1024;

/// One map answer per public Endpoint and popup language, kept for [`MAX_AGE_SECONDS`]: a burst
/// of anonymous visitors on a dataset page costs its Endpoint one read of up to [`MAP_ROWS`]
/// entities a minute, the same rule as [`SEARCHES`].
// ponytail: one replica's memory; share it only when the Portal runs more than one pod.
type MapKey = (String, String);

static MAPS: LazyLock<Mutex<ByteLru<MapKey, Arc<CatalogueMap>>>> = LazyLock::new(|| {
    Mutex::new(ByteLru::new(
        MAPS_MAX_BYTES,
        Some(Duration::from_secs(MAX_AGE_SECONDS)),
    ))
});

/// The map kept for `(slug, lang)`, or the one `build` makes, kept only when it succeeded.
async fn map_cached<F>(slug: &str, lang: &str, build: F) -> Result<Arc<CatalogueMap>, ApiError>
where
    F: std::future::Future<Output = Result<CatalogueMap, ApiError>>,
{
    let key = (slug.to_owned(), lang.to_owned());
    if let Some(map) = MAPS.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(map);
    }
    let map = build.await?;
    let bytes = serde_json::to_vec(&map).map_or(usize::MAX, |b| b.len());
    let map = Arc::new(map);
    MAPS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, map.clone(), bytes);
    Ok(map)
}

/// One page of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CataloguePage {
    /// Datasets matching the search and every filter.
    pub total: usize,
    /// The page shown, from 1.
    pub page: usize,
    /// Datasets per page.
    pub page_size: usize,
    /// The datasets of this page.
    pub datasets: Vec<CatalogueDataset>,
    /// Counts per facet value, each over the datasets every other filter keeps.
    pub facets: CatalogueFacets,
    /// Catalogues that did not answer; the others still did.
    pub unavailable: Vec<String>,
}

/// A dataset as the list shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueDataset {
    pub name: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<CataloguePublisher>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence: Option<CatalogueLicence>,
    pub formats: Vec<String>,
    /// EU data-theme codes (`ECON`, `ENVI`, …); the UI names them in the reader's language.
    pub themes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

/// The CKAN organization a dataset belongs to: one per project (Architecture/21 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CataloguePublisher {
    pub name: String,
    pub title: String,
}

/// A licence as the catalogue's register names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueLicence {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Every facet of the list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueFacets {
    pub publisher: Vec<CatalogueFacetValue>,
    pub theme: Vec<CatalogueFacetValue>,
    pub format: Vec<CatalogueFacetValue>,
    pub licence: Vec<CatalogueFacetValue>,
    pub spatial: Vec<CatalogueFacetValue>,
    pub year: Vec<CatalogueFacetValue>,
    /// The keywords datasets carry, offered when a dataset is published (T-4948); no filter.
    #[serde(default)]
    pub keyword: Vec<CatalogueFacetValue>,
}

/// One value of a facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueFacetValue {
    pub value: String,
    pub label: String,
    pub count: usize,
}

/// One dataset, as its page shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueDatasetDetail {
    pub name: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<CataloguePublisher>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence: Option<CatalogueLicence>,
    /// The EU frequency the record names (`dct:accrualPeriodicity`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<String>,
    pub themes: Vec<CatalogueTheme>,
    pub spatial: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal: Option<CatalogueTemporal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<CatalogueContact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// The dataset on the catalogue's own site.
    pub catalogue_url: String,
    pub resources: Vec<CatalogueResource>,
    /// The Endpoint of this installation the dataset describes, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<CatalogueEndpoint>,
    /// The model the Endpoint's space holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<CatalogueDataModel>,
    /// The dataset's quality score in its public form: counts and dates, no names (T-5112).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<crate::dataset_score::DatasetScore>,
}

/// An EU data theme: its code and its English name (the UI names it in the reader's language).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueTheme {
    pub code: String,
    pub label: String,
}

/// The EU data-theme vocabulary (`http://publications.europa.eu/resource/authority/data-theme`).
const THEMES: [(&str, &str); 14] = [
    ("AGRI", "Agriculture, fisheries, forestry and food"),
    ("ECON", "Economy and finance"),
    ("EDUC", "Education, culture and sport"),
    ("ENER", "Energy"),
    ("ENVI", "Environment"),
    ("GOVE", "Government and public sector"),
    ("HEAL", "Health"),
    ("INTR", "International issues"),
    ("JUST", "Justice, legal system and public safety"),
    ("OP_DATPRO", "Provisional data"),
    ("REGI", "Regions and cities"),
    ("SOCI", "Population and society"),
    ("TECH", "Science and technology"),
    ("TRAN", "Transport"),
];

/// The English name of a theme code; a code outside the vocabulary names itself.
fn theme_label(code: &str) -> String {
    THEMES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or_else(|| code.to_owned(), |(_, label)| (*label).to_owned())
}

/// The period a dataset covers; either end may be open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueTemporal {
    pub start: Option<String>,
    pub end: Option<String>,
}

/// Where questions about the dataset go: a role address (EP-80).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueContact {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// One resource of a dataset (EP-64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueResource {
    pub name: String,
    pub format: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The Endpoint a dataset describes, on this installation's own host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueEndpoint {
    pub url: String,
    /// The representations it serves, which decide the "Use this data" snippets.
    pub representations: Vec<String>,
    /// Its feed outputs, in the order the Endpoint lists them (EP-150, EP-151).
    pub feeds: Vec<CatalogueFeed>,
    /// Its calendar outputs, in the order the Endpoint lists them (API/11 EP-159).
    pub calendars: Vec<CatalogueCalendar>,
}

/// One calendar output of an Endpoint: its name, its title per language and its `.ics` address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueCalendar {
    pub name: String,
    /// The title per language (`sk`, `en`); empty: the name stands for it.
    pub title: BTreeMap<String, String>,
    pub url: String,
}

/// The calendar outputs an Endpoint spec serves with their absolute addresses under `url`: none
/// unless it lists `ical` and an output (off by default, owner 2026-10-22T09:25Z); a spec the
/// platform would refuse reads as none.
fn calendars_of(spec: &Value, url: &str) -> Vec<CatalogueCalendar> {
    let Ok(endpoint) = serde_json::from_value::<jc_core::kinds::EndpointSpec>(spec.clone()) else {
        return Vec::new();
    };
    jc_core::kinds::ical_links(endpoint.ical_outputs())
        .into_iter()
        .map(|(output, path)| CatalogueCalendar {
            name: output.name.clone(),
            title: output.title.clone(),
            url: format!("{url}{path}"),
        })
        .collect()
}

/// One feed output of an Endpoint: its name, its title per language and its three documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueFeed {
    pub name: String,
    /// The title per language (`sk`, `en`); empty: the name stands for it.
    pub title: BTreeMap<String, String>,
    pub links: Vec<CatalogueFeedLink>,
}

/// One document of a feed output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueFeedLink {
    /// `Atom`, `RSS 2.0` or `JSON Feed`.
    pub format: String,
    pub media_type: String,
    pub url: String,
}

/// The feed outputs of an Endpoint spec with their absolute addresses under `url` (the
/// Endpoint's address, ending in `/`); a list the platform would refuse reads as none.
fn feeds_of(spec: &Value, url: &str) -> Vec<CatalogueFeed> {
    let outputs: Vec<jc_core::kinds::FeedOutput> =
        serde_json::from_value(spec["representations"]["feed"].clone()).unwrap_or_default();
    outputs
        .iter()
        .map(|output| CatalogueFeed {
            name: output.name.clone(),
            title: output.title.clone(),
            links: jc_core::kinds::feed_links(std::slice::from_ref(output))
                .into_iter()
                .map(|(_, path, format, media_type)| CatalogueFeedLink {
                    format: format.to_owned(),
                    media_type: media_type.to_owned(),
                    url: format!("{url}{path}"),
                })
                .collect(),
        })
        .collect()
}

/// The data model behind a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueDataModel {
    pub name: String,
    pub classes: Vec<CatalogueClass>,
    /// The model's documentation, the dataset's Markdown schema resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

/// One class of the model, described as its LinkML describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueClass {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The model's classes with their LinkML `description`: a plain string, or the English (else
/// the first) entry of a map per language. A source that does not parse describes nothing.
pub fn classes(names: &[String], linkml: Option<&str>) -> Vec<CatalogueClass> {
    let model = linkml.and_then(|l| serde_norway::from_str::<serde_norway::Value>(l).ok());
    names
        .iter()
        .map(|name| {
            let description = model.as_ref().and_then(|m| {
                let d = &m["classes"][name.as_str()]["description"];
                d.as_str()
                    .or_else(|| d["en"].as_str())
                    .or_else(|| d.as_mapping()?.values().find_map(|v| v.as_str()))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            });
            CatalogueClass {
                name: name.clone(),
                description,
            }
        })
        .collect()
}

/// Up to ten entities of a dataset, read through its Endpoint (EP-82).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueSample {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// The located entities of a dataset, read through its Endpoint for the dataset page's map
/// (EP-82, T-5628).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueMap {
    #[serde(rename = "type")]
    pub entity_type: String,
    /// Entities of the type the Endpoint holds, located or not, when it counted them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The key attributes a popup and the list show, in display order, labelled in `lang`.
    pub fields: Vec<CatalogueMapField>,
    /// One per entity with a GeoJSON geometry, at most 1000.
    pub features: Vec<CatalogueFeature>,
    /// The project whose basemap draws under the features, when the platform has a basemap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basemap: Option<String>,
}

/// One attribute a popup shows (DM-99, DM-100).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueMapField {
    pub slot: String,
    pub label: String,
}

/// One located entity: its id, its geometry and its key attributes as text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CatalogueFeature {
    pub id: String,
    #[schema(value_type = Object)]
    pub geometry: Value,
    pub properties: BTreeMap<String, String>,
}

/// The query of the map route.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct MapQuery {
    /// The language the labels are in, e.g. `sk` (default `en`).
    pub lang: Option<String>,
}

/// The filters of one search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub q: String,
    pub publisher: Vec<String>,
    pub theme: Vec<String>,
    pub format: Vec<String>,
    pub licence: Vec<String>,
    pub spatial: Vec<String>,
    pub year: Vec<String>,
    pub page: usize,
}

impl Filters {
    /// Reads `?q=…&publisher=a&publisher=b&page=2`; an unknown key is ignored, a page that is
    /// not a positive number is the first.
    pub fn parse(query: Option<&str>) -> Self {
        let mut filters = Filters {
            page: 1,
            ..Filters::default()
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            let value = value.trim().to_owned();
            if value.is_empty() {
                continue;
            }
            match key.as_ref() {
                "q" => filters.q = value.chars().take(200).collect(),
                "publisher" => filters.publisher.push(value),
                "theme" => filters.theme.push(value),
                "format" => filters.format.push(value),
                "licence" => filters.licence.push(value),
                "spatial" => filters.spatial.push(value),
                "year" => filters.year.push(value),
                "page" => filters.page = value.parse().ok().filter(|p| *p > 0).unwrap_or(1),
                _ => {}
            }
        }
        filters
    }
}

/// What one dataset is filed under, per facet.
struct Filed {
    summary: CatalogueDataset,
    publisher: Vec<(String, String)>,
    theme: Vec<(String, String)>,
    format: Vec<(String, String)>,
    licence: Vec<(String, String)>,
    spatial: Vec<(String, String)>,
    year: Vec<(String, String)>,
    keyword: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Facet {
    Publisher,
    Theme,
    Format,
    Licence,
    Spatial,
    Year,
    Keyword,
}

const FACETS: [Facet; 7] = [
    Facet::Publisher,
    Facet::Theme,
    Facet::Format,
    Facet::Licence,
    Facet::Spatial,
    Facet::Year,
    Facet::Keyword,
];

impl Filed {
    fn of(dataset: &Value, this_year: i32) -> Self {
        let summary = summary(dataset);
        let pairs = |values: &[String]| {
            values
                .iter()
                .map(|v| (v.clone(), v.clone()))
                .collect::<Vec<_>>()
        };
        Filed {
            publisher: summary
                .publisher
                .iter()
                .map(|p| (p.name.clone(), p.title.clone()))
                .collect(),
            theme: summary
                .themes
                .iter()
                .map(|code| (code.clone(), theme_label(code)))
                .collect(),
            format: pairs(&summary.formats),
            licence: summary
                .licence
                .iter()
                .map(|l| (l.id.clone(), l.title.clone()))
                .collect(),
            spatial: spatial(dataset)
                .into_iter()
                .map(|s| (s.clone(), spatial_label(&s)))
                .collect(),
            year: years(dataset, this_year)
                .into_iter()
                .map(|y| (y.to_string(), y.to_string()))
                .collect(),
            keyword: keywords(dataset)
                .into_iter()
                .map(|k| (k.clone(), k))
                .collect(),
            summary,
        }
    }

    fn values(&self, facet: Facet) -> &[(String, String)] {
        match facet {
            Facet::Publisher => &self.publisher,
            Facet::Theme => &self.theme,
            Facet::Format => &self.format,
            Facet::Licence => &self.licence,
            Facet::Spatial => &self.spatial,
            Facet::Year => &self.year,
            Facet::Keyword => &self.keyword,
        }
    }

    /// Kept by the filter of `facet`: no filter, or one of its values.
    fn kept_by(&self, facet: Facet, filters: &Filters) -> bool {
        let wanted = match facet {
            Facet::Publisher => &filters.publisher,
            Facet::Theme => &filters.theme,
            Facet::Format => &filters.format,
            Facet::Licence => &filters.licence,
            Facet::Spatial => &filters.spatial,
            Facet::Year => &filters.year,
            // Offered to a publisher, never a search filter.
            Facet::Keyword => return true,
        };
        wanted.is_empty()
            || self
                .values(facet)
                .iter()
                .any(|(value, _)| wanted.contains(value))
    }
}

/// Filters, facets and pages the datasets a search returned (EP-81). Pure, so every rule is
/// tested without a catalogue.
pub fn page_of(datasets: &[Value], filters: &Filters, this_year: i32) -> CataloguePage {
    let filed: Vec<Filed> = datasets
        .iter()
        .filter(|d| d.get("private").and_then(Value::as_bool) != Some(true))
        .map(|d| Filed::of(d, this_year))
        .collect();

    let mut facets = CatalogueFacets::default();
    for facet in FACETS {
        let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for dataset in filed.iter().filter(|d| {
            FACETS
                .iter()
                .all(|other| *other == facet || d.kept_by(*other, filters))
        }) {
            for (value, label) in dataset.values(facet) {
                counts.entry(value.clone()).or_insert((label.clone(), 0)).1 += 1;
            }
        }
        let mut values: Vec<CatalogueFacetValue> = counts
            .into_iter()
            .map(|(value, (label, count))| CatalogueFacetValue {
                value,
                label,
                count,
            })
            .collect();
        values.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
        *match facet {
            Facet::Publisher => &mut facets.publisher,
            Facet::Theme => &mut facets.theme,
            Facet::Format => &mut facets.format,
            Facet::Licence => &mut facets.licence,
            Facet::Spatial => &mut facets.spatial,
            Facet::Year => &mut facets.year,
            Facet::Keyword => &mut facets.keyword,
        } = values;
    }

    let kept: Vec<CatalogueDataset> = filed
        .into_iter()
        .filter(|d| FACETS.iter().all(|facet| d.kept_by(*facet, filters)))
        .map(|d| d.summary)
        .collect();
    let total = kept.len();
    let datasets = kept
        .into_iter()
        .skip((filters.page - 1).saturating_mul(PAGE_SIZE))
        .take(PAGE_SIZE)
        .collect();
    CataloguePage {
        total,
        page: filters.page,
        page_size: PAGE_SIZE,
        datasets,
        facets,
        unavailable: Vec::new(),
    }
}

/// A dataset's CKAN tags, by the name a person reads.
fn keywords(dataset: &Value) -> Vec<String> {
    dataset
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| text(t, "display_name").or_else(|| text(t, "name")))
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The value of one CKAN extra.
fn extra(dataset: &Value, key: &str) -> Option<String> {
    dataset
        .get("extras")?
        .as_array()?
        .iter()
        .find(|e| e.get("key").and_then(Value::as_str) == Some(key))
        .and_then(|e| text(e, "value"))
}

/// An extra that holds one value or a JSON array of them.
fn extra_list(dataset: &Value, key: &str) -> Vec<String> {
    let Some(raw) = extra(dataset, key) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => vec![raw],
    }
}

fn last_segment(iri: &str) -> String {
    iri.trim_end_matches('/')
        .rsplit(['/', '#'])
        .next()
        .unwrap_or(iri)
        .to_owned()
}

fn summary(dataset: &Value) -> CatalogueDataset {
    let name = text(dataset, "name").unwrap_or_default();
    let mut formats: Vec<String> = dataset
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| text(r, "format"))
        .collect();
    formats.sort();
    formats.dedup();
    CatalogueDataset {
        title: text(dataset, "title").unwrap_or_else(|| name.clone()),
        name,
        notes: text(dataset, "notes"),
        publisher: publisher(dataset),
        licence: licence(dataset),
        formats,
        themes: extra_list(dataset, "theme")
            .iter()
            .map(|iri| last_segment(iri))
            .collect(),
        modified: text(dataset, "metadata_modified"),
    }
}

fn publisher(dataset: &Value) -> Option<CataloguePublisher> {
    let organization = dataset.get("organization")?;
    let name = text(organization, "name")?;
    Some(CataloguePublisher {
        title: text(organization, "title").unwrap_or_else(|| name.clone()),
        name,
    })
}

fn licence(dataset: &Value) -> Option<CatalogueLicence> {
    let id = text(dataset, "license_id")?;
    Some(CatalogueLicence {
        title: text(dataset, "license_title").unwrap_or_else(|| id.clone()),
        url: text(dataset, "license_url"),
        id,
    })
}

fn spatial(dataset: &Value) -> Vec<String> {
    let uris = extra_list(dataset, "spatial_uri");
    if uris.is_empty() {
        // `spatial` alone is GeoJSON to the spatial extension; only a plain code is a facet.
        extra_list(dataset, "spatial")
            .into_iter()
            .filter(|s| !s.starts_with('{'))
            .collect()
    } else {
        uris
    }
}

fn spatial_label(value: &str) -> String {
    last_segment(value)
}

/// The calendar years a dataset covers; an open end runs to `this_year`.
fn years(dataset: &Value, this_year: i32) -> Vec<i32> {
    let year = |key| {
        extra(dataset, key)
            .and_then(|date| date.get(..4).and_then(|y| y.parse::<i32>().ok()))
            .filter(|y| (1000..=9999).contains(y))
    };
    match (year("temporal_start"), year("temporal_end")) {
        (Some(start), end) => {
            let end = end.unwrap_or(this_year).max(start);
            // A century is the most a facet lists for one dataset.
            (start..=end.min(start + 99)).collect()
        }
        (None, Some(end)) => vec![end],
        (None, None) => Vec::new(),
    }
}

/// The distinct catalogues the installation's `CkanInstance`s name.
fn catalogues(state: &AppState) -> Vec<String> {
    let mut urls: Vec<String> = state
        .mirror
        .namespaces()
        .into_iter()
        .flat_map(|project| {
            state
                .mirror
                .list(&project, "CkanInstance", &ListOptions::default())
                .items
        })
        .filter_map(|env| {
            let spec: CkanInstanceSpec = serde_json::from_value(env.spec).ok()?;
            Some(spec.base_url().to_owned())
        })
        .filter(|url| Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

/// Why a CKAN action gave no result.
enum Failure {
    /// CKAN answered `success: false`: not found, not authorised, or a bad parameter.
    Refused,
    /// No CKAN document came back.
    Unreachable(String),
}

/// A CKAN action as an anonymous caller: no token is ever sent (EP-67).
async fn action(
    http: &reqwest::Client,
    base: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Result<Value, Failure> {
    // An address written into the URL is never resolved, so the resolver never judges it.
    if let Some(ip) = crate::sync::guard::internal_host(base) {
        return Err(Failure::Unreachable(format!(
            "{base} names the internal address {ip}; a catalogue is read on the public internet only"
        )));
    }
    action_with(http, base, action, params).await
}

/// One CKAN action through `http`, its answer read up to [`MAX_CKAN_ANSWER`].
async fn action_with(
    http: &reqwest::Client,
    base: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Result<Value, Failure> {
    let url = format!("{base}/api/3/action/{action}");
    let response = http
        .get(&url)
        .query(params)
        .send()
        .await
        .map_err(|e| Failure::Unreachable(format!("{base} did not answer: {e}")))?;
    let status = response.status();
    let bytes = jc_core::outbound::read_capped(response, MAX_CKAN_ANSWER)
        .await
        .map_err(|e| match e {
            jc_core::outbound::BodyError::Transport(e) => {
                Failure::Unreachable(format!("{base} broke off its answer: {e}"))
            }
            jc_core::outbound::BodyError::TooLarge(max) => {
                Failure::Unreachable(format!("{base} answered more than {max} bytes"))
            }
        })?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| {
        Failure::Unreachable(format!(
            "{base} answered {status} with no CKAN document: {e}"
        ))
    })?;
    if body.get("success").and_then(Value::as_bool) == Some(true) {
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(Failure::Refused)
    }
}

async fn search(http: &reqwest::Client, base: &str, q: &str) -> Result<Arc<Vec<Value>>, String> {
    let key = (base.to_owned(), q.to_owned());
    if let Some(datasets) = SEARCHES.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(datasets);
    }
    let rows = ROWS.to_string();
    let result = action(
        http,
        base,
        "package_search",
        &[("q", q), ("rows", &rows), ("include_private", "false")],
    )
    .await;
    let result = match result {
        Ok(result) => result,
        // CKAN refuses a search text Solr cannot parse (`a:`): no dataset matches it.
        Err(Failure::Refused) => Value::Null,
        Err(Failure::Unreachable(reason)) => return Err(reason),
    };
    let datasets: Vec<Value> = result
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(slim)
        .collect();
    let bytes = serde_json::to_vec(&datasets).map_or(usize::MAX, |b| b.len());
    let datasets = Arc::new(datasets);
    SEARCHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, datasets.clone(), bytes);
    Ok(datasets)
}

/// The extras a page reads.
const PAGE_EXTRAS: [&str; 5] = [
    "theme",
    "spatial_uri",
    "spatial",
    "temporal_start",
    "temporal_end",
];

/// A searched dataset cut to the fields [`page_of`] reads: the resources' formats, the page's
/// extras and the top-level fields of the summary. The detail reads `package_show`, not this.
fn slim(dataset: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for key in [
        "name",
        "title",
        "notes",
        "private",
        "license_id",
        "license_title",
        "license_url",
        "metadata_modified",
    ] {
        if let Some(value) = dataset.get(key) {
            kept.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(organization) = dataset.get("organization") {
        let mut org = serde_json::Map::new();
        for key in ["name", "title"] {
            if let Some(value) = organization.get(key) {
                org.insert(key.to_owned(), value.clone());
            }
        }
        kept.insert("organization".to_owned(), Value::Object(org));
    }
    // The keyword facet reads each tag's name (T-4948).
    if let Some(tags) = dataset.get("tags").and_then(Value::as_array) {
        let names = tags
            .iter()
            .map(|t| {
                let mut tag = serde_json::Map::new();
                for key in ["name", "display_name"] {
                    if let Some(value) = t.get(key) {
                        tag.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(tag)
            })
            .collect();
        kept.insert("tags".to_owned(), Value::Array(names));
    }
    if let Some(resources) = dataset.get("resources").and_then(Value::as_array) {
        let formats = resources
            .iter()
            .map(|r| match r.get("format") {
                Some(format) => serde_json::json!({ "format": format }),
                None => serde_json::json!({}),
            })
            .collect();
        kept.insert("resources".to_owned(), Value::Array(formats));
    }
    if let Some(extras) = dataset.get("extras").and_then(Value::as_array) {
        let page_extras = extras
            .iter()
            .filter(|e| {
                e.get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|k| PAGE_EXTRAS.contains(&k))
            })
            .cloned()
            .collect();
        kept.insert("extras".to_owned(), Value::Array(page_extras));
    }
    Value::Object(kept)
}

/// A catalogue answer anybody may keep for [`MAX_AGE_SECONDS`], gzipped for a client that takes
/// gzip: the list is about 44 KB of JSON, 0.2 s of a phone's first load on 4G (T-5165). Every
/// caller gets the same public answer, so no secret sits beside the reflected search text to be
/// read through the compressed length (BREACH).
fn cached<T: Serialize>(headers: &HeaderMap, body: T) -> Response {
    let cache = (
        header::CACHE_CONTROL,
        format!("public, max-age={MAX_AGE_SECONDS}"),
    );
    let vary = (header::VARY, "accept-encoding".to_owned());
    if takes_gzip(headers) {
        if let Some(gz) = gzipped(&body) {
            let json = (header::CONTENT_TYPE, "application/json".to_owned());
            let encoding = (header::CONTENT_ENCODING, "gzip".to_owned());
            return ([cache, vary, json, encoding], gz).into_response();
        }
    }
    ([cache, vary], Json(body)).into_response()
}

/// Whether `Accept-Encoding` names gzip and does not refuse it (`gzip;q=0`).
fn takes_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|item| {
            let mut parts = item.split(';').map(str::trim);
            let gzip = parts.next().is_some_and(|c| c.eq_ignore_ascii_case("gzip"));
            gzip && !parts
                .any(|p| p.strip_prefix("q=").and_then(|q| q.parse::<f32>().ok()) == Some(0.0))
        })
}

/// `body` as gzipped JSON; `None` sends it plain.
fn gzipped<T: Serialize>(body: &T) -> Option<Vec<u8>> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut gz, body).ok()?;
    gz.finish().ok()
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue",
    summary = "Search the Catalogue",
    description = "Every public dataset of the installation's open-data catalogues, searched and faceted (EP-81). Public: it lists only what an anonymous caller of the catalogue sees.",
    tag = "ckan",
    params(
        ("q" = Option<String>, Query, description = "Full-text search, passed to CKAN"),
        ("publisher" = Option<Vec<String>>, Query, description = "CKAN organization names"),
        ("theme" = Option<Vec<String>>, Query, description = "EU data-theme codes"),
        ("format" = Option<Vec<String>>, Query, description = "Resource formats"),
        ("licence" = Option<Vec<String>>, Query, description = "CKAN licence ids"),
        ("spatial" = Option<Vec<String>>, Query, description = "NUTS codes or location IRIs"),
        ("year" = Option<Vec<String>>, Query, description = "Calendar years covered"),
        ("page" = Option<usize>, Query, description = "Page, from 1"),
    ),
    responses(
        (status = 200, description = "One page of datasets and the facets", body = CataloguePage),
        (status = 503, description = "No catalogue answered", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_catalogue(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let filters = Filters::parse(query.as_deref());
    let urls = catalogues(&state);
    let mut datasets = Vec::new();
    let mut unavailable = Vec::new();
    for url in &urls {
        match search(&state.catalogue_http, url, &filters.q).await {
            Ok(found) => datasets.extend(found.iter().cloned()),
            Err(reason) => {
                tracing::warn!(catalogue = %url, %reason, "catalogue search failed");
                unavailable.push(url.clone());
            }
        }
    }
    if !urls.is_empty() && unavailable.len() == urls.len() {
        return Err(ApiError::Unavailable(format!(
            "no open-data catalogue answered ({}); try again in a minute",
            unavailable.join(", ")
        )));
    }
    let mut page = page_of(&datasets, &filters, chrono::Utc::now().year());
    page.unavailable = unavailable;
    Ok(cached(&headers, page))
}

/// The open-data datasets `q` finds in every catalogue of the installation, at most `limit`, each
/// with its page in the catalogue (`url`), as the public catalogue page lists them (EP-82), and
/// the catalogues that did not answer. A connector's catalogue search reads this (T-6027).
pub(crate) async fn datasets_matching(
    state: &AppState,
    q: &str,
    limit: usize,
) -> (Vec<Value>, Vec<String>) {
    let mut hits = Vec::new();
    let mut unavailable = Vec::new();
    for base in catalogues(state) {
        match search(&state.catalogue_http, &base, q).await {
            Ok(found) => hits.extend(
                found
                    .iter()
                    .filter(|dataset| dataset["private"] != Value::Bool(true))
                    .map(|dataset| {
                        let mut hit = hit(&base, dataset);
                        // The slug jc_data_query reads, only for a public Endpoint here (T-6272).
                        if let Some(slug) = public_slug(state, dataset) {
                            hit["endpoint"] = Value::String(slug);
                        }
                        hit
                    }),
            ),
            Err(reason) => {
                tracing::warn!(catalogue = %base, %reason, "catalogue search failed");
                unavailable.push(base);
            }
        }
    }
    hits.truncate(limit);
    (hits, unavailable)
}

/// The most resources one search hit lists.
const HIT_RESOURCES: usize = 10;

/// One search hit: the dataset page and each resource's `{format, url}`, the download a
/// DataSource reads; the page is HTML, which no pipeline maps (T-6269).
fn hit(base: &str, dataset: &Value) -> Value {
    let summary = summary(dataset);
    let resources: Vec<Value> = dataset
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let url = text(r, "url")
                .filter(|u| Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))?;
            Some(serde_json::json!({ "format": text(r, "format"), "url": url }))
        })
        .take(HIT_RESOURCES)
        .collect();
    serde_json::json!({
        "title": summary.title,
        "url": format!("{base}/dataset/{}", summary.name),
        "publisher": summary.publisher.map(|p| p.title),
        "formats": summary.formats,
        "resources": resources,
    })
}

/// The public dataset `name` and the catalogue that holds it.
async fn dataset(state: &AppState, name: &str) -> Result<(String, Value), ApiError> {
    let not_found = || ApiError::NotFound(format!("dataset '{name}' is not in the catalogue"));
    if name.is_empty() || name.len() > 100 {
        return Err(not_found());
    }
    let urls = catalogues(state);
    let mut answered = false;
    for url in &urls {
        match action(&state.catalogue_http, url, "package_show", &[("id", name)]).await {
            Ok(found) => {
                answered = true;
                if found.get("private").and_then(Value::as_bool) != Some(true)
                    && text(&found, "name").as_deref() == Some(name)
                {
                    return Ok((url.clone(), found));
                }
            }
            // CKAN answers an unknown or private dataset with `success: false`: an answer.
            Err(Failure::Refused) => answered = true,
            Err(Failure::Unreachable(reason)) => {
                tracing::warn!(catalogue = %url, %reason, "catalogue dataset read failed");
            }
        }
    }
    if urls.is_empty() || answered {
        Err(not_found())
    } else {
        Err(ApiError::Unavailable(
            "no open-data catalogue answered; try again in a minute".to_owned(),
        ))
    }
}

/// The slug of an Endpoint of this installation, read from the dataset's `endpoint` extra. A
/// URL on any other host, or of any other shape, names none: the Portal never fetches an
/// address the catalogue supplies (EP-82).
pub fn endpoint_slug(extra: &str, public_base: &Url) -> Option<String> {
    let url = Url::parse(extra).ok()?;
    if url.host_str() != public_base.host_str()
        || url.port_or_known_default() != public_base.port_or_known_default()
    {
        return None;
    }
    let slug = url
        .path()
        .strip_prefix("/api/endpoint/")?
        .trim_end_matches('/');
    let valid = slug.len() >= 26
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b));
    valid.then(|| slug.to_owned())
}

/// The slug of an Endpoint URL on the data host or, as written before it (T-4632), the Portal's.
fn our_endpoint_slug(state: &AppState, extra: &str) -> Option<String> {
    endpoint_slug(extra, &state.config.api_base_url())
        .or_else(|| endpoint_slug(extra, &state.config.public_base_url))
}

/// The slug of the dataset's Endpoint when it is a public Endpoint of this installation.
fn public_slug(state: &AppState, dataset: &Value) -> Option<String> {
    extra(dataset, "endpoint")
        .and_then(|e| our_endpoint_slug(state, &e))
        .filter(|slug| public_endpoint(state, slug).is_some())
}

/// The public Endpoint with `slug`, and the project it belongs to.
pub(crate) fn public_endpoint(
    state: &AppState,
    slug: &str,
) -> Option<(String, crate::resource::ResourceEnvelope)> {
    state.mirror.namespaces().into_iter().find_map(|project| {
        let endpoint = state
            .mirror
            .list(&project, "Endpoint", &ListOptions::default())
            .items
            .into_iter()
            .find(|env| env.spec["slug"].as_str() == Some(slug))?;
        (endpoint.spec["audience"].as_str() == Some("public")).then_some((project, endpoint))
    })
}

/// The DataModel of the space the Endpoint serves, with its classes described from the LinkML
/// source; a source the forge cannot give leaves the classes undescribed.
async fn model_of(
    state: &AppState,
    project: &str,
    endpoint: &Value,
    docs_url: Option<String>,
) -> Option<CatalogueDataModel> {
    let (name, names, linkml) = model_source(state, project, endpoint).await?;
    Some(CatalogueDataModel {
        classes: classes(&names, linkml.as_deref()),
        docs_url,
        name,
    })
}

pub(crate) fn endpoint_url(base: &Url, slug: &str) -> String {
    let origin = base.origin().ascii_serialization();
    format!("{origin}/api/endpoint/{slug}/")
}

/// What an Endpoint spec serves: the listed representations or, with no list, every one but
/// the opt-in `sta` and `ical` less `disabledRepresentations` (EP-104); then `ngsi-ld`, which every Endpoint serves
/// (EP-10), and `mcp` unless `spec.mcp` is `false` (EP-24). CSV is the one file: the retired
/// ones (`xlsx`, `parquet`, `json`, `zip`, `geojson`, T-4600) are never listed, whatever a
/// manifest the migration has not reached yet still names. The default set is jc-core's
/// `Representation::ALL`, so the page and the instance agree.
pub(crate) fn served_representations(spec: &Value) -> Vec<String> {
    let names = |field: &str| -> Option<Vec<String>> {
        spec[field].as_array().map(|list| {
            list.iter()
                .filter_map(|r| r.as_str().map(str::to_owned))
                .filter(|r| !crate::reconciler::representations::RETIRED.contains(&r.as_str()))
                .collect()
        })
    };
    let mcp_off = spec["mcp"] == Value::Bool(false);
    let mut served: Vec<String> = names("enabledRepresentations").unwrap_or_else(|| {
        let disabled = names("disabledRepresentations").unwrap_or_default();
        jc_core::kinds::endpoint::Representation::ALL
            .iter()
            // `sta` and `ical` are served only where listed (EP-104, jc-core `opt_in`).
            .filter(|r| !r.opt_in())
            .map(|r| r.as_str())
            .filter(|r| !(mcp_off && *r == "mcp"))
            .filter(|r| !crate::reconciler::representations::RETIRED.contains(r))
            .filter(|r| !disabled.iter().any(|d| d == r))
            .map(str::to_owned)
            .collect()
    });
    // A feed is its outputs, listed or not (EP-150, jc-core `served_representations`).
    served.retain(|r| r != "feed");
    let feed = spec["representations"]["feed"]
        .as_array()
        .is_some_and(|outputs| !outputs.is_empty())
        .then_some("feed");
    let mcp = (!mcp_off).then_some("mcp");
    for always in ["ngsi-ld"].into_iter().chain(mcp).chain(feed) {
        if !served.iter().any(|r| r == always) {
            served.push(always.to_owned());
        }
    }
    served
}

/// The dataset page's answer, from `package_show` and, when it names one, the Endpoint in the
/// mirror (EP-82).
pub async fn detail(state: &AppState, catalogue: &str, dataset: &Value) -> CatalogueDatasetDetail {
    let summary = summary(dataset);
    let name = summary.name.clone();
    let resources: Vec<CatalogueResource> = dataset
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let url = text(r, "url")?;
            Some(CatalogueResource {
                name: text(r, "name").unwrap_or_else(|| url.clone()),
                format: text(r, "format").unwrap_or_default(),
                description: text(r, "description"),
                url,
            })
        })
        .collect();

    let base = &state.config.api_base_url();
    let found = extra(dataset, "endpoint")
        .and_then(|e| our_endpoint_slug(state, &e))
        .and_then(|slug| public_endpoint(state, &slug).map(|found| (slug, found)));
    let (endpoint, model, quality) = match found {
        Some((slug, (project, endpoint))) => {
            let url = endpoint_url(base, &slug);
            let docs_url = resources
                .iter()
                .map(|r| &r.url)
                .find(|u| u.starts_with(&url) && u.ends_with(".md"))
                .cloned();
            let model = model_of(state, &project, &endpoint.spec, docs_url).await;
            let quality = crate::dataset_score::of_endpoint(state, &project, &endpoint)
                .await
                .public();
            (
                Some(CatalogueEndpoint {
                    feeds: feeds_of(&endpoint.spec, &url),
                    calendars: calendars_of(&endpoint.spec, &url),
                    url,
                    representations: served_representations(&endpoint.spec),
                }),
                model,
                Some(quality),
            )
        }
        None => (None, None, None),
    };

    CatalogueDatasetDetail {
        keywords: keywords(dataset),
        frequency: extra(dataset, "frequency"),
        spatial: spatial(dataset),
        temporal: match (
            extra(dataset, "temporal_start"),
            extra(dataset, "temporal_end"),
        ) {
            (None, None) => None,
            (start, end) => Some(CatalogueTemporal { start, end }),
        },
        contact: match (
            extra(dataset, "contact_name"),
            extra(dataset, "contact_email"),
        ) {
            (None, None) => None,
            (name, email) => Some(CatalogueContact { name, email }),
        },
        catalogue_url: format!("{catalogue}/dataset/{name}"),
        resources,
        endpoint,
        model,
        quality,
        name,
        title: summary.title,
        notes: summary.notes,
        publisher: summary.publisher,
        licence: summary.licence,
        themes: summary
            .themes
            .into_iter()
            .map(|code| CatalogueTheme {
                label: theme_label(&code),
                code,
            })
            .collect(),
        modified: summary.modified,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue/datasets/{name}",
    summary = "Read a Catalogue Dataset",
    description = "One public dataset: its description, resources, and the Endpoint and model behind it when it is this installation's (EP-82).",
    tag = "ckan",
    params(("name" = String, Path, description = "The dataset's name in the catalogue")),
    responses(
        (status = 200, description = "The dataset", body = CatalogueDatasetDetail),
        (status = 404, description = "No public dataset has this name", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_dataset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let (catalogue, found) = dataset(&state, &name).await?;
    Ok(cached(&headers, detail(&state, &catalogue, &found).await))
}

/// The members of a postal address in the order a letter writes them; any other member follows.
const ADDRESS_ORDER: [&str; 6] = [
    "streetAddress",
    "postOfficeBoxNumber",
    "postalCode",
    "addressLocality",
    "addressRegion",
    "addressCountry",
];

/// A cell of the sample as a person reads it (UI-16): text as it is, a list and a structured value
/// as their parts joined by commas (an address in postal order), a point as latitude, longitude,
/// any other geometry by its type's name.
fn cell(value: Option<&Value>) -> String {
    let join = |parts: Vec<String>| {
        parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => join(items.iter().map(|v| cell(Some(v))).collect()),
        Some(geometry) if is_geometry(geometry) => {
            let c = &geometry["coordinates"];
            match (&geometry["type"], c.get(0), c.get(1)) {
                (Value::String(t), Some(lon), Some(lat)) if t == "Point" => format!("{lat}, {lon}"),
                _ => cell(geometry.get("type")),
            }
        }
        Some(Value::Object(members)) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_by_key(|k| {
                ADDRESS_ORDER
                    .iter()
                    .position(|a| a == k)
                    .unwrap_or(ADDRESS_ORDER.len())
            });
            join(keys.into_iter().map(|k| cell(members.get(k))).collect())
        }
        Some(other) => other.to_string(),
    }
}

/// The table of up to ten keyValues entities: `id` and `type` first, then every other member
/// in the order it first appears.
pub fn table(entity_type: &str, entities: &[Value]) -> CatalogueSample {
    let mut columns = vec!["id".to_owned(), "type".to_owned()];
    let entities: Vec<&serde_json::Map<String, Value>> = entities
        .iter()
        .filter_map(Value::as_object)
        .take(SAMPLE_ROWS)
        .collect();
    for entity in &entities {
        for key in entity.keys() {
            if key != "@context" && !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    CatalogueSample {
        entity_type: entity_type.to_owned(),
        rows: entities
            .iter()
            .map(|entity| columns.iter().map(|c| cell(entity.get(c))).collect())
            .collect(),
        columns,
    }
}

/// Whether `value` is a GeoJSON geometry a map draws.
fn is_geometry(value: &Value) -> bool {
    value["type"]
        .as_str()
        .is_some_and(|t| GEOMETRIES.contains(&t))
        && value["coordinates"].is_array()
}

/// An entity's geometry: its `location`, else the first attribute holding a GeoJSON geometry.
fn geometry_of(entity: &serde_json::Map<String, Value>) -> Option<&Value> {
    entity
        .get("location")
        .filter(|v| is_geometry(v))
        .or_else(|| entity.values().find(|v| is_geometry(v)))
}

/// The map of up to [`MAP_ROWS`] keyValues entities of `class`: the key attributes the Endpoint's
/// `display` override, then the model, then jc-core's field order choose (EP-135, DM-98, DM-99),
/// labelled in `lang`, and every entity with a geometry as a feature.
pub fn map_of(
    class: &str,
    entities: &[Value],
    linkml: Option<&str>,
    view: Option<&jc_core::display_order::Display>,
    lang: &str,
) -> CatalogueMap {
    let entities: Vec<&serde_json::Map<String, Value>> = entities
        .iter()
        .filter_map(Value::as_object)
        .take(MAP_ROWS)
        .collect();
    let mut attributes: Vec<(&str, bool)> = Vec::new();
    for entity in &entities {
        for (key, value) in entity.iter() {
            if !["id", "type", "@context"].contains(&key.as_str())
                && !attributes.iter().any(|(name, _)| name == key)
            {
                attributes.push((key.as_str(), value.is_number()));
            }
        }
    }
    // A model that does not parse orders by the heuristic, as a type without one does.
    let order = jc_core::display_order::display_order(linkml, class, &attributes, view)
        .or_else(|_| jc_core::display_order::display_order(None, class, &attributes, view))
        .unwrap_or_default();
    // A popup shows text: a geometry or a structured value is no line of it.
    let scalar = |name: &str| {
        entities.iter().any(|e| {
            e.get(name)
                .is_some_and(|v| !v.is_object() && !v.is_array() && !v.is_null())
        })
    };
    let mut fields: Vec<CatalogueMapField> = order
        .iter()
        .filter(|f| f.summary && scalar(&f.slot))
        .map(|f| CatalogueMapField {
            slot: f.slot.clone(),
            label: f.label(lang),
        })
        .collect();
    // A type without a model class marks no key attribute: its first four name it.
    if fields.is_empty() {
        fields = order
            .iter()
            .filter(|f| scalar(&f.slot))
            .take(4)
            .map(|f| CatalogueMapField {
                slot: f.slot.clone(),
                label: f.label(lang),
            })
            .collect();
    }
    let features = entities
        .iter()
        .filter_map(|entity| {
            let id = entity.get("id")?.as_str()?.to_owned();
            let geometry = geometry_of(entity)?.clone();
            let properties = fields
                .iter()
                .filter_map(|f| {
                    let value = entity.get(&f.slot)?;
                    let text = cell(Some(value));
                    (!value.is_object() && !value.is_array() && !text.is_empty())
                        .then(|| (f.slot.clone(), text))
                })
                .collect();
            Some(CatalogueFeature {
                id,
                geometry,
                properties,
            })
        })
        .collect();
    CatalogueMap {
        entity_type: class.to_owned(),
        total: None,
        fields,
        features,
        basemap: None,
    }
}

/// The address of the Endpoint `slug` this Portal reads: in the cluster the gateway answers
/// directly; the public host is the same Endpoint.
fn gateway_endpoint(state: &AppState, slug: &str) -> String {
    let base = state
        .config
        .gateway_url
        .as_deref()
        .and_then(|g| Url::parse(g).ok())
        .unwrap_or_else(|| state.config.public_base_url.clone());
    endpoint_url(&base, slug)
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue/datasets/{name}/sample",
    summary = "Sample a Catalogue Dataset",
    description = "Up to ten entities of the dataset's first model class, read anonymously through its Endpoint (EP-82, EP-66).",
    tag = "ckan",
    params(("name" = String, Path, description = "The dataset's name in the catalogue")),
    responses(
        (status = 200, description = "The sample", body = CatalogueSample),
        (status = 404, description = "No public dataset of an Endpoint here that serves NGSI-LD", body = crate::error::ProblemDetails),
        (status = 503, description = "The Endpoint did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_sample(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let (catalogue, found) = dataset(&state, &name).await?;
    let detail = detail(&state, &catalogue, &found).await;
    let no_sample = || {
        ApiError::NotFound(format!(
            "dataset '{name}' has no endpoint here that serves NGSI-LD"
        ))
    };
    let endpoint = detail.endpoint.ok_or_else(no_sample)?;
    if !endpoint.representations.iter().any(|r| r == "ngsi-ld") {
        return Err(no_sample());
    }
    let entity_type = detail
        .model
        .and_then(|m| m.classes.into_iter().next())
        .map(|c| c.name)
        .ok_or_else(no_sample)?;
    let slug = our_endpoint_slug(&state, &endpoint.url).ok_or_else(no_sample)?;
    let url = format!("{}ngsi-ld/v1/entities", gateway_endpoint(&state, &slug));
    let limit = SAMPLE_ROWS.to_string();
    let unavailable = |reason: String| {
        tracing::warn!(dataset = %name, %reason, "catalogue sample failed");
        ApiError::Unavailable(format!(
            "the endpoint of dataset '{name}' did not answer; try again in a minute"
        ))
    };
    let response = HTTP
        .get(&url)
        .query(&[
            ("type", entity_type.as_str()),
            ("limit", limit.as_str()),
            ("options", "keyValues"),
        ])
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| unavailable(e.to_string()))?;
    if !response.status().is_success() {
        return Err(unavailable(format!("status {}", response.status())));
    }
    let entities: Vec<Value> = crate::capped::json(response, crate::capped::ANSWER_BYTES)
        .await
        .map_err(unavailable)?;
    Ok(cached(&headers, table(&entity_type, &entities)))
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue/datasets/{name}/map",
    summary = "Map a Catalogue Dataset",
    description = "Up to 1000 located entities of the first model class the dataset's Endpoint serves, read anonymously through its public Endpoint, with the key attributes its popups show (EP-82, T-5628). A dataset without locations answers no features.",
    tag = "ckan",
    params(("name" = String, Path, description = "The dataset's name in the catalogue"), MapQuery),
    responses(
        (status = 200, description = "The located entities", body = CatalogueMap),
        (status = 400, description = "`lang` is no language code", body = crate::error::ProblemDetails),
        (status = 404, description = "No public dataset of an Endpoint here that serves NGSI-LD", body = crate::error::ProblemDetails),
        (status = 503, description = "The Endpoint did not answer", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn get_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<MapQuery>,
) -> Result<Response, ApiError> {
    let lang = map_lang(query.lang)?;
    let (_, found) = dataset(&state, &name).await?;
    let no_map = || ApiError::NotFound(format!("dataset '{name}' has no public endpoint here"));
    let slug = extra(&found, "endpoint")
        .and_then(|e| our_endpoint_slug(&state, &e))
        .ok_or_else(no_map)?;
    let (project, endpoint) = public_endpoint(&state, &slug).ok_or_else(no_map)?;
    let map = map_cached(&slug, &lang, async {
        let (_, classes, linkml) = model_source(&state, &project, &endpoint.spec)
            .await
            .ok_or_else(no_map)?;
        let gateway = gateway_endpoint(&state, &slug);
        let class = mapped_class(classes, &served(&gateway).await).ok_or_else(no_map)?;
        let (total, entities) = located(&gateway, &class).await.map_err(|reason| {
            tracing::warn!(dataset = %name, %reason, "catalogue map failed");
            ApiError::Unavailable(format!(
                "The endpoint of dataset '{name}' did not answer; try again in a minute."
            ))
        })?;
        let view: Option<jc_core::display_order::Display> =
            serde_json::from_value(endpoint.spec["display"][&class].clone()).ok();
        let mut map = map_of(&class, &entities, linkml.as_deref(), view.as_ref(), &lang);
        map.total = total;
        map.basemap =
            crate::api::basemap::style_url(&state.config, &project).map(|_| project.clone());
        Ok(map)
    })
    .await?;
    Ok(cached(&headers, &*map))
}

/// The language a map labels its popups in: two or three lowercase letters, English when absent.
fn map_lang(lang: Option<String>) -> Result<String, ApiError> {
    let lang = lang.unwrap_or_else(|| "en".to_owned());
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(ApiError::BadRequest(format!(
            "lang '{lang}' is no language code; send two or three lowercase letters, e.g. lang=sk"
        )));
    }
    Ok(lang)
}

/// The class a dataset's map draws: the first model class its Endpoint serves. A space model
/// holds every class of the space while an Endpoint's Policy narrows it to one slice, so the first
/// model class drew an empty map for every Endpoint but the first (T-6539). The model's first
/// class stays when the Endpoint lists none it knows.
fn mapped_class(classes: Vec<String>, served: &[String]) -> Option<String> {
    classes
        .iter()
        .find(|class| served.contains(class))
        .or(classes.first())
        .cloned()
}

/// The types the Endpoint at `endpoint` lists to an anonymous reader; empty when it did not answer.
async fn served(endpoint: &str) -> Vec<String> {
    let Ok(response) = anonymous_read(format!("{endpoint}ngsi-ld/v1/types"), &[]).await else {
        return Vec::new();
    };
    let list: Value = crate::capped::json(response, crate::capped::ANSWER_BYTES)
        .await
        .unwrap_or_default();
    list["typeList"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}

/// One anonymous JSON GET of `url` on the Portal's own gateway: the only outbound call the map
/// route makes, so its reads share one request shape; the reason as text when it did not answer.
async fn anonymous_read(url: String, query: &[(&str, &str)]) -> Result<reqwest::Response, String> {
    let response = HTTP
        .get(url)
        .query(query)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("status {}", response.status()));
    }
    Ok(response)
}

/// Up to [`MAP_ROWS`] keyValues entities of `class` from the Endpoint at `endpoint`, read
/// anonymously, and how many the type holds; the reason as text when it did not answer.
///
/// The rows are read without a count (T-6541): a count walks the whole match and outlasted the
/// gateway's 10 s cut on Praha Tree while the rows alone took 0.2 s. A short page is its own
/// count; only a full page asks [`counted`], whose failure leaves the total unknown.
async fn located(endpoint: &str, class: &str) -> Result<(Option<u64>, Vec<Value>), String> {
    let url = format!("{endpoint}ngsi-ld/v1/entities");
    let limit = MAP_ROWS.to_string();
    let response = anonymous_read(
        url.clone(),
        &[
            ("type", class),
            ("limit", limit.as_str()),
            ("options", "keyValues"),
        ],
    )
    .await?;
    let entities: Vec<Value> = crate::capped::json(response, crate::capped::ANSWER_BYTES).await?;
    let total = if entities.len() < MAP_ROWS {
        u64::try_from(entities.len()).ok()
    } else {
        counted(&url, class).await
    };
    Ok((total, entities))
}

/// How long the map waits for a type's count before it shows its rows without one.
const COUNT_WAIT: Duration = Duration::from_secs(2);

/// The count the Endpoint reports for `class`, `None` when it sends none or takes past [`COUNT_WAIT`].
async fn counted(url: &str, class: &str) -> Option<u64> {
    let query = [("type", class), ("limit", "0"), ("count", "true")];
    let response = tokio::time::timeout(COUNT_WAIT, anonymous_read(url.to_owned(), &query))
        .await
        .ok()?
        .ok()?;
    response
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok()?.parse().ok())
}

/// The DataModel of the space an Endpoint serves: its name, its classes and its LinkML source,
/// `None` where the forge cannot give the source.
async fn model_source(
    state: &AppState,
    project: &str,
    endpoint: &Value,
) -> Option<(String, Vec<String>, Option<String>)> {
    let space = crate::resource::reference_name(&endpoint["contextSpaceRef"])?;
    let space = state.mirror.get(project, "ContextSpace", space)?;
    let (namespace, name) = crate::resource::model_reference(&space.spec["dataModelRef"], project)?;
    let model = state.mirror.get(namespace, "DataModel", name)?;
    let classes: Vec<String> = model.spec["classes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    let linkml = crate::api::datamodels::read_source(state, namespace, name)
        .await
        .ok();
    Some((name.to_owned(), classes, linkml))
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/catalogue", axum::routing::get(get_catalogue))
        .route(
            "/catalogue/datasets/{name}",
            axum::routing::get(get_dataset),
        )
        .route(
            "/catalogue/datasets/{name}/sample",
            axum::routing::get(get_sample),
        )
        .route(
            "/catalogue/datasets/{name}/map",
            axum::routing::get(get_map),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn accepting(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            value.parse().expect("a header value"),
        );
        headers
    }

    #[test]
    fn a_search_hit_lists_each_resource_with_its_format_and_download_url() {
        let dataset = json!({
            "name": "parkoviska", "title": "Parkoviská",
            "resources": [
                { "format": "CSV", "url": "https://data.example/parkoviska.csv" },
                { "format": "GeoJSON", "url": "https://data.example/parkoviska.geojson" },
                { "format": "HTML", "url": "javascript:alert(1)" },
                { "format": "XLS" }
            ]
        });
        let hit = hit("https://ckan.example", &dataset);
        assert_eq!(hit["url"], "https://ckan.example/dataset/parkoviska");
        assert_eq!(
            hit["resources"],
            json!([
                { "format": "CSV", "url": "https://data.example/parkoviska.csv" },
                { "format": "GeoJSON", "url": "https://data.example/parkoviska.geojson" }
            ])
        );
        let many =
            json!({ "name": "n", "resources": vec![json!({ "url": "https://d.example/a" }); 30] });
        assert_eq!(hit_resources(&many), HIT_RESOURCES);
    }

    fn hit_resources(dataset: &Value) -> usize {
        hit("https://ckan.example", dataset)["resources"]
            .as_array()
            .map_or(0, Vec::len)
    }

    #[test]
    fn takes_gzip_only_where_the_client_names_it_and_does_not_refuse_it() {
        assert!(takes_gzip(&accepting("gzip, deflate, br")));
        assert!(takes_gzip(&accepting("br;q=1.0, GZIP;q=0.5")));
        assert!(!takes_gzip(&accepting("gzip;q=0")));
        assert!(!takes_gzip(&accepting("br, gzip; q=0.0")));
        assert!(!takes_gzip(&accepting("identity")));
        assert!(!takes_gzip(&accepting("x-gzip-like")));
        assert!(!takes_gzip(&HeaderMap::new()));
    }

    #[tokio::test]
    async fn a_catalogue_answer_is_gzipped_for_a_client_that_takes_it_and_plain_otherwise() {
        use std::io::Read;
        let body = json!({"datasets": [{"name": "bbsk-kpi", "title": "Ukazovatele kraja"}]});
        let zipped = cached(&accepting("gzip, br"), body.clone());
        assert_eq!(zipped.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(zipped.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(zipped.headers()[header::VARY], "accept-encoding");
        assert_eq!(
            zipped.headers()[header::CACHE_CONTROL],
            format!("public, max-age={MAX_AGE_SECONDS}")
        );
        let bytes = axum::body::to_bytes(zipped.into_body(), usize::MAX)
            .await
            .expect("a body");
        let mut text = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_string(&mut text)
            .expect("gzip");
        assert_eq!(serde_json::from_str::<Value>(&text).expect("json"), body);

        let plain = cached(&accepting("gzip;q=0"), body.clone());
        assert!(plain.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(plain.headers()[header::VARY], "accept-encoding");
        let bytes = axum::body::to_bytes(plain.into_body(), usize::MAX)
            .await
            .expect("a body");
        assert_eq!(serde_json::from_slice::<Value>(&bytes).expect("json"), body);
    }

    /// A local server answering every request with `head` and then `body` bytes of `x`.
    async fn answering(head: &'static str, body: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _ = socket.read(&mut [0u8; 2048]).await;
                    let _ = socket.write_all(head.as_bytes()).await;
                    let chunk = vec![b'x'; 64 * 1024];
                    let mut left = body;
                    while left > 0
                        && socket
                            .write_all(&chunk[..left.min(chunk.len())])
                            .await
                            .is_ok()
                    {
                        left -= left.min(chunk.len());
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    /// T-4776: a `CkanInstance` naming an address inside the cluster is not read, though a
    /// service answers there like CKAN.
    #[tokio::test]
    async fn a_catalogue_inside_the_cluster_is_not_read() {
        let inside = answering(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 27\r\n\r\n{\"success\":true,\"result\":1}",
            0,
        )
        .await;
        assert!(matches!(
            action(&CKAN_HTTP, &inside, "package_search", &[]).await,
            Err(Failure::Unreachable(reason)) if reason.contains("internal address")
        ));
    }

    /// T-4776: a catalogue that streams past the cap is cut off rather than read whole.
    #[tokio::test]
    async fn an_answer_past_the_cap_is_not_read() {
        let base = answering(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            MAX_CKAN_ANSWER + 1024 * 1024,
        )
        .await;
        let read = action_with(&reqwest::Client::new(), &base, "package_search", &[]).await;
        assert!(matches!(read, Err(Failure::Unreachable(reason)) if reason.contains("more than")));
    }

    fn dataset(name: &str, org: &str, extras: Value, formats: &[&str], private: bool) -> Value {
        json!({
            "name": name,
            "title": name.to_uppercase(),
            "private": private,
            "organization": { "name": org, "title": format!("{org} title") },
            "license_id": "cc-by",
            "license_title": "Creative Commons Attribution",
            "resources": formats.iter().map(|f| json!({ "format": f, "url": "https://x/" })).collect::<Vec<_>>(),
            "extras": extras,
        })
    }

    fn extras(pairs: &[(&str, &str)]) -> Value {
        Value::Array(
            pairs
                .iter()
                .map(|(k, v)| json!({ "key": k, "value": v }))
                .collect(),
        )
    }

    /// T-6272: a hit names its Endpoint's slug only when it is a public Endpoint here; a
    /// project-only Endpoint or another host's address names none.
    /// JC1-53: a space whose dataModelRef names an organization model finds that model and
    /// its classes, as the policy check and the assistant context already did.
    #[tokio::test]
    async fn an_organization_model_of_a_space_is_found() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let put = |kind: &str, name: &str, namespace: &str, spec: Value| {
            state.mirror.upsert(crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        };
        put(
            "ContextSpace",
            "air",
            "helsinki",
            json!({ "dataModelRef": { "name": "air-quality", "namespace": "org" } }),
        );
        put(
            "DataModel",
            "air-quality",
            "org",
            json!({ "classes": ["AirQualityObserved"] }),
        );
        let found = model_source(&state, "helsinki", &json!({ "contextSpaceRef": "air" })).await;
        let (name, classes, _) = found.expect("the organization's model");
        assert_eq!(
            (name.as_str(), classes),
            ("air-quality", vec!["AirQualityObserved".to_owned()])
        );
    }

    #[test]
    fn a_hit_names_the_slug_of_a_public_endpoint_alone() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        for (name, slug, audience) in [
            ("stops", "stopshelsinkizzzzzzzzzzzzzzz", "public"),
            ("staff", "staffhelsinkizzzzzzzzzzzzzzz", "project"),
        ] {
            state.mirror.upsert(crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, "helsinki"),
                spec: json!({ "slug": slug, "audience": audience }),
                status: None,
            });
        }
        let named = |url: String| {
            public_slug(
                &state,
                &dataset("d", "o", extras(&[("endpoint", &url)]), &[], false),
            )
        };
        let ours = |slug: &str| endpoint_url(&state.config.api_base_url(), slug);
        assert_eq!(
            named(ours("stopshelsinkizzzzzzzzzzzzzzz")).as_deref(),
            Some("stopshelsinkizzzzzzzzzzzzzzz")
        );
        assert_eq!(named(ours("staffhelsinkizzzzzzzzzzzzzzz")), None);
        assert_eq!(
            named("https://other.example/api/endpoint/stopshelsinkizzzzzzzzzzzzzzz/".into()),
            None
        );
    }

    fn fixture() -> Vec<Value> {
        vec![
            dataset(
                "air",
                "banskabystrica",
                extras(&[
                    (
                        "theme",
                        r#"["http://publications.europa.eu/resource/authority/data-theme/ENVI"]"#,
                    ),
                    ("temporal_start", "2024-03-01"),
                    ("temporal_end", "2025-06-30"),
                    ("spatial_uri", "http://data.europa.eu/nuts/code/SK032"),
                ]),
                &["CSV", "NGSI-LD"],
                false,
            ),
            dataset(
                "kpi",
                "bbsk",
                extras(&[(
                    "theme",
                    r#"["http://publications.europa.eu/resource/authority/data-theme/ECON","http://publications.europa.eu/resource/authority/data-theme/ENVI"]"#,
                )]),
                &["CSV"],
                false,
            ),
            dataset("secret", "bbsk", json!([]), &["CSV"], true),
        ]
    }

    /// A searched dataset as CKAN sends it: the page's fields among many it never reads.
    fn heavy(mut dataset: Value) -> Value {
        let object = dataset.as_object_mut().unwrap();
        object.insert("tags".into(), json!([{ "name": "ovzdusie" }]));
        object.insert("author_email".into(), json!("a@b.sk"));
        object["organization"]["description"] = json!("x".repeat(500));
        object["organization"]["image_url"] = json!("https://img");
        for resource in object["resources"].as_array_mut().unwrap() {
            resource["url"] = json!("https://data/x.csv");
            resource["description"] = json!("y".repeat(300));
        }
        object["extras"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "key": "contact_email", "value": "c@d.sk" }));
        dataset
    }

    #[test]
    fn a_slim_search_answer_pages_exactly_as_the_full_one() {
        let full: Vec<Value> = fixture().into_iter().map(heavy).collect();
        let slimmed: Vec<Value> = full.iter().map(slim).collect();
        for query in [
            None,
            Some("year=2025"),
            Some("publisher=bbsk"),
            Some("format=CSV&theme=ENVI"),
            Some("spatial=http://data.europa.eu/nuts/code/SK032"),
            Some("page=2"),
        ] {
            let filters = Filters::parse(query);
            assert_eq!(
                page_of(&slimmed, &filters, 2026),
                page_of(&full, &filters, 2026),
                "{query:?}"
            );
        }
        let (full_bytes, slim_bytes) = (
            serde_json::to_vec(&full).unwrap().len(),
            serde_json::to_vec(&slimmed).unwrap().len(),
        );
        assert!(slim_bytes * 2 < full_bytes, "{slim_bytes} of {full_bytes}");
    }

    #[test]
    fn a_thousand_distinct_queries_keep_the_searches_under_their_byte_cap() {
        let answer: Vec<Value> = (0..200)
            .flat_map(|_| fixture())
            .map(|d| slim(&heavy(d)))
            .collect();
        let bytes = serde_json::to_vec(&answer).unwrap().len();
        let answer = Arc::new(answer);
        for n in 0..1000 {
            let key = ("https://ckan.test".to_owned(), format!("query {n}"));
            let mut cache = SEARCHES.lock().unwrap();
            cache.insert(key, answer.clone(), bytes);
            assert!(cache.bytes() <= SEARCHES_MAX_BYTES);
        }
        let cache = SEARCHES.lock().unwrap();
        assert!(cache.len() < 1000 && !cache.is_empty(), "{}", cache.len());
    }

    #[test]
    fn the_keywords_datasets_carry_are_a_facet_that_filters_nothing() {
        let mut datasets: Vec<Value> = fixture().into_iter().map(heavy).collect();
        datasets[0]["tags"] =
            json!([{ "name": "ovzdusie" }, { "name": "pm10", "display_name": "PM10" }]);
        let filters = Filters::parse(Some("publisher=bbsk"));
        let page = page_of(&datasets, &filters, 2026);
        let keyword = |name: &str| {
            page.facets
                .keyword
                .iter()
                .find(|v| v.value == name)
                .map(|v| v.count)
        };
        // The private dataset's tag is not counted, and the publisher filter narrows the keywords.
        assert_eq!(keyword("ovzdusie"), Some(1), "{:?}", page.facets.keyword);
        assert_eq!(keyword("PM10"), None, "{:?}", page.facets.keyword);
        let all = page_of(&datasets, &Filters::parse(None), 2026);
        assert_eq!(all.total, 2, "a keyword facet filters no dataset away");
        assert!(all
            .facets
            .keyword
            .iter()
            .any(|v| v.value == "PM10" && v.count == 1));
    }

    #[test]
    fn a_private_dataset_is_never_listed_nor_counted() {
        let page = page_of(&fixture(), &Filters::parse(None), 2026);
        assert_eq!(page.total, 2);
        assert!(page.datasets.iter().all(|d| d.name != "secret"));
        let bbsk = page
            .facets
            .publisher
            .iter()
            .find(|v| v.value == "bbsk")
            .unwrap();
        assert_eq!(bbsk.count, 1);
    }

    #[test]
    fn facets_count_over_the_other_filters_and_values_of_one_filter_are_alternatives() {
        let filters = Filters::parse(Some("theme=ECON&theme=ENVI&publisher=bbsk"));
        let page = page_of(&fixture(), &filters, 2026);
        assert_eq!(page.total, 1);
        assert_eq!(page.datasets[0].name, "kpi");
        // The publisher facet ignores its own filter: both publishers stay selectable.
        assert_eq!(page.facets.publisher.len(), 2);
        // The theme facet counts under the publisher filter only.
        let themes: Vec<(&str, usize)> = page
            .facets
            .theme
            .iter()
            .map(|v| (v.value.as_str(), v.count))
            .collect();
        assert_eq!(themes, vec![("ECON", 1), ("ENVI", 1)]);
    }

    #[test]
    fn temporal_coverage_is_one_value_per_year_and_an_open_end_runs_to_this_year() {
        let page = page_of(&fixture(), &Filters::parse(Some("year=2025")), 2026);
        assert_eq!(page.total, 1);
        let years: Vec<&str> = page.facets.year.iter().map(|v| v.value.as_str()).collect();
        assert_eq!(years, vec!["2024", "2025"]);
        let open = dataset(
            "open",
            "o",
            extras(&[("temporal_start", "2025-01-01")]),
            &[],
            false,
        );
        assert_eq!(years_of(&open), vec![2025, 2026]);
        let bad = dataset(
            "bad",
            "o",
            extras(&[("temporal_start", "someday")]),
            &[],
            false,
        );
        assert!(years_of(&bad).is_empty());
    }

    fn years_of(d: &Value) -> Vec<i32> {
        years(d, 2026)
    }

    #[test]
    fn spatial_is_labelled_by_its_code_and_geojson_is_no_facet() {
        let page = page_of(&fixture(), &Filters::default_page(), 2026);
        assert_eq!(page.facets.spatial[0].label, "SK032");
        let geo = dataset(
            "geo",
            "o",
            extras(&[("spatial", r#"{"type":"Point"}"#)]),
            &[],
            false,
        );
        assert!(spatial(&geo).is_empty());
    }

    #[test]
    fn pages_are_twenty_datasets_and_a_bad_page_is_the_first() {
        let many: Vec<Value> = (0..45)
            .map(|i| dataset(&format!("d{i}"), "o", json!([]), &[], false))
            .collect();
        assert_eq!(
            page_of(&many, &Filters::parse(Some("page=3")), 2026)
                .datasets
                .len(),
            5
        );
        assert_eq!(Filters::parse(Some("page=0")).page, 1);
        assert_eq!(Filters::parse(Some("page=x")).page, 1);
        assert_eq!(
            page_of(&many, &Filters::parse(Some("page=9")), 2026)
                .datasets
                .len(),
            0
        );
    }

    #[test]
    fn only_an_endpoint_url_on_this_host_names_a_slug() {
        let base = Url::parse("https://api.dev.joinedcontext.com").unwrap();
        let slug = "mluyob4nz52lok3ssk7pgn5vwt";
        assert_eq!(
            endpoint_slug(
                &format!("https://api.dev.joinedcontext.com/api/endpoint/{slug}/"),
                &base
            ),
            Some(slug.to_owned())
        );
        for foreign in [
            format!("https://evil.example/api/endpoint/{slug}/"),
            format!("https://api.dev.joinedcontext.com:8443/api/endpoint/{slug}/"),
            format!("https://api.dev.joinedcontext.com/api/endpoint/{slug}/../../internal"),
            "https://api.dev.joinedcontext.com/api/endpoint/short/".to_owned(),
            format!("https://api.dev.joinedcontext.com/cs/{slug}"),
            "not a url".to_owned(),
        ] {
            assert_eq!(endpoint_slug(&foreign, &base), None, "{foreign}");
        }
    }

    const AIR: &str = r#"
id: https://example.org/air
name: air
in_language: sk
classes:
  AirQualityObserved:
    attributes:
      name:
        annotations: { summary: true, label: { sk: Názov, en: Name } }
      pm10:
        annotations: { summary: true, label: { sk: Prachové častice PM10 } }
      address:
        annotations: { summary: true }
      location: {}
      dateObserved: {}
"#;

    fn air(id: &str, location: Option<Value>) -> Value {
        let mut entity = json!({
            "id": id,
            "type": "AirQualityObserved",
            "name": "Stanica",
            "pm10": 21.5,
            "address": { "addressLocality": "Praha" },
            "dateObserved": "2026-10-24T10:00:00Z",
        });
        if let Some(location) = location {
            entity["location"] = location;
        }
        entity
    }

    #[test]
    fn the_map_draws_each_located_entity_with_the_models_key_attributes_in_the_language_asked() {
        let point = json!({ "type": "Point", "coordinates": [14.42, 50.08] });
        let entities = vec![
            air("urn:ngsi-ld:AirQualityObserved:a", Some(point.clone())),
            air("urn:ngsi-ld:AirQualityObserved:b", None),
            air(
                "urn:ngsi-ld:AirQualityObserved:c",
                Some(json!({ "type": "Point", "coordinates": "nowhere" })),
            ),
        ];
        let map = map_of("AirQualityObserved", &entities, Some(AIR), None, "sk");
        // The address is a structured value: no popup line holds raw JSON.
        assert_eq!(
            map.fields,
            vec![
                CatalogueMapField {
                    slot: "name".into(),
                    label: "Názov".into()
                },
                CatalogueMapField {
                    slot: "pm10".into(),
                    label: "Prachové častice PM10".into()
                },
            ]
        );
        assert_eq!(map.features.len(), 1);
        assert_eq!(map.features[0].id, "urn:ngsi-ld:AirQualityObserved:a");
        assert_eq!(map.features[0].geometry, point);
        assert_eq!(
            map.features[0].properties,
            BTreeMap::from([
                ("name".into(), "Stanica".into()),
                ("pm10".into(), "21.5".into())
            ])
        );
        assert_eq!(
            map_of("AirQualityObserved", &entities, Some(AIR), None, "en").fields[0].label,
            "Name"
        );
    }

    #[test]
    fn the_endpoints_display_override_chooses_the_popup_before_the_model() {
        let point = json!({ "type": "Point", "coordinates": [14.42, 50.08] });
        let view: jc_core::display_order::Display =
            serde_json::from_value(json!({ "summary": ["dateObserved"] })).expect("a display");
        let entities = vec![air("urn:ngsi-ld:AirQualityObserved:a", Some(point))];
        let map = map_of(
            "AirQualityObserved",
            &entities,
            Some(AIR),
            Some(&view),
            "en",
        );
        let slots: Vec<&str> = map.fields.iter().map(|f| f.slot.as_str()).collect();
        assert_eq!(slots, vec!["dateObserved"]);
    }

    #[test]
    fn a_dataset_without_locations_has_no_features_and_a_type_without_a_model_still_names_its_points(
    ) {
        let entities = vec![air("urn:ngsi-ld:AirQualityObserved:a", None)];
        assert!(
            map_of("AirQualityObserved", &entities, Some(AIR), None, "en")
                .features
                .is_empty()
        );
        let located = vec![air(
            "urn:ngsi-ld:AirQualityObserved:a",
            Some(json!({ "type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]] })),
        )];
        let map = map_of("AirQualityObserved", &located, None, None, "en");
        assert_eq!(map.features.len(), 1);
        assert!(!map.fields.is_empty());
        assert!(map
            .fields
            .iter()
            .all(|f| f.slot != "location" && f.slot != "address"));
    }

    #[test]
    fn the_sample_puts_id_and_type_first_and_writes_nested_values_as_text() {
        let sample = table(
            "KeyPerformanceIndicator",
            &[
                json!({ "id": "urn:a", "type": "KeyPerformanceIndicator", "value": 12, "@context": "x" }),
                json!({ "id": "urn:b", "type": "KeyPerformanceIndicator", "name": "b", "loc": { "type": "Point" } }),
            ],
        );
        assert_eq!(sample.columns, vec!["id", "type", "value", "loc", "name"]);
        assert_eq!(
            sample.rows[0],
            vec!["urn:a", "KeyPerformanceIndicator", "12", "", ""]
        );
        assert_eq!(sample.rows[1][3], "Point");
        // The walker's raw-json finding on /catalogue/bbsk-podujatia (T-5850): no cell is JSON.
        let event = table(
            "Event",
            &[json!({
                "id": "urn:e",
                "type": "Event",
                "address": {
                    "addressCountry": "SK", "addressLocality": "Brezno",
                    "addressRegion": "Banskobystrický kraj", "postalCode": "977 01",
                    "streetAddress": "Námestie gen. M.R.Štefánika 7/9"
                },
                "category": ["exhibition-museum", "music"],
                "location": { "type": "Point", "coordinates": [19.64271, 48.807343] },
                "area": { "type": "Polygon", "coordinates": [[[0, 0], [1, 0], [0, 1], [0, 0]]] },
                "count": [1, null, true]
            })],
        );
        assert_eq!(
            event.rows[0],
            vec![
                "urn:e",
                "Event",
                "Námestie gen. M.R.Štefánika 7/9, 977 01, Brezno, Banskobystrický kraj, SK",
                "Polygon",
                "exhibition-museum, music",
                "1, true",
                "48.807343, 19.64271",
            ]
        );
        let many: Vec<Value> = (0..30)
            .map(|i| json!({ "id": format!("urn:{i}") }))
            .collect();
        assert_eq!(table("T", &many).rows.len(), 10);
        assert!(table("T", &[]).rows.is_empty());
    }

    #[test]
    fn a_theme_facet_is_named_by_the_eu_vocabulary_and_an_unknown_code_by_itself() {
        let page = page_of(&fixture(), &Filters::parse(None), 2026);
        let labels: Vec<(&str, &str)> = page
            .facets
            .theme
            .iter()
            .map(|v| (v.value.as_str(), v.label.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![("ENVI", "Environment"), ("ECON", "Economy and finance")]
        );
        assert_eq!(theme_label("XYZ"), "XYZ");
    }

    #[test]
    fn classes_are_described_from_the_linkml_in_any_of_its_shapes() {
        let linkml = r#"
classes:
  School:
    description: A school of the region.
  Course:
    description:
      sk: Kurz
      en: A course taught at a school.
  Pupil:
    description:
      sk: Žiak
  Room: {}
"#;
        let names: Vec<String> = ["School", "Course", "Pupil", "Room", "Ghost"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let got = classes(&names, Some(linkml));
        assert_eq!(
            got[0].description.as_deref(),
            Some("A school of the region.")
        );
        assert_eq!(
            got[1].description.as_deref(),
            Some("A course taught at a school.")
        );
        assert_eq!(got[2].description.as_deref(), Some("Žiak"));
        assert_eq!(got[3].description, None);
        assert_eq!(got[4].name, "Ghost");
        assert_eq!(got[4].description, None);
        assert!(classes(&names, Some(": not yaml ["))
            .iter()
            .all(|c| c.description.is_none()));
        assert!(classes(&names, None)
            .iter()
            .all(|c| c.description.is_none()));
    }

    impl Filters {
        fn default_page() -> Self {
            Filters::parse(None)
        }
    }

    /// EP-10, EP-24: NGSI-LD and MCP are on without being listed, internal or public; only
    /// `mcp: false` leaves MCP out, a listed one is not doubled, and CSV is the one file
    /// (T-4600): a retired representation a manifest still names is not served.
    #[test]
    fn every_endpoint_serves_ngsi_ld_and_mcp_and_csv_is_the_one_file() {
        let served = |spec: Value| served_representations(&spec);
        assert_eq!(
            served(
                json!({ "audience": "project-list", "enabledRepresentations": ["ngsi-ld", "csv"] })
            ),
            ["ngsi-ld", "csv", "mcp"]
        );
        assert_eq!(
            served(json!({ "enabledRepresentations": ["mcp", "geojson", "xlsx", "ngsi-ld"] })),
            ["mcp", "ngsi-ld"]
        );
        assert_eq!(
            served(json!({ "enabledRepresentations": ["csv"], "mcp": false })),
            ["csv", "ngsi-ld"]
        );
        let every_but_sta = [
            "ngsi-ld",
            "mcp",
            "csv",
            "ogc-features",
            "odata",
            "websocket",
        ];
        assert_eq!(
            served(json!({})),
            every_but_sta,
            "a spec with no list serves every standard but the opt-in sta and ical (EP-104)"
        );
        assert_eq!(
            served(json!({ "enabledRepresentations": ["ical", "sta"] })),
            ["ical", "sta", "ngsi-ld", "mcp"],
            "an opt-in one is served where it is listed"
        );
        assert_eq!(served(json!({ "mcp": true })), every_but_sta);
        assert_eq!(
            served(json!({ "disabledRepresentations": ["odata", "csv", "ngsi-ld"] })),
            ["mcp", "ogc-features", "websocket", "ngsi-ld"],
            "disabled ones are off, the uniform surface stays"
        );
        assert_eq!(
            served(json!({ "mcp": false, "disabledRepresentations": ["websocket"] })),
            ["ngsi-ld", "csv", "ogc-features", "odata"]
        );
        assert_eq!(
            served(
                json!({ "enabledRepresentations": ["sta"], "disabledRepresentations": ["csv"] })
            ),
            ["sta", "ngsi-ld", "mcp"],
            "a list wins; sta is served only where listed"
        );
    }

    #[test]
    fn a_map_is_labelled_in_english_unless_a_language_code_is_asked() {
        assert_eq!(map_lang(None).ok().as_deref(), Some("en"));
        assert_eq!(map_lang(Some("sk".into())).ok().as_deref(), Some("sk"));
        assert_eq!(map_lang(Some("fil".into())).ok().as_deref(), Some("fil"));
        for bad in ["", "s", "SK", "sk-SK", "slov", "s1", "<b>"] {
            assert!(
                matches!(map_lang(Some(bad.into())), Err(ApiError::BadRequest(_))),
                "{bad:?} is refused"
            );
        }
    }

    #[tokio::test]
    async fn two_map_reads_in_the_window_build_the_map_once() {
        let builds = std::sync::atomic::AtomicUsize::new(0);
        let build = || async {
            builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(map_of("Thing", &[], None, None, "en"))
        };
        let slug = "t5728-cached-map";
        let first = map_cached(slug, "en", build()).await.expect("a map");
        let second = map_cached(slug, "en", build()).await.expect("a map");
        assert_eq!(first, second);
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
        map_cached(slug, "sk", build()).await.expect("a map");
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 2);
        let failing = map_cached("t5728-failing-map", "en", async {
            Err(ApiError::Unavailable("down".into()))
        })
        .await;
        assert!(failing.is_err());
        map_cached("t5728-failing-map", "en", build())
            .await
            .expect("a failure is not kept");
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn a_map_draws_the_first_class_its_endpoint_serves() {
        let classes = || {
            vec![
                "AirQualityObserved".to_owned(),
                "StreetFurniture".to_owned(),
            ]
        };
        assert_eq!(
            mapped_class(classes(), &["StreetFurniture".to_owned()]).as_deref(),
            Some("StreetFurniture"),
            "the space model's first class is not the Endpoint's"
        );
        assert_eq!(
            mapped_class(classes(), &[]).as_deref(),
            Some("AirQualityObserved"),
            "an Endpoint that lists nothing keeps the model's first class"
        );
        assert_eq!(mapped_class(Vec::new(), &["Tree".to_owned()]), None);
    }

    #[tokio::test]
    async fn the_served_types_are_the_endpoints_type_list() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ngsi-ld/v1/types"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "urn:ngsi-ld:EntityTypeList:1",
                "type": "EntityTypeList",
                "typeList": ["StreetFurniture"]
            })))
            .mount(&gateway)
            .await;
        assert_eq!(
            served(&format!("{}/", gateway.uri())).await,
            vec!["StreetFurniture"]
        );
        assert!(
            served("http://localhost:9/").await.is_empty(),
            "no answer serves nothing"
        );
    }

    /// A broker whose `count=true` answers 503 late, as the gateway's 10 s cut did on Praha Tree.
    async fn counts_cut(rows: usize) -> wiremock::MockServer {
        use wiremock::matchers::{method, path, query_param};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .and(path("/ngsi-ld/v1/entities"))
            .and(query_param("count", "true"))
            .respond_with(wiremock::ResponseTemplate::new(503).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let body: Vec<Value> = (0..rows)
            .map(|i| json!({"id": format!("urn:ngsi-ld:Tree:{i}"), "type": "Tree"}))
            .collect();
        wiremock::Mock::given(method("GET"))
            .and(path("/ngsi-ld/v1/entities"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_short_page_is_its_own_count_and_asks_no_other() {
        let server = counts_cut(3).await;
        let (total, entities) = located(&format!("{}/", server.uri()), "Tree")
            .await
            .expect("the rows answer without a count");
        assert_eq!((total, entities.len()), (Some(3), 3));
        let asked = server.received_requests().await.expect("recorded");
        assert_eq!(asked.len(), 1, "one read of rows, no count");
    }

    #[tokio::test]
    async fn a_count_cut_by_the_gateway_leaves_the_map_its_rows() {
        let server = counts_cut(MAP_ROWS).await;
        let started = std::time::Instant::now();
        let (total, entities) = located(&format!("{}/", server.uri()), "Tree")
            .await
            .expect("a failed count is no failed map");
        assert_eq!((total, entities.len()), (None, MAP_ROWS));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the map waits at most COUNT_WAIT for a count"
        );
    }

    #[tokio::test]
    async fn an_endpoint_that_does_not_answer_gives_its_reason() {
        let refused = located("http://localhost:9/", "AirQualityObserved").await;
        assert!(refused.is_err(), "a closed port is a reason, not a map");
    }
}
