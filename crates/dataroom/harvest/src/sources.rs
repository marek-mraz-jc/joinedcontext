#![forbid(unsafe_code)]
//! Source parsers for ArcGIS Hub (`harvest-hub.py`), NKOD SPARQL (`harvest-nkod.py`),
//! DCAT-AP RDF feeds (with Hydra pagination), and CKAN Action API (`package_search`).

use crate::guard::validate_outbound_url;
use dataroom_core::{
    AgentDetails, CatalogueId, DataroomError, Dataset, Distribution, HarvestSkippedItem,
    HarvestSource, OrgId, RecordState, Result, TermsOfUse, Visibility,
};
use dataroom_rdf::{ns, parse_datasets_from_rdf, parse_rdf_graph, RdfFormat, RdfLimits};
use jc_time::TimestampTz;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// Strips HTML markup into plain paragraphs (`plain_text` in `ours/deployment/harvest-hub.py`,
/// an `HTMLParser` with `convert_charrefs`): block tags break lines, character references are
/// decoded, and a `<` that opens no tag stays text.
pub fn plain_text_from_html(markup: &str) -> String {
    let mut out = String::new();
    let mut rest = markup;
    while let Some(i) = rest.find('<') {
        out.push_str(&decode_char_refs(&rest[..i]));
        let after = &rest[i + 1..];
        let opens_tag = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !opens_tag {
            out.push('<');
            rest = after;
            continue;
        }
        let (tag, tail) = if let Some(comment) = after.strip_prefix("!--") {
            let end = comment.find("-->").map_or(comment.len(), |e| e + 3);
            ("", &comment[end..])
        } else {
            let end = after.find('>').map_or(after.len(), |e| e + 1);
            (&after[..end], &after[end..])
        };
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        ) {
            out.push('\n');
        }
        rest = tail;
    }
    out.push_str(&decode_char_refs(rest));
    let lines: Vec<String> = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n\n").chars().take(10_000).collect()
}

