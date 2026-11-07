#![forbid(unsafe_code)]
//! Source parsers for ArcGIS Hub (`harvest-hub.py`), NKOD SPARQL (`harvest-nkod.py`),
//! DCAT-AP RDF feeds (with Hydra pagination), and CKAN Action API (`package_search`).

use crate::guard::validate_outbound_url;
use chrono::{TimeZone, Utc};
use dataroom_core::{
    AgentDetails, CatalogueId, DataroomError, Dataset, Distribution, HarvestSkippedItem,
    HarvestSource, OrgId, RecordState, Result, TermsOfUse, Visibility,
};
use dataroom_rdf::{
    ns, parse_datasets_from_rdf, parse_rdf_graph, RdfFormat, RdfLimits,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// Strips HTML markup into plain paragraphs (`plain_text` in `ours/deployment/harvest-hub.py`).
pub fn plain_text_from_html(markup: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for c in markup.chars() {
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if c == '>' && in_tag {
            in_tag = false;
            let tag_name = tag_buf
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(
                tag_name.as_str(),
                "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                out.push('\n');
            }
        } else if in_tag {
            tag_buf.push(c);
        } else {
            out.push(c);
        }
    }
    let lines: Vec<String> = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n\n").chars().take(10_000).collect()
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

    let mut kept_raw: Vec<(&Value, String)> = Vec::new();
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
        if !open_licences.contains_key(lic) {
            skipped.push(HarvestSkippedItem {
                item_id,
                title,
                reason: format!("no open licence ({lic})"),
            });
            continue;
        }

        if item_type == "Feature Service" {
            let service_url = item.get("url").and_then(Value::as_str).unwrap_or("");
            if validate_outbound_url(
                service_url,
                &source.allowed_hosts,
                cat_allowed_hosts,
                false,
            )
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

        kept_raw.push((item, title));
    }

    // Deduplicate by title: newest `modified` wins (`harvest-hub.py` lines 106-115).
    let mut newest_by_title: BTreeMap<String, (&Value, i64)> = BTreeMap::new();
    for (item, title) in &kept_raw {
        let mod_ms = item.get("modified").and_then(Value::as_i64).unwrap_or(0);
        match newest_by_title.get(title) {
            Some((_, prev_ms)) if *prev_ms >= mod_ms => {}
            _ => {
                newest_by_title.insert(title.clone(), (*item, mod_ms));
            }
        }
    }

    let mut datasets = Vec::new();
    for (item, title) in kept_raw {
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

        let lic_code = item.get("license").and_then(Value::as_str).unwrap();
        let (licence_id, licence_url) = open_licences[lic_code];
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

        let desc_raw = item
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| item.get("snippet").and_then(Value::as_str))
            .unwrap_or(&title);
        let notes = plain_text_from_html(desc_raw);

        let mut tags = BTreeSet::new();
        if let Some(arr) = item.get("tags").and_then(Value::as_array) {
            for t in arr.iter().filter_map(Value::as_str) {
                let clean = t.trim();
                if (2..=100).contains(&clean.chars().count()) {
                    tags.insert(clean.to_owned());
                }
            }
        }

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
            download_url: Some(format!("{hub_base}/api/download/v1/items/{item_id}/csv?layers=0")),
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
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);
        let modified_dt = item
            .get("modified")
            .and_then(Value::as_i64)
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);

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
            publisher: source.target_publisher_iri.as_ref().map(|uri| AgentDetails {
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
            themes: vec![
                "http://publications.europa.eu/resource/authority/data-theme/REGI".into(),
            ],
            eurovoc_themes: Vec::new(),
            keywords: BTreeMap::from([(
                default_lang.to_owned(),
                tags.iter().cloned().collect(),
            )]),
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
            let d = entry.distributions.entry(dist_iri).or_insert_with(|| RawDist {
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
        if let Some(reason) = source.skip_items.get(&raw.title).or_else(|| source.skip_items.get(&raw.iri)) {
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
            let mut url_str = d.url.clone();
            for (old_host, new_host) in &source.rewrite_hosts {
                url_str = url_str.replace(old_host, new_host);
            }
            if validate_outbound_url(&url_str, &source.allowed_hosts, cat_allowed_hosts, false)
                .is_err()
            {
                continue;
            }
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

        let base_slug = ascii_slug(&raw.title);
        let name: String = format!("{}{}", source.name_prefix, base_slug)
            .chars()
            .take(100)
            .collect();
        let fixed_uuid = Uuid::new_v5(&Uuid::NAMESPACE_URL, raw.iri.as_bytes()).to_string();

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
            publisher: source.target_publisher_iri.as_ref().map(|uri| AgentDetails {
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
            themes: vec![
                "http://publications.europa.eu/resource/authority/data-theme/GOVE".into(),
            ],
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
            issued: Utc::now(),
            modified: Utc::now(),
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
        let source_item_id = ds
            .uri
            .clone()
            .unwrap_or_else(|| ds.name.clone());
        if !source.name_prefix.is_empty() && !ds.name.starts_with(&source.name_prefix) {
            ds.name = format!("{}{}", source.name_prefix, ds.name)
                .chars()
                .take(100)
                .collect();
        }
        ds.id = Uuid::new_v5(&Uuid::NAMESPACE_URL, source_item_id.as_bytes()).to_string();
        ds.harvest_source_id = Some(source.name.clone());
        ds.harvest_source_item_id = Some(source_item_id.clone());
        ds.extras
            .insert("harvest_source".into(), source.name.clone());
        ds.extras
            .insert("source_id".into(), source_item_id.clone());
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
        ds.publisher_id = source.publisher_id.clone();
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
