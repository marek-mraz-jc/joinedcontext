//! One-click configuration export (T-0192, CC-49, MF-16…MF-19).
//!
//! The repository at a revision *is* the export, so this module never invents a format: it reads
//! the forge at that revision, strips what must not leave (`status`, literal secret values) and
//! hands back what a `git archive` of the same path would hold. Nothing here reads the live
//! mirror: an export names a revision, and the mirror only ever knows one.
//!
//! A whole-project export also says what its files mean (MF-41): the JSON Schema of every kind it
//! holds, every data model's LinkML source and JSON Schema, and a README that ties them together,
//! so a person or another tool can read the bundle without this platform's documentation.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::mutate::SECRET_KEYS;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::git::GiteaClient;
use crate::resource::{self, ResourceEnvelope, API_VERSION};
use crate::state::AppState;

/// Revision picker page size (`limit`), and the ceiling a caller may ask for.
const DEFAULT_REVISIONS: usize = 20;
const MAX_REVISIONS: usize = 100;

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    /// Comma-separated plurals, e.g. `endpoints,pipelines`.
    #[serde(default)]
    pub kinds: Option<String>,
    /// Comma-separated `metadata.name` values.
    #[serde(default)]
    pub names: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevisionsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionList {
    pub items: Vec<Revision>,
}

/// Drops every scalar stored under a credential key, at any depth (MF-17): a string, and a number
/// or a boolean too, which YAML reads from an unquoted `password: 123456` and which every write
/// door refuses as a literal secret all the same (`find_literal_secret`, T-2540, T-3020). `null`
/// is an absent value, and an object (a `secretRef`) or an array is walked, not dropped.
///
/// The key stays with an empty string rather than disappearing: a bundle must stay re-importable,
/// and a manifest that lost a required field would not validate on the way back in.
pub(crate) fn strip_secret_values(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                // A whole `${VAR}` is a reference, which a bundle keeps like a `secretRef` (T-3972).
                let scalar = (child.is_string()
                    && !child
                        .as_str()
                        .is_some_and(crate::api::mutate::is_variable_reference))
                    || child.is_number()
                    || child.is_boolean();
                if SECRET_KEYS.contains(&key.as_str()) && scalar {
                    *child = Value::String(String::new());
                } else {
                    strip_secret_values(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_secret_values),
        _ => {}
    }
}

/// The repository that holds `project`: its own in layout 2, the organization's otherwise
/// (CC-87). Paths stay render paths either way.
fn gitea(state: &AppState, project: &str) -> Result<std::sync::Arc<GiteaClient>, ApiError> {
    state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("no repository is configured".into()))
}