/// Decodes numeric and the common named character references.
/// ponytail: six names, not the whole HTML5 table; an unknown name stays as written.
fn decode_char_refs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Converts text into a lowercase ASCII slug (`slug` in `harvest-hub.py` and `harvest-nkod.py`).
pub fn ascii_slug(text: &str) -> String {
    let ascii: String = text
        .nfkd()
        .filter(|c| !is_combining_mark(*c) && c.is_ascii())
        .collect();
    let mut slug = String::new();
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

/// A harvested dataset name: `prefix` + `slug` cut to the 100 characters `validate_slug` allows,
/// with no trailing separator. `fallback` (the item's UUID) stands in for an empty slug (a title
/// with no Latin letter), so such items do not all share the bare prefix, and for a name under
/// 2 characters.
fn harvested_name(prefix: &str, slug: &str, fallback: &str) -> String {
    let stem = if slug.is_empty() { fallback } else { slug };
    let name: String = format!("{prefix}{stem}").chars().take(100).collect();
    let name = name.trim_end_matches(['-', '_']);
    if name.len() < 2 {
        fallback.to_owned()
    } else {
        name.to_owned()
    }
}

/// Computes a deterministic SHA-256 content digest for a harvested dataset (`harvest_hash`).
pub fn compute_harvest_hash(ds: &Dataset) -> String {
    let mut hasher = Sha256::new();
    let canonical = json!({
        "name": ds.name,
        "title": ds.title,
        "description": ds.description,
        "publisher_id": ds.publisher_id,
        "licence_id": ds.licence_id,
        "licence_url": ds.licence_url,
        "tags": ds.tags,
        "themes": ds.themes,
        "spatial": ds.spatial,
        "landing_page": ds.landing_page,
        "distributions": ds.distributions.iter().map(|d| json!({
            "title": d.title,
            "access_url": d.access_url,
            "download_url": d.download_url,
            "format": d.format,
            "media_type": d.media_type,
        })).collect::<Vec<_>>(),
    });
    hasher.update(canonical.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Selects and transforms ArcGIS Hub items (`ours/deployment/harvest-hub.py` `select` & `dataset`).
pub fn parse_arcgis_hub_items(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    cat_allowed_hosts: &[String],
    features: &[Value],
    default_lang: &str,
) -> (Vec<(String, Dataset)>, Vec<HarvestSkippedItem>) {
    let open_licences: BTreeMap<&str, (&str, &str)> = BTreeMap::from([
        (
            "CC-BY-4.0",
            ("cc-by", "https://creativecommons.org/licenses/by/4.0/"),
        ),
        (
            "CC-BY-SA-4.0",
            (
                "cc-by-sa",
                "https://creativecommons.org/licenses/by-sa/4.0/",
            ),
        ),
        (
            "CC0-1.0",
            (
                "cc-zero",
                "https://creativecommons.org/publicdomain/zero/1.0/",
            ),
        ),
    ]);

    let mut kept_raw: Vec<(&Value, String, (&str, &str))> = Vec::new();
    let mut skipped: Vec<HarvestSkippedItem> = Vec::new();

    for feature in features {
        let item = feature.get("properties").unwrap_or(feature);
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if item_id.is_empty() {
            continue;
        }
        let raw_title = source
            .title_overrides
            .get(&item_id)
            .cloned()
            .or_else(|| item.get("title").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| item_id.clone());
        let mut title = raw_title;
        for (wrong, right) in &source.spelling_fixes {
            title = title.replace(wrong, right);
        }
        title = title.split_whitespace().collect::<Vec<_>>().join(" ");

        if let Some(reason) = source.skip_items.get(&item_id) {
            skipped.push(HarvestSkippedItem {
                item_id,
                title,
                reason: reason.clone(),
            });
            continue;
        }

        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(item_type, "Feature Service" | "Shapefile") {
            skipped.push(HarvestSkippedItem {
                item_id,
                title,
                reason: format!("type {item_type} is not a dataset"),
            });
            continue;
        }

        let lic = item.get("license").and_then(Value::as_str).unwrap_or("");
        let Some(&licence) = open_licences.get(lic) else {
            skipped.push(HarvestSkippedItem {
                item_id,
                title,
                reason: format!("no open licence ({lic})"),
            });
            continue;
        };

        if item_type == "Feature Service" {
            let service_url = item.get("url").and_then(Value::as_str).unwrap_or("");
            if validate_outbound_url(service_url, &source.allowed_hosts, cat_allowed_hosts, false)
                .is_err()
            {
                skipped.push(HarvestSkippedItem {
                    item_id,
                    title,
                    reason: "its service is not on an allowed host".into(),
                });
                continue;
            }
        }

        kept_raw.push((item, title, licence));
    }

    // Deduplicate by title: newest `modified` wins (`harvest-hub.py` lines 106-115).
    let mut newest_by_title: BTreeMap<String, (&Value, i64)> = BTreeMap::new();
    for (item, title, _) in &kept_raw {
        let mod_ms = item.get("modified").and_then(Value::as_i64).unwrap_or(0);
        match newest_by_title.get(title) {
            Some((_, prev_ms)) if *prev_ms >= mod_ms => {}
            _ => {
                newest_by_title.insert(title.clone(), (*item, mod_ms));
            }
        }
    }

    let mut datasets = Vec::new();
    for (item, title, (licence_id, licence_url)) in kept_raw {
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let winner = newest_by_title.get(&title).map(|(w, _)| *w);
        if winner != Some(item) {
            let winner_id = winner
                .and_then(|w| w.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            skipped.push(HarvestSkippedItem {
                item_id,
                title,
                reason: format!("duplicate of {winner_id}"),
            });
            continue;
        }

        let hub_base = source.url.trim_end_matches('/');
        let source_url = format!("{hub_base}/datasets/{item_id}");
        let fixed_uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, source_url.as_bytes()).to_string();
        let slug_part: String = ascii_slug(&title).chars().take(70).collect();
        let short_id: String = item_id.chars().take(6).collect();
        let name = format!(
            "{}-{}-{}",
            source.publisher_id,
            slug_part.trim_matches('-'),
            short_id.to_ascii_lowercase()
        );

        // `plain_text(description) or plain_text(snippet)`: a description of empty markup
        // falls through to the snippet.
        let text_of =
            |key: &str| plain_text_from_html(item.get(key).and_then(Value::as_str).unwrap_or(""));
        let mut notes = text_of("description");
        if notes.is_empty() {
            notes = text_of("snippet");
        }
        if notes.is_empty() {
            notes = title.clone();
        }

        // Tags and the last segment of each category, kept when they match `[\w .-]{2,100}`.
        let strs = |key: &str| {
            item.get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        };
        let tags: BTreeSet<String> = strs("tags")
            .map(str::trim)
            .chain(strs("categories").map(|c| c.rsplit('/').next().unwrap_or(c).trim()))
            .filter(|t| {
                (2..=100).contains(&t.chars().count())
                    && t.chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'))
            })
            .map(str::to_owned)
            .collect();

        let service_url = item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or(&source_url)
            .to_owned();
        let dist = Distribution {
            id: format!("res-{name}-0"),
            dataset_id: fixed_uuid.clone(),
            uri: None,
            title: BTreeMap::from([(default_lang.to_owned(), format!("{title} (CSV)"))]),
            description: BTreeMap::new(),
            access_url: service_url,
            download_url: Some(format!(
                "{hub_base}/api/download/v1/items/{item_id}/csv?layers=0"
            )),
            format: Some("CSV".into()),
            media_type: Some("text/csv".into()),
            compress_format: None,
            package_format: None,
            byte_size: None,
            hash: None,
            hash_algorithm: None,
            licence: Some(licence_url.to_owned()),
            rights: BTreeMap::new(),
            status: None,
            availability: None,
            resource_type: None,
            conforms_to: Vec::new(),
            documentation: Vec::new(),
            languages: Vec::new(),
            applicable_legislation: Vec::new(),
            temporal_resolution: None,
            spatial_resolution_in_meters: None,
            terms_of_use: None,
            access_services: Vec::new(),
            issued: None,
            modified: None,
            extras: BTreeMap::new(),
        };

        let created_dt = item
            .get("created")
            .and_then(Value::as_i64)
            .and_then(TimestampTz::from_millis)
            .unwrap_or_else(TimestampTz::now);
        let modified_dt = item
            .get("modified")
            .and_then(Value::as_i64)
            .and_then(TimestampTz::from_millis)
            .unwrap_or_else(TimestampTz::now);

        let mut ds = Dataset {
            org_id,
            catalogue_id,
            id: fixed_uuid,
            name,
            uri: Some(source_url.clone()),
            dataset_type: "dataset".into(),
            title: BTreeMap::from([(default_lang.to_owned(), title)]),
            description: BTreeMap::from([(default_lang.to_owned(), notes)]),
            publisher_id: source.publisher_id.clone(),
            publisher: source
                .target_publisher_iri
                .as_ref()
                .map(|uri| AgentDetails {
                    uri: Some(uri.clone()),
                    name: BTreeMap::from([(default_lang.to_owned(), source.publisher_id.clone())]),
                    ..AgentDetails::default()
                }),
            creator: None,
            contact_point: None,
            visibility: Visibility::Public,
            state: RecordState::Active,
            licence_id: Some(licence_id.to_owned()),
            licence_url: Some(licence_url.to_owned()),
            themes: vec!["http://publications.europa.eu/resource/authority/data-theme/REGI".into()],
            eurovoc_themes: Vec::new(),
            keywords: BTreeMap::from([(default_lang.to_owned(), tags.iter().cloned().collect())]),
            tags: tags.into_iter().collect(),
            spatial: source.spatial_uri.iter().cloned().collect(),
            spatial_resolution_in_meters: None,
            temporal: None,
            temporal_resolution: None,
            frequency: Some(
                "http://publications.europa.eu/resource/authority/frequency/IRREG".into(),
            ),
            landing_page: Some(source_url.clone()),
            documentation: Vec::new(),
            conforms_to: Vec::new(),
            applicable_legislation: Vec::new(),
            in_series: None,
            relations: Vec::new(),
            sources: Vec::new(),
            languages: vec![default_lang.to_owned()],
            version: None,
            version_notes: BTreeMap::new(),
            access_rights: None,
            provenance: BTreeMap::new(),
            dcat_type: None,
            identifier: Some(item_id.clone()),
            alternate_identifiers: Vec::new(),
            endpoint_url: None,
            generated_by: None,
            harvest_source_id: Some(source.name.clone()),
            harvest_source_item_id: Some(item_id.clone()),
            harvest_hash: None,
            custom_shacl_shapes: None,
            distributions: vec![dist],
            extras: BTreeMap::from([
                ("harvest_source".into(), source.name.clone()),
                ("source_id".into(), item_id.clone()),
                ("source_url".into(), source_url),
                ("license_url".into(), licence_url.to_owned()),
            ]),
            restricted_grants: Vec::new(),
            issued: created_dt,
            modified: modified_dt,
        };
        let hash = compute_harvest_hash(&ds);
        ds.harvest_hash = Some(hash.clone());
        ds.extras.insert("harvest_hash".into(), hash);
        datasets.push((item_id, ds));
    }

    (datasets, skipped)
}

/// `url` with its host (and port) moved when `rewrites` names exactly that host
/// (`corrected_url` in `harvest-nkod.py`); the path and query stay as they are.
fn rewrite_host(url: &str, rewrites: &BTreeMap<String, String>) -> String {
    let url = url.trim();
    let Some(start) = url.find("://").map(|i| i + 3) else {
        return url.to_owned();
    };
    let end = url[start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| start + i);
    match rewrites.get(&url[start..end]) {
        Some(host) => format!("{}{host}{}", &url[..start], &url[end..]),
        None => url.to_owned(),
    }
}

/// Parses NKOD SPARQL JSON bindings (`ours/deployment/harvest-nkod.py` `datasets` & `licence`).
pub fn parse_nkod_sparql_bindings(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    cat_allowed_hosts: &[String],
    sparql_json: &Value,
    default_lang: &str,
) -> (Vec<(String, Dataset)>, Vec<HarvestSkippedItem>) {
    let bindings = sparql_json
        .pointer("/results/bindings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    struct RawDist {
        title: String,
        url: String,
        format: String,
        licences: BTreeSet<String>,
    }
    struct RawDs {
        iri: String,
        title: String,
        description: String,
        landing: String,
        keywords: BTreeSet<String>,
        distributions: BTreeMap<String, RawDist>,
    }

    let mut folded: BTreeMap<String, RawDs> = BTreeMap::new();
    for row in bindings {
        let cell = |k: &str| -> Option<String> {
            row.get(k)
                .and_then(|c| c.get("value"))
                .and_then(Value::as_str)
                .map(|s| s.trim().to_owned())
        };
        let Some(ds_iri) = cell("ds") else {
            continue;
        };
        let Some(title) = cell("title") else {
            continue;
        };
        let entry = folded.entry(ds_iri.clone()).or_insert_with(|| RawDs {
            iri: ds_iri,
            title: title.clone(),
            description: cell("description").unwrap_or_default(),
            landing: cell("landing").unwrap_or_default(),
            keywords: BTreeSet::new(),
            distributions: BTreeMap::new(),
        });
        if let Some(kw) = cell("keyword") {
            if !kw.is_empty() {
                entry.keywords.insert(kw);
            }
        }
        if let (Some(dist_iri), Some(url)) = (cell("dist"), cell("url")) {
            let d = entry
                .distributions
                .entry(dist_iri)
                .or_insert_with(|| RawDist {
                    title: cell("distTitle").unwrap_or_else(|| entry.title.clone()),
                    url,
                    format: cell("format")
                        .map(|f| f.rsplit('/').next().unwrap_or(&f).to_ascii_uppercase())
                        .unwrap_or_else(|| "JSON".into()),
                    licences: BTreeSet::new(),
                });
            for k in ["work", "database", "special"] {
                if let Some(v) = cell(k) {
                    d.licences.insert(v);
                }
            }
        }
    }

    let eu_prefix = "http://publications.europa.eu/resource/authority/licence/";
    let strictness = ["cc-zero", "cc-by", "cc-by-sa"];
    let map_code = |code: &str| -> Option<&'static str> {
        let stripped = code.strip_prefix(eu_prefix).unwrap_or(code);
        match stripped {
            "CC0" | "https://creativecommons.org/publicdomain/zero/1.0/" => Some("cc-zero"),
            "CC_BY_4_0" | "https://creativecommons.org/licenses/by/4.0/" => Some("cc-by"),
            "CC_BYSA_4_0" | "https://creativecommons.org/licenses/by-sa/4.0/" => Some("cc-by-sa"),
            _ => None,
        }
    };

    let mut kept = Vec::new();
    let mut skipped = Vec::new();

    for (_, raw) in folded {
        if let Some(reason) = source
            .skip_items
            .get(&raw.title)
            .or_else(|| source.skip_items.get(&raw.iri))
        {
            skipped.push(HarvestSkippedItem {
                item_id: raw.iri,
                title: raw.title,
                reason: reason.clone(),
            });
            continue;
        }
        if raw.distributions.is_empty() {
            skipped.push(HarvestSkippedItem {
                item_id: raw.iri,
                title: raw.title,
                reason: "no distribution with a download URL".into(),
            });
            continue;
        }

        let mut lic_ids = Vec::new();
        let mut all_open = true;
        for d in raw.distributions.values() {
            if d.licences.is_empty() {
                all_open = false;
                break;
            }
            for l in &d.licences {
                if let Some(mapped) = map_code(l) {
                    lic_ids.push(mapped);
                } else {
                    all_open = false;
                }
            }
        }
        if !all_open || lic_ids.is_empty() {
            skipped.push(HarvestSkippedItem {
                item_id: raw.iri,
                title: raw.title,
                reason: "no open licence in legislation:termsOfUse".into(),
            });
            continue;
        }
        let chosen_lic = lic_ids
            .into_iter()
            .max_by_key(|id| strictness.iter().position(|s| s == id).unwrap_or(0))
            .unwrap_or("cc-by");

        let mut dists = Vec::new();
        for (idx, d) in raw.distributions.values().enumerate() {
            let Ok(url) = validate_outbound_url(
                &rewrite_host(&d.url, &source.rewrite_hosts),
                &source.allowed_hosts,
                cat_allowed_hosts,
                false,
            ) else {
                continue;
            };
            let url_str = url.to_string();
            dists.push(Distribution {
                id: format!("res-{}-{idx}", ascii_slug(&raw.title)),
                dataset_id: String::new(),
                uri: None,
                title: BTreeMap::from([(default_lang.to_owned(), d.title.clone())]),
                description: BTreeMap::new(),
                access_url: url_str.clone(),
                download_url: Some(url_str),
                format: Some(d.format.clone()),
                media_type: None,
                compress_format: None,
                package_format: None,
                byte_size: None,
                hash: None,
                hash_algorithm: None,
                licence: None,
                rights: BTreeMap::new(),
                status: None,
                availability: None,
                resource_type: None,
                conforms_to: Vec::new(),
                documentation: Vec::new(),
                languages: Vec::new(),
                applicable_legislation: Vec::new(),
                temporal_resolution: None,
                spatial_resolution_in_meters: None,
                terms_of_use: Some(TermsOfUse::default()),
                access_services: Vec::new(),
                issued: None,
                modified: None,
                extras: BTreeMap::new(),
            });
        }
        if dists.is_empty() {
            skipped.push(HarvestSkippedItem {
                item_id: raw.iri,
                title: raw.title,
                reason: "all distribution URLs refused by host allowlist".into(),
            });
            continue;
        }

        let fixed_uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, raw.iri.as_bytes()).to_string();
        let name = harvested_name(&source.name_prefix, &ascii_slug(&raw.title), &fixed_uuid);

        let mut ds = Dataset {
            org_id,
            catalogue_id,
            id: fixed_uuid,
            name,
            uri: Some(raw.iri.clone()),
            dataset_type: "dataset".into(),
            title: BTreeMap::from([(default_lang.to_owned(), raw.title)]),
            description: BTreeMap::from([(
                default_lang.to_owned(),
                if raw.description.is_empty() {
                    format!("Zdroj: {}", raw.iri)
                } else {
                    raw.description
                },
            )]),
            publisher_id: source.publisher_id.clone(),
            publisher: source
                .target_publisher_iri
                .as_ref()
                .map(|uri| AgentDetails {
                    uri: Some(uri.clone()),
                    name: BTreeMap::from([(default_lang.to_owned(), source.publisher_id.clone())]),
                    ..AgentDetails::default()
                }),
            creator: None,
            contact_point: None,
            visibility: Visibility::Public,
            state: RecordState::Active,
            licence_id: Some(chosen_lic.to_owned()),
            licence_url: None,
            themes: vec!["http://publications.europa.eu/resource/authority/data-theme/GOVE".into()],
            eurovoc_themes: Vec::new(),
            keywords: BTreeMap::from([(
                default_lang.to_owned(),
                raw.keywords.iter().cloned().collect(),
            )]),
            tags: raw.keywords.into_iter().collect(),
            spatial: source.spatial_uri.iter().cloned().collect(),
            spatial_resolution_in_meters: None,
            temporal: None,
            temporal_resolution: None,
            frequency: Some(
                "http://publications.europa.eu/resource/authority/frequency/IRREG".into(),
            ),
            landing_page: (!raw.landing.is_empty()).then_some(raw.landing),
            documentation: Vec::new(),
            conforms_to: Vec::new(),
            applicable_legislation: Vec::new(),
            in_series: None,
            relations: Vec::new(),
            sources: Vec::new(),
            languages: vec![default_lang.to_owned()],
            version: None,
            version_notes: BTreeMap::new(),
            access_rights: None,
            provenance: BTreeMap::new(),
            dcat_type: None,
            identifier: Some(raw.iri.clone()),
            alternate_identifiers: Vec::new(),
            endpoint_url: None,
            generated_by: None,
            harvest_source_id: Some(source.name.clone()),
            harvest_source_item_id: Some(raw.iri.clone()),
            harvest_hash: None,
            custom_shacl_shapes: None,
            distributions: dists,
            extras: BTreeMap::from([
                ("harvest_source".into(), source.name.clone()),
                ("source_id".into(), raw.iri.clone()),
                ("source_uri".into(), raw.iri.clone()),
            ]),
            restricted_grants: Vec::new(),
            issued: TimestampTz::now(),
            modified: TimestampTz::now(),
        };
        let hash = compute_harvest_hash(&ds);
        ds.harvest_hash = Some(hash.clone());
        ds.extras.insert("harvest_hash".into(), hash);
        kept.push((raw.iri, ds));
    }

    (kept, skipped)
}

/// Records of one feed page by source item id, and the next page URL.
type ParsedPage = (Vec<(String, Dataset)>, Option<String>);

/// Parses a DCAT-AP RDF document (Turtle, JSON-LD, RDF/XML) and extracts datasets plus any
/// `hydra:next` / `hydra:nextPage` link.
pub fn parse_dcat_page(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    body: &str,
    content_type: Option<&str>,
    default_lang: &str,
) -> Result<ParsedPage> {
    let format = content_type
        .and_then(|ct| dataroom_rdf::negotiate_rdf_format(None, Some(ct)))
        .or_else(|| {
            if body.trim_start().starts_with('{') || body.trim_start().starts_with('[') {
                Some(RdfFormat::JsonLd)
            } else if body.trim_start().starts_with('<') {
                Some(RdfFormat::RdfXml)
            } else {
                Some(RdfFormat::Turtle)
            }
        })
        .unwrap_or(RdfFormat::Turtle);

    let g = parse_rdf_graph(body, format, &RdfLimits::default())?;
    let next_page = g
        .triples
        .iter()
        .find(|t| {
            t.predicate == format!("{}next", ns::HYDRA)
                || t.predicate == format!("{}nextPage", ns::HYDRA)
        })
        .map(|t| t.object.string_value().to_owned());

    let parsed = parse_datasets_from_rdf(
        body,
        format,
        org_id,
        catalogue_id,
        &source.publisher_id,
        default_lang,
        &RdfLimits::default(),
    )?;

    let mut out = Vec::new();
    for mut ds in parsed {
        let source_item_id = ds.uri.clone().unwrap_or_else(|| ds.name.clone());
        ds.id = Uuid::new_v5(&Uuid::NAMESPACE_URL, source_item_id.as_bytes()).to_string();
        if !source.name_prefix.is_empty() && !ds.name.starts_with(&source.name_prefix) {
            ds.name = harvested_name(&source.name_prefix, &ds.name, &ds.id);
        }
        ds.harvest_source_id = Some(source.name.clone());
        ds.harvest_source_item_id = Some(source_item_id.clone());
        ds.extras
            .insert("harvest_source".into(), source.name.clone());
        ds.extras.insert("source_id".into(), source_item_id.clone());
        let hash = compute_harvest_hash(&ds);
        ds.harvest_hash = Some(hash.clone());
        ds.extras.insert("harvest_hash".into(), hash);
        out.push((source_item_id, ds));
    }

    Ok((out, next_page))
}

/// Parses a CKAN `package_search` response JSON into harvested datasets.
pub fn parse_ckan_search_page(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    body: &str,
    default_lang: &str,
) -> Result<(Vec<(String, Dataset)>, usize)> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| DataroomError::OutboundRefused(format!("invalid CKAN JSON: {e}")))?;
    if val.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(DataroomError::OutboundRefused(
            "CKAN package_search returned success=false".into(),
        ));
    }
    let count = val
        .pointer("/result/count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let results = val
        .pointer("/result/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let ckan_base = source
        .url
        .split("/api/3/")
        .next()
        .unwrap_or(&source.url)
        .trim_end_matches('/');
    let mut out = Vec::new();
    for pkg in results {
        let mut ds = dataroom_core::ckan_compat::dataset_from_ckan(
            org_id,
            catalogue_id,
            &pkg,
            None,
            default_lang,
        );
        let item_id = pkg
            .get("id")
            .or_else(|| pkg.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(&ds.name)
            .to_owned();
        // The remote CKAN picks its own ids: a local id taken from it could name a dataset this
        // catalogue already holds, so the record id is derived from the source's own address.
        ds.id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("{ckan_base}/dataset/{item_id}").as_bytes(),
        )
        .to_string();
        ds.publisher_id = source.publisher_id.clone();
        // Fields with a meaning on this platform are this catalogue's, never the remote's: a
        // harvested record is a published dataset with no local Endpoint, provenance or grants.
        ds.dataset_type = "dataset".into();
        ds.state = RecordState::Active;
        ds.endpoint_url = None;
        ds.generated_by = None;
        ds.restricted_grants.clear();
        for key in ["endpoint", "generated_by"] {
            ds.extras.remove(key);
        }
        ds.harvest_source_id = Some(source.name.clone());
        ds.harvest_source_item_id = Some(item_id.clone());
        ds.extras
            .insert("harvest_source".into(), source.name.clone());
        ds.extras.insert("source_id".into(), item_id.clone());
        let hash = compute_harvest_hash(&ds);
        ds.harvest_hash = Some(hash.clone());
        ds.extras.insert("harvest_hash".into(), hash);
        out.push((item_id, ds));
    }

    Ok((out, count))
}

/// The open licence a licence text names, as the catalogue's id and IRI: an SPDX id, a CKAN id
/// or a Creative Commons address. `None` for any other text, which leaves the record without a
/// licence rather than claiming one.
pub fn open_licence(text: &str) -> Option<(&'static str, &'static str)> {
    const BY: (&str, &str) = ("cc-by", "https://creativecommons.org/licenses/by/4.0/");
    const BY_SA: (&str, &str) = (
        "cc-by-sa",
        "https://creativecommons.org/licenses/by-sa/4.0/",
    );
    const ZERO: (&str, &str) = (
        "cc-zero",
        "https://creativecommons.org/publicdomain/zero/1.0/",
    );
    let t = text.trim().to_ascii_lowercase();
    let t = t.trim_end_matches('/');
    let addr = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    match addr {
        "cc-by-4.0" | "cc-by" | "creativecommons.org/licenses/by/4.0" => Some(BY),
        "cc-by-sa-4.0" | "cc-by-sa" | "creativecommons.org/licenses/by-sa/4.0" => Some(BY_SA),
        "cc0-1.0" | "cc0" | "cc-zero" | "creativecommons.org/publicdomain/zero/1.0" => Some(ZERO),
        _ => None,
    }
}

/// The GeoJSON polygon of a WGS 84 box `[west, south, east, north]`, the `spatial` extra CKAN's
/// spatial extension reads; `None` for a box outside the globe or with a side of no length.
pub fn bbox_polygon([w, s, e, n]: [f64; 4]) -> Option<String> {
    let ok = [w, s, e, n].iter().all(|v| v.is_finite())
        && (-180.0..=180.0).contains(&w)
        && (-180.0..=180.0).contains(&e)
        && (-90.0..=90.0).contains(&s)
        && (-90.0..=90.0).contains(&n)
        && w <= e
        && s <= n;
    ok.then(|| {
        json!({"type": "Polygon",
               "coordinates": [[[w, s], [e, s], [e, n], [w, n], [w, s]]]})
        .to_string()
    })
}

/// An ArcGIS extent (`fullExtent`) in WGS 84: kept as given in geographic degrees (EPSG:4326,
/// 4258, 4269: the datums differ by under 2 m), taken back from
/// Web Mercator (102100, 3857, 102113, 900913); any other reference gives none.
pub fn arcgis_extent_wgs84(extent: &Value) -> Option<[f64; 4]> {
    let num = |k: &str| extent.get(k).and_then(Value::as_f64);
    let (w, s, e, n) = (num("xmin")?, num("ymin")?, num("xmax")?, num("ymax")?);
    let sr = extent.get("spatialReference")?;
    let wkid = sr
        .get("latestWkid")
        .or_else(|| sr.get("wkid"))
        .and_then(Value::as_u64)?;
    match wkid {
        4326 | 4258 | 4269 => Some([w, s, e, n]),
        3857 | 102100 | 102113 | 900913 => {
            const R: f64 = 6_378_137.0;
            let lon = |x: f64| (x / R).to_degrees();
            let lat =
                |y: f64| (2.0 * (y / R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
            Some([lon(w), lat(s), lon(e), lat(n)])
        }
        _ => None,
    }
}

/// The box around every position of a GeoJSON geometry; `None` when it holds none.
fn geometry_bbox(geometry: &Value) -> Option<[f64; 4]> {
    fn walk(v: &Value, b: &mut Option<[f64; 4]>) {
        let Some(arr) = v.as_array() else { return };
        match (
            arr.first().and_then(Value::as_f64),
            arr.get(1).and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => {
                let [w, s, e, n] = b.get_or_insert([x, y, x, y]);
                *w = w.min(x);
                *s = s.min(y);
                *e = e.max(x);
                *n = n.max(y);
            }
            _ => arr.iter().for_each(|c| walk(c, b)),
        }
    }
    let mut b = None;
    walk(geometry.get("coordinates")?, &mut b);
    b
}

/// The title a record keeps after the source's overrides and spelling fixes, or the reason the
/// source skips it.
fn source_title(source: &HarvestSource, item_id: &str, title: &str) -> (String, Option<String>) {
    let mut title = source
        .title_overrides
        .get(item_id)
        .cloned()
        .unwrap_or_else(|| title.to_owned());
    for (wrong, right) in &source.spelling_fixes {
        title = title.replace(wrong, right);
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (title, source.skip_items.get(item_id).cloned())
}

/// Keywords kept as tags when they match `[\w .-]{2,100}`, as the Hub's are.
fn keyword_tags<'a>(words: impl Iterator<Item = &'a str>) -> Vec<String> {
    words
        .map(str::trim)
        .filter(|t| {
            (2..=100).contains(&t.chars().count())
                && t.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'))
        })
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Title, access URL, download URL, format and media type of a harvested record's distribution.
type Link = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A distribution of a harvested record: a link and what it serves.
fn link_distribution(ds_id: &str, position: usize, link: Link, lang: &str) -> Distribution {
    let (title, access_url, download_url, format, media_type) = link;
    Distribution {
        id: format!("res-{ds_id}-{position}"),
        dataset_id: ds_id.to_owned(),
        uri: None,
        title: BTreeMap::from([(lang.to_owned(), title)]),
        description: BTreeMap::new(),
        access_url,
        download_url,
        format,
        media_type,
        compress_format: None,
        package_format: None,
        byte_size: None,
        hash: None,
        hash_algorithm: None,
        licence: None,
        rights: BTreeMap::new(),
        status: None,
        availability: None,
        resource_type: None,
        conforms_to: Vec::new(),
        documentation: Vec::new(),
        languages: Vec::new(),
        applicable_legislation: Vec::new(),
        temporal_resolution: None,
        spatial_resolution_in_meters: None,
        terms_of_use: None,
        access_services: Vec::new(),
        issued: None,
        modified: None,
        extras: BTreeMap::new(),
    }
}

/// What a harvested record of the ArcGIS REST and OGC API Records sources says.
struct Record<'a> {
    item_id: &'a str,
    /// The record's own address: its IRI and the source of its id.
    address: String,
    landing_page: String,
    title: String,
    notes: String,
    tags: Vec<String>,
    licence: Option<(&'static str, &'static str)>,
    /// The licence or copyright text when it names no licence the catalogue knows.
    licence_text: Option<String>,
    bbox: Option<[f64; 4]>,
    issued: Option<TimestampTz>,
    modified: Option<TimestampTz>,
    links: Vec<Link>,
}

/// The dataset of a harvested [`Record`], with its harvest hash.
fn record_dataset(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    r: Record<'_>,
    lang: &str,
) -> Dataset {
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, r.address.as_bytes()).to_string();
    let slug: String = ascii_slug(&r.title).chars().take(60).collect();
    let stem = format!("{}-{}", slug.trim_matches('-'), &id[..8]);
    let name = harvested_name(&source.name_prefix, stem.trim_start_matches('-'), &id);
    let distributions = r
        .links
        .into_iter()
        .enumerate()
        .map(|(i, link)| {
            let mut d = link_distribution(&id, i, link, lang);
            d.licence = r.licence.map(|(_, url)| url.to_owned());
            d
        })
        .collect();
    let mut extras = BTreeMap::from([
        ("harvest_source".to_owned(), source.name.clone()),
        ("source_id".to_owned(), r.item_id.to_owned()),
        ("source_url".to_owned(), r.address.clone()),
    ]);
    if let Some(polygon) = r.bbox.and_then(bbox_polygon) {
        extras.insert("spatial".into(), polygon);
    }
    if let Some(text) = r.licence_text.filter(|_| r.licence.is_none()) {
        extras.insert("licence_text".into(), text.chars().take(500).collect());
    }
    let now = TimestampTz::now();
    let mut ds = Dataset {
        org_id,
        catalogue_id,
        id,
        name,
        uri: Some(r.address),
        dataset_type: "dataset".into(),
        title: BTreeMap::from([(lang.to_owned(), r.title)]),
        description: BTreeMap::from([(lang.to_owned(), r.notes)]),
        publisher_id: source.publisher_id.clone(),
        publisher: source
            .target_publisher_iri
            .as_ref()
            .map(|uri| AgentDetails {
                uri: Some(uri.clone()),
                name: BTreeMap::from([(lang.to_owned(), source.publisher_id.clone())]),
                ..AgentDetails::default()
            }),
        creator: None,
        contact_point: None,
        visibility: Visibility::Public,
        state: RecordState::Active,
        licence_id: r.licence.map(|(id, _)| id.to_owned()),
        licence_url: r.licence.map(|(_, url)| url.to_owned()),
        themes: Vec::new(),
        eurovoc_themes: Vec::new(),
        keywords: BTreeMap::from([(lang.to_owned(), r.tags.clone())]),
        tags: r.tags,
        spatial: source.spatial_uri.iter().cloned().collect(),
        spatial_resolution_in_meters: None,
        temporal: None,
        temporal_resolution: None,
        frequency: None,
        landing_page: Some(r.landing_page),
        documentation: Vec::new(),
        conforms_to: Vec::new(),
        applicable_legislation: Vec::new(),
        in_series: None,
        relations: Vec::new(),
        sources: Vec::new(),
        languages: vec![lang.to_owned()],
        version: None,
        version_notes: BTreeMap::new(),
        access_rights: None,
        provenance: BTreeMap::new(),
        dcat_type: None,
        identifier: Some(r.item_id.to_owned()),
        alternate_identifiers: Vec::new(),
        endpoint_url: None,
        generated_by: None,
        harvest_source_id: Some(source.name.clone()),
        harvest_source_item_id: Some(r.item_id.to_owned()),
        harvest_hash: None,
        custom_shacl_shapes: None,
        distributions,
        extras,
        restricted_grants: Vec::new(),
        issued: r.issued.unwrap_or(now),
        modified: r.modified.or(r.issued).unwrap_or(now),
    };
    // ponytail: the hash leaves out the `spatial` extra (it is shared with the other kinds); an
    // extent that changes alone is picked up with the record's next other change.
    let hash = compute_harvest_hash(&ds);
    ds.harvest_hash = Some(hash.clone());
    ds.extras.insert("harvest_hash".into(), hash);
    ds
}

/// One page of an ArcGIS REST services directory (`{url}?f=json`): its folders and its
/// `(name, type)` services, names as the directory gives them (a folder's carry its prefix).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ArcgisDirectory {
    pub folders: Vec<String>,
    pub services: Vec<(String, String)>,
}

/// Reads one ArcGIS REST directory page. An `error` answer (a secured directory: 499 "Token
/// Required") or an answer that lists neither folders nor services fails the run.
pub fn parse_arcgis_directory(body: &str) -> Result<ArcgisDirectory> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| DataroomError::OutboundRefused(format!("invalid ArcGIS REST JSON: {e}")))?;
    if let Some(err) = val.get("error") {
        return Err(DataroomError::OutboundRefused(format!(
            "the ArcGIS directory answers error {}: {}; only a public directory is harvested",
            err.get("code").map(Value::to_string).unwrap_or_default(),
            err.get("message").and_then(Value::as_str).unwrap_or("")
        )));
    }
    let (Some(folders), Some(services)) = (
        val.get("folders").and_then(Value::as_array),
        val.get("services").and_then(Value::as_array),
    ) else {
        return Err(DataroomError::OutboundRefused(
            "the answer lists no folders and services; check the URL names an ArcGIS REST \
             directory, e.g. https://gis.example.sk/arcgis/rest/services"
                .into(),
        ));
    };
    let safe = |n: &str| {
        !n.is_empty()
            && n.split('/').all(|seg| {
                !seg.is_empty()
                    && seg != ".."
                    && seg
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
            })
    };
    Ok(ArcgisDirectory {
        folders: folders
            .iter()
            .filter_map(Value::as_str)
            .filter(|f| safe(f))
            .map(str::to_owned)
            .collect(),
        services: services
            .iter()
            .filter_map(|s| Some((s.get("name")?.as_str()?, s.get("type")?.as_str()?)))
            .filter(|(n, t)| safe(n) && safe(t))
            .map(|(n, t)| (n.to_owned(), t.to_owned()))
            .collect(),
    })
}

