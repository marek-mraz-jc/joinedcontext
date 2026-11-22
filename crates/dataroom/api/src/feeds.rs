#![forbid(unsafe_code)]
//! DCAT-AP feeds (`/catalog.ttl`, `/catalog.jsonld`, `/catalog.rdf`, `/catalog.nt`,
//! `/profiles/{profile}/catalog.*`, `/publisher/{id}/catalog.*`) with `Accept-Profile`
//! negotiation and Hydra pagination, plus Atom feeds (`/feeds/*.atom`) (C7, C16, C17).

use crate::{problem_response, DataroomState};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use dataroom_core::{
    text_in_lang, Catalogue, DataroomError, Dataset, Publisher, RecordState, RequestCtx, Result,
    ShaclSeverity, Visibility,
};
use dataroom_rdf::rdfxml::xml_escape as escape_xml;
use dataroom_rdf::{
    accepted_profiles, negotiate_rdf_format, serialize_catalogue, DcatProfile, HydraPageInfo,
    RdfFormat,
};
use dataroom_shacl::{predicate_to_field_name, validate_graph_against_shapes, Stack};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex};

/// Datasets per DCAT feed page (`ours/deployment/README.md`: 100 datasets per page).
pub const DCAT_PAGE_SIZE: usize = 100;
/// Datasets per Atom feed page.
pub const ATOM_PAGE_SIZE: usize = 20;

/// Verdicts a feed keeps before it starts over (a few MB at most).
const CONFORMANCE_CAP: usize = 100_000;

/// The fields that keep a dataset out of a profile's feed, none when it conforms, by a hash of the exact graph validated,
/// the shapes and the report subject: a feed page validates only a dataset, catalogue,
/// publisher or profile that changed since the last page (P-7-02). The hasher's keys are
/// random per process, so nobody can craft a dataset that collides with another's verdict.
#[derive(Debug, Default)]
pub struct FeedConformance {
    hasher: std::hash::RandomState,
    verdicts: Mutex<HashMap<u64, Arc<[String]>>>,
}

