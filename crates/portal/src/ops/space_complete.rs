//! `jc_space_complete`: completes a partially defined ContextSpace from files or an endpoint (T-0642).

use std::collections::BTreeMap;

use jcctl::pipeline_test::{Sample, SampleFormat};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use utoipa::ToSchema;

use super::Operation;
use super::OperationAnnotations;
use crate::api::import::ImportReport;
use crate::change::{Change, Lane};
use crate::error::ApiError;
use crate::ops::feed_shape::{self, Records};
use crate::ops::verdict::{Finding, Level, Verdict};
use crate::ops::{self, Caller, OpError};
use crate::resource::{self, is_dns1123, API_VERSION};
use crate::state::AppState;
use crate::tools::model_tools;
use jc_core::kinds::Verb;

const MAX_TOTAL_FILES_BYTES: usize = 10 * 1024 * 1024; // 10 MiB (DM-55)
/// The longest description a completion carries onto its drafts (AG-73).
const MAX_DESCRIPTION_CHARS: usize = 4000;

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceCompleteInput {
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub files: Option<Vec<InputFile>>,
    #[serde(default)]
    pub url: Option<String>,
    /// The entity type the inferred model and the pipeline name, PascalCase (AG-73).
    #[serde(default)]
    pub type_name: Option<String>,
    /// What the data is, carried onto the drafted model, space and source (AG-73).
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub propose: bool,
    /// The person uploads the sample's rows themselves (DM-81, T-3840): the space gets its
    /// Endpoint when it has none, and the person a grant to write its type by batch.
    #[serde(default)]
    pub upload: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletedDraft {
    pub kind: String,
    pub name: String,
    pub inferred: bool,
    pub manifest: Value,
    pub verdict: Option<Verdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCompleteOutput {
    pub space: String,
    pub found: Vec<String>,
    pub drafts: Vec<CompletedDraft>,
    pub propose_ready: bool,
    pub lane: Lane,
    pub change: Option<Change>,
    /// This space already serves this model, so there is nothing to propose: the rows of the
    /// same file load straight away.
    pub already_live: bool,
}

/// What the same file met when it came back: a retry or a second attach opens nothing new.
enum Standing {
    Nothing,
    Open(Box<Change>),
    Live,
}

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string", "description": "Context space name (dns-1123 label)", "maxLength": 63 },
            "files": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The file's name, as the person uploaded it", "maxLength": super::bounds::TITLE },
                        "content": {
                            "type": "string",
                            "description": "The file's text; all files at most 10 MiB together",
                            "maxLength": MAX_TOTAL_FILES_BYTES
                        }
                    },
                    "required": ["name", "content"],
                    "additionalProperties": false
                },
                "description": "Text files defining or describing the space"
            },
            "url": { "type": "string", "description": "HTTP endpoint (JSON, GeoJSON, GBFS or NGSI-LD)", "maxLength": super::bounds::URL },
            "typeName": {
                "type": "string",
                "pattern": "^[A-Z][A-Za-z0-9]{0,62}$",
                "description": "Entity type of the model and pipeline output, PascalCase"
            },
            "description": {
                "type": "string",
                "maxLength": MAX_DESCRIPTION_CHARS,
                "description": "What the data is, in the person's words"
            },
            "propose": { "type": "boolean", "description": "Propose all drafts as one change when ready" },
            "upload": { "type": "boolean", "description": "The person writes the sample's rows: draft the space's Endpoint and write grant when missing" }
        },
        "additionalProperties": false
    })
}

pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string" },
            "found": { "type": "array", "items": { "type": "string" } },
            "drafts": { "type": "array", "items": { "type": "object" } },
            "proposeReady": { "type": "boolean" },
            "lane": { "type": "string" },
            "change": { "type": ["object", "null"] },
            "alreadyLive": { "type": "boolean" }
        },
        "required": ["space", "found", "drafts", "proposeReady", "lane", "alreadyLive"]
    })
}

pub fn validate_input(val: &Value) -> Result<(), OpError> {
    let input: SpaceCompleteInput = ops::parse_input(val.clone())?;

    let has_files = input.files.as_ref().is_some_and(|f| !f.is_empty());
    let has_url = input.url.as_ref().is_some_and(|u| !u.trim().is_empty());

    if has_files == has_url {
        return Err(OpError::InvalidInput {
            path: "/".into(),
            message: "exactly one of files or url is required".into(),
        });
    }

    if let Some(type_name) = &input.type_name {
        if !is_type_name(type_name) {
            return Err(OpError::InvalidInput {
                path: "/typeName".into(),
                message: format!(
                    "'{type_name}' is not a PascalCase type name (a capital letter, then up to 62 letters or digits)"
                ),
            });
        }
    }
    if input
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS)
    {
        return Err(OpError::InvalidInput {
            path: "/description".into(),
            message: format!("description is longer than {MAX_DESCRIPTION_CHARS} characters"),
        });
    }

    if let Some(files) = &input.files {
        let total_bytes: usize = files.iter().map(|f| f.content.len()).sum();
        if total_bytes > MAX_TOTAL_FILES_BYTES {
            return Err(OpError::InvalidInput {
                path: "/files".into(),
                message: format!("total file size exceeds limit of {MAX_TOTAL_FILES_BYTES} bytes"),
            });
        }
    }

    Ok(())
}

fn sanitize_dns1123(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == '-' || c == '_' || c == ' ' || c == '.')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "space".into()
    } else {
        trimmed.chars().take(63).collect()
    }
}

/// A NGSI-LD type name the model and the pipeline can carry: PascalCase letters and digits.
fn is_type_name(raw: &str) -> bool {
    let mut chars = raw.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && raw.len() <= 63
        && chars.all(|c| c.is_ascii_alphanumeric())
}

/// The class a completion infers: the type the person named, else the sample file's name,
/// else the space's own name made singular. Never `Entity`, the model's abstract base class.
fn class_name_of(type_name: Option<&str>, sample_name: Option<&str>, space_name: &str) -> String {
    if let Some(name) = type_name.filter(|name| is_type_name(name)) {
        return name.to_owned();
    }
    let derived = to_pascal_case(sample_name.unwrap_or(space_name));
    let derived = if derived == "Entity" {
        to_pascal_case(space_name)
    } else {
        derived
    };
    let mut class = if derived.starts_with(|c: char| c.is_ascii_uppercase()) && derived != "Entity"
    {
        derived
    } else {
        format!("Feed{derived}")
    };
    // ASCII by construction, so the cut lands between characters.
    class.truncate(63);
    class
}

/// The inferred LinkML source with `description` on `class`, when the source parses and the
/// class carries none yet; the source unchanged otherwise.
fn describe_class(source: &str, class: &str, description: &str) -> String {
    let Ok(mut doc) = serde_norway::from_str::<serde_norway::Value>(source) else {
        return source.to_owned();
    };
    let Some(entry) = doc
        .get_mut("classes")
        .and_then(|classes| classes.get_mut(class))
    else {
        return source.to_owned();
    };
    if entry.is_null() {
        *entry = serde_norway::Value::Mapping(serde_norway::Mapping::new());
    }
    let Some(mapping) = entry.as_mapping_mut() else {
        return source.to_owned();
    };
    if mapping.contains_key("description") {
        return source.to_owned();
    }
    mapping.insert("description".into(), description.into());
    serde_norway::to_string(&doc).unwrap_or_else(|_| source.to_owned())
}

/// `metadata.description` of a drafted manifest, in the same language map as its title, unless
/// the manifest already says something (AG-73).
fn describe(manifest: &mut Value, description: Option<&str>) {
    let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) else {
        return;
    };
    if let Some(metadata) = manifest.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata
            .entry("description")
            .or_insert_with(|| json!({ "en": description }));
    }
}

/// `raw` with its diacritics dropped and the letters Unicode does not decompose spelled out.
/// Any other non-ASCII character stays, for the caller to read as a word break.
fn fold_to_ascii(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.nfkd().filter(|c| !is_combining_mark(*c)) {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ł' | 'Ł' => out.push('l'),
            'đ' | 'Đ' => out.push('d'),
            'ø' | 'Ø' => out.push('o'),
            'æ' | 'Æ' => out.push_str("ae"),
            'œ' | 'Œ' => out.push_str("oe"),
            _ => out.push(c),
        }
    }
    out
}

/// A plural `s` to drop: an ASCII name not ending in -ss, -us or -is. A name with diacritics is
/// no English plural (Vozidlá Mercedes), and address, autobus and analysis stay whole.
fn is_english_plural(stem: &str) -> bool {
    stem.is_ascii()
        && stem.len() > 1
        && stem.ends_with('s')
        && !["ss", "us", "is"].iter().any(|end| stem.ends_with(end))
}