/// The `.../rest/services` root of an ArcGIS REST address, the base every directory names its
/// folders and services from; the address itself when it holds no such segment.
pub fn arcgis_services_root(url: &str) -> &str {
    let url = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    match url.to_ascii_lowercase().find("/rest/services") {
        Some(i) => &url[..i + "/rest/services".len()],
        None => url,
    }
}

/// Whether an ArcGIS REST address names one service, `.../FeatureServer` or `.../MapServer`.
pub fn arcgis_service_type(url: &str) -> Option<&'static str> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or("");
    ["FeatureServer", "MapServer"]
        .into_iter()
        .find(|t| last.eq_ignore_ascii_case(t))
}

/// The most layers of one service listed as distributions.
const MAX_LAYER_LINKS: usize = 20;

/// One ArcGIS REST service (`{service_url}?f=json`) as a record: its title, description,
/// keywords, licence or copyright, extent in WGS 84, the service itself and a GeoJSON query of
/// each feature layer as distributions. `Err` holds the reason the run log gives for a skip: an
/// answer that is not a service's JSON, or an `error` answer.
pub fn parse_arcgis_service(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    service_url: &str,
    item_id: &str,
    body: &str,
    lang: &str,
) -> std::result::Result<Dataset, String> {
    let val: Value =
        serde_json::from_str(body).map_err(|_| "the service answers no JSON".to_owned())?;
    if let Some(err) = val.get("error") {
        return Err(format!(
            "the service answers error {}",
            err.get("code").map(Value::to_string).unwrap_or_default()
        ));
    }
    if !val.is_object() || val.get("layers").is_some_and(|l| !l.is_array()) {
        return Err("the answer is not an ArcGIS service's JSON".into());
    }
    let text = |v: Option<&Value>| plain_text_from_html(v.and_then(Value::as_str).unwrap_or(""));
    let doc = val.get("documentInfo");
    let fallback = item_id
        .rsplit('/')
        .nth(1)
        .unwrap_or(item_id)
        .replace('_', " ");
    let title = Some(text(doc.and_then(|d| d.get("Title"))))
        .filter(|t| !t.is_empty())
        .unwrap_or(fallback);
    let (title, _) = source_title(source, item_id, &title);
    let notes = [
        val.get("serviceDescription"),
        val.get("description"),
        doc.and_then(|d| d.get("Subject")),
    ]
    .into_iter()
    .map(text)
    .find(|t| !t.is_empty())
    .unwrap_or_else(|| title.clone());
    let keywords = doc
        .and_then(|d| d.get("Keywords"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let rights = [val.get("licenseInfo"), val.get("copyrightText")]
        .into_iter()
        .map(text)
        .find(|t| !t.is_empty());
    let licence = rights.as_deref().and_then(open_licence);
    let bbox = val
        .get("fullExtent")
        .or_else(|| val.get("initialExtent"))
        .and_then(arcgis_extent_wgs84);
    let kind = arcgis_service_type(service_url).unwrap_or("MapServer");
    let mut links = vec![(
        format!("{title} ({kind})"),
        service_url.to_owned(),
        None,
        Some("ArcGIS REST".to_owned()),
        Some("application/json".to_owned()),
    )];
    let layers = val
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for layer in layers
        .filter(|l| {
            l.get("subLayerIds")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        })
        .take(MAX_LAYER_LINKS)
    {
        let Some(id) = layer.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let name = layer.get("name").and_then(Value::as_str).unwrap_or("");
        links.push((
            format!("{} (GeoJSON)", if name.is_empty() { &title } else { name }),
            format!("{service_url}/{id}"),
            Some(format!(
                "{service_url}/{id}/query?where=1%3D1&outFields=*&f=geojson"
            )),
            Some("GeoJSON".to_owned()),
            Some("application/geo+json".to_owned()),
        ));
    }
    Ok(record_dataset(
        org_id,
        catalogue_id,
        source,
        Record {
            item_id,
            address: service_url.to_owned(),
            landing_page: service_url.to_owned(),
            title,
            notes,
            tags: keyword_tags(keywords.split([',', ';'])),
            licence,
            licence_text: rights,
            bbox,
            issued: None,
            modified: None,
            links,
        },
        lang,
    ))
}

/// One page of an OGC API Records collection's items.
#[derive(Debug)]
pub struct RecordsPage {
    pub kept: Vec<(String, Dataset)>,
    pub skipped: Vec<HarvestSkippedItem>,
    /// Records on the page, skipped ones included.
    pub read: usize,
    /// `numberMatched`, when the page gives it.
    pub matched: Option<usize>,
    /// The `rel: next` link, made absolute.
    pub next: Option<String>,
}

/// Link relations that name the record or the collection itself, not a distribution.
const NOT_DISTRIBUTIONS: [&str; 11] = [
    "self",
    "alternate",
    "collection",
    "root",
    "parent",
    "next",
    "prev",
    "license",
    "preview",
    "icon",
    "describedby",
];

/// Reads one page of an OGC API Records collection (`{collection}/items`, GeoJSON): a record
/// with no id or no title is skipped with its reason, the page's `next` link and
/// `numberMatched` drive the walk.
pub fn parse_ogc_records_page(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    page_url: &str,
    body: &str,
    lang: &str,
) -> Result<RecordsPage> {
    let val: Value = serde_json::from_str(body).map_err(|e| {
        DataroomError::OutboundRefused(format!("invalid OGC API Records JSON: {e}"))
    })?;
    let features = val
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DataroomError::OutboundRefused(
                "the answer has no features; check the URL names an OGC API Records collection, \
             e.g. https://data.example.sk/collections/metadata"
                    .into(),
            )
        })?;
    let base = url::Url::parse(page_url)
        .map_err(|e| DataroomError::OutboundRefused(format!("invalid page URL: {e}")))?;
    let absolute = |href: &str| {
        base.join(href)
            .ok()
            .filter(|u| matches!(u.scheme(), "https" | "http"))
            .map(String::from)
    };
    let next = links_of(&val)
        .find(|l| l.get("rel").and_then(Value::as_str) == Some("next"))
        .and_then(|l| l.get("href").and_then(Value::as_str))
        .and_then(absolute);
    let items = records_collection(page_url).to_owned();
    let mut page = RecordsPage {
        kept: Vec::new(),
        skipped: Vec::new(),
        read: features.len(),
        matched: val
            .get("numberMatched")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
        next,
    };
    for f in features {
        match ogc_record(org_id, catalogue_id, source, &items, f, &absolute, lang) {
            Ok(Some(kept)) => page.kept.push(kept),
            Ok(None) => {}
            Err(skip) => page.skipped.push(skip),
        }
    }
    Ok(page)
}