impl FeedConformance {
    fn missing(
        &self,
        stack: &Stack,
        graph: &dataroom_rdf::RdfGraph,
        cat: &Catalogue,
        ds_id: &str,
    ) -> Arc<[String]> {
        let profile = &stack.record.id;
        let key = self.hasher.hash_one((
            stack.key,
            &graph.triples,
            cat.org_id,
            cat.id,
            ds_id,
            profile,
        ));
        let verdicts = || {
            self.verdicts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        if let Some(missing) = verdicts().get(&key) {
            return missing.clone();
        }
        let report = validate_graph_against_shapes(
            graph,
            &stack.shapes,
            cat.org_id,
            cat.id,
            ds_id,
            profile,
            &stack.record.version,
        );
        let missing: Arc<[String]> = if report.conforms {
            Arc::default()
        } else {
            let fields: BTreeSet<String> = report
                .findings
                .iter()
                .filter(|f| f.severity == ShaclSeverity::Violation)
                .map(|f| match &f.result_path {
                    Some(path) => predicate_to_field_name(path.trim_matches(['<', '>'])),
                    None => f.source_shape.clone(),
                })
                .collect();
            fields.into_iter().collect()
        };
        let mut v = verdicts();
        if v.len() >= CONFORMANCE_CAP {
            v.clear();
        }
        v.insert(key, missing.clone());
        missing
    }
}

/// The profile one request is answered in: one selection for every feed and for a dataset's
/// own RDF (T-7521 C8).
pub(crate) struct Selected {
    pub(crate) stack: Arc<Stack>,
    /// It is not what the catalogue answers unasked, so a paging link has to name it.
    asked: bool,
}

impl Selected {
    pub(crate) fn uri(&self) -> String {
        // An upload's URI goes into a header: only one a header can carry.
        let uri = self.stack.record.uri.as_str();
        match header::HeaderValue::from_str(uri) {
            Ok(_) if !uri.contains(['<', '>']) => uri.to_owned(),
            _ => self.stack.renderer.profile_uri().to_owned(),
        }
    }
}

/// The profile a request asks for, among those the catalogue serves: EU DCAT-AP, its main
/// profile and the ones it pins. One named in the path or query and not served is a 404; an
/// `Accept-Profile` entry not served is passed over; unasked, the main profile answers.
pub(crate) async fn select_profile(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    explicit: Option<&str>,
    accept: Option<&str>,
) -> Result<Selected> {
    let mut served = vec![
        dataroom_rdf::profile::BASE_PROFILE,
        cat.main_profile.as_str(),
    ];
    served.extend(
        cat.active_profiles
            .iter()
            .map(|pin| pin.profile_id.as_str()),
    );
    served.dedup();
    if served.iter().any(|id| DcatProfile::by_id(id).is_none()) {
        let stored = state.store.list_profiles(ctx).await?;
        state.profiles.load_stored((cat.org_id, cat.id), &stored);
    }
    let pinned = |id: &str| {
        cat.active_profiles
            .iter()
            .find(|pin| pin.profile_id == id && !pin.version.is_empty())
            .map(|pin| pin.version.as_str())
    };
    let named = |token: &str| -> Option<&str> {
        let token = token.trim().trim_matches(['<', '>', '"']);
        let installed = DcatProfile::from_token(token).map(DcatProfile::id);
        served.iter().copied().find(|id| match installed {
            Some(installed) => *id == installed,
            // An upload answers to its id and its URI.
            None => {
                id.eq_ignore_ascii_case(token)
                    || state
                        .profiles
                        .get((cat.org_id, cat.id), id, pinned(id))
                        .is_some_and(|record| record.uri == token)
            }
        })
    };
    let id = match explicit {
        Some(token) => named(token).ok_or_else(|| {
            let all: BTreeSet<&str> = served.iter().copied().collect();
            DataroomError::NotFound(format!(
                "this catalogue has no '{token}' feed; it serves {}",
                all.into_iter().collect::<Vec<_>>().join(", ")
            ))
        })?,
        None => accept
            .and_then(|header| accepted_profiles(header).into_iter().find_map(named))
            .unwrap_or(cat.main_profile.as_str()),
    };
    // A pin that is not installed here is no feed, never an unfiltered or an empty one (C7).
    let stack = state
        .profiles
        .stack((cat.org_id, cat.id), id, pinned(id))
        .map_err(|e| {
            DataroomError::FeedUnavailable(format!(
                "the profile {id} of this catalogue is not installed here: {e}"
            ))
        })?;
    Ok(Selected {
        stack,
        asked: id != cat.main_profile,
    })
}

/// The fields that keep `ds` out of the stack's feed.
pub(crate) fn dataset_gaps(
    state: &DataroomState,
    cat: &Catalogue,
    ds: &Dataset,
    publisher: Option<&Publisher>,
    stack: &Stack,
) -> Arc<[String]> {
    let graph = dataroom_rdf::dataset_to_graph(ds, cat, publisher, stack.renderer);
    state.feed_conformance.missing(stack, &graph, cat, &ds.id)
}

#[derive(Debug, Deserialize, Default)]
pub struct FeedQuery {
    pub page: Option<usize>,
    #[serde(rename = "_profile")]
    pub profile: Option<String>,
}

/// Filters datasets for a public DCAT feed under `profile`:
/// 1. Only `Active` and `Public` datasets whose publisher is not `Withdrawn`.
/// 2. A dataset with SHACL violations in `profile` is hidden from that profile's feed only (C17).
/// 3. A feed that needs the catalogue's publisher and has none, or one beside EU DCAT-AP with
///    candidates of which none conforms, answers a problem naming what is missing.
///
/// A store failure is an error, never an empty feed: a harvester reads an empty catalogue as
/// "every dataset was dropped" and withdraws them all (C9).
async fn public_feed_datasets(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    stack: &Stack,
    publisher_filter: Option<&str>,
) -> Result<(Vec<Dataset>, BTreeMap<String, Publisher>)> {
    let all_ds = state.store.list_datasets(ctx).await?;
    let all_pubs = state.store.list_publishers(ctx).await?;
    let pub_map: BTreeMap<String, Publisher> = all_pubs
        .into_iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
        .collect();

    let mut visible = Vec::new();
    let mut gaps = BTreeSet::new();
    for ds in all_ds {
        if ds.state != RecordState::Active
            || ds.visibility != Visibility::Public
            || cat.hidden_datasets.contains_key(&ds.name)
        {
            continue;
        }
        let pub_rec = pub_map.get(&ds.publisher_id);
        if pub_rec.is_some_and(|p| p.state == RecordState::Withdrawn) {
            continue;
        }
        if let Some(pf) = publisher_filter {
            let matches_pub =
                ds.publisher_id == pf || pub_rec.is_some_and(|p| p.id == pf || p.name == pf);
            if !matches_pub {
                continue;
            }
        }
        let missing = dataset_gaps(state, cat, &ds, pub_rec, stack);
        if !missing.is_empty() {
            // C17: Violations in a profile hide the record from that profile's feed.
            gaps.extend(missing.iter().cloned());
            continue;
        }
        visible.push(ds);
    }

    let (profile, id) = (stack.renderer, &stack.record.id);
    if !profile.is_base() {
        if profile.requires_publisher() && cat.declared_publisher_uri().is_none() {
            return Err(DataroomError::FeedUnavailable(format!(
                "missing publisher: the {id} feed needs the catalogue's publisher IRI; declare \
                 spec.catalog.publisher.uri on the Organization"
            )));
        }
        if visible.is_empty() && !gaps.is_empty() {
            return Err(DataroomError::FeedUnavailable(format!(
                "no dataset declares what {id} requires; missing or not valid: {}",
                gaps.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    }

    Ok((visible, pub_map))
}

/// What the Atom feeds list: the datasets of the EU DCAT-AP feed.
async fn public_eu_datasets(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    publisher_filter: Option<&str>,
) -> Result<(Vec<Dataset>, BTreeMap<String, Publisher>)> {
    let base = dataroom_rdf::profile::BASE_PROFILE;
    let stack = state.profiles.stack((cat.org_id, cat.id), base, None)?;
    public_feed_datasets(state, ctx, cat, &stack, publisher_filter).await
}

/// Moves the Turtle block of the Hydra page `<page_uri>` right under the prefixes, where CKAN's
/// DCAT feed puts it, so a reader or harvester sees the paging before the 100 datasets (G6).
/// The serializer writes one subject per paragraph; a body without that paragraph is returned
/// unchanged.
fn paging_first(body: String, page_uri: &str) -> String {
    let Some(start) = body.find(&format!("\n<{page_uri}> a ")).map(|i| i + 1) else {
        return body;
    };
    let Some(len) = body[start..].find(" .\n").map(|i| i + " .\n".len()) else {
        return body;
    };
    let Some(after_prefixes) = body.find("\n\n").map(|i| i + 2).filter(|&i| i <= start) else {
        return body;
    };
    let block = &body[start..start + len];
    let mut out = String::with_capacity(body.len() + 1);
    out.push_str(&body[..after_prefixes]);
    out.push_str(block);
    out.push('\n');
    out.push_str(&body[after_prefixes..start]);
    let rest = &body[start + len..];
    match rest.strip_prefix('\n') {
        Some(rest) => out.push_str(rest),
        None => {
            out.pop();
            out.push_str(rest);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_dcat_catalog_response(
    cat: &Catalogue,
    all_visible: Vec<Dataset>,
    pub_map: &BTreeMap<String, Publisher>,
    format: RdfFormat,
    selected: &Selected,
    page_param: Option<usize>,
    feed_path: &str,
) -> Response {
    let base = cat.base_url();
    let total = all_visible.len();
    let page = page_param.unwrap_or(1).max(1);
    let last_page = total.div_ceil(DCAT_PAGE_SIZE).max(1);
    let offset = (page - 1).saturating_mul(DCAT_PAGE_SIZE);
    let slice: Vec<Dataset> = all_visible
        .into_iter()
        .skip(offset)
        .take(DCAT_PAGE_SIZE)
        .collect();

    let named = if selected.asked {
        let id: String =
            url::form_urlencoded::byte_serialize(selected.stack.record.id.as_bytes()).collect();
        format!("&_profile={id}")
    } else {
        String::new()
    };
    let page_uri = |n: usize| format!("{base}{feed_path}?page={n}{named}");
    let hydra = (page_param.is_some() || total > DCAT_PAGE_SIZE).then(|| HydraPageInfo {
        collection_uri: match named.strip_prefix('&') {
            Some(query) => format!("{base}{feed_path}?{query}"),
            None => format!("{base}{feed_path}"),
        },
        current_page_uri: page_uri(page),
        total_items: total,
        items_per_page: DCAT_PAGE_SIZE,
        first_page_uri: page_uri(1),
        last_page_uri: page_uri(last_page),
        next_page_uri: (page < last_page).then(|| page_uri(page + 1)),
        prev_page_uri: (page > 1).then(|| page_uri(page - 1)),
    });

    let profile_uri = selected.uri();
    let renderer = selected.stack.renderer;
    let mut body = serialize_catalogue(cat, &slice, pub_map, format, renderer, hydra.as_ref());
    if let (RdfFormat::Turtle, Some(h)) = (format, &hydra) {
        body = paging_first(body, &h.current_page_uri);
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, format.content_type().to_owned()),
            (header::LINK, format!("<{profile_uri}>; rel=\"profile\"")),
            (
                header::HeaderName::from_static("content-profile"),
                profile_uri,
            ),
            // The format and profile follow these request headers.
            (header::VARY, "Accept, Accept-Profile".to_owned()),
        ],
        body,
    )
        .into_response()
}

pub async fn handle_catalog_negotiated(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
    let fmt = negotiate_rdf_format(None, accept).unwrap_or(RdfFormat::Turtle);
    serve_catalog(state, headers, query, fmt, None, None).await
}

pub async fn handle_catalog_ttl(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(state, headers, query, RdfFormat::Turtle, None, None).await
}

pub async fn handle_catalog_jsonld(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(state, headers, query, RdfFormat::JsonLd, None, None).await
}

pub async fn handle_catalog_rdf(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(state, headers, query, RdfFormat::RdfXml, None, None).await
}

pub async fn handle_catalog_nt(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(state, headers, query, RdfFormat::NTriples, None, None).await
}

/// `/profiles/{profile}/catalog.{suffix}`, in every serialisation the profile offers.
pub async fn handle_profile_catalog(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path((profile, file)): Path<(String, String)>,
    Query(query): Query<FeedQuery>,
) -> Response {
    let format = [
        RdfFormat::Turtle,
        RdfFormat::JsonLd,
        RdfFormat::RdfXml,
        RdfFormat::NTriples,
    ]
    .into_iter()
    .find(|format| file.strip_prefix("catalog.") == Some(format.suffix()));
    let Some(format) = format else {
        return problem_response(DataroomError::NotFound(format!(
            "no '{file}' under a profile; its feeds are catalog.ttl, .jsonld, .rdf and .nt"
        )));
    };
    serve_catalog(state, headers, query, format, Some(profile), None).await
}

pub async fn handle_publisher_catalog_ttl(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(publisher): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(
        state,
        headers,
        query,
        RdfFormat::Turtle,
        None,
        Some(publisher),
    )
    .await
}

pub async fn handle_publisher_catalog_jsonld(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(publisher): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(
        state,
        headers,
        query,
        RdfFormat::JsonLd,
        None,
        Some(publisher),
    )
    .await
}

async fn serve_catalog(
    state: DataroomState,
    headers: HeaderMap,
    query: FeedQuery,
    format: RdfFormat,
    explicit_profile: Option<String>,
    publisher_slug: Option<String>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };

    if let Some(ref pub_name) = publisher_slug {
        let all_pubs = match state.store.list_publishers(&ctx).await {
            Ok(p) => p,
            Err(e) => return problem_response(e),
        };
        if let Some(raw_pub) = all_pubs
            .iter()
            .find(|p| &p.name == pub_name || &p.id == pub_name)
        {
            if let Some(ref merged_target) = raw_pub.merged_into {
                let ext = format.suffix();
                return crate::safe_redirect(
                    StatusCode::PERMANENT_REDIRECT,
                    &format!(
                        "{}/publisher/{merged_target}/catalog.{ext}",
                        state.mount_prefix()
                    ),
                );
            }
            if raw_pub.state == RecordState::Withdrawn {
                return problem_response(DataroomError::NotFound(format!(
                    "publisher '{pub_name}' is withdrawn"
                )));
            }
        } else {
            return problem_response(DataroomError::NotFound(format!(
                "publisher '{pub_name}' not found"
            )));
        }
    }

    let accept_profile = headers.get("Accept-Profile").and_then(|v| v.to_str().ok());
    let chosen_qp = explicit_profile.as_deref().or(query.profile.as_deref());
    let selected = match select_profile(&state, &ctx, &cat, chosen_qp, accept_profile).await {
        Ok(selected) => selected,
        Err(e) => return problem_response(e),
    };
    if explicit_profile.is_some() && !selected.stack.renderer.serialisations.contains(&format) {
        return problem_response(DataroomError::NotFound(format!(
            "the {} feed has no catalog.{}",
            selected.stack.record.id,
            format.suffix()
        )));
    }

    let (datasets, pub_map) = match public_feed_datasets(
        &state,
        &ctx,
        &cat,
        &selected.stack,
        publisher_slug.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };

    let ext = format.suffix();
    let feed_path = match &publisher_slug {
        Some(p) => format!("/publisher/{p}/catalog.{ext}"),
        None => format!("/catalog.{ext}"),
    };

    render_dcat_catalog_response(
        &cat, datasets, &pub_map, format, &selected, query.page, &feed_path,
    )
}

/// Renders an RFC 4287 Atom 1.0 feed with proper paging links (`first`, `last`, `next`, `previous`).
pub async fn handle_general_atom(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Response {
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let (mut datasets, _) = match public_eu_datasets(&state, &ctx, &cat, None).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    datasets.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    let title = text_in_lang(&cat.title, Some(&cat.default_language))
        .unwrap_or(&cat.name)
        .to_owned();
    render_atom_xml(&cat, &datasets, &title, "/feeds/dataset.atom", query.page)
}

pub async fn handle_organization_atom(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(file): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    // axum 0.8 takes one parameter per segment, so the `.atom` suffix is checked here.
    let Some(id) = file.strip_suffix(".atom").map(str::to_owned) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let (mut datasets, _) = match public_eu_datasets(&state, &ctx, &cat, Some(&id)).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    datasets.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    let title = format!("{} - Publisher: {}", cat.name, id);
    render_atom_xml(
        &cat,
        &datasets,
        &title,
        &format!("/feeds/organization/{id}.atom"),
        query.page,
    )
}

pub async fn handle_tag_atom(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(file): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    // axum 0.8 takes one parameter per segment, so the `.atom` suffix is checked here.
    let Some(tag) = file.strip_suffix(".atom").map(str::to_owned) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (ctx, cat) = match state.resolve_context(&headers).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let (datasets, _) = match public_eu_datasets(&state, &ctx, &cat, None).await {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };
    let mut filtered: Vec<Dataset> = datasets
        .into_iter()
        .filter(|d| d.tags.contains(&tag))
        .collect();
    filtered.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    let title = format!("{} - Tag: {}", cat.name, tag);
    render_atom_xml(
        &cat,
        &filtered,
        &title,
        &format!("/feeds/tag/{tag}.atom"),
        query.page,
    )
}

fn render_atom_xml(
    cat: &Catalogue,
    datasets: &[Dataset],
    feed_title: &str,
    feed_path: &str,
    page_param: Option<usize>,
) -> Response {
    let base = cat.base_url();
    let total = datasets.len();
    let page = page_param.unwrap_or(1).max(1);
    let last_page = total.div_ceil(ATOM_PAGE_SIZE).max(1);
    let offset = (page - 1).saturating_mul(ATOM_PAGE_SIZE);
    let slice = datasets.iter().skip(offset).take(ATOM_PAGE_SIZE);

    let updated = datasets
        .first()
        .map(|d| d.modified.to_rfc3339())
        .unwrap_or_else(|| cat.modified.to_rfc3339());

    // The feed path carries the tag or publisher id from the request: escaped like every text.
    let feed = escape_xml(&format!("{base}{feed_path}"));
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    xml.push_str(&format!("  <title>{}</title>\n", escape_xml(feed_title)));
    xml.push_str(&format!("  <id>{feed}</id>\n"));
    xml.push_str(&format!("  <updated>{updated}</updated>\n"));
    xml.push_str(&format!(
        "  <link rel=\"self\" href=\"{feed}?page={page}\"/>\n"
    ));
    xml.push_str(&format!("  <link rel=\"first\" href=\"{feed}?page=1\"/>\n"));
    xml.push_str(&format!(
        "  <link rel=\"last\" href=\"{feed}?page={last_page}\"/>\n"
    ));
    if page < last_page {
        xml.push_str(&format!(
            "  <link rel=\"next\" href=\"{feed}?page={}\"/>\n",
            page + 1
        ));
    }
    if page > 1 {
        xml.push_str(&format!(
            "  <link rel=\"previous\" href=\"{feed}?page={}\"/>\n",
            page - 1
        ));
    }

    for ds in slice {
        let ds_title = text_in_lang(&ds.title, Some(&cat.default_language)).unwrap_or(&ds.name);
        let ds_desc = text_in_lang(&ds.description, Some(&cat.default_language)).unwrap_or("");
        let ds_url = escape_xml(&format!("{base}/dataset/{}", ds.name));
        xml.push_str("  <entry>\n");
        xml.push_str(&format!("    <title>{}</title>\n", escape_xml(ds_title)));
        xml.push_str(&format!("    <id>{ds_url}</id>\n"));
        xml.push_str(&format!("    <link href=\"{ds_url}\"/>\n"));
        xml.push_str(&format!(
            "    <updated>{}</updated>\n",
            ds.modified.to_rfc3339()
        ));
        xml.push_str(&format!(
            "    <published>{}</published>\n",
            ds.issued.to_rfc3339()
        ));
        xml.push_str(&format!("    <summary>{}</summary>\n", escape_xml(ds_desc)));
        xml.push_str("  </entry>\n");
    }

    xml.push_str("</feed>\n");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/atom+xml; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'"),
        ],
        xml,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{escape_xml, paging_first, FeedConformance};
    use dataroom_core::Catalogue;
    use dataroom_rdf::{DcatProfile, RdfLimits};

    /// A dataset title or description lands in the Atom feed as XML 1.0 text: a character
    /// XML cannot carry becomes U+FFFD, so one record never makes the whole feed unreadable.
    #[test]
    fn feed_text_carries_only_xml_characters() {
        assert_eq!(
            escape_xml("a\u{1}b\u{FFFE}<c>"),
            "a\u{FFFD}b\u{FFFD}&lt;c&gt;"
        );
    }

    #[test]
    fn a_kept_verdict_follows_the_graph_it_was_given_for() {
        let sg = dataroom_shacl::parse_shapes_graph(
            "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> .
             ex:S a sh:NodeShape ; sh:targetClass ex:R ; sh:property [ sh:path ex:v ; sh:minCount 1 ] .",
            &RdfLimits::uploaded_shapes(),
        )
        .unwrap();
        let graph =
            |ttl: &str| dataroom_rdf::turtle::parse_turtle(ttl, &RdfLimits::default()).unwrap();
        let full = graph("<urn:a> a <http://example.org/R> ; <http://example.org/v> 1 .");
        let bare = graph("<urn:a> a <http://example.org/R> .");
        let cat: Catalogue = serde_json::from_value(serde_json::json!({
            "orgId": uuid::Uuid::now_v7(), "id": uuid::Uuid::now_v7(), "name": "city",
            "host": "data.city.test", "title": {"sk": "Katalóg"}, "description": {},
            "defaultLanguage": "sk", "languages": ["sk"], "mainProfile": "dcat-ap-3.0.1",
            "activeProfiles": [], "modified": "2026-11-01T10:00:00Z"
        }))
        .unwrap();
        let seen = FeedConformance::default();
        let profile = DcatProfile::eu();
        let stack = |key| dataroom_shacl::Stack {
            record: dataroom_shacl::built_in_profiles().remove(0),
            shapes: sg.clone(),
            renderer: profile,
            key,
        };
        let (held, changed) = (stack(1), stack(2));
        // The same dataset id, edited between two pages: each edit gets its own verdict.
        assert!(seen.missing(&held, &full, &cat, "pkg-a").is_empty());
        assert!(!seen.missing(&held, &bare, &cat, "pkg-a").is_empty());
        assert!(seen.missing(&held, &full, &cat, "pkg-a").is_empty());
        assert_eq!(seen.verdicts.lock().unwrap().len(), 2);
        // Other shapes (a profile change) validate again.
        assert!(!seen.missing(&changed, &bare, &cat, "pkg-a").is_empty());
        assert_eq!(seen.verdicts.lock().unwrap().len(), 3);
    }

    #[test]
    fn the_paging_block_moves_under_the_prefixes() {
        let body = "@prefix a: <x#> .\n\n<s1> a:p <o> .\n\n<P?page=1> a a:Page ;\n  a:n 1 .\n\n<s2> a:p <o> .\n";
        assert_eq!(
            paging_first(body.into(), "P?page=1"),
            "@prefix a: <x#> .\n\n<P?page=1> a a:Page ;\n  a:n 1 .\n\n<s1> a:p <o> .\n\n<s2> a:p <o> .\n"
        );
        // Last paragraph, and no paging block at all.
        let last = "@prefix a: <x#> .\n\n<s1> a:p <o> .\n\n<P> a a:Page .\n";
        assert_eq!(
            paging_first(last.into(), "P"),
            "@prefix a: <x#> .\n\n<P> a a:Page .\n\n<s1> a:p <o> .\n"
        );
        assert_eq!(paging_first(body.into(), "Q"), body);
    }
}