fn to_pascal_case(raw: &str) -> String {
    let stem = raw
        .rsplit_once('.')
        .filter(|(_, suffix)| {
            ["csv", "tsv", "json", "geojson", "xlsx"]
                .iter()
                .any(|known| suffix.eq_ignore_ascii_case(known))
        })
        .map_or(raw, |(stem, _)| stem);
    let singular = if is_english_plural(stem) {
        &stem[..stem.len() - 1]
    } else {
        stem
    };
    let mut result = String::new();
    let mut cap_next = true;
    for c in fold_to_ascii(singular).chars() {
        if c.is_ascii_alphanumeric() {
            if cap_next {
                result.extend(c.to_uppercase());
                cap_next = false;
            } else {
                result.push(c);
            }
        } else {
            cap_next = true;
        }
    }
    if result.is_empty() {
        "Entity".into()
    } else {
        result
    }
}

fn words_capitalized(raw: &str) -> String {
    let parts: Vec<&str> = raw
        .split(['-', '_', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    parts
        .iter()
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn run(
    caller: &Caller,
    state: &AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    let input: SpaceCompleteInput = ops::parse_input(val)?;

    // Determine space name
    let space_name = space_name_of(&input);

    if !is_dns1123(&space_name) {
        return Err(OpError::InvalidInput {
            path: "/space".into(),
            message: format!("'{space_name}' is not a valid DNS-1123 label"),
        });
    }

    // A name is local to its project and its ids render `{project}-{name}` (PF-84); only a
    // segment another space pins is taken, and that is said here while it is still a draft.
    let proposal = crate::spaces::propose_name(state, &caller.identity, project, &space_name);
    let space_name = if proposal.available {
        proposal.name
    } else {
        return Err(OpError::InvalidInput {
            path: "/space".into(),
            message: format!(
                "the ids of '{space_name}' would carry {}; choose another name",
                proposal
                    .reason
                    .unwrap_or_else(|| "a taken segment".to_owned()),
            ),
        });
    };

    let mut found_kinds = Vec::new();
    let mut manifests: BTreeMap<String, Value> = BTreeMap::new();
    let mut linkml_source: Option<String> = None;
    let mut bloblang_content: Option<String> = None;
    let mut sample_bytes: Option<(String, Vec<u8>)> = None;
    let mut described_source_spec: Option<(String, Value, Vec<Finding>)> = None;

    if let Some(url_str) = &input.url {
        let ds_manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": {
                "name": format!("{space_name}-source"),
                "namespace": project,
                "title": { "en": format!("{} source", words_capitalized(&space_name)) }
            },
            "spec": {
                "type": "http",
                "http": {
                    "url": url_str.trim(),
                    "verb": "GET",
                    "headers": { "Accept": "application/json" },
                    "timeout": "15s"
                }
            }
        });
        let mut ds_manifest = ds_manifest;
        describe(&mut ds_manifest, input.description.as_deref());
        manifests.insert("DataSource".into(), ds_manifest);
    }

    if let Some(files) = &input.files {
        for (n, file) in files.iter().enumerate() {
            let name = file.name.as_str();
            if name.ends_with(".yaml") || name.ends_with(".yml") {
                if name.ends_with(".linkml.yaml") {
                    linkml_source = Some(file.content.clone());
                    continue;
                }
                // The parser is not Send: read every document before anything awaits.
                let docs: Vec<Value> = serde_norway::Deserializer::from_str(&file.content)
                    .filter_map(|doc| Value::deserialize(doc).ok())
                    .collect();
                for mut val in docs {
                    // An uploaded Endpoint's slug is settled before its draft is kept: a draft's
                    // slug is one the platform minted, which a later plan of the same manifest
                    // takes as it is (EP-75).
                    crate::endpoint_slug::settle_in(
                        state,
                        project,
                        None,
                        caller.brings_its_own_slug(),
                        &mut val,
                    )
                    .await
                    .map_err(|message| OpError::InvalidInput {
                        path: format!("/files/{n}/content{}", crate::endpoint_slug::PATH),
                        message,
                    })?;
                    if let Some(kind) = val.get("kind").and_then(Value::as_str) {
                        found_kinds.push(kind.to_string());
                        manifests.insert(kind.to_string(), val);
                    }
                }
            } else if name.ends_with(".blobl") {
                bloblang_content = Some(file.content.clone());
            } else if (name.ends_with(".csv")
                || name.ends_with(".json")
                || name.ends_with(".geojson"))
                && sample_bytes.is_none()
            {
                sample_bytes = Some((name.to_string(), file.content.as_bytes().to_vec()));
            } else if name.eq_ignore_ascii_case("readme.md") || name.eq_ignore_ascii_case("readme")
            {
                if described_source_spec.is_none() {
                    described_source_spec = source_in_readme(&file.content);
                    no_credential_in(described_source_spec.as_ref(), n)?;
                }
            } else if name == ".env" || name.ends_with(".env") {
                if let Some(described) = source_in_env(&file.content, &space_name) {
                    no_credential_in(Some(&described), n)?;
                    described_source_spec = Some(described);
                }
            }
        }
    }

    let class_name = class_name_of(
        input.type_name.as_deref(),
        sample_bytes.as_ref().map(|(n, _)| n.as_str()),
        &space_name,
    );
    let description = input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned);

    // 1. DataModel
    let has_datamodel = manifests.contains_key("DataModel");
    let mut inferred_model = false;
    let mut model_verdict = None;
    let mut model_manifest = manifests.get("DataModel").cloned();
    let mut model_linkml_text = linkml_source.clone();
    // The records of the probed feed, when its sample holds an array of them (AG-79).
    let mut feed: Option<Records> = None;

    if !has_datamodel {
        let (inferred_text, infer_findings, records) = infer_linkml(
            state,
            project,
            &class_name,
            linkml_source.as_deref(),
            sample_bytes.as_ref(),
            input.url.as_deref(),
        )
        .await;
        feed = records;

        let inferred_text = match &description {
            Some(description) if !inferred_text.is_empty() => {
                describe_class(&inferred_text, &class_name, description)
            }
            _ => inferred_text,
        };
        model_linkml_text = Some(inferred_text.clone());
        let classes_array = json!([class_name]);
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "DataModel",
            "metadata": {
                "name": space_name,
                "namespace": project,
                "title": { "en": format!("{} data model", words_capitalized(&space_name)) }
            },
            "spec": {
                "contextSpaceRef": space_name,
                "linkml": format!("./{space_name}.linkml.yaml"),
                "version": "0.1.0",
                "lifecycle": "draft",
                "classes": classes_array,
                "source": inferred_text
            }
        });

        let mut manifest = manifest;
        describe(&mut manifest, description.as_deref());
        let is_ok = infer_findings.is_empty()
            && !inferred_text.is_empty()
            && inferred_text.contains("classes:");
        // Taken on the described manifest, the one the draft holds, so it is fresh for it.
        model_verdict = Some(Verdict::new(is_ok, infer_findings, None, &manifest));
        model_manifest = Some(manifest);
        inferred_model = true;
    }

    // 2. ContextSpace
    let has_space = manifests.contains_key("ContextSpace");
    let mut inferred_space = false;
    let mut space_manifest = manifests.get("ContextSpace").cloned();
    if !has_space {
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": space_name,
                "namespace": project,
                "title": { "en": words_capitalized(&space_name) }
            },
            "spec": {
                "isSandbox": false,
                "defaultLocale": "en",
                "dataModelRef": {
                    "kind": "DataModel",
                    "name": space_name
                }
            }
        });
        let mut manifest = manifest;
        describe(&mut manifest, description.as_deref());
        space_manifest = Some(manifest);
        inferred_space = true;
    }

    // 3. DataSource
    let has_datasource = manifests.contains_key("DataSource");
    let mut inferred_datasource = false;
    let mut datasource_manifest = manifests.get("DataSource").cloned();
    let mut ds_findings = Vec::new();

    if !has_datasource {
        if let Some((ds_type, spec, findings)) = described_source_spec {
            let mut ds_spec = spec;
            ds_spec["type"] = Value::String(ds_type);
            let manifest = json!({
                "apiVersion": API_VERSION,
                "kind": "DataSource",
                "metadata": {
                    "name": format!("{space_name}-source"),
                    "namespace": project,
                    "title": { "en": format!("{} source", words_capitalized(&space_name)) }
                },
                "spec": ds_spec
            });
            let mut manifest = manifest;
            describe(&mut manifest, description.as_deref());
            datasource_manifest = Some(manifest);
            ds_findings = findings;
            inferred_datasource = true;
        }
    }

    // 4. Pipeline, and the Endpoint it writes through: a dropped one, the one the mirror
    // already serves for the space, or a drafted organization-wide one (EP-02 slug).
    let has_pipeline = manifests.contains_key("Pipeline");
    let mut inferred_pipeline = false;
    let mut pipeline_manifest = manifests.get("Pipeline").cloned();
    let mut inferred_endpoint = false;
    let mut endpoint_manifest = manifests.get("Endpoint").cloned();
    // The endpoint the pipeline writes through, which a drafted map reads (AG-79).
    let mut map_endpoint: Option<String> = None;

    if !has_pipeline && datasource_manifest.is_some() {
        let ds_name = datasource_manifest
            .as_ref()
            .and_then(|m| m.pointer("/metadata/name"))
            .and_then(Value::as_str)
            .unwrap_or("source");
        let org = crate::api::assistant::org_domain(state, project);
        let mapping = bloblang_content.unwrap_or_else(|| match &feed {
            Some(records) => {
                let slots = model_linkml_text
                    .as_deref()
                    .map(|linkml| feed_shape::slot_names(linkml, &class_name))
                    .unwrap_or_default();
                records.mapping(&class_name, &slots)
            }
            None => format!(
                "root = this\nroot.id = \"urn:ngsi-ld:{class_name}:\" + env(\"JC_ORG_DOMAIN\") + \":\" + env(\"JC_SPACE\") + \":\" + (this.stationId | this.id | this.station_id | uuid_v4()).string()\nroot.type = \"{class_name}\"\n"
            ),
        });
        let endpoint_name = endpoint_manifest
            .as_ref()
            .and_then(|m| m.pointer("/metadata/name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                state
                    .mirror
                    .list(project, "Endpoint", &crate::store::ListOptions::default())
                    .items
                    .into_iter()
                    .find(|ep| {
                        ep.spec.get("contextSpaceRef").and_then(crate::resource::reference_name) == Some(&space_name)
                    })
                    .map(|ep| ep.metadata.name)
            })
            .unwrap_or_else(|| {
                let name = format!("{space_name}-all");
                endpoint_manifest = Some(json!({
                    "apiVersion": API_VERSION,
                    "kind": "Endpoint",
                    "metadata": {
                        "name": name,
                        "namespace": project,
                        "title": { "en": format!("{} context, everything", words_capitalized(&space_name)) }
                    },
                    "spec": {
                        "contextSpaceRef": space_name,
                        "slug": crate::agents::share::slug(),
                        "audience": "organization",
                        // The read Policy drafted with it names the class (`policies_for`);
                        // an Endpoint has no class list of its own (T-5109).
                        "enabledRepresentations": ["ngsi-ld"]
                    }
                }));
                inferred_endpoint = true;
                name
            });
        // A new space has no pin, so its segment is the rendered one (PF-84).
        let segment = crate::spaces::segment(&state.mirror, project, &space_name);
        let target_endpoint = format!("urn:ngsi-ld:Endpoint:{org}:{segment}:{endpoint_name}");
        map_endpoint = Some(endpoint_name);

        let spec = json!({
            "class": "auto",
            "period": "60s",
            "targetEndpoint": target_endpoint,
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": ds_name
                }
            },
            "output": {
                "type": class_name,
                "mode": "upsert"
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": mapping
            },
            "quotas": {
                "maxMemoryMb": 64,
                "cpuMillicores": 100
            }
        });
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": {
                "name": format!("{space_name}-load"),
                "namespace": project,
                "title": { "en": format!("{} pipeline", words_capitalized(&space_name)) }
            },
            "spec": spec
        });
        pipeline_manifest = Some(manifest);
        inferred_pipeline = true;
    }

    // 4a. A CSV the person uploads (DM-81, T-3840): no pipeline loads it, the person writes its
    // rows through the space, so a space with no Endpoint gets one, granted as a fed one is.
    let space_has_endpoint = state
        .mirror
        .list(project, "Endpoint", &crate::store::ListOptions::default())
        .items
        .iter()
        .any(|ep| {
            ep.spec
                .get("contextSpaceRef")
                .and_then(crate::resource::reference_name)
                == Some(&space_name)
        });
    if input.upload && sample_bytes.is_some() && endpoint_manifest.is_none() && !space_has_endpoint
    {
        endpoint_manifest = Some(json!({
            "apiVersion": API_VERSION,
            "kind": "Endpoint",
            "metadata": {
                "name": format!("{space_name}-all"),
                "namespace": project,
                "title": { "en": format!("{} context, everything", words_capitalized(&space_name)) }
            },
            "spec": {
                "contextSpaceRef": space_name,
                "slug": crate::agents::share::slug(),
                "audience": "organization",
                "enabledRepresentations": ["ngsi-ld"]
            }
        }));
        inferred_endpoint = true;
    }

    // 4b. A type an App needs and no feed fills, described by its LinkML alone (T-3608, AP-22):
    // the space gets an Endpoint the app builder reads the type's schema through, for the
    // project only. No grant is drafted: nothing loads it, and the App's own Endpoint and Policy
    // come with the App from its dataNeeds (AP-05).
    let mut app_endpoint = false;
    if pipeline_manifest.is_none()
        && endpoint_manifest.is_none()
        && datasource_manifest.is_none()
        && linkml_source.is_some()
        && !state
            .mirror
            .list(project, "Endpoint", &crate::store::ListOptions::default())
            .items
            .iter()
            .any(|ep| {
                ep.spec
                    .get("contextSpaceRef")
                    .and_then(crate::resource::reference_name)
                    == Some(&space_name)
            })
    {
        endpoint_manifest = Some(json!({
            "apiVersion": API_VERSION,
            "kind": "Endpoint",
            "metadata": {
                "name": format!("{space_name}-all"),
                "namespace": project,
                "title": { "en": format!("{} context, everything", words_capitalized(&space_name)) }
            },
            "spec": {
                "contextSpaceRef": space_name,
                "slug": crate::agents::share::slug(),
                // Only this project reads it until someone widens it (EP-14, EP-15).
                "audience": "project-list",
                "allowedProjects": [project],
                "enabledRepresentations": ["ngsi-ld"]
            }
        }));
        app_endpoint = true;
    }

    // 5. A map of the records over that endpoint, when they carry a position (AG-79): a Layer
    // and a Dashboard with one full-map page, proposed with the rest. None while the
    // installation hides dashboards (T-2874).
    let map_manifests = match (&feed, &map_endpoint, &model_linkml_text) {
        (Some(records), Some(endpoint), Some(linkml))
            if state.config.dashboards
                && records.position().is_some()
                && !manifests.contains_key("Dashboard")
                && !manifests.contains_key("Layer") =>
        {
            map_of(state, project, &space_name, &class_name, endpoint, linkml)
        }
        _ => Vec::new(),
    };

    // Now compute verdicts and store drafts
    let mut drafts = Vec::new();
    let store = state.drafts.clone();

    // Save DataModel draft
    if let Some(m) = model_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or(&space_name);
        let stored = put_draft(caller, state, project, "DataModel", name, m.clone()).await?;
        let verdict = if let Some(v) = model_verdict {
            store
                .set_verdict(project, "DataModel", name, v)
                .await
                .map_err(ops::drafts::draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "DataModel".into(),
            name: name.into(),
            inferred: inferred_model,
            manifest: m,
            verdict,
        });
    }

    // Save ContextSpace draft
    if let Some(m) = space_manifest {
        let checked = checked_draft(
            caller,
            state,
            project,
            m,
            inferred_space,
            bundle_of(&drafts),
        )
        .await?;
        drafts.push(checked);
    }

    // Save DataSource draft
    if let Some(m) = datasource_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("source");
        let stored = put_draft(caller, state, project, "DataSource", name, m.clone()).await?;

        let v = Some(
            datasource_verdict(caller, state, project, &m, input.url.is_some(), ds_findings).await,
        );
        let final_v = if let Some(verdict) = v {
            store
                .set_verdict(project, "DataSource", name, verdict)
                .await
                .map_err(ops::drafts::draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "DataSource".into(),
            name: name.into(),
            inferred: inferred_datasource,
            manifest: m,
            verdict: final_v,
        });
    }

    // Save Endpoint draft
    if let Some(m) = endpoint_manifest {
        // A new endpoint on a new space is a door nobody may pass yet: without a Policy the
        // pipeline's upsert is refused and the space answers no read tool, so the space the
        // agent just made holds nothing and shows nothing (T-0908). Two grants, no more: the
        // service account that loads it writes, and the endpoint's own audience reads.
        if inferred_endpoint {
            let audience = m
                .pointer("/spec/audience")
                .and_then(Value::as_str)
                .unwrap_or("organization")
                .to_owned();
            let name = m
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .unwrap_or("endpoint")
                .to_owned();
            for policy in policies_for(
                state,
                project,
                &space_name,
                &name,
                &class_name,
                &audience,
                &caller.identity.subject,
            ) {
                // The Endpoint below binds these grants in the same Change (PL-111).
                let mut bundle = bundle_of(&drafts);
                bundle.push(m.clone());
                let checked = checked_draft(caller, state, project, policy, true, bundle).await?;
                drafts.push(checked);
            }
        }
        let checked = checked_draft(
            caller,
            state,
            project,
            m,
            inferred_endpoint || app_endpoint,
            bundle_of(&drafts),
        )
        .await?;
        drafts.push(checked);
    }
    if input.upload && sample_bytes.is_some() {
        let grant = upload_policy(project, &space_name, &class_name, &caller.identity.subject);
        let checked =
            checked_draft(caller, state, project, grant, true, bundle_of(&drafts)).await?;
        drafts.push(checked);
    }

    // Save Pipeline draft
    if let Some(m) = pipeline_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("pipeline");
        let stored = put_draft(caller, state, project, "Pipeline", name, m.clone()).await?;

        let p_verdict = pipeline_verdict(
            caller,
            state,
            project,
            &m,
            sample_bytes.as_ref(),
            input.url.as_deref(),
        )
        .await;

        let final_v = if let Some(verdict) = p_verdict {
            store
                .set_verdict(project, "Pipeline", name, verdict)
                .await
                .map_err(ops::drafts::draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "Pipeline".into(),
            name: name.into(),
            inferred: inferred_pipeline,
            manifest: m,
            verdict: final_v,
        });
    }

    for m in map_manifests {
        let checked = checked_draft(caller, state, project, m, true, bundle_of(&drafts)).await?;
        drafts.push(checked);
    }

    let lane = riskiest_lane(&drafts);

    // One at a time per space: a retry waits for the proposal it repeats, then finds it open.
    let forge = state.forge_for(project);
    let _one_proposal = match forge.as_ref().filter(|_| input.propose) {
        Some(gitea) => Some(crate::api::mutate::turn(gitea, &change_branch(&space_name)).await),
        None => None,
    };
    let standing = standing_of(
        state,
        project,
        &space_name,
        (model_linkml_text.as_deref(), input.upload),
        (lane, drafts.len(), input.propose),
    )
    .await?;
    let already_live = matches!(standing, Standing::Live);

    let propose_ready = !already_live
        && !drafts.is_empty()
        && drafts.iter().all(|d| {
            d.verdict
                .as_ref()
                .is_some_and(|v| v.is_fresh_for(&d.manifest))
        });

    let change = if let Standing::Open(open) = standing {
        Some(*open)
    } else if input.propose && !already_live {
        if !propose_ready {
            let failing_check = failing_check(&drafts);
            // The draft and the field a person opens in Drafts to fix it (T-5109); never the
            // finding's message, which can quote a value typed into the manifest (T-2238).
            let (draft, at) = not_ready(&drafts);
            return Err(OpError::Conflict(json!({
                "error": "verdict_required",
                "check": failing_check,
                "reason": "propose_not_ready",
                "draft": draft,
                "detail": format!(
                    "The {} draft '{}' has not passed its check{}; open it in Drafts, fix it, then propose again.",
                    draft["kind"].as_str().unwrap_or("resource"),
                    draft["name"].as_str().unwrap_or_default(),
                    at.map(|path| format!(" (at {path})")).unwrap_or_default(),
                )
            })));
        }

        Some(
            propose_drafts(
                caller,
                state,
                project,
                &space_name,
                &drafts,
                model_linkml_text,
                lane,
            )
            .await?,
        )
    } else {
        None
    };

    let out = SpaceCompleteOutput {
        space: space_name,
        found: found_kinds,
        drafts,
        propose_ready,
        lane,
        change,
        already_live,
    };

    Ok(serde_json::to_value(out)?)
}