/// Whether a revision is a commit id rather than a branch name (MF-17).
pub(crate) fn is_commit(revision: &str) -> bool {
    (7..=40).contains(&revision.len())
        && revision
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A revision is put into a forge URL, so it is checked before it is used. Branch names and
/// commit shas are the whole of what a caller may name here.
fn valid_revision(revision: &str) -> bool {
    !revision.is_empty()
        && revision.len() <= 128
        && !revision.starts_with('-')
        && !revision.contains("..")
        && revision
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn selected(raw: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<String> = raw?
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    (!list.is_empty()).then_some(list)
}

/// `kinds=endpoints,pipelines` as the kind names the manifests carry. An unknown plural filters
/// everything out rather than being ignored: a filter that silently does nothing is a bug that
/// ships a bigger bundle than the caller asked for.
fn kind_filter(raw: Option<&str>) -> Option<Vec<String>> {
    selected(raw).map(|plurals| {
        plurals
            .iter()
            .map(|plural| {
                resource::by_plural(plural)
                    .map(|info| info.kind.to_string())
                    .unwrap_or_else(|| format!("unknown:{plural}"))
            })
            .collect()
    })
}

fn is_manifest_path(path: &str) -> bool {
    path.ends_with(".yaml") || path.ends_with(".yml")
}

/// One file of the project subtree, ready to be written into the answer.
struct Exported {
    path: String,
    content: String,
    manifest: Option<ResourceEnvelope>,
}

/// Reads `projects/{project}/` at one revision, sanitising every manifest it finds.
///
/// A file the forge cannot hand back as text (a binary blob committed next to the manifests) is
/// counted and left out rather than corrupted; the count is what the bundle index reports as
/// omitted, so a caller always learns that something was not included (MF-18).
/// The Endpoints of this organization by slug, as `(project, name)`: what an exported
/// SharedSpaceReference names instead of the slug (EP-77, MF-43).
fn endpoints_by_slug(state: &AppState) -> HashMap<String, (String, String)> {
    state
        .mirror
        .matching(|env| env.kind == "Endpoint")
        .into_iter()
        .filter_map(|env| {
            let slug = env.spec.get("slug")?.as_str()?.to_owned();
            Some((slug, (env.metadata.namespace?, env.metadata.name)))
        })
        .collect()
}

/// A reference to an Endpoint of this organization is written by name, which the loader
/// resolves wherever the bundle lands; one to another instance keeps its slug (EP-77, MF-43).
fn reference_by_name(spec: &mut Value, endpoints: &HashMap<String, (String, String)>) {
    let Some(object) = spec.as_object_mut() else {
        return;
    };
    let Some((project, name)) = object
        .get("endpointSlug")
        .and_then(Value::as_str)
        .and_then(|slug| endpoints.get(slug))
        .cloned()
    else {
        return;
    };
    object.remove("endpointSlug");
    object.insert(
        "endpointRef".to_owned(),
        serde_json::json!({ "project": project, "name": name }),
    );
}

async fn read_project(
    gitea: &GiteaClient,
    project: &str,
    revision: &str,
    endpoints: &HashMap<String, (String, String)>,
) -> Result<(Vec<Exported>, usize), ApiError> {
    let prefix = format!("projects/{project}/");
    let paths: Vec<String> = gitea
        .list_tree(revision)
        .await?
        .into_iter()
        .filter(|path| path.starts_with(&prefix))
        .collect();

    let mut files = Vec::new();
    let mut omitted = 0usize;
    for path in paths {
        let file = match gitea.get_file(&path, revision).await {
            Ok(Some(file)) => file,
            Ok(None) => {
                omitted += 1;
                continue;
            }
            Err(err) => {
                // Not fatal: one unreadable file must not cost the operator the whole export.
                tracing::warn!(path = %path, error = %err, "file skipped in export");
                omitted += 1;
                continue;
            }
        };

        let manifest = if is_manifest_path(&path) {
            serde_norway::from_str::<ResourceEnvelope>(&file.content).ok()
        } else {
            None
        };

        match manifest {
            Some(mut envelope) if resource::by_kind(&envelope.kind).is_some() => {
                // Status is the Portal's own computation and a secret is nobody's (MF-17).
                envelope.strip_status();
                // The digest this environment's build lane published is this environment's
                // fact: carried into another instance it would deploy an image that instance
                // never built (AP-11, AP-13a, T-0822).
                for key in crate::apps::converge::BUILT_ANNOTATIONS {
                    envelope.metadata.annotations.remove(key);
                }
                strip_secret_values(&mut envelope.spec);
                if envelope.kind == "SharedSpaceReference" {
                    reference_by_name(&mut envelope.spec, endpoints);
                }
                let content = serde_norway::to_string(&envelope).map_err(|e| {
                    ApiError::Internal(format!("manifest '{path}' did not serialise: {e}"))
                })?;
                files.push(Exported {
                    path,
                    content,
                    manifest: Some(envelope),
                });
            }
            // Native files (`bento.yaml`, LinkML, generated schema artifacts) travel as they
            // are: they are the pipeline's or the model's own format, not ours to rewrite.
            _ => files.push(Exported {
                path,
                content: file.content,
                manifest: None,
            }),
        }
    }
    Ok((files, omitted))
}

fn matches_filters(
    envelope: &ResourceEnvelope,
    kinds: Option<&Vec<String>>,
    names: Option<&Vec<String>>,
) -> bool {
    kinds.is_none_or(|wanted| wanted.contains(&envelope.kind))
        && names.is_none_or(|wanted| wanted.contains(&envelope.metadata.name))
}

/// The manifest a native file belongs to: in the same directory, and either the resource that
/// directory is (`pipelines/aq/pipeline.yaml` for `pipelines/aq/bento.yaml`) or the one the file
/// is named after (`datamodels/air.yaml` for `datamodels/air.linkml.yaml`).
fn owner<'a>(native: &str, files: &'a [Exported]) -> Option<&'a ResourceEnvelope> {
    files.iter().find_map(|candidate| {
        let envelope = candidate.manifest.as_ref()?;
        crate::api::import::owns_native(native, &candidate.path, &envelope.metadata.name)
            .then_some(envelope)
    })
}

/// The filters of an export asked of a native file: its kind is the one its directory names, and
/// it belongs to a named resource when a directory of its path is that name (`pipelines/aq/…`,
/// `apps/bikes/…`) or its file is named after it (`datamodels/air.linkml.yaml`).
fn native_matches_filters(
    path: &str,
    kinds: Option<&Vec<String>>,
    names: Option<&Vec<String>>,
) -> bool {
    let kind = crate::api::import::native_kind(path);
    let file = path.rsplit('/').next().unwrap_or_default();
    kinds.is_none_or(|wanted| kind.is_some_and(|kind| wanted.iter().any(|k| k == kind)))
        && names.is_none_or(|wanted| {
            wanted.iter().any(|name| {
                // The directories below `projects/{project}/`, never the project's own name.
                let dirs: Vec<&str> = path.split('/').skip(2).collect();
                dirs[..dirs.len().saturating_sub(1)].contains(&name.as_str())
                    || file.starts_with(&format!("{name}."))
            })
        })
}

fn pretty(value: &Value) -> Result<String, ApiError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ApiError::Internal(format!("schema did not serialise: {e}")))
}

