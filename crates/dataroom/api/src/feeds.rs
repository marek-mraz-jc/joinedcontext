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
    Visibility,
};
use dataroom_rdf::{
    negotiate_profile, negotiate_rdf_format, serialize_catalogue, DcatProfileKind, HydraPageInfo,
    RdfFormat,
};
use dataroom_shacl::validate_graph_against_shapes;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Datasets per DCAT feed page (`ours/deployment/README.md`: 100 datasets per page).
pub const DCAT_PAGE_SIZE: usize = 100;
/// Datasets per Atom feed page.
pub const ATOM_PAGE_SIZE: usize = 20;

#[derive(Debug, Deserialize, Default)]
pub struct FeedQuery {
    pub page: Option<usize>,
    #[serde(rename = "_profile")]
    pub profile: Option<String>,
}

/// Filters datasets for a public DCAT feed under `profile`:
/// 1. Only `Active` and `Public` datasets whose publisher is not `Withdrawn`.
/// 2. If `profile` is not the catalogue's main profile, any dataset with SHACL violations in
///    `profile` is hidden from that profile's feed only (C17).
///
/// A store failure is an error, never an empty feed: a harvester reads an empty catalogue as
/// "every dataset was dropped" and withdraws them all (C9).
async fn public_feed_datasets(
    state: &DataroomState,
    ctx: &RequestCtx,
    cat: &Catalogue,
    profile: DcatProfileKind,
    publisher_filter: Option<&str>,
) -> Result<(Vec<Dataset>, BTreeMap<String, Publisher>)> {
    let all_ds = state.store.list_datasets(ctx).await?;
    let all_pubs = state.store.list_publishers(ctx).await?;
    let pub_map: BTreeMap<String, Publisher> = all_pubs
        .into_iter()
        .flat_map(|p| [(p.id.clone(), p.clone()), (p.name.clone(), p)])
        .collect();

    let stacked_shapes = state
        .profiles
        .stacked_shapes(profile.id(), None)
        .ok()
        .map(|(_, sg)| sg);

    let mut visible = Vec::new();
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
        if let Some(ref sg) = stacked_shapes {
            let ds_graph = dataroom_rdf::dataset_to_graph(&ds, cat, pub_rec, profile);
            let rep = validate_graph_against_shapes(
                &ds_graph,
                sg,
                cat.org_id,
                cat.id,
                &ds.id,
                profile.id(),
                "1",
            );
            if !rep.conforms {
                // C17: Violations in a profile hide the record from that profile's feed.
                continue;
            }
        }
        visible.push(ds);
    }

    Ok((visible, pub_map))
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
    profile_kind: DcatProfileKind,
    profile_uri: String,
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

    let hydra = (page_param.is_some() || total > DCAT_PAGE_SIZE).then(|| HydraPageInfo {
        collection_uri: format!("{base}{feed_path}"),
        current_page_uri: format!("{base}{feed_path}?page={page}"),
        total_items: total,
        items_per_page: DCAT_PAGE_SIZE,
        first_page_uri: format!("{base}{feed_path}?page=1"),
        last_page_uri: format!("{base}{feed_path}?page={last_page}"),
        next_page_uri: (page < last_page).then(|| format!("{base}{feed_path}?page={}", page + 1)),
        prev_page_uri: (page > 1).then(|| format!("{base}{feed_path}?page={}", page - 1)),
    });

    let mut body = serialize_catalogue(cat, &slice, pub_map, format, profile_kind, hydra.as_ref());
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

pub async fn handle_profile_catalog_ttl(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(profile): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(
        state,
        headers,
        query,
        RdfFormat::Turtle,
        Some(profile),
        None,
    )
    .await
}

pub async fn handle_profile_catalog_jsonld(
    State(state): State<DataroomState>,
    headers: HeaderMap,
    Path(profile): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Response {
    serve_catalog(
        state,
        headers,
        query,
        RdfFormat::JsonLd,
        Some(profile),
        None,
    )
    .await
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
                let ext = match format {
                    RdfFormat::Turtle => "ttl",
                    RdfFormat::JsonLd => "jsonld",
                    RdfFormat::RdfXml => "rdf",
                    RdfFormat::NTriples => "nt",
                };
                return crate::safe_redirect(
                    StatusCode::PERMANENT_REDIRECT,
                    &format!(
                        "{}/publisher/{merged_target}/catalog.{ext}",
                        state.mount_prefix
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
    let (prof_kind, prof_uri) = negotiate_profile(chosen_qp, accept_profile, &cat.main_profile);

    let (datasets, pub_map) = match public_feed_datasets(
        &state,
        &ctx,
        &cat,
        prof_kind,
        publisher_slug.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return problem_response(e),
    };

    let ext = match format {
        RdfFormat::Turtle => "ttl",
        RdfFormat::JsonLd => "jsonld",
        RdfFormat::RdfXml => "rdf",
        RdfFormat::NTriples => "nt",
    };
    let feed_path = match &publisher_slug {
        Some(p) => format!("/publisher/{p}/catalog.{ext}"),
        None => format!("/catalog.{ext}"),
    };

    render_dcat_catalog_response(
        &cat, datasets, &pub_map, format, prof_kind, prof_uri, query.page, &feed_path,
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
    let (mut datasets, _) =
        match public_feed_datasets(&state, &ctx, &cat, DcatProfileKind::Eu301, None).await {
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
    let (mut datasets, _) =
        match public_feed_datasets(&state, &ctx, &cat, DcatProfileKind::Eu301, Some(&id)).await {
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
    let (datasets, _) =
        match public_feed_datasets(&state, &ctx, &cat, DcatProfileKind::Eu301, None).await {
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

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::paging_first;

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