/// The branch name a space's creating Change starts with, its digest after the dash.
fn change_branch(space: &str) -> String {
    format!("portal/create-contextspace-{space}")
}

/// Whether `branch` is the creating Change of `space` and of no longer name that starts with it.
fn creates_space(branch: &str, space: &str) -> bool {
    branch
        .strip_prefix(&change_branch(space))
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split('_').next())
        .is_some_and(|hash| hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// What already stands for this model in this space: the open Change that holds it, or the space
/// that serves it with the grant its rows need. Compared by the model's text, because the rest
/// of a bundle (a minted slug, a version) differs from one run to the next. A *different* model
/// while a Change is open is refused when proposed: the approver reads one Change per space.
async fn standing_of(
    state: &AppState,
    project: &str,
    space: &str,
    (model, upload): (Option<&str>, bool),
    (lane, files, propose): (Lane, usize, bool),
) -> Result<Standing, OpError> {
    let (Some(model), Some(gitea)) = (model, state.forge_for(project)) else {
        return Ok(Standing::Nothing);
    };
    let path = format!("projects/{project}/spaces/{space}/datamodels/{space}.linkml.yaml");
    let holds = |file: Option<crate::git::RepoFile>| file.is_some_and(|f| f.content == model);
    let open = gitea
        .list_pull_requests("open")
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .find(|pull| creates_space(&pull.head_branch, space));
    if let Some(pull) = open {
        let held = gitea
            .get_file(&path, &pull.head_branch)
            .await
            .map_err(ApiError::from)?;
        if holds(held) {
            return Ok(Standing::Open(Box::new(
                crate::api::import::waiting_change(
                    state,
                    &gitea,
                    project,
                    lane,
                    crate::change::PlanSummary::new(files, 0, 0),
                    pull,
                ),
            )));
        }
        if propose {
            return Err(OpError::Api(ApiError::Conflict(format!(
                "a change is already open on this space: chg-{:08x}; approve or reject it first",
                pull.number
            ))));
        }
        return Ok(Standing::Nothing);
    }
    let serves = state.mirror.get(project, "ContextSpace", space).is_some()
        && (!upload
            || state
                .mirror
                .get(project, "Policy", &format!("{space}-upload"))
                .is_some());
    if !serves {
        return Ok(Standing::Nothing);
    }
    let main = gitea.default_branch().await.map_err(ApiError::from)?;
    let held = gitea.get_file(&path, &main).await.map_err(ApiError::from)?;
    Ok(if holds(held) {
        Standing::Live
    } else {
        Standing::Nothing
    })
}

/// The LinkML of the space's model: the file the person gave, else inferred from the sample
/// they uploaded, else from one probe of their URL on the project's runner (MF-39). The records
/// the sample or the feed holds come back too, so a map can be drafted over them (AG-79).
/// Nothing to infer from is a finding, never an empty model passed off as one.
async fn infer_linkml(
    state: &AppState,
    project: &str,
    class_name: &str,
    linkml_source: Option<&str>,
    sample_bytes: Option<&(String, Vec<u8>)>,
    url: Option<&str>,
) -> (String, Vec<Finding>, Option<Records>) {
    let mut feed: Option<Records> = None;
    let (text, findings) = if let Some(text) = linkml_source.map(str::to_owned) {
        (text, Vec::new())
    } else if let Some((name, bytes)) = sample_bytes {
        let fmt = if name.ends_with(".csv") {
            Some("csv")
        } else if name.ends_with(".json") {
            Some("json")
        } else {
            None
        };
        // A JSON sample's model is its records', like a feed's (AG-79).
        if fmt == Some("json") {
            feed = serde_json::from_slice::<Value>(bytes)
                .ok()
                .as_ref()
                .and_then(feed_shape::records);
        }
        let records = feed
            .as_ref()
            .map(|records| records.inference_sample().to_string().into_bytes());
        let bytes = records.as_deref().unwrap_or(bytes);
        match model_tools::infer_schema_from_bytes(state, Some(class_name), bytes, fmt).await {
            Ok(resp) => {
                let text = resp
                    .get("linkml")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                (text, Vec::new())
            }
            Err(err) => {
                let err_msg = err.to_string();
                (
                    String::new(),
                    vec![Finding {
                        level: Level::Error,
                        path: "linkml".into(),
                        message: err_msg,
                    }],
                )
            }
        }
    } else if let Some(url_str) = url {
        // The person's URL is probed the way the data source Check probes it: one fetch on
        // the project's runner (MF-39), never from the Portal's own network.
        let spec = json!({ "type": "http", "http": { "url": url_str } });
        match crate::api::pipeline_test::probe_source(state, project, &spec).await {
            Some(probe) if probe.skipped.is_none() && probe.sample.is_some() => {
                // The model is the records', not the envelope's around them (AG-79).
                let whole = probe.sample.unwrap_or(Value::Null);
                feed = feed_shape::records(&whole);
                let sample = feed
                    .as_ref()
                    .map_or(whole, Records::inference_sample)
                    .to_string();
                match model_tools::infer_schema_from_bytes(
                    state,
                    Some(class_name),
                    sample.as_bytes(),
                    Some("json"),
                )
                .await
                {
                    Ok(res) => (
                        res.get("linkml")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        Vec::new(),
                    ),
                    Err(e) => (
                        String::new(),
                        vec![Finding {
                            level: Level::Error,
                            path: "linkml".into(),
                            message: e.to_string(),
                        }],
                    ),
                }
            }
            Some(probe) => (
                String::new(),
                vec![Finding {
                    level: Level::Error,
                    path: "url".into(),
                    message: probe
                        .skipped
                        .unwrap_or_else(|| "the feed answered no sample".to_string()),
                }],
            ),
            None => (
                String::new(),
                vec![Finding {
                    level: Level::Error,
                    path: "url".into(),
                    message: "the URL is not an http(s) URL".to_string(),
                }],
            ),
        }
    } else {
        (
            String::new(),
            vec![Finding {
                level: Level::Error,
                path: "linkml".into(),
                message: "no sample file or url to infer schema from".into(),
            }],
        )
    };
    (text, findings, feed)
}

/// The check of a drafted DataSource: a probe of the feed when it came as a URL, a dry run
/// otherwise. What the drafting found is kept beside what the check found.
async fn datasource_verdict(
    caller: &Caller,
    state: &AppState,
    project: &str,
    m: &Value,
    from_url: bool,
    ds_findings: Vec<Finding>,
) -> Verdict {
    let check = if from_url {
        "jc_datasource_check"
    } else {
        "jc_manifest_dry_run"
    };
    let check_res = ops::call(
        ops::find(check).unwrap(),
        caller,
        state,
        project,
        json!({ "manifest": m }),
    )
    .await;
    match check_res {
        Ok(val) => {
            let mut verdict: Verdict =
                serde_json::from_value(val.get("verdict").cloned().unwrap_or(Value::Null))
                    .unwrap_or_else(|_| Verdict::green(m, None));
            verdict.findings.extend(ds_findings);
            verdict
        }
        Err(e) => {
            let mut findings = ds_findings;
            findings.push(Finding {
                level: Level::Error,
                path: "".into(),
                message: e.to_string(),
            });
            Verdict::red(m, findings, None)
        }
    }
}

fn sample_format(filename: &str) -> SampleFormat {
    if filename.ends_with(".csv") {
        SampleFormat::Csv
    } else if filename.ends_with(".json") {
        SampleFormat::Json
    } else {
        SampleFormat::Text
    }
}

fn red(m: &Value, message: String) -> Verdict {
    Verdict::red(
        m,
        vec![Finding {
            level: Level::Error,
            path: "".into(),
            message,
        }],
        None,
    )
}

/// The test of a drafted Pipeline on the uploaded sample, else on the feed's URL; with neither
/// there is nothing to test the mapping on, which is a red verdict and not a pass.
async fn pipeline_verdict(
    caller: &Caller,
    state: &AppState,
    project: &str,
    m: &Value,
    sample_bytes: Option<&(String, Vec<u8>)>,
    url: Option<&str>,
) -> Option<Verdict> {
    let sample = match (sample_bytes, url) {
        (Some((filename, bytes)), _) => Sample {
            text: Some(String::from_utf8_lossy(bytes).to_string()),
            url: None,
            format: sample_format(filename),
            records: None,
        },
        (None, Some(url)) => Sample {
            text: None,
            url: Some(url.to_owned()),
            format: SampleFormat::Json,
            records: None,
        },
        (None, None) => return Some(red(m, "no sample to test the mapping on".into())),
    };
    let test_res = ops::call(
        ops::find("jc_pipeline_test").unwrap(),
        caller,
        state,
        project,
        json!({ "pipeline": m, "sample": sample }),
    )
    .await;
    match test_res {
        Ok(val) => {
            serde_json::from_value::<Verdict>(val.get("verdict").cloned().unwrap_or(Value::Null))
                .ok()
        }
        Err(e) => Some(red(m, e.to_string())),
    }
}

/// The check to run again for the first draft whose verdict is missing or stale.
/// The first draft that blocks the proposal, as `{kind, name}`, and the path of its first error.
fn not_ready(drafts: &[CompletedDraft]) -> (Value, Option<String>) {
    let Some(draft) = drafts.iter().find(|d| {
        d.verdict
            .as_ref()
            .is_none_or(|v| !v.is_fresh_for(&d.manifest))
    }) else {
        return (Value::Null, None);
    };
    let at = draft
        .verdict
        .as_ref()
        .and_then(|v| v.findings.iter().find(|f| f.level == Level::Error))
        .map(|f| f.path.clone())
        .filter(|path| !path.is_empty());
    (json!({ "kind": draft.kind, "name": draft.name }), at)
}

fn failing_check(drafts: &[CompletedDraft]) -> &'static str {
    drafts
        .iter()
        .find(|d| {
            d.verdict
                .as_ref()
                .is_none_or(|v| !v.is_fresh_for(&d.manifest))
        })
        .map(|d| super::check_for_kind(&d.kind))
        .unwrap_or("jc_manifest_dry_run")
}