/// The collection an OGC API Records address names, its `/items` and query left off.
pub fn records_collection(url: &str) -> &str {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    path.strip_suffix("/items").unwrap_or(path)
}

fn links_of(v: &Value) -> impl Iterator<Item = &Value> {
    v.get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// One OGC API Records feature: `Ok(None)` for an item the source skips by name, `Err` with the
/// reason for a record that cannot be listed.
fn ogc_record(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    source: &HarvestSource,
    items: &str,
    f: &Value,
    absolute: &dyn Fn(&str) -> Option<String>,
    lang: &str,
) -> std::result::Result<Option<(String, Dataset)>, HarvestSkippedItem> {
    let props = f.get("properties").filter(|p| p.is_object());
    let item_id = match f.get("id") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let raw_title = props
        .and_then(|p| p.get("title"))
        .and_then(Value::as_str)
        .map(plain_text_from_html)
        .unwrap_or_default();
    let skip = |item_id: &str, title: &str, reason: &str| HarvestSkippedItem {
        item_id: item_id.to_owned(),
        title: title.to_owned(),
        reason: reason.to_owned(),
    };
    if item_id.is_empty() {
        return Err(skip("", &raw_title, "the record has no id"));
    }
    let Some(props) = props.filter(|_| !raw_title.is_empty()) else {
        return Err(skip(&item_id, &item_id, "the record has no title"));
    };
    let (title, skipped) = source_title(source, &item_id, &raw_title);
    if let Some(reason) = skipped {
        return Err(skip(&item_id, &title, &reason));
    }
    let str_of = |k: &str| props.get(k).and_then(Value::as_str);
    let notes = Some(plain_text_from_html(str_of("description").unwrap_or("")))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| title.clone());
    // Keywords and the ids of each theme's concepts (OGC API Records `themes`).
    let concepts = props
        .get("themes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("concepts")?.as_array())
        .flatten()
        .filter_map(|c| c.get("id")?.as_str());
    let keywords = props
        .get("keywords")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(concepts);
    let licence_text = str_of("license").map(str::to_owned).or_else(|| {
        links_of(f)
            .find(|l| l.get("rel").and_then(Value::as_str) == Some("license"))
            .and_then(|l| l.get("href").and_then(Value::as_str))
            .map(str::to_owned)
    });
    let licence = licence_text.as_deref().and_then(open_licence);
    let bbox = f.get("geometry").and_then(geometry_bbox).or_else(|| {
        let b: Vec<f64> = f
            .get("bbox")?
            .as_array()?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        <[f64; 4]>::try_from(b).ok()
    });
    let time = |k: &str| str_of(k).and_then(|t| t.parse::<TimestampTz>().ok());
    let address = format!("{items}/items/{}", utf8_percent(&item_id));
    let landing_page = links_of(f)
        .find(|l| {
            l.get("rel").and_then(Value::as_str) == Some("alternate")
                && l.get("type").and_then(Value::as_str) == Some("text/html")
        })
        .and_then(|l| l.get("href").and_then(Value::as_str))
        .and_then(absolute)
        .unwrap_or_else(|| address.clone());
    let links = links_of(f)
        .filter(|l| {
            !NOT_DISTRIBUTIONS.contains(&l.get("rel").and_then(Value::as_str).unwrap_or("self"))
        })
        .filter_map(|l| {
            let href = absolute(l.get("href")?.as_str()?)?;
            // `type` is a media type, or a protocol name some catalogues write (`OGC:WMS`).
            let kind = l.get("type").and_then(Value::as_str).map(str::trim);
            let media = kind.filter(|t| t.contains('/')).map(str::to_owned);
            let format = kind.filter(|t| *t != "download").map(format_of);
            let name = l
                .get("title")
                .and_then(Value::as_str)
                .map(plain_text_from_html)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| title.clone());
            Some((name, href, None, format, media))
        })
        .take(MAX_LAYER_LINKS)
        .collect();
    let record = Record {
        item_id: &item_id,
        address,
        landing_page,
        title,
        notes,
        tags: keyword_tags(keywords),
        licence,
        licence_text,
        bbox,
        issued: time("created"),
        modified: time("updated"),
        links,
    };
    let ds = record_dataset(org_id, catalogue_id, source, record, lang);
    Ok(Some((item_id, ds)))
}

