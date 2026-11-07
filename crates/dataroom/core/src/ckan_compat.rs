#![forbid(unsafe_code)]
//! Bidirectional conversion between [`Dataset`] / [`Publisher`] / [`Distribution`] and the exact
//! CKAN Action API v3 JSON dictionaries read and written by `jcctl`, the Portal and harvesters.

use crate::model::{
    text_in_lang, AgentDetails, CatalogueId, ContactKind, ContactPoint, DataService, Dataset,
    Distribution, OrgId, Publisher, RecordState, ResourceView, TemporalPeriod, Visibility,
};
use crate::vocab::{default_licence_register, find_licence};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Converts a [`Dataset`] and its [`Publisher`] + [`ResourceView`]s into a CKAN `package_show` /
/// `package_search` dictionary with every field `ours/platform/jcctl-publish/ckan.rs`,
/// `ours/portal/catalogue.rs` and `ours/deployment/harvest-*.py` read.
pub fn dataset_to_ckan(
    dataset: &Dataset,
    publisher: Option<&Publisher>,
    views_by_resource: &BTreeMap<String, Vec<ResourceView>>,
    default_lang: &str,
) -> Value {
    let title = text_in_lang(&dataset.title, Some(default_lang))
        .unwrap_or(&dataset.name)
        .to_owned();
    let notes = text_in_lang(&dataset.description, Some(default_lang)).map(str::to_owned);

    let licence = dataset
        .licence_id
        .as_deref()
        .and_then(find_licence)
        .or_else(|| dataset.licence_url.as_deref().and_then(find_licence));
    let license_id = dataset
        .licence_id
        .clone()
        .or_else(|| licence.as_ref().map(|l| l.id.clone()));
    let license_title = licence
        .as_ref()
        .map(|l| l.title.clone())
        .or_else(|| license_id.clone());
    let license_url = dataset
        .licence_url
        .clone()
        .or_else(|| licence.as_ref().map(|l| l.url.clone()))
        .filter(|u| !u.is_empty());

    let org_value = match publisher {
        Some(pub_rec) => {
            let org_title = text_in_lang(&pub_rec.title, Some(default_lang))
                .unwrap_or(&pub_rec.name)
                .to_owned();
            json!({
                "id": pub_rec.id,
                "name": pub_rec.name,
                "title": org_title,
                "type": "organization",
                "description": text_in_lang(&pub_rec.description, Some(default_lang)).unwrap_or(""),
                "image_url": pub_rec.logo_url.as_deref().unwrap_or(""),
                "created": pub_rec.created.to_rfc3339(),
                "is_organization": true,
                "approval_status": "approved",
                "state": pub_rec.state.ckan_state(),
            })
        }
        None => json!({
            "id": dataset.publisher_id,
            "name": dataset.publisher_id,
            "title": dataset.publisher_id,
            "type": "organization",
            "is_organization": true,
            "state": "active",
        }),
    };

    let tags: Vec<Value> = dataset
        .tags
        .iter()
        .map(|t| {
            json!({
                "id": format!("tag-{t}"),
                "name": t,
                "display_name": t,
                "state": "active",
                "vocabulary_id": Value::Null,
            })
        })
        .collect();

    let resources: Vec<Value> = dataset
        .distributions
        .iter()
        .enumerate()
        .map(|(idx, dist)| {
            distribution_to_ckan(
                dist,
                &dataset.id,
                idx,
                views_by_resource
                    .get(&dist.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                default_lang,
            )
        })
        .collect();

    let mut extras_map: BTreeMap<String, String> = dataset.extras.clone();
    if let Some(ident) = &dataset.identifier {
        extras_map
            .entry("identifier".into())
            .or_insert_with(|| ident.clone());
    }
    if let Some(freq) = &dataset.frequency {
        extras_map
            .entry("frequency".into())
            .or_insert_with(|| freq.clone());
    }
    if !dataset.themes.is_empty() {
        if let Ok(s) = serde_json::to_string(&dataset.themes) {
            extras_map.entry("theme".into()).or_insert(s);
        }
    }
    if !dataset.spatial.is_empty() {
        if dataset.spatial.len() == 1 {
            extras_map
                .entry("spatial_uri".into())
                .or_insert_with(|| dataset.spatial[0].clone());
        } else if let Ok(s) = serde_json::to_string(&dataset.spatial) {
            extras_map.entry("spatial_uri".into()).or_insert(s);
        }
    }
    if let Some(temp) = &dataset.temporal {
        if let Some(start) = temp.start {
            extras_map
                .entry("temporal_start".into())
                .or_insert_with(|| start.to_string());
        }
        if let Some(end) = temp.end {
            extras_map
                .entry("temporal_end".into())
                .or_insert_with(|| end.to_string());
        }
    }
    if let Some(cp) = &dataset.contact_point {
        if let Some(cname) = text_in_lang(&cp.name, Some(default_lang)) {
            extras_map
                .entry("contact_name".into())
                .or_insert_with(|| cname.to_owned());
        }
        if !cp.email.is_empty() {
            extras_map
                .entry("contact_email".into())
                .or_insert_with(|| cp.email.trim_start_matches("mailto:").to_owned());
        }
    }
    if let Some(pub_agent) = &dataset.publisher {
        if let Some(pname) = text_in_lang(&pub_agent.name, Some(default_lang)) {
            extras_map
                .entry("publisher_name".into())
                .or_insert_with(|| pname.to_owned());
        }
        if let Some(puri) = &pub_agent.uri {
            extras_map
                .entry("publisher_uri".into())
                .or_insert_with(|| puri.clone());
        }
    }
    if let Some(ep) = &dataset.endpoint_url {
        extras_map
            .entry("endpoint".into())
            .or_insert_with(|| ep.clone());
    }
    if let Some(gen_by) = &dataset.generated_by {
        extras_map
            .entry("generated_by".into())
            .or_insert_with(|| gen_by.clone());
    }
    if let Some(lic_url) = &dataset.licence_url {
        extras_map
            .entry("license_url".into())
            .or_insert_with(|| lic_url.clone());
    }
    if let Some(h_source) = &dataset.harvest_source_id {
        extras_map
            .entry("harvest_source".into())
            .or_insert_with(|| h_source.clone());
    }
    if let Some(h_id) = &dataset.harvest_source_item_id {
        extras_map
            .entry("source_id".into())
            .or_insert_with(|| h_id.clone());
    }
    if let Some(h_hash) = &dataset.harvest_hash {
        extras_map
            .entry("harvest_hash".into())
            .or_insert_with(|| h_hash.clone());
    }

    let extras: Vec<Value> = extras_map
        .into_iter()
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect();

    let maintainer = dataset
        .contact_point
        .as_ref()
        .and_then(|c| text_in_lang(&c.name, Some(default_lang)))
        .map(str::to_owned);
    let maintainer_email = dataset
        .contact_point
        .as_ref()
        .map(|c| c.email.trim_start_matches("mailto:").to_owned());

    let owner_org = publisher
        .map(|p| p.name.clone())
        .unwrap_or_else(|| dataset.publisher_id.clone());

    let mut out = Map::new();
    out.insert("id".into(), json!(dataset.id));
    out.insert("name".into(), json!(dataset.name));
    out.insert("title".into(), json!(title));
    out.insert("title_translated".into(), json!(dataset.title));
    if let Some(n) = notes {
        out.insert("notes".into(), json!(n));
    } else {
        out.insert("notes".into(), Value::Null);
    }
    out.insert("notes_translated".into(), json!(dataset.description));
    out.insert("owner_org".into(), json!(owner_org));
    out.insert("organization".into(), org_value);
    out.insert(
        "private".into(),
        json!(dataset.visibility != Visibility::Public),
    );
    out.insert("visibility".into(), json!(dataset.visibility.as_str()));
    out.insert("state".into(), json!(dataset.state.ckan_state()));
    out.insert("type".into(), json!(dataset.dataset_type));
    out.insert("url".into(), json!(dataset.landing_page));
    out.insert("version".into(), json!(dataset.version));
    out.insert("license_id".into(), json!(license_id));
    out.insert("license_title".into(), json!(license_title));
    if let Some(lu) = &license_url {
        out.insert("license_url".into(), json!(lu));
        out.insert("license_link".into(), json!(lu));
    }
    out.insert("maintainer".into(), json!(maintainer));
    out.insert("maintainer_email".into(), json!(maintainer_email));
    out.insert(
        "metadata_created".into(),
        json!(dataset.issued.to_rfc3339()),
    );
    out.insert(
        "metadata_modified".into(),
        json!(dataset.modified.to_rfc3339()),
    );
    out.insert("num_resources".into(), json!(resources.len()));
    out.insert("num_tags".into(), json!(tags.len()));
    out.insert("tags".into(), Value::Array(tags));
    out.insert("resources".into(), Value::Array(resources));
    out.insert("extras".into(), Value::Array(extras));
    out.insert("groups".into(), json!([]));
    out.insert("relationships_as_subject".into(), json!([]));
    out.insert("relationships_as_object".into(), json!([]));
    Value::Object(out)
}

/// Converts a [`Distribution`] into a CKAN resource dictionary.
pub fn distribution_to_ckan(
    dist: &Distribution,
    package_id: &str,
    position: usize,
    views: &[ResourceView],
    default_lang: &str,
) -> Value {
    let name = text_in_lang(&dist.title, Some(default_lang))
        .unwrap_or(&dist.access_url)
        .to_owned();
    let description = text_in_lang(&dist.description, Some(default_lang)).map(str::to_owned);
    let url = dist
        .download_url
        .as_ref()
        .unwrap_or(&dist.access_url)
        .clone();

    let mut obj = Map::new();
    obj.insert("id".into(), json!(dist.id));
    obj.insert("package_id".into(), json!(package_id));
    obj.insert("name".into(), json!(name));
    obj.insert("name_translated".into(), json!(dist.title));
    obj.insert("url".into(), json!(url));
    obj.insert("access_url".into(), json!(dist.access_url));
    if let Some(dl) = &dist.download_url {
        obj.insert("download_url".into(), json!(dl));
    }
    obj.insert("format".into(), json!(dist.format.as_deref().unwrap_or("")));
    obj.insert("mimetype".into(), json!(dist.media_type));
    obj.insert("description".into(), json!(description));
    obj.insert("description_translated".into(), json!(dist.description));
    obj.insert("hash".into(), json!(dist.hash.as_deref().unwrap_or("")));
    if let Some(algo) = &dist.hash_algorithm {
        obj.insert("hash_algorithm".into(), json!(algo));
    }
    obj.insert("size".into(), json!(dist.byte_size));
    obj.insert("position".into(), json!(position));
    obj.insert("state".into(), json!("active"));
    obj.insert("resource_type".into(), json!(dist.resource_type.as_deref()));
    if !dist.conforms_to.is_empty() {
        if dist.conforms_to.len() == 1 {
            obj.insert("conforms_to".into(), json!(dist.conforms_to[0]));
        } else {
            obj.insert("conforms_to".into(), json!(dist.conforms_to));
        }
    }
    obj.insert("has_views".into(), json!(!views.is_empty()));
    for (k, v) in &dist.extras {
        obj.entry(k.clone()).or_insert_with(|| v.clone());
    }
    Value::Object(obj)
}

/// Converts a [`ResourceView`] into a CKAN `resource_view` dictionary.
pub fn resource_view_to_ckan(view: &ResourceView) -> Value {
    let mut obj = Map::new();
    obj.insert("id".into(), json!(view.id));
    obj.insert("resource_id".into(), json!(view.resource_id));
    obj.insert("view_type".into(), json!(view.view_type));
    obj.insert("title".into(), json!(view.title));
    obj.insert("description".into(), json!(view.description));
    obj.insert("order".into(), json!(view.order));
    for (k, v) in &view.config {
        obj.insert(k.clone(), v.clone());
    }
    Value::Object(obj)
}

/// Parses a CKAN `package_create` / `package_update` payload into a [`Dataset`].
pub fn dataset_from_ckan(
    org_id: OrgId,
    catalogue_id: CatalogueId,
    payload: &Value,
    existing: Option<&Dataset>,
    default_lang: &str,
) -> Dataset {
    let now = Utc::now();
    // CKAN callers address a dataset by id or by name in `id`; an update of an existing record
    // keeps that record's id, so `{"id": "<name>"}` never forks a second record.
    let id = existing
        .map(|d| d.id.clone())
        .or_else(|| {
            payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("pkg-{}", Uuid::now_v7()));

    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| existing.map(|d| d.name.clone()))
        .unwrap_or_default();

    let mut title = BTreeMap::new();
    if let Some(Value::Object(map)) = payload.get("title_translated") {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    title.insert(k.clone(), s.to_owned());
                }
            }
        }
    }
    if let Some(t) = payload.get("title").and_then(Value::as_str) {
        if !t.is_empty() {
            title.insert(default_lang.to_owned(), t.to_owned());
        }
    }
    if title.is_empty() {
        if let Some(ex) = existing {
            title = ex.title.clone();
        } else if !name.is_empty() {
            title.insert(default_lang.to_owned(), name.clone());
        }
    }

    let mut description = BTreeMap::new();
    if let Some(Value::Object(map)) = payload.get("notes_translated") {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    description.insert(k.clone(), s.to_owned());
                }
            }
        }
    }
    // When `notes` is explicitly supplied in the payload (e.g. `package_update` or `package_patch`),
    // it overrides `notes_translated[default_lang]` so patching `notes` is never clobbered by a stale
    // `notes_translated` entry (A3).
    if let Some(n) = payload.get("notes").and_then(Value::as_str) {
        if !n.is_empty() {
            description.insert(default_lang.to_owned(), n.to_owned());
        }
    }
    if description.is_empty() {
        if let Some(ex) = existing {
            description = ex.description.clone();
        }
    }

    let publisher_id = payload
        .get("owner_org")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            payload
                .pointer("/organization/name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| existing.map(|d| d.publisher_id.clone()))
        .unwrap_or_default();

    let mut extras_map = BTreeMap::new();
    if let Some(Value::Array(items)) = payload.get("extras") {
        for item in items {
            if let Some(k) = item.get("key").and_then(Value::as_str) {
                let val_str = match item.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => continue,
                    Some(other) => other.to_string(),
                };
                extras_map.insert(k.to_owned(), val_str);
            }
        }
    } else if let Some(ex) = existing {
        extras_map = ex.extras.clone();
    }

    let visibility = if let Some(vis_str) = payload.get("visibility").and_then(Value::as_str) {
        match vis_str {
            "restricted" => Visibility::Restricted,
            "internal" => Visibility::Internal,
            _ => Visibility::Public,
        }
    } else if let Some(priv_bool) = payload.get("private").and_then(Value::as_bool) {
        if priv_bool {
            Visibility::Internal
        } else {
            Visibility::Public
        }
    } else if let Some(ex) = existing {
        ex.visibility
    } else {
        Visibility::Public
    };

    let state = match payload.get("state").and_then(Value::as_str) {
        Some("deleted" | "withdrawn") => RecordState::Withdrawn,
        Some("draft") => RecordState::Draft,
        Some("active") => RecordState::Active,
        _ => existing.map(|d| d.state).unwrap_or(RecordState::Active),
    };

    let mut tags = Vec::new();
    if let Some(Value::Array(tag_items)) = payload.get("tags") {
        for item in tag_items {
            if let Some(n) = item.get("name").and_then(Value::as_str) {
                if !tags.contains(&n.to_owned()) {
                    tags.push(n.to_owned());
                }
            } else if let Some(n) = item.as_str() {
                if !tags.contains(&n.to_owned()) {
                    tags.push(n.to_owned());
                }
            }
        }
    } else if let Some(ex) = existing {
        tags = ex.tags.clone();
    }

    let mut keywords: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(Value::Object(kw_map)) = payload.get("tags_translated") {
        for (lang, arr) in kw_map {
            if let Some(items) = arr.as_array() {
                let words: Vec<String> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                if !words.is_empty() {
                    keywords.insert(lang.clone(), words);
                }
            }
        }
    }
    if keywords.is_empty() && !tags.is_empty() {
        keywords.insert(default_lang.to_owned(), tags.clone());
    }

    let parse_json_or_single = |raw: Option<&String>| -> Vec<String> {
        let Some(raw) = raw else {
            return Vec::new();
        };
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => vec![raw.clone()],
        }
    };

    let themes = payload
        .get("theme")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| parse_json_or_single(extras_map.get("theme")));

    let spatial = {
        let s_uri = parse_json_or_single(extras_map.get("spatial_uri"));
        if !s_uri.is_empty() {
            s_uri
        } else {
            parse_json_or_single(extras_map.get("spatial"))
                .into_iter()
                .filter(|s| !s.starts_with('{'))
                .collect()
        }
    };

    let temporal = {
        let start = extras_map
            .get("temporal_start")
            .and_then(|s| s.get(..10))
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        let end = extras_map
            .get("temporal_end")
            .and_then(|s| s.get(..10))
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        if start.is_some() || end.is_some() {
            Some(TemporalPeriod { start, end })
        } else {
            existing.and_then(|e| e.temporal.clone())
        }
    };

    let contact_point = {
        let cname = extras_map.get("contact_name").cloned().or_else(|| {
            payload
                .get("maintainer")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let cemail = extras_map.get("contact_email").cloned().or_else(|| {
            payload
                .get("maintainer_email")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        if cname.is_some() || cemail.is_some() {
            let mut names = BTreeMap::new();
            if let Some(n) = cname {
                names.insert(default_lang.to_owned(), n);
            }
            Some(ContactPoint {
                uri: extras_map.get("contact_uri").cloned(),
                kind: ContactKind::Organization,
                name: names,
                email: cemail.unwrap_or_default(),
                url: None,
                identifier: None,
            })
        } else {
            existing.and_then(|e| e.contact_point.clone())
        }
    };

    let publisher_agent = {
        let pname = extras_map.get("publisher_name").cloned();
        let puri = extras_map.get("publisher_uri").cloned();
        if pname.is_some() || puri.is_some() {
            let mut names = BTreeMap::new();
            if let Some(n) = pname {
                names.insert(default_lang.to_owned(), n);
            }
            Some(AgentDetails {
                uri: puri,
                name: names,
                email: extras_map.get("publisher_email").cloned(),
                homepage: extras_map.get("publisher_url").cloned(),
                agent_type: extras_map.get("publisher_type").cloned(),
                identifier: extras_map.get("publisher_identifier").cloned(),
            })
        } else {
            existing.and_then(|e| e.publisher.clone())
        }
    };

    let licence_id = payload
        .get("license_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| existing.and_then(|e| e.licence_id.clone()));

    let licence_url = extras_map
        .get("license_url")
        .cloned()
        .or_else(|| {
            payload
                .get("license_url")
                .or_else(|| payload.get("license_link"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| existing.and_then(|e| e.licence_url.clone()));

    let distributions = if let Some(Value::Array(res_items)) = payload.get("resources") {
        let old_ids: Vec<String> = existing
            .map(|e| e.distributions.iter().map(|r| r.id.clone()).collect())
            .unwrap_or_default();
        res_items
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let fallback_id = old_ids
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| format!("res-{id}-{idx}"));
                distribution_from_ckan(r, &id, &fallback_id, default_lang)
            })
            .collect()
    } else if let Some(ex) = existing {
        ex.distributions.clone()
    } else {
        Vec::new()
    };

    let issued = extras_map
        .get("issued")
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| existing.map(|e| e.issued))
        .unwrap_or(now);

    let applicable_legislation = parse_json_or_single(extras_map.get("applicable_legislation"));
    let conforms_to = parse_json_or_single(extras_map.get("conforms_to"));
    let documentation = parse_json_or_single(extras_map.get("documentation"));
    let languages = parse_json_or_single(extras_map.get("language"));

    Dataset {
        org_id,
        catalogue_id,
        id,
        name,
        uri: payload
            .get("uri")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| extras_map.get("uri").cloned())
            .or_else(|| extras_map.get("source_uri").cloned()),
        dataset_type: payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("dataset")
            .to_owned(),
        title,
        description,
        publisher_id,
        publisher: publisher_agent,
        creator: existing.and_then(|e| e.creator.clone()),
        contact_point,
        visibility,
        state,
        licence_id,
        licence_url,
        themes,
        eurovoc_themes: existing
            .map(|e| e.eurovoc_themes.clone())
            .unwrap_or_default(),
        keywords,
        tags,
        spatial,
        spatial_resolution_in_meters: extras_map
            .get("spatial_resolution_in_meters")
            .and_then(|s| s.parse().ok()),
        temporal,
        temporal_resolution: extras_map.get("temporal_resolution").cloned(),
        frequency: extras_map.get("frequency").cloned(),
        landing_page: payload
            .get("url")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        documentation,
        conforms_to,
        applicable_legislation,
        in_series: existing.and_then(|e| e.in_series.clone()),
        relations: parse_json_or_single(extras_map.get("related_resource")),
        sources: parse_json_or_single(extras_map.get("source")),
        languages,
        version: payload
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        version_notes: BTreeMap::new(),
        access_rights: extras_map.get("access_rights").cloned(),
        provenance: BTreeMap::new(),
        dcat_type: extras_map.get("dcat_type").cloned(),
        identifier: extras_map.get("identifier").cloned(),
        alternate_identifiers: parse_json_or_single(extras_map.get("alternate_identifier")),
        endpoint_url: extras_map.get("endpoint").cloned(),
        generated_by: extras_map.get("generated_by").cloned(),
        harvest_source_id: extras_map.get("harvest_source").cloned(),
        harvest_source_item_id: extras_map.get("source_id").cloned(),
        harvest_hash: extras_map.get("harvest_hash").cloned(),
        custom_shacl_shapes: existing.and_then(|e| e.custom_shacl_shapes.clone()),
        distributions,
        extras: extras_map,
        restricted_grants: payload
            .get("restricted_grants")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .or_else(|| existing.map(|e| e.restricted_grants.clone()))
            .unwrap_or_default(),
        issued,
        modified: now,
    }
}

/// Parses a CKAN resource dictionary into a [`Distribution`].
pub fn distribution_from_ckan(
    raw: &Value,
    dataset_id: &str,
    fallback_id: &str,
    default_lang: &str,
) -> Distribution {
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_id)
        .to_owned();

    let url = raw
        .get("url")
        .or_else(|| raw.get("access_url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let download_url = raw
        .get("download_url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let mut title = BTreeMap::new();
    if let Some(Value::Object(map)) = raw.get("name_translated") {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    title.insert(k.clone(), s.to_owned());
                }
            }
        }
    }
    if let Some(n) = raw.get("name").and_then(Value::as_str) {
        if !n.is_empty() {
            title
                .entry(default_lang.to_owned())
                .or_insert_with(|| n.to_owned());
        }
    }

    let mut description = BTreeMap::new();
    if let Some(Value::Object(map)) = raw.get("description_translated") {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    description.insert(k.clone(), s.to_owned());
                }
            }
        }
    }
    if let Some(d) = raw.get("description").and_then(Value::as_str) {
        if !d.is_empty() {
            description.insert(default_lang.to_owned(), d.to_owned());
        }
    }

    let conforms_to = match raw.get("conforms_to") {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };

    let access_services = match raw.get("access_services") {
        Some(Value::Array(arr)) => arr
            .iter()
            .enumerate()
            .map(|(i, s)| DataService {
                id: format!("svc-{id}-{i}"),
                uri: s.get("uri").and_then(Value::as_str).map(str::to_owned),
                title: s
                    .get("title")
                    .and_then(Value::as_str)
                    .map(|t| BTreeMap::from([(default_lang.to_owned(), t.to_owned())]))
                    .unwrap_or_default(),
                endpoint_urls: s
                    .get("endpoint_url")
                    .and_then(Value::as_array)
                    .map(|u| {
                        u.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                ..DataService::default()
            })
            .collect(),
        _ => Vec::new(),
    };

    Distribution {
        id,
        dataset_id: dataset_id.to_owned(),
        uri: raw
            .get("uri")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        title,
        description,
        access_url: url,
        download_url,
        format: raw
            .get("format")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        media_type: raw
            .get("mimetype")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        compress_format: raw
            .get("compress_format")
            .and_then(Value::as_str)
            .map(str::to_owned),
        package_format: raw
            .get("package_format")
            .and_then(Value::as_str)
            .map(str::to_owned),
        byte_size: raw.get("size").and_then(Value::as_u64),
        hash: raw
            .get("hash")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        hash_algorithm: raw
            .get("hash_algorithm")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        licence: raw
            .get("license")
            .and_then(Value::as_str)
            .map(str::to_owned),
        rights: BTreeMap::new(),
        status: raw.get("status").and_then(Value::as_str).map(str::to_owned),
        availability: raw
            .get("availability")
            .and_then(Value::as_str)
            .map(str::to_owned),
        resource_type: raw
            .get("resource_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        conforms_to,
        documentation: Vec::new(),
        languages: Vec::new(),
        applicable_legislation: Vec::new(),
        temporal_resolution: raw
            .get("temporal_resolution")
            .and_then(Value::as_str)
            .map(str::to_owned),
        spatial_resolution_in_meters: raw
            .get("spatial_resolution_in_meters")
            .and_then(Value::as_f64),
        terms_of_use: None,
        access_services,
        issued: None,
        modified: None,
        extras: BTreeMap::new(),
    }
}

/// Converts the built-in licence register to CKAN `license_list` JSON format.
pub fn ckan_license_list() -> Vec<Value> {
    default_licence_register()
        .into_iter()
        .map(|l| {
            json!({
                "id": l.id,
                "title": l.title,
                "url": l.url,
                "status": l.status,
                "is_okd_compliant": l.is_open,
                "is_osi_compliant": false,
                "is_generic": l.id == "notspecified",
                "domain_data": l.domain_data,
                "domain_content": l.domain_content,
                "domain_software": l.domain_software,
                "family": "",
                "maintainer": "",
                "od_conformance": if l.is_open { "approved" } else { "not reviewed" },
                "osd_conformance": "not reviewed",
            })
        })
        .collect()
}