/// The space's drafts, committed as one Change with the model's LinkML beside them.
async fn propose_drafts(
    caller: &Caller,
    state: &AppState,
    project: &str,
    space_name: &str,
    drafts: &[CompletedDraft],
    model_linkml_text: Option<String>,
    lane: Lane,
) -> Result<Change, OpError> {
    // Commit bundle
    let mut bundle_files = Vec::new();
    let mut created = Vec::new();

    for d in drafts {
        let mut committed = committed(&d.manifest);
        let info = resource::by_kind(&d.kind).ok_or_else(|| {
            OpError::Api(ApiError::BadRequest(format!("unknown kind '{}'", d.kind)))
        })?;
        let path = resource::repository_path(info, project, Some(space_name), &d.name)
            .map_err(ApiError::BadRequest)?;
        if let (true, Some(source)) = (d.kind == "DataModel", model_linkml_text.as_deref()) {
            let folder = path.rsplit_once('/').map(|(f, _)| f).unwrap_or_default();
            bundle_files.extend(
                compiled_model(caller, state, project, &d.name, &mut committed, source)
                    .await?
                    .into_iter()
                    .map(|(file, content)| (format!("{folder}/{file}"), content)),
            );
        }
        let yaml_text =
            serde_norway::to_string(&committed).map_err(|e| ApiError::Internal(e.to_string()))?;
        bundle_files.push((path, yaml_text));
        created.push(d.name.clone());
    }

    if let Some(linkml) = model_linkml_text {
        let linkml_path =
            format!("projects/{project}/spaces/{space_name}/datamodels/{space_name}.linkml.yaml");
        bundle_files.push((linkml_path, linkml));
    }

    let report = ImportReport {
        created,
        replaced: Vec::new(),
        skipped: Vec::new(),
        renamed: BTreeMap::new(),
        reassigned: BTreeMap::new(),
        native_files: if drafts.iter().any(|d| d.kind == "DataModel") {
            1
        } else {
            0
        },
        lane,
        source: Some("jc_space_complete".into()),
        verified: Vec::new(),
        needs: Vec::new(),
    };
    Ok(crate::api::import::propose_bundle(
        state,
        &caller.identity,
        project,
        report,
        bundle_files,
        Some(("ContextSpace", space_name)),
    )
    .await?)
}