fn attachment(body: Vec<u8>, content_type: &'static str, filename: String) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

/// The `kind: Bundle` index that makes a download re-importable (MF-17).
///
/// It is the platform's own kind, not a shape of this module's own: `jcctl validate` reads the
/// tree a person unpacks, and an index it refuses is a bundle the platform rejects as soon as
/// it is looked at (T-0823).
fn bundle_index(
    header: &IndexHeader<'_>,
    items: Vec<jc_core::kinds::BundleItem>,
    native_files: Vec<String>,
    files: Vec<jc_core::kinds::BundleFile>,
    description: Option<&Description>,
) -> Result<String, ApiError> {
    let spec = jc_core::kinds::BundleSpec {
        exported_at: chrono::Utc::now(),
        exported_by: header.exporter.to_owned(),
        source_instance: None,
        source_revision: header.revision.to_owned(),
        items,
        native_files,
        files,
        omitted: header.omitted as u32,
        readme: description.map(|d| d.readme.clone()),
        schemas: description.map(|d| jc_core::kinds::BundleSchemas {
            kinds: d.kinds.clone(),
            models: match d.models_json() {
                Value::Object(members) => members.into_iter().collect(),
                _ => BTreeMap::new(),
            },
        }),
        // A YAML bundle; a git-native export lists its repositories here (MF-45).
        repositories: Vec::new(),
        models: Vec::new(),
    };
    let bundle = serde_json::json!({
        "apiVersion": API_VERSION,
        "kind": "Bundle",
        // A Bundle is organization-scoped, whichever project it describes (MF-17).
        "metadata": { "name": header.project, "namespace": crate::permissions::ORG_NAMESPACE },
        "spec": spec,
    });
    serde_norway::to_string(&bundle)
        .map_err(|e| ApiError::Internal(format!("bundle index did not serialise: {e}")))
}

/// The checksum of every file the bundle carries, over the bytes as exported (MF-42).
///
/// An import compares what it wrote against these, with the namespace mapping undone, so a
/// transfer between two instances is verified before anybody deletes the source.
fn bundle_files(included: &[&Exported]) -> Vec<jc_core::kinds::BundleFile> {
    use sha2::{Digest, Sha256};
    let mut files: Vec<jc_core::kinds::BundleFile> = included
        .iter()
        .map(|file| jc_core::kinds::BundleFile {
            path: file.path.clone(),
            sha256: format!("{:x}", Sha256::digest(file.content.as_bytes())),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// One manifest of an export as the index lists it (MF-17).
pub(crate) fn bundle_item(envelope: &ResourceEnvelope, path: &str) -> jc_core::kinds::BundleItem {
    let namespace = envelope.metadata.namespace.clone().filter(|namespace| {
        !resource::belongs_to_the_organization(&envelope.kind, Some(namespace))
    });
    jc_core::kinds::BundleItem {
        kind: envelope.kind.clone(),
        namespace,
        name: envelope.metadata.name.clone(),
        path: path.to_owned(),
    }
}

/// Where a complete archive puts its README and its schemas. Both describe the bundle rather than
/// belonging to the project, so import leaves them out (MF-41).
pub(crate) const README_PATH: &str = "README.md";
pub(crate) const SCHEMAS_DIR: &str = "schemas/";

/// What each kind is, in one sentence, for the README of a complete export (MF-41). The field
/// by field meaning is in the kind's JSON Schema; this is the line a person reads first.
const KIND_ABOUT: &[(&str, &str)] = &[
    ("Organization", "The city or institution running the instance: its domain, name and defaults."),
    ("Project", "A workspace of one department or programme, holding its spaces, pipelines and applications."),
    ("ContextSpace", "One topic's live data, such as air quality: the broker tenant and the data model it serves."),
    ("DataModel", "The entity types of a space, authored in LinkML; its JSON Schema and JSON-LD context are generated from it."),
    ("Mapping", "How a source's records become the model's entities, compiled to Bloblang."),
    ("Policy", "Who may read or write which entities and attributes of a space."),
    ("ScopeDefinition", "One node of the scope tree (/geo, /domain, /admin) that policies grant by."),
    ("Endpoint", "A published door onto a space: its address, audience, formats and limits."),
    ("ModelProjection", "A reduced or reshaped view of a data model that an endpoint serves."),
    ("SharedSpaceReference", "A space another project or instance shares with this one."),
    ("ContextSourceRegistration", "A federated source whose entities a space answers for."),
    ("ServiceAccount", "A machine identity and the grants it holds."),
    ("Pipeline", "A job that loads or transforms data into a space through an endpoint."),
    ("DataSource", "The connection to one external feed and the references to its credentials."),
    ("App", "An application built on endpoints: its data needs, visibility and build."),
    ("CkanInstance", "An open data catalogue the project publishes its datasets to."),
    ("Blueprint", "A reusable template that creates a set of resources."),
    ("AgentProfile", "The model, limits and permitted operations of an AI agent."),
    ("DataSpaceParticipant", "The organization's identity in a data space: its DID and connector."),
    ("DataOffer", "An endpoint offered in a data space under an ODRL policy."),
    ("DataAgreement", "A concluded data space contract and the access compiled from it."),
    ("SyncSource", "A repository, bundle or instance this configuration follows."),
    ("Bundle", "The index of a download: what it holds and where it came from."),
    ("UiSchema", "How the Portal arranges the form of one kind."),
    ("Role", "A named set of permissions."),
    ("RoleBinding", "Who holds which role, and where."),
    ("Dashboard", "A page of charts, maps and indicators over endpoints."),
    ("Layer", "A map layer a dashboard or an application draws."),
];

fn about(kind: &str) -> &'static str {
    KIND_ABOUT
        .iter()
        .find(|(known, _)| *known == kind)
        .map_or("", |(_, about)| about)
}

/// One data model's files, as a complete export carries them.
#[derive(Debug, Default)]
struct ModelFiles {
    classes: Vec<String>,
    linkml: Option<String>,
    json_schema: Option<Value>,
    /// What could not be had, in words, for the README.
    missing: Vec<String>,
}

/// What a complete export adds so that it explains itself (MF-41).
struct Description {
    readme: String,
    kinds: BTreeMap<String, Value>,
    models: BTreeMap<String, ModelFiles>,
}

impl Description {
    /// The models as the YAML index and the JSON list carry them.
    fn models_json(&self) -> Value {
        Value::Object(
            self.models
                .iter()
                .map(|(name, files)| {
                    (
                        name.clone(),
                        serde_json::json!({
                            "classes": files.classes,
                            "linkml": files.linkml,
                            "jsonSchema": files.json_schema,
                            "missing": files.missing,
                        }),
                    )
                })
                .collect(),
        )
    }
}

/// A path written relative to a manifest, resolved against the manifest's own directory. A path
/// that climbs out of the repository answers `None`; one that climbs out of the project simply
/// finds no file, because only this project's files are looked in.
fn beside(manifest_path: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = manifest_path.split('/').collect();
    parts.pop();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The LinkML source and JSON Schema of one `DataModel`: the repository's own files, a JSON
/// Schema generated by Model Tools when the repository has none, and the reason for anything
/// that could not be had.
async fn model_files(state: &AppState, file: &Exported, all: &[Exported]) -> ModelFiles {
    let Some(envelope) = &file.manifest else {
        return ModelFiles::default();
    };
    let spec = &envelope.spec;
    let content_at = |relative: &str| {
        let wanted = beside(&file.path, relative)?;
        all.iter()
            .find(|other| other.path == wanted)
            .map(|other| other.content.clone())
    };
    let mut model = ModelFiles {
        classes: spec
            .get("classes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        ..ModelFiles::default()
    };
    match spec.get("linkml").and_then(Value::as_str) {
        Some(relative) => match content_at(relative) {
            Some(source) => model.linkml = Some(source),
            None => model.missing.push(format!(
                "LinkML source: `{relative}` is not in the repository"
            )),
        },
        None => model
            .missing
            .push("LinkML source: the manifest names none (`spec.linkml`)".to_owned()),
    }
    if let Some(relative) = spec
        .pointer("/artifacts/jsonSchema")
        .and_then(Value::as_str)
    {
        match content_at(relative).map(|text| serde_json::from_str::<Value>(&text)) {
            Some(Ok(schema)) => model.json_schema = Some(schema),
            Some(Err(err)) => model
                .missing
                .push(format!("JSON Schema: `{relative}` is not JSON ({err})")),
            None => {}
        }
    }
    if model.json_schema.is_none() {
        match &model.linkml {
            Some(source) => {
                // A model saved through the Portal commits its JSON Schema, imports resolved
                // (DM-76), so this fallback compiles only a hand-committed source; one that
                // imports a platform model says so in `missing` (T-2885 carries the imports).
                match crate::api::datamodels::compile_artifacts(
                    state,
                    source,
                    &std::collections::BTreeMap::new(),
                )
                .await
                {
                    Ok(artifacts) if artifacts.json_schema.is_some() => {
                        model.json_schema = artifacts.json_schema;
                    }
                    Ok(artifacts) => model.missing.push(format!(
                        "JSON Schema: not in the repository, and the LinkML source does not compile: {}",
                        artifacts.errors.join("; ")
                    )),
                    Err(err) => model.missing.push(format!(
                        "JSON Schema: not in the repository, and Model Tools could not generate it: {err}"
                    )),
                }
            }
            None => model
                .missing
                .push("JSON Schema: no LinkML source to generate it from".to_owned()),
        }
    }
    model
}

/// The README, the kind schemas and the model files of a complete export (MF-41). `included`
/// are the manifests the export holds after its filters; `all` is every file of the project, in
/// which a model's LinkML source and schema artifacts are looked up.
async fn describe(
    state: &AppState,
    included: &[&Exported],
    all: &[Exported],
    header: &IndexHeader<'_>,
    archive: bool,
) -> Description {
    let mut by_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut models = BTreeMap::new();
    for file in included {
        let Some(envelope) = &file.manifest else {
            continue;
        };
        by_kind
            .entry(envelope.kind.clone())
            .or_default()
            .push(envelope.metadata.name.clone());
        if envelope.kind == "DataModel" {
            models.insert(
                envelope.metadata.name.clone(),
                model_files(state, file, all).await,
            );
        }
    }
    let kinds: BTreeMap<String, Value> = by_kind
        .keys()
        .filter_map(|kind| Some((kind.clone(), jc_core::registry::schema_of(kind)?)))
        .collect();
    let readme = readme(header, &by_kind, &models, archive);
    Description {
        readme,
        kinds,
        models,
    }
}

/// Who exported what, from where: the head every index and README of one export shares.
struct IndexHeader<'a> {
    project: &'a str,
    revision: &'a str,
    exporter: &'a str,
    omitted: usize,
}

fn readme(
    header: &IndexHeader<'_>,
    by_kind: &BTreeMap<String, Vec<String>>,
    models: &BTreeMap<String, ModelFiles>,
    archive: bool,
) -> String {
    let IndexHeader {
        project,
        revision,
        exporter,
        omitted,
    } = header;
    let mut out = format!(
        "# Project {project}\n\n\
         The configuration of project `{project}` as the repository held it at revision \
         `{revision}`, exported by {exporter}. It can be imported again as it is: `status` is \
         removed and secret values are empty strings, while the references to secrets \
         (`secretRef`) are kept.\n\n## What is where\n\n"
    );
    if archive {
        out.push_str(&format!(
            "- `projects/{project}/`: the manifests (`apiVersion`, `kind`, `metadata`, `spec`) and the \
             files beside them (Bento configuration, LinkML sources, generated schema artifacts).\n\
             - `projects/{project}/bundle.yaml`: the index of this download.\n\
             - `{SCHEMAS_DIR}kinds/{{Kind}}.schema.json`: the JSON Schema (draft-07) of each kind \
             below; every field carries its description.\n\
             - `{SCHEMAS_DIR}models/{{name}}/`: each data model's LinkML source \
             (`{{name}}.linkml.yaml`) and the JSON Schema of its entities (`{{name}}.schema.json`).\n"
        ));
    } else {
        out.push_str(
            "- The manifests (`apiVersion`, `kind`, `metadata`, `spec`), one per YAML document or \
             one per `items` entry of the JSON list.\n\
             - `schemas.kinds`: the JSON Schema (draft-07) of each kind below; every field carries \
             its description.\n\
             - `schemas.models`: each data model's LinkML source (`linkml`) and the JSON Schema of \
             its entities (`jsonSchema`).\n\
             - In YAML these sit in the `spec` of the closing `kind: Bundle` document; in JSON, \
             beside `items`.\n",
        );
    }
    out.push_str("\n## Kinds\n\n| Kind | Resources | What it is | Schema |\n|---|---|---|---|\n");
    for (kind, names) in by_kind {
        const SHOWN: usize = 12;
        let mut listed = names
            .iter()
            .take(SHOWN)
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        if names.len() > SHOWN {
            listed.push_str(&format!(" and {} more", names.len() - SHOWN));
        }
        let schema = if archive {
            format!("`{SCHEMAS_DIR}kinds/{kind}.schema.json`")
        } else {
            format!("`schemas.kinds.{kind}`")
        };
        out.push_str(&format!(
            "| {kind} | {} ({listed}) | {} | {schema} |\n",
            names.len(),
            about(kind)
        ));
    }
    if !models.is_empty() {
        out.push_str(
            "\n## Data models\n\n| Model | Entity types | LinkML | JSON Schema |\n|---|---|---|---|\n",
        );
        for (name, files) in models {
            let (linkml, schema) = if archive {
                (
                    format!("`{SCHEMAS_DIR}models/{name}/{name}.linkml.yaml`"),
                    format!("`{SCHEMAS_DIR}models/{name}/{name}.schema.json`"),
                )
            } else {
                (
                    format!("`schemas.models.{name}.linkml`"),
                    format!("`schemas.models.{name}.jsonSchema`"),
                )
            };
            let present =
                |found: bool, place: String| if found { place } else { "missing".to_owned() };
            out.push_str(&format!(
                "| {name} | {} | {} | {} |\n",
                files.classes.join(", "),
                present(files.linkml.is_some(), linkml),
                present(files.json_schema.is_some(), schema),
            ));
        }
        let missing: Vec<String> = models
            .iter()
            .flat_map(|(name, files)| {
                files
                    .missing
                    .iter()
                    .map(move |why| format!("- {name}: {why}"))
            })
            .collect();
        if !missing.is_empty() {
            out.push_str("\n## Missing\n\n");
            out.push_str(&missing.join("\n"));
            out.push('\n');
        }
    }
    if *omitted > 0 {
        out.push_str(&format!(
            "\n{omitted} files of the project are not in this download: the repository did not hand them back as text.\n"
        ));
    }
    out
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/export",
    summary = "Export Project",
    description = "The project's manifests as one bundle, narrowed to the kinds and names asked for; the whole project is for organization administrators.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("format" = Option<String>, Query, description = "yaml (default), json, zip, or git: one git bundle per repository of a layout 2 project"),
        ("revision" = Option<String>, Query, description = "Commit or branch; default branch head when absent"),
        ("kinds" = Option<String>, Query, description = "Comma-separated plurals to include"),
        ("names" = Option<String>, Query, description = "Comma-separated resource names to include"),
    ),
    responses(
        (status = 200, description = "The project's configuration at the revision"),
        (status = 400, description = "Unknown format or malformed revision", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "A whole-project or git export by anybody but an organization administrator (UI-87), or a git export the caller may not read in full", body = ProblemDetails),
        (status = 404, description = "No such project or revision", body = ProblemDetails),
        (status = 409, description = "A git export of a layout 1 project, of a repository that moved during it, or of an App outside the forge", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails)
    )
)]
pub async fn export(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // A download used to answer any session at all (T-0819). It answers the caller's read
    // grants now: no binding covering the project is `404`, the one answer for "missing" and
    // "not yours" (PF-59, MF-18, R20).
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let format = query.format.as_deref().unwrap_or("yaml").to_string();
    if !matches!(format.as_str(), "yaml" | "json" | "zip" | "git") {
        return Err(ApiError::BadRequest(format!(
            "format '{format}' is not yaml, json, zip or git"
        )));
    }
    // The whole project leaves only through an administrator (UI-87); manifests named one by
    // one stay with the project's readers.
    if format == "git" || selected(query.names.as_deref()).is_none() {
        crate::permissions::require_organization_admin(
            &state,
            &user.0.identity,
            "exporting a whole project",
        )?;
    }
    let forge = gitea(&state, &project)?;
    let gitea: &GiteaClient = &forge;
    if format == "git" {
        if !gitea.is_project_repository() {
            return Err(ApiError::Conflict(
                "format=git exports a project that lives in a repository of its own (layout 2); \
                 this one is a folder of the organization repository, so export it as zip (MF-45)"
                    .into(),
            ));
        }
        if query.revision.is_some() || query.kinds.is_some() || query.names.is_some() {
            return Err(ApiError::BadRequest(
                "format=git is the whole repository at its default branch: it takes no \
                 revision, kinds or names (MF-45)"
                    .into(),
            ));
        }
    }

    let revision = match query.revision.as_deref() {
        Some(revision) => {
            if !valid_revision(revision) {
                return Err(ApiError::BadRequest(
                    "revision must be a commit id or a branch name".into(),
                ));
            }
            revision.to_string()
        }
        None => {
            let branch = gitea.default_branch().await?;
            gitea.branch_head(&branch).await?
        }
    };

    // `sourceRevision` of a Bundle is a commit (7 to 40 hex, MF-17), so a branch name given as
    // `?revision=` is resolved before it is written into the index.
    let commit = if is_commit(&revision) {
        revision.clone()
    } else {
        gitea.branch_head(&revision).await?
    };

    let (files, unreadable) =
        read_project(gitea, &project, &revision, &endpoints_by_slug(&state)).await?;
    // A manifest the caller may not read is counted, never named (MF-18, R20). The question is
    // the manifest's, not the kind's: a grant bound to one context space reads that space's
    // manifests and no other, which is what the resource lists already ask (PF-60, T-0986).
    // A native file (a Bento stream, a LinkML source, a mapping) belongs to the manifest beside
    // it and is read exactly when that manifest is; one with no such manifest is read under the
    // kind its directory names and the space its path names, the rule its approval is held to
    // (T-1404), and a file of no kind is nobody's but the bootstrap group's (T-1405).
    let readable_manifest = |envelope: &ResourceEnvelope| {
        effective.may_read_manifest(
            &envelope.kind,
            &serde_json::to_value(envelope).unwrap_or(Value::Null),
        )
    };
    let readable: Vec<bool> = files
        .iter()
        .map(|file| match &file.manifest {
            Some(envelope) => readable_manifest(envelope),
            None => match owner(&file.path, &files) {
                Some(envelope) => readable_manifest(envelope),
                None => match crate::api::import::native_kind(&file.path) {
                    Some(kind) => {
                        effective.may_read_in(kind, crate::api::import::space_in_path(&file.path))
                    }
                    None => effective.bootstrap,
                },
            },
        })
        .collect();
    let (files, refused): (Vec<Exported>, Vec<Exported>) = {
        let mut keep = Vec::new();
        let mut drop = Vec::new();
        for (file, readable) in files.into_iter().zip(readable) {
            if readable {
                keep.push(file)
            } else {
                drop.push(file)
            }
        }
        (keep, drop)
    };
    let omitted = unreadable + refused.len();
    // A git bundle is every file of the repository with its history, so it answers only a
    // caller who reads every manifest of the project; nothing is left out of a bundle (MF-18).
    if format == "git" {
        if omitted > 0 {
            return Err(ApiError::Denied(format!(
                "a git export carries the whole project; {omitted} of its manifests are not \
                 yours to read, so export it as zip, which leaves them out (MF-45, MF-18)"
            )));
        }
        let archive =
            crate::api::export_git::archive(&state, gitea, &project, &user.0.identity.username)
                .await?;
        return Ok(attachment(
            archive.bytes,
            "application/zip",
            format!("{project}-git-{}.zip", &archive.head[..7]),
        ));
    }
    let kinds = kind_filter(query.kinds.as_deref());
    let names = selected(query.names.as_deref());
    let short = revision.chars().take(7).collect::<String>();

    let included: Vec<&Exported> = files
        .iter()
        .filter(|file| {
            file.manifest
                .as_ref()
                .is_some_and(|envelope| matches_filters(envelope, kinds.as_ref(), names.as_ref()))
        })
        .collect();
    let header = IndexHeader {
        project: &project,
        // The commit, never the branch name a caller may have asked for: `sourceRevision` is a
        // commit id (MF-17) and the README says which one the export was taken at.
        revision: &commit,
        exporter: &user.0.identity.username,
        omitted,
    };
    // An export that names resources is those manifests and nothing else; every other export of
    // something is complete and says what its files mean (MF-41).
    let complete = names.is_none() && !included.is_empty();

    if format == "zip" {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut items = Vec::new();
        let mut native_files = Vec::new();
        // What the archive actually carries, which is not `included`: the native files of the
        // resources it holds come too. The checksums are of these (MF-42).
        let mut archived: Vec<&Exported> = Vec::new();
        for file in &files {
            match &file.manifest {
                Some(envelope) => {
                    if !matches_filters(envelope, kinds.as_ref(), names.as_ref()) {
                        continue;
                    }
                    items.push(bundle_item(envelope, &file.path));
                }
                // A native file travels with the resource it belongs to, under the same filters
                // (T-1405).
                None => {
                    let travels = match owner(&file.path, &files) {
                        Some(envelope) => matches_filters(envelope, kinds.as_ref(), names.as_ref()),
                        None => native_matches_filters(&file.path, kinds.as_ref(), names.as_ref()),
                    };
                    if !travels {
                        continue;
                    }
                    native_files.push(file.path.clone());
                }
            }
            archived.push(file);
            let zipped = |err: zip::result::ZipError| {
                ApiError::Internal(format!("archive entry failed: {err}"))
            };
            writer.start_file(&file.path, options).map_err(zipped)?;
            writer
                .write_all(file.content.as_bytes())
                .map_err(|e| ApiError::Internal(format!("archive write failed: {e}")))?;
        }
        let description = if complete {
            Some(describe(&state, &included, &files, &header, true).await)
        } else {
            None
        };
        // A Bundle lists at least one resource, so an archive whose filters matched no manifest
        // carries no index rather than one the platform refuses (MF-17).
        let mut entries = Vec::new();
        if !items.is_empty() {
            entries.push((
                "bundle.yaml".to_owned(),
                bundle_index(
                    &header,
                    items,
                    native_files,
                    bundle_files(&archived),
                    description.as_ref(),
                )?,
            ));
        }
        if let Some(description) = &description {
            entries.push((README_PATH.to_owned(), description.readme.clone()));
            for (kind, schema) in &description.kinds {
                entries.push((
                    format!("{SCHEMAS_DIR}kinds/{kind}.schema.json"),
                    pretty(schema)?,
                ));
            }
            for (name, model) in &description.models {
                if let Some(linkml) = &model.linkml {
                    entries.push((
                        format!("{SCHEMAS_DIR}models/{name}/{name}.linkml.yaml"),
                        linkml.clone(),
                    ));
                }
                if let Some(schema) = &model.json_schema {
                    entries.push((
                        format!("{SCHEMAS_DIR}models/{name}/{name}.schema.json"),
                        pretty(schema)?,
                    ));
                }
            }
        }
        for (entry, content) in entries {
            writer
                .start_file(entry, options)
                .map_err(|err| ApiError::Internal(format!("archive entry failed: {err}")))?;
            writer
                .write_all(content.as_bytes())
                .map_err(|e| ApiError::Internal(format!("archive write failed: {e}")))?;
        }
        let cursor = writer
            .finish()
            .map_err(|err| ApiError::Internal(format!("archive did not close: {err}")))?;
        return Ok(attachment(
            cursor.into_inner(),
            "application/zip",
            format!("{project}-{short}.zip"),
        ));
    }

    let manifests: Vec<&ResourceEnvelope> = included
        .iter()
        .filter_map(|file| file.manifest.as_ref())
        .collect();
    let description = if complete {
        Some(describe(&state, &included, &files, &header, false).await)
    } else {
        None
    };

    if format == "json" {
        let mut list = serde_json::json!({
            "apiVersion": API_VERSION,
            "kind": "List",
            "metadata": { "revision": revision, "omitted": omitted },
            "items": manifests,
        });
        if let Some(description) = &description {
            list["readme"] = Value::String(description.readme.clone());
            list["schemas"] = serde_json::json!({
                "kinds": description.kinds,
                "models": description.models_json(),
            });
        }
        let body = serde_json::to_vec_pretty(&list)
            .map_err(|e| ApiError::Internal(format!("export did not serialise: {e}")))?;
        return Ok(attachment(
            body,
            "application/json",
            format!("{project}-{short}.json"),
        ));
    }

    let mut body = String::new();
    for envelope in &manifests {
        let document = serde_norway::to_string(envelope)
            .map_err(|e| ApiError::Internal(format!("export did not serialise: {e}")))?;
        body.push_str("---\n");
        body.push_str(&document);
    }
    if let Some(description) = &description {
        // The closing index carries what the manifests mean; import reads it as provenance and
        // writes none of it (MF-17, MF-41).
        let items: Vec<jc_core::kinds::BundleItem> = included
            .iter()
            .filter_map(|file| {
                file.manifest
                    .as_ref()
                    .map(|envelope| bundle_item(envelope, &file.path))
            })
            .collect();
        // A stream is documents, not files, so it carries no checksums: there is no path for
        // an import to verify against (MF-42). The archive is the transfer format.
        let document = bundle_index(&header, items, Vec::new(), Vec::new(), Some(description))?;
        body.push_str("---\n");
        body.push_str(&document);
    }
    Ok(attachment(
        body.into_bytes(),
        "application/yaml",
        format!("{project}-{short}.yaml"),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/revisions",
    summary = "List Project Revisions",
    description = "The project's history: the commits an export can be read from.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("limit" = Option<usize>, Query, description = "1…100, default 20"),
    ),
    responses(
        (status = 200, description = "Commits touching the project, newest first", body = RevisionList),
        (status = 400, description = "limit out of range", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails)
    )
)]
pub async fn revisions(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<RevisionsQuery>,
) -> Result<Json<RevisionList>, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // The history of a project says who changed what and when, so it is a read like any other.
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let limit = query.limit.unwrap_or(DEFAULT_REVISIONS);
    if limit == 0 || limit > MAX_REVISIONS {
        return Err(ApiError::BadRequest(format!(
            "limit must be between 1 and {MAX_REVISIONS}"
        )));
    }
    let gitea = gitea(&state, &project)?;
    let branch = gitea.default_branch().await?;
    let commits = gitea
        .list_commits(&branch, &format!("projects/{project}"), limit)
        .await?;
    Ok(Json(RevisionList {
        items: commits
            .into_iter()
            .map(|commit| Revision {
                sha: commit.sha,
                message: commit.message,
                author: commit.author,
                date: commit.date,
            })
            .collect(),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/export", get(export))
        .route("/projects/{project}/revisions", get(revisions))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reference_to_this_organization_is_exported_by_name_and_another_instance_keeps_its_slug() {
        let endpoints = HashMap::from([(
            "scsd2eehkx42n53z2zyd6vshfh7s7irf".to_owned(),
            ("helsinki".to_owned(), "helsinki-bikes".to_owned()),
        )]);
        let mut ours = serde_json::json!({ "endpointSlug": "scsd2eehkx42n53z2zyd6vshfh7s7irf", "alias": "city-bikes" });
        super::reference_by_name(&mut ours, &endpoints);
        assert_eq!(
            ours,
            serde_json::json!({ "endpointRef": { "project": "helsinki", "name": "helsinki-bikes" }, "alias": "city-bikes" })
        );
        let mut theirs =
            serde_json::json!({ "endpointSlug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "alias": "x" });
        let before = theirs.clone();
        super::reference_by_name(&mut theirs, &endpoints);
        assert_eq!(
            theirs, before,
            "a slug of another instance is all there is to name it by"
        );
        let mut already =
            serde_json::json!({ "endpointRef": { "project": "a", "name": "b" }, "alias": "x" });
        let before = already.clone();
        super::reference_by_name(&mut already, &endpoints);
        assert_eq!(already, before);
    }

    use super::*;

    #[test]
    fn secret_values_are_emptied_wherever_they_hide() {
        let mut spec = serde_json::json!({
            "url": "mqtt://broker",
            "auth": { "password": "hunter2", "secretRef": { "name": "mqtt", "key": "password" } },
            "outputs": [{ "apiKey": "jc_dead_beef" }],
        });
        strip_secret_values(&mut spec);
        let dumped = spec.to_string();
        assert!(!dumped.contains("hunter2"), "{dumped}");
        assert!(!dumped.contains("jc_dead_beef"), "{dumped}");
        assert_eq!(spec["auth"]["password"], "");
        assert_eq!(
            spec["auth"]["secretRef"]["key"], "password",
            "a reference to a secret is not a secret and must survive the export"
        );
        assert_eq!(spec["url"], "mqtt://broker");
        // T-3972: a step's `${VAR}` names the secret and stays; a value around it does not.
        let mut step = serde_json::json!({ "basic_auth": { "password": "${FEED_PW}" }, "token": "t${FEED_PW}" });
        strip_secret_values(&mut step);
        assert_eq!(step["basic_auth"]["password"], "${FEED_PW}");
        assert_eq!(step["token"], "");
    }

    #[test]
    fn a_number_or_a_boolean_under_a_credential_key_is_emptied_too() {
        let mut spec = serde_json::json!({
            "auth": { "password": 918273, "token": true, "clientSecret": null },
            "outputs": [{ "apiKey": 4.5e3 }, { "password": { "secretRef": { "name": "db" } } }],
            "port": 8883,
            "enabled": true,
        });
        strip_secret_values(&mut spec);
        let dumped = spec.to_string();
        assert!(!dumped.contains("918273"), "{dumped}");
        assert!(!dumped.contains("4500"), "{dumped}");
        assert_eq!(spec["auth"]["password"], "");
        assert_eq!(spec["auth"]["token"], "");
        assert_eq!(spec["outputs"][0]["apiKey"], "");
        // An absent value stays absent, a reference stays a reference, and the rest is untouched.
        assert_eq!(spec["auth"]["clientSecret"], Value::Null);
        assert_eq!(spec["outputs"][1]["password"]["secretRef"]["name"], "db");
        assert_eq!(spec["port"], 8883);
        assert_eq!(spec["enabled"], true);
    }

    #[test]
    fn a_revision_is_a_commit_or_a_branch_and_never_a_path_traversal() {
        assert!(valid_revision("main"));
        assert!(valid_revision("8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8"));
        assert!(valid_revision("release/2026-09"));
        assert!(!valid_revision(""));
        assert!(!valid_revision("../../etc/passwd"));
        assert!(!valid_revision("main;rm -rf"));
        assert!(!valid_revision("--upload-pack=x"));
        assert!(!valid_revision(&"a".repeat(129)));
    }

    #[test]
    fn an_unknown_plural_filters_everything_out_rather_than_nothing() {
        let filter = kind_filter(Some("endpoints,teapots")).expect("a filter");
        assert!(filter.contains(&"Endpoint".to_string()));
        assert!(filter.contains(&"unknown:teapots".to_string()));
        assert!(kind_filter(None).is_none(), "no filter is not an empty one");
        assert!(kind_filter(Some(" , ")).is_none());
    }
}