/// A record id as one path segment.
fn utf8_percent(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// The catalogue's format label of a media type, the media type itself for one it does not name.
fn format_of(media: &str) -> String {
    match media.split(';').next().unwrap_or(media).trim() {
        "text/csv" => "CSV",
        "application/geo+json" => "GeoJSON",
        "application/json" => "JSON",
        "application/zip" => "ZIP",
        "application/pdf" => "PDF",
        "text/html" => "HTML",
        "application/xml" | "text/xml" => "XML",
        "application/vnd.ogc.wms_xml" => "WMS",
        "application/x-shapefile" => "SHP",
        "OGC:WMS" => "WMS",
        "OGC:WFS" => "WFS",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::harvested_name;
    use dataroom_core::model::validate_slug;

    #[test]
    fn a_harvested_name_is_always_a_valid_slug() {
        let long = format!("{}-b", "a".repeat(95));
        for (prefix, slug) in [("src-", long.as_str()), ("", "x"), ("", ""), ("src_", "")] {
            let name = harvested_name(prefix, slug, "0190a3b2-7c1d-7e00-8000-000000000001");
            assert!(
                validate_slug("name", &name).is_ok(),
                "{prefix:?} {slug:?} -> {name:?}"
            );
        }
        assert_eq!(harvested_name("src-", "air", "id"), "src-air");
        assert_ne!(
            harvested_name("src-", "", "0190a3b2-0001"),
            harvested_name("src-", "", "0190a3b2-0002"),
            "items whose titles have no Latin letter keep apart"
        );
    }
}