/// The drafted model checked and compiled the way the model editor's save does it, with
/// `spec.version` and `spec.artifacts` set on `manifest`, and its artifact files (DM-02, JC1-06):
/// a source Model Tools refuses is refused here too, so no model lands without its artifacts.
async fn compiled_model(
    caller: &Caller,
    state: &AppState,
    project: &str,
    name: &str,
    manifest: &mut Value,
    source: &str,
) -> Result<Vec<(String, String)>, OpError> {
    let version = manifest["spec"]["version"].as_str().map(str::to_owned);
    let (checked, _, _) = crate::api::datamodels::check_source(
        state,
        &caller.identity,
        project,
        name,
        &manifest["spec"],
        source,
        version.as_deref(),
    )
    .await?;
    let version = jc_core::kinds::SemVer::new(&checked.version)
        .map_err(|e| ApiError::BadRequest(format!("invalid version: {e}")))?;
    Ok(crate::api::datamodels::place_artifacts(
        &mut manifest["spec"],
        name,
        &version,
        &checked.artifacts,
    )?)
}

/// A source described by its address, read the way a `GET` of JSON is.
fn http_source(url: &str) -> (String, Value, Vec<Finding>) {
    (
        "http".into(),
        json!({
            "type": "http",
            "http": {
                "url": url,
                "verb": "GET",
                "headers": { "Accept": "application/json" },
                "timeout": "15s"
            }
        }),
        Vec::new(),
    )
}

/// A source address read out of file `n` goes into the drafted DataSource and so into Git: one
/// carrying a credential (`user:pass@`, `?apikey=`) is refused, as the operations door refuses
/// it in an argument (F-R2POR4-001). The message never repeats the address.
fn no_credential_in(
    described: Option<&(String, Value, Vec<Finding>)>,
    n: usize,
) -> Result<(), OpError> {
    let address = described.and_then(|(_, spec, _)| spec.pointer("/http/url")?.as_str());
    if address.is_some_and(crate::secret_scan::secret_text) {
        return Err(OpError::InvalidInput {
            path: format!("/files/{n}/content"),
            message: "the source address in this file carries a user name and password or a key; \
                      write it without, and keep the credential as a secret the DataSource names"
                .into(),
        });
    }
    Ok(())
}

/// The first http(s) address a README names, as the source it describes.
fn source_in_readme(content: &str) -> Option<(String, Value, Vec<Finding>)> {
    content
        .split_whitespace()
        .map(|word| word.trim_matches(|c| matches!(c, '(' | ')' | '<' | '>' | '"' | '\'')))
        .find(|word| word.starts_with("http://") || word.starts_with("https://"))
        .map(http_source)
}

/// The source a `.env` file describes: `URL`, or an MQTT host and topic, or Kafka brokers and a
/// topic. A password, secret or token in it becomes a `secretRef` to fill in, never a value.
fn source_in_env(content: &str, space_name: &str) -> Option<(String, Value, Vec<Finding>)> {
    let mut env_map = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            env_map.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    if let Some(url) = env_map.get("URL") {
        return Some(http_source(url));
    }
    if let (Some(host), Some(topic)) = (env_map.get("MQTT_HOST"), env_map.get("MQTT_TOPIC")) {
        let mut findings = Vec::new();
        let mut mqtt_spec = json!({ "urls": [host], "topics": [topic] });
        for k in env_map.keys() {
            if k.ends_with("_PASSWORD") || k.ends_with("_SECRET") || k.ends_with("_TOKEN") {
                let secret_name =
                    format!("{space_name}-{}", k.to_ascii_lowercase().replace('_', "-"));
                mqtt_spec["password"] = json!({ "secretRef": { "name": secret_name, "key": k } });
                findings.push(Finding {
                    level: Level::Warning,
                    path: "spec.mqtt.password".to_string(),
                    message: format!("fill in the secret '{secret_name}' with key '{k}'"),
                });
            }
        }
        return Some((
            "mqtt".into(),
            json!({ "type": "mqtt", "mqtt": mqtt_spec }),
            findings,
        ));
    }
    if let (Some(brokers), Some(topic)) = (env_map.get("KAFKA_BROKERS"), env_map.get("KAFKA_TOPIC"))
    {
        let broker_list: Vec<&str> = brokers.split(',').map(str::trim).collect();
        let kafka_spec = json!({ "brokers": broker_list, "topics": [topic] });
        return Some((
            "kafka".into(),
            json!({ "type": "kafka", "kafka": kafka_spec }),
            Vec::new(),
        ));
    }
    None
}

/// The space's name as asked, or taken from the feed's last path segment or the first file.
fn space_name_of(input: &SpaceCompleteInput) -> String {
    if let Some(s) = input.space.as_deref().filter(|s| !s.trim().is_empty()) {
        sanitize_dns1123(s)
    } else if let Some(url_str) = input.url.as_deref().filter(|u| !u.trim().is_empty()) {
        let seg = url_str
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .unwrap_or("space");
        let stem = seg
            .strip_suffix(".json")
            .or_else(|| seg.strip_suffix(".geojson"))
            .unwrap_or(seg);
        sanitize_dns1123(stem)
    } else if let Some(files) = &input.files {
        let first_name = files.first().map(|f| f.name.as_str()).unwrap_or("space");
        let seg = if let Some((dir, _)) = first_name.split_once('/') {
            dir
        } else {
            first_name
                .strip_suffix(".csv")
                .or_else(|| first_name.strip_suffix(".json"))
                .or_else(|| first_name.strip_suffix(".geojson"))
                .unwrap_or(first_name)
        };
        sanitize_dns1123(seg)
    } else {
        "space".into()
    }
}

/// The Layer and the Dashboard that draw the records over `endpoint`, or none when the project
/// already has either name: circles coloured by the first measure of the model, the name and that
/// measure in the popup, `project` visibility (UI-19).
/// The two grants a space the agent just made needs to be loaded and read (T-0908, PF-31):
/// the write for the account the runner authenticates as, taken from a write Policy the project
/// already has so a project that renamed it keeps working, and the reads for the audience the
/// endpoint declares — the same mapping `propose_endpoint` uses (EP-72).
fn policies_for(
    state: &AppState,
    project: &str,
    space: &str,
    endpoint: &str,
    class: &str,
    audience: &str,
    owner: &str,
) -> Vec<Value> {
    let (writer, account) = writer_of(state, project, space, owner);
    // Whom the audience admits, as `propose_endpoint` grants it (T-6464): the organization's
    // domain names no group, so a grant to it matched nobody and the assistant read no row.
    let project_group = json!({ "kind": "group", "id": project });
    let readers = match audience {
        "public" => vec![("read", json!({ "kind": "role", "id": "public" }))],
        "organization" => vec![
            ("read", json!({ "kind": "organization" })),
            // A person of the project's root group carries no `jc_org` (T-6408).
            ("project-read", project_group),
        ],
        _ => vec![("read", project_group)],
    };
    let policy = |name: String, title: String, assignee: Value, operations: Value, typed: bool| {
        let mut spec = json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": space },
            "assigner": "did:web:{orgDomain}",
            "assignee": assignee,
            "operations": operations,
        });
        if typed {
            // A read grant that names no type matches nothing on a read (PL-45).
            spec["information"] = json!([{ "entities": [{ "type": class }] }]);
        }
        json!({
            "apiVersion": API_VERSION,
            "kind": "Policy",
            "metadata": { "name": name, "namespace": project, "title": { "en": title } },
            "spec": spec,
        })
    };
    let mut drafted = Vec::new();
    // The account the grant names comes first, so a person reading the card sees what is being
    // granted to before the grant.
    drafted.extend(account);
    drafted.push(policy(
        format!("{space}-pipelines-write"),
        format!("The pipeline writes {space}"),
        writer,
        json!(["upsertBatch", "createBatch", "queryBatch"]),
        false,
    ));
    for (suffix, reader) in readers {
        drafted.push(policy(
            format!("{endpoint}-{suffix}"),
            format!("Read {space} through {endpoint}"),
            reader,
            json!(["retrieveOps"]),
            true,
        ));
    }
    drafted
}

/// The person's grant to write the uploaded type into the space by batch (DM-81, T-3840): upsert,
/// so the same file uploaded again updates the same entities, and the read the upsert needs.
/// `person` is the uploader's Keycloak user id; no name or address goes into Git (ADR-N-061).
fn upload_policy(project: &str, space: &str, class: &str, person: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Policy",
        "metadata": {
            "name": format!("{space}-upload"),
            "namespace": project,
            "title": { "en": format!("The uploader writes {class} into {space}") }
        },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": space },
            "assigner": "did:web:{orgDomain}",
            "assignee": { "kind": "user", "id": person },
            "operations": ["upsertBatch", "createBatch", "queryBatch"],
            "information": [{ "entities": [{ "type": class }] }]
        }
    })
}

/// The account a write grant is for, and the `ServiceAccount` to draft beside it when the project
/// has none (PF-39, T-1494).
///
/// The assignee of a write Policy the project already has, so a project that renamed its account
/// keeps working; otherwise the account named `pipelines`, when it is there. When it is neither, the
/// grant used to name `pipelines` regardless: no such identity existed, so the pipeline's upsert was
/// refused at admission far from the cause, and an account created later under that name inherited a
/// grant written for another purpose. A grant names something that exists — in the project or in the
/// same set of drafts a person approves.
fn writer_of(state: &AppState, project: &str, space: &str, owner: &str) -> (Value, Option<Value>) {
    const ACCOUNT: &str = "pipelines";
    let assignee = |id: &str| json!({ "kind": "serviceAccount", "id": id });
    let from_policy = state
        .mirror
        .list(project, "Policy", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .find(|p| {
            p.spec
                .get("operations")
                .and_then(Value::as_array)
                .is_some_and(|ops| ops.iter().any(|op| op == "upsertBatch"))
        })
        .map(|p| p.spec.get("assignee").cloned().unwrap_or(Value::Null))
        .filter(|assignee| assignee.is_object());
    if let Some(assignee) = from_policy {
        return (assignee, None);
    }
    if state
        .mirror
        .get(project, "ServiceAccount", ACCOUNT)
        .is_some()
    {
        return (assignee(ACCOUNT), None);
    }
    (
        assignee(ACCOUNT),
        Some(json!({
            "apiVersion": API_VERSION,
            "kind": "ServiceAccount",
            "metadata": {
                "name": ACCOUNT,
                "namespace": project,
                "title": { "en": "The pipelines of this project" }
            },
            "spec": {
                "owner": { "user": owner },
                "purpose": "The runner authenticates as this identity to write what the pipelines \
                            of this project load",
                // One OAuth client, and the writer's role in this space alone, as the seeded
                // accounts hold it: what it may write is the Policy beside it (PF-31, PF-39).
                // A ServiceAccount without a role is refused (T-5109).
                "credentials": [{ "kind": "oauth-client", "name": "main" }],
                "roles": [{ "role": "space-writer", "scope": { "contextSpace": space } }]
            }
        })),
    )
}

fn map_of(
    state: &AppState,
    project: &str,
    space: &str,
    class: &str,
    endpoint: &str,
    linkml: &str,
) -> Vec<Value> {
    let layer = format!("{space}-map");
    let taken = |kind: &str, name: &str| state.mirror.get(project, kind, name).is_some();
    if taken("Layer", &layer) || taken("Dashboard", space) {
        return Vec::new();
    }
    let colour = feed_shape::colour_slot(linkml, class);
    let mut spec = json!({ "sourceEndpointRef": endpoint, "entityType": class, "style": "circle" });
    if let Some(slot) = &colour {
        spec["colorBy"] = json!({ "property": slot });
    }
    let popup: Vec<String> = feed_shape::has_slot(linkml, class, "name")
        .then(|| "name".to_owned())
        .into_iter()
        .chain(colour)
        .collect();
    if !popup.is_empty() {
        spec["popupProperties"] = json!(popup);
    }
    let words = words_capitalized(space);
    vec![
        json!({
            "apiVersion": API_VERSION,
            "kind": "Layer",
            "metadata": { "name": layer, "namespace": project, "title": { "en": format!("{words} on the map") } },
            "spec": spec
        }),
        json!({
            "apiVersion": API_VERSION,
            "kind": "Dashboard",
            "metadata": { "name": space, "namespace": project, "title": { "en": format!("{words} map") } },
            "spec": {
                "title": format!("{words} map"),
                "visibility": "project",
                "pages": [{ "title": words, "layout": "full-map", "layers": [layer] }]
            }
        }),
    ]
}

/// The manifests of the drafts so far, which the next draft is proposed with.
fn bundle_of(drafts: &[CompletedDraft]) -> Vec<Value> {
    drafts.iter().map(|d| d.manifest.clone()).collect()
}

/// One draft kept with the verdict of `jc_manifest_dry_run` on it; `inferred` when the completion
/// drafted it rather than found it. `bundle` holds the drafts proposed with it as one Change.
/// Stores `manifest` as the project's draft of `kind/name` for this caller: propose judged on
/// the manifest itself, and another person's draft of the same name is a conflict, never
/// overwritten (F-POR3-012).
async fn put_draft(
    caller: &Caller,
    state: &AppState,
    project: &str,
    kind: &str,
    name: &str,
    manifest: Value,
) -> Result<ops::drafts::Draft, OpError> {
    crate::permissions::for_request(state, &caller.identity, project).check(
        kind,
        Verb::Propose,
        Some(&manifest),
    )?;
    let store = state.drafts.clone();
    let existing = store
        .get(project, kind, name)
        .await
        .map_err(ops::drafts::draft_error)?;
    if let Some(theirs) = existing
        .as_ref()
        .filter(|draft| draft.touched_by != caller.identity.username)
    {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "{kind} '{name}' already has a draft {} is working on; finish or drop it first, or name this one differently",
            theirs.touched_by
        ))));
    }
    store
        .put(
            project,
            kind,
            name,
            manifest,
            Some(existing.map_or(0, |draft| draft.version)),
            &caller.identity.username,
            caller.via.touched_kind(),
        )
        .await
        .map_err(ops::drafts::draft_error)
}

async fn checked_draft(
    caller: &Caller,
    state: &AppState,
    project: &str,
    manifest: Value,
    inferred: bool,
    bundle: Vec<Value>,
) -> Result<CompletedDraft, OpError> {
    let kind = manifest["kind"].as_str().unwrap_or_default().to_owned();
    let name = manifest["metadata"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let store = state.drafts.clone();
    let stored = put_draft(caller, state, project, &kind, &name, manifest.clone()).await?;
    // The bundle is proposed with this draft as one Change, so a reference to one of its drafts
    // (the space naming its new model, the Endpoint its new space) resolves (T-5109).
    let check = ops::call(
        ops::find("jc_manifest_dry_run").expect("jc_manifest_dry_run is registered"),
        caller,
        state,
        project,
        json!({ "manifest": manifest }),
    );
    let verdict = match crate::references::BUNDLE.scope(bundle, check).await {
        Ok(val) => {
            serde_json::from_value::<Verdict>(val.get("verdict").cloned().unwrap_or(Value::Null))
                .ok()
        }
        Err(e) => Some(Verdict::red(
            &manifest,
            vec![Finding {
                level: Level::Error,
                path: "".into(),
                message: e.to_string(),
            }],
            None,
        )),
    };
    let verdict = match verdict {
        Some(verdict) => {
            store
                .set_verdict(project, &kind, &name, verdict)
                .await
                .map_err(ops::drafts::draft_error)?
                .verdict
        }
        None => stored.verdict,
    };
    Ok(CompletedDraft {
        kind,
        name,
        inferred,
        manifest,
        verdict,
    })
}

/// The manifest as the repository takes it: no `status`, and no inline `spec.source` on a
/// DataModel (the draft carries the LinkML text for the editor; the repository gets it as the
/// `spec.linkml` file beside the manifest, which is what the kind validates, DM-56).
fn committed(manifest: &Value) -> Value {
    let mut committed = manifest.clone();
    if let Some(obj) = committed.as_object_mut() {
        obj.remove("status");
    }
    if committed["kind"] == "DataModel" {
        if let Some(spec) = committed["spec"].as_object_mut() {
            spec.remove("source");
        }
    }
    committed
}

fn riskiest_lane(drafts: &[CompletedDraft]) -> Lane {
    let mut lane = Lane::Green;
    for d in drafts {
        let l = match d.kind.as_str() {
            "DataModel" | "ContextSpace" | "DataSource" => Lane::Yellow,
            "Pipeline" => Lane::Green,
            // A public Endpoint is the Red lane (CC-19); an organization-wide one is Yellow.
            "Endpoint" if d.manifest["spec"]["audience"] == "public" => Lane::Red,
            _ => Lane::Yellow,
        };
        lane = match (lane, l) {
            (Lane::Red, _) | (_, Lane::Red) => Lane::Red,
            (Lane::Yellow, _) | (_, Lane::Yellow) => Lane::Yellow,
            _ => Lane::Green,
        };
    }
    lane
}

// ---------------------------------------------------------------------------------------------
// The registry's operation that completes a space (AG-62).
// ---------------------------------------------------------------------------------------------
/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![        Operation {
            name: "jc_space_complete",
            title: "Complete this space",
            description: "Opens an endpoint or folder that partly defines a space and completes LinkML, data source, and pipeline drafts (MCP clients pass the files)",
            input: input_schema,
            output: output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "ContextSpace",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: validate_input,
            run: |caller, state, project, val| {
                Box::pin(async move {
                    run(caller, state, project, val).await
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{class_name_of, describe, describe_class, is_type_name};
    use serde_json::json;

    /// T-4470: the parts `run` was cut into that read nothing but their input.
    #[test]
    fn the_space_is_named_as_asked_else_after_the_feed_else_after_the_first_file() {
        let name = |input: serde_json::Value| {
            super::space_name_of(&serde_json::from_value(input).expect("input"))
        };
        assert_eq!(
            name(json!({ "space": "Bikes Now" })),
            super::sanitize_dns1123("Bikes Now")
        );
        assert_eq!(
            name(json!({ "url": "https://x.example/api/stations.geojson?x=1" })),
            "stations"
        );
        assert_eq!(
            name(json!({ "files": [{ "name": "feed/a.csv", "content": "" }] })),
            "feed"
        );
        assert_eq!(
            name(json!({ "files": [{ "name": "rows.csv", "content": "" }] })),
            "rows"
        );
        assert_eq!(name(json!({ "space": "  " })), "space");
    }

    /// F-R2POR4-001: an address a README or a `.env` names with a credential in it is refused
    /// at that file, before anything is drafted; one without, or naming a `${VAR}`, is kept.
    #[test]
    fn a_source_address_with_a_credential_in_a_file_is_refused() {
        let user = ["sc", "ott"].concat();
        let pass = ["tig", "er7"].concat();
        let key = ["abcdef", "0123456789"].concat();
        for (readme, env) in [
            (
                format!("Data at https://{user}:{pass}@x.example/feed"),
                format!("URL=https://{user}:{pass}@x.example/feed\n"),
            ),
            (
                format!("Data at https://x.example/feed?apikey={key}"),
                format!("URL=\"https://x.example/feed?api_key={key}\"\n"),
            ),
        ] {
            let refused = super::no_credential_in(super::source_in_readme(&readme).as_ref(), 2)
                .expect_err("README");
            assert!(
                matches!(&refused, crate::ops::OpError::InvalidInput { path, message }
                    if path == "/files/2/content" && !message.contains("x.example")),
                "{refused}"
            );
            assert!(super::no_credential_in(super::source_in_env(&env, "b").as_ref(), 0).is_err());
        }
        for kept in [
            "https://x.example/feed",
            "https://x.example/feed?key=${FEED_KEY}",
        ] {
            assert!(super::no_credential_in(super::source_in_readme(kept).as_ref(), 0).is_ok());
        }
        assert!(super::no_credential_in(None, 0).is_ok());
    }

    #[test]
    fn a_readme_or_an_env_describes_a_source_and_a_secret_stays_a_reference() {
        let (kind, spec, _) =
            super::source_in_readme("Data at (https://x.example/feed) daily").expect("a source");
        assert_eq!(
            (kind.as_str(), &spec["http"]["url"]),
            ("http", &json!("https://x.example/feed"))
        );
        assert!(super::source_in_readme("no address here").is_none());

        let env =
            "# comment\nMQTT_HOST=tcp://broker:1883\nMQTT_TOPIC=t\nMQTT_PASSWORD=\"hunter2\"\n";
        let (kind, spec, findings) = super::source_in_env(env, "bikes").expect("a source");
        assert_eq!(kind, "mqtt");
        assert_eq!(
            spec["mqtt"]["password"]["secretRef"]["name"],
            json!("bikes-mqtt-password")
        );
        assert!(
            !spec.to_string().contains("hunter2"),
            "the value never reaches the manifest"
        );
        assert_eq!(findings.len(), 1);
        let (kind, spec, _) =
            super::source_in_env("KAFKA_BROKERS=a:9092, b:9092\nKAFKA_TOPIC=t", "s")
                .expect("kafka");
        assert_eq!(
            (kind.as_str(), &spec["kafka"]["brokers"]),
            ("kafka", &json!(["a:9092", "b:9092"]))
        );
        assert!(super::source_in_env("MQTT_HOST=only-a-host", "s").is_none());
    }

    #[tokio::test]
    async fn the_model_is_the_given_linkml_else_a_finding_when_there_is_nothing_to_infer_from() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let (text, findings, feed) =
            super::infer_linkml(&state, "p", "Bike", Some("classes: {}"), None, None).await;
        assert_eq!(
            (text.as_str(), findings.len(), feed.is_none()),
            ("classes: {}", 0, true)
        );
        let (text, findings, _) = super::infer_linkml(&state, "p", "Bike", None, None, None).await;
        assert!(text.is_empty());
        assert_eq!(findings[0].path, "linkml");
    }

    /// EP-75 (T-6285): an Endpoint uploaded with a slug the caller typed is refused before its
    /// draft is kept, so the draft never vouches for that slug in a later plan or dry run.
    #[tokio::test]
    async fn an_uploaded_endpoints_typed_slug_is_refused_before_it_is_drafted() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let caller = crate::ops::Caller::new(
            crate::auth::session::Identity {
                client: None,
                subject: "sub-jana".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            crate::ops::Via::Mcp,
        );
        let typed = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let endpoint = format!(
            "kind: Endpoint\nmetadata: {{name: bikes, namespace: doprava}}\n\
             spec: {{contextSpaceRef: bikes, audience: public, slug: {typed}}}\n"
        );
        let input = json!({
            "space": "bikes",
            "files": [{ "name": "endpoint.yaml", "content": endpoint }]
        });
        match super::run(&caller, &state, "doprava", input).await {
            Err(crate::ops::OpError::InvalidInput { path, message }) => {
                assert_eq!(path, "/files/0/content/spec/slug");
                assert!(!message.contains(typed), "{message}");
            }
            other => panic!("expected a refusal at the slug, got {other:?}"),
        }
        let drafted = state
            .drafts
            .get("doprava", "Endpoint", "bikes")
            .await
            .expect("the store answers");
        assert!(drafted.is_none(), "nothing drafted");
    }

    #[test]
    fn the_check_to_rerun_is_the_first_stale_drafts() {
        let draft = |kind: &str| super::CompletedDraft {
            kind: kind.into(),
            name: "n".into(),
            inferred: true,
            manifest: json!({ "kind": kind }),
            verdict: None,
        };
        assert_eq!(super::failing_check(&[]), "jc_manifest_dry_run");
        assert_eq!(
            super::failing_check(&[draft("Pipeline"), draft("DataModel")]),
            "jc_pipeline_test"
        );
        assert_eq!(
            super::failing_check(&[draft("Endpoint")]),
            "jc_manifest_dry_run"
        );
        // The refusal names the draft to open, never a finding's text (T-5109, T-2238).
        assert_eq!(super::not_ready(&[]), (serde_json::Value::Null, None));
        assert_eq!(
            super::not_ready(&[draft("Endpoint")]),
            (json!({ "kind": "Endpoint", "name": "n" }), None)
        );
        assert!(matches!(
            super::sample_format("a.csv"),
            super::SampleFormat::Csv
        ));
        assert!(matches!(
            super::sample_format("a.geojson"),
            super::SampleFormat::Text
        ));
    }

    #[test]
    fn the_class_is_the_named_type_else_the_sample_else_the_space_never_entity() {
        assert_eq!(
            class_name_of(Some("BikeStation"), None, "bikes"),
            "BikeStation"
        );
        assert_eq!(
            class_name_of(Some("bad name"), None, "helsinki-bikes"),
            "HelsinkiBike"
        );
        assert_eq!(
            class_name_of(None, Some("stations.csv"), "bikes"),
            "Station"
        );
        assert_eq!(class_name_of(None, None, "helsinki-bikes"), "HelsinkiBike");
        assert_eq!(class_name_of(None, None, "entity"), "FeedEntity");
        assert_eq!(class_name_of(None, None, "7-feeds"), "Feed7Feed");
        assert!(is_type_name("WeatherObserved"));
        assert!(!is_type_name("weatherObserved"));
        assert!(!is_type_name("Weather-Observed"));
        assert!(!is_type_name(""));
    }

    #[test]
    fn a_name_with_diacritics_folds_to_a_readable_class() {
        for (sample, class) in [
            ("Kultúrne miesta.csv", "KulturneMiesta"),
            ("Školské zariadenia", "SkolskeZariadenia"),
            ("Dětská hřiště.csv", "DetskaHriste"),
            ("Přírodní rezervace.json", "PrirodniRezervace"),
            ("Straßenbäume.csv", "Strassenbaume"),
            ("Öffnungszeiten", "Offnungszeiten"),
            ("Łódź ulice", "LodzUlice"),
        ] {
            let name = class_name_of(None, Some(sample), "bikes");
            assert_eq!(name, class);
            assert!(is_type_name(&name), "{name}");
        }
    }

    #[test]
    fn a_name_with_nothing_to_fold_falls_back_to_the_space() {
        for sample in ["北京市.csv", "Киев", "…"] {
            let name = class_name_of(None, Some(sample), "helsinki-bikes");
            assert_eq!(name, "HelsinkiBike");
        }
        let name = class_name_of(None, Some("北京市.csv"), "Киев");
        assert_eq!(name, "FeedEntity");
        assert!(is_type_name(&name), "{name}");
    }

    #[test]
    fn a_sample_name_loses_its_file_suffix_whatever_its_case() {
        for sample in [
            "kulturne-miesta.xlsx",
            "kulturne-miesta.XLSX",
            "kulturne-miesta.tsv",
            "kulturne-miesta.geojson",
        ] {
            assert_eq!(class_name_of(None, Some(sample), "bikes"), "KulturneMiesta");
        }
        assert_eq!(class_name_of(None, Some("v1.2 feed"), "bikes"), "V12Feed");
    }

    #[test]
    fn only_an_english_plural_loses_its_last_letter() {
        for (sample, class) in [
            ("stations.csv", "Station"),
            ("Autobus.csv", "Autobus"),
            ("status.json", "Status"),
            ("analysis.csv", "Analysis"),
            ("address.csv", "Address"),
            ("Vozidlá Mercedes.csv", "VozidlaMercedes"),
        ] {
            assert_eq!(class_name_of(None, Some(sample), "bikes"), class);
        }
    }

    #[test]
    fn a_class_stays_within_the_length_a_type_name_may_have() {
        let name = class_name_of(None, Some(&format!("{}ž.csv", "a".repeat(80))), "bikes");
        assert_eq!(name.len(), 63);
        assert!(is_type_name(&name), "{name}");
        let name = class_name_of(None, Some(&format!("7{}.csv", "a".repeat(80))), "bikes");
        assert!(is_type_name(&name), "{name}");
    }

    #[test]
    fn a_description_lands_on_the_inferred_class_unless_it_has_one() {
        let source = "id: https://example.com/bikes\nclasses:\n  BikeStation:\n    slots: [name]\n";
        let described = describe_class(source, "BikeStation", "Docking stations of city bikes");
        let doc: serde_norway::Value = serde_norway::from_str(&described).unwrap();
        assert_eq!(
            doc["classes"]["BikeStation"]["description"].as_str(),
            Some("Docking stations of city bikes")
        );
        assert_eq!(
            describe_class(&described, "BikeStation", "other"),
            described
        );
        assert_eq!(
            describe_class("not: [yaml", "BikeStation", "x"),
            "not: [yaml"
        );
        let mut manifest = json!({ "metadata": { "name": "bikes" } });
        describe(&mut manifest, Some("  City bikes  "));
        assert_eq!(
            manifest["metadata"]["description"],
            json!({ "en": "City bikes" })
        );
        describe(&mut manifest, Some("else"));
        assert_eq!(
            manifest["metadata"]["description"],
            json!({ "en": "City bikes" })
        );
    }

    #[test]
    fn a_committed_datamodel_keeps_its_linkml_path_and_loses_the_inline_source() {
        let draft = serde_json::json!({
            "kind": "DataModel",
            "spec": { "linkml": "./bikes.linkml.yaml", "source": "classes: {}" },
            "status": { "x": 1 }
        });
        let out = super::committed(&draft);
        assert_eq!(out["spec"]["linkml"], "./bikes.linkml.yaml");
        assert!(out["spec"].get("source").is_none());
        assert!(out.get("status").is_none());
        let space = serde_json::json!({ "kind": "ContextSpace", "spec": { "source": "kept" } });
        assert_eq!(super::committed(&space)["spec"]["source"], "kept");
    }
}
