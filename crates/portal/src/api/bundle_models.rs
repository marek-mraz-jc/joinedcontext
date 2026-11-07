//! The organization models a project's git export carries, and what its import does with each
//! (MF-49, MF-50, ADR-N-039 §3.4).
//!
//! The export copies every organization model a model of the project imports, transitively, as
//! the organization holds it: the manifest and the LinkML source, nothing else. The import maps
//! each onto a published model of the destination organization with a byte-identical source of
//! that major, or lands it as a project model and points the project's imports at it. The
//! destination's organization repository is never written.

use std::collections::BTreeMap;

use jc_core::kinds::data_model::{DataModelLifecycle, ModelImport};
use jc_core::kinds::{Bundle, BundleModel, BundleModelOrigin, DataModelSpec};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::api::datamodels::{
    artifact_files, compile_artifacts, confine_linkml_path, model_folder, read_source,
};
use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::git::GiteaClient;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::tools::model_tools::Artifacts;

/// The index entries and the archive files of every organization model `project` imports,
/// transitively, for an export of it (MF-49). An import of a model the organization no longer
/// holds at the pinned major refuses the export by name.
pub(crate) async fn carry(
    state: &AppState,
    project: &str,
    organization: &str,
) -> Result<(Vec<BundleModel>, Vec<(String, Vec<u8>)>), ApiError> {
    let mut todo = Vec::new();
    for model in state
        .mirror
        .list(project, "DataModel", &crate::store::ListOptions::default())
        .items
    {
        todo.push(read_source(state, project, &model.metadata.name).await?);
    }
    let forge = state
        .forge_for(ORG_NAMESPACE)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let branch = forge.default_branch().await?;

    let mut carried: BTreeMap<String, (jc_core::kinds::SemVer, String, String)> = BTreeMap::new();
    while let Some(text) = todo.pop() {
        // A source that does not parse imports nothing a check would not have refused already.
        let Ok(document) = serde_norway::from_str::<Value>(&text) else {
            continue;
        };
        let imports = ModelImport::all_in(&document)
            .map_err(|err| ApiError::Conflict(format!("the project's models: {err} (MF-49)")))?;
        for import in imports {
            if !import.organization || carried.contains_key(&import.name) {
                continue;
            }
            let envelope = state
                .mirror
                .get(ORG_NAMESPACE, "DataModel", &import.name)
                .ok_or_else(|| {
                    ApiError::Conflict(format!(
                        "the project imports '{import}', and the organization holds no model '{}'; \
                         change that import before exporting (MF-49)",
                        import.name
                    ))
                })?;
            let spec: DataModelSpec = serde_json::from_value(envelope.spec.clone())
                .map_err(|err| ApiError::Internal(format!("{}: {err}", import.name)))?;
            if spec.version.major() != import.major {
                return Err(ApiError::Conflict(format!(
                    "the project imports '{import}', and the organization holds '{}' at version \
                     {}; move the import to that major before exporting (MF-49)",
                    import.name, spec.version
                )));
            }
            let source = read_source(state, ORG_NAMESPACE, &import.name).await?;
            let path = format!(
                "{}/{}.yaml",
                model_folder(ORG_NAMESPACE, &envelope.spec, &import.name),
                import.name
            );
            let manifest = forge
                .get_file(&path, &branch)
                .await?
                .ok_or_else(|| {
                    ApiError::Conflict(format!("{path} is not in the organization repository"))
                })?
                .content;
            todo.push(source.clone());
            carried.insert(import.name.clone(), (spec.version, manifest, source));
        }
    }

    let mut models = Vec::new();
    let mut files = Vec::new();
    for (name, (version, manifest, source)) in carried {
        let major = version.major();
        let manifest_path = format!("models/{name}.v{major}.yaml");
        let file = format!("models/{name}.v{major}.linkml.yaml");
        models.push(BundleModel {
            sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
            origin: BundleModelOrigin {
                organization: organization.to_owned(),
                name: name.clone(),
            },
            name,
            version,
            manifest: manifest_path.clone(),
            file: file.clone(),
        });
        files.push((manifest_path, manifest.into_bytes()));
        files.push((file, source.into_bytes()));
    }
    Ok((models, files))
}

/// What an import does with one carried model (MF-50).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelAction {
    /// The destination organization holds it, byte for byte: the imports stay.
    Map,
    /// It becomes a model of the imported project, and the imports point at it.
    Land,
}

/// One carried model and what the import does with it, as the dry run shows it (MF-50).
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedModel {
    pub name: String,
    pub version: String,
    pub action: ModelAction,
}

/// Map or land, per carried model: map when the destination organization holds a published
/// model of that name and major whose source is the carried one byte for byte (MF-50). A model
/// that lands is compiled here, before anything is created, and its artifacts are the second
/// half of the answer, by name: a source Model Tools refuses refuses the import (DM-02).
pub(crate) async fn plan(
    state: &AppState,
    index: &Bundle,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<PlannedModel>, BTreeMap<String, Artifacts>), ApiError> {
    let mut planned = Vec::new();
    for model in &index.spec.models {
        let source = files.get(&model.file).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "bundle.yaml carries the model '{}', and the archive has no {} (MF-49)",
                model.name, model.file
            ))
        })?;
        if format!("{:x}", Sha256::digest(source)) != model.sha256 {
            return Err(ApiError::BadRequest(format!(
                "{} is not the source bundle.yaml lists for '{}': its SHA-256 differs (MF-42)",
                model.file, model.name
            )));
        }
        let holds = state
            .mirror
            .get(ORG_NAMESPACE, "DataModel", &model.name)
            .and_then(|envelope| serde_json::from_value::<DataModelSpec>(envelope.spec).ok())
            .is_some_and(|spec| {
                spec.version.major() == model.version.major()
                    && matches!(
                        spec.lifecycle,
                        DataModelLifecycle::Published | DataModelLifecycle::Deprecated
                    )
            });
        let identical = holds
            && read_source(state, ORG_NAMESPACE, &model.name)
                .await
                .ok()
                .is_some_and(|held| held.as_bytes() == source.as_slice());
        planned.push(PlannedModel {
            name: model.name.clone(),
            version: model.version.to_string(),
            action: if identical {
                ModelAction::Map
            } else {
                ModelAction::Land
            },
        });
    }
    let artifacts = compile(state, index, files, &planned).await?;
    Ok((planned, artifacts))
}

/// The artifacts of every model `planned` lands, compiled from its source as it lands, with
/// every carried model as an import under the name it has after the import (DM-02, DM-76).
async fn compile(
    state: &AppState,
    index: &Bundle,
    files: &BTreeMap<String, Vec<u8>>,
    planned: &[PlannedModel],
) -> Result<BTreeMap<String, Artifacts>, ApiError> {
    let lands = |name: &str| {
        planned
            .iter()
            .any(|plan| plan.name == name && plan.action == ModelAction::Land)
    };
    let landed: Vec<(String, u32)> = index
        .spec
        .models
        .iter()
        .filter(|model| lands(&model.name))
        .map(|model| (model.name.clone(), model.version.major()))
        .collect();
    if landed.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut sources = BTreeMap::new();
    for model in &index.spec.models {
        let text = std::str::from_utf8(&files[&model.file])
            .map_err(|_| ApiError::BadRequest(format!("{} is not UTF-8 text", model.file)))?;
        let (namespace, text) = if lands(&model.name) {
            ("project", rewrite_imports(text, &landed))
        } else {
            ("org", text.to_owned())
        };
        let import = format!("{namespace}.{}.v{}", model.name, model.version.major());
        sources.insert(import, (model.name.clone(), text));
    }
    let mut compiled = BTreeMap::new();
    for (import, (name, source)) in &sources {
        if !lands(name) {
            continue;
        }
        let imports: BTreeMap<String, String> = sources
            .iter()
            .filter(|(other, _)| *other != import)
            .map(|(other, (_, text))| (other.clone(), text.clone()))
            .collect();
        let artifacts = compile_artifacts(state, source, &imports).await?;
        if !artifacts.errors.is_empty() {
            return Err(ApiError::Conflict(format!(
                "the carried organization model '{name}' lands in the project, and Model Tools \
                 refuses its source: {} (DM-02, MF-50)",
                artifacts.errors.join("; ")
            )));
        }
        compiled.insert(name.clone(), artifacts);
    }
    Ok(compiled)
}

/// `text` with every import of `org.{name}.v{major}` of `landed` turned into
/// `project.{name}.v{major}` (MF-50). An import name is a whole token: `org.stations.v1` inside
/// `org.stations.v10` or `my-org.stations.v1` is left alone.
///
/// ponytail: a textual rewrite keeps the source as its authors wrote it, comments and order
/// included; the same token in a description would be rewritten too, which a YAML-aware edit
/// avoids when that ever matters.
pub(crate) fn rewrite_imports(text: &str, landed: &[(String, u32)]) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    let mut out = text.to_owned();
    for (name, major) in landed {
        let from = format!("org.{name}.v{major}");
        let to = format!("project.{name}.v{major}");
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(&from) {
            let before = rest[..at].chars().next_back();
            let after = rest[at + from.len()..].chars().next();
            result.push_str(&rest[..at]);
            if before.is_some_and(token) || after.is_some_and(token) {
                result.push_str(&from);
            } else {
                result.push_str(&to);
            }
            rest = &rest[at + from.len()..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Lands the models `planned` lands in the project repository `mounted` (mounted at
/// `projects/{project}/`), in one commit on `main`: each as a project model in a folder of its
/// own with its origin kept, and every model source of the project that imports it pointing at
/// it. A model the project holds a model of that name already refuses the import (MF-50).
pub(crate) async fn land(
    mounted: &GiteaClient,
    identity: &Identity,
    project: &str,
    index: &Bundle,
    files: &BTreeMap<String, Vec<u8>>,
    planned: &[PlannedModel],
    artifacts: &BTreeMap<String, Artifacts>,
) -> Result<Option<String>, ApiError> {
    let landing: Vec<&BundleModel> = index
        .spec
        .models
        .iter()
        .filter(|model| {
            planned
                .iter()
                .any(|plan| plan.name == model.name && plan.action == ModelAction::Land)
        })
        .collect();
    if landing.is_empty() {
        return Ok(None);
    }

    // The project's own models: what a landed one may not be named, and what imports it.
    let mut own = Vec::new();
    for path in mounted.list_tree("main").await? {
        if !path.contains("/datamodels/")
            || !path.ends_with(".yaml")
            || path.ends_with(".linkml.yaml")
        {
            continue;
        }
        let Some(file) = mounted.get_file(&path, "main").await? else {
            continue;
        };
        let Ok(envelope) = serde_norway::from_str::<ResourceEnvelope>(&file.content) else {
            continue;
        };
        if envelope.kind == "DataModel" {
            own.push((path, envelope));
        }
    }
    for model in &landing {
        if own
            .iter()
            .any(|(_, envelope)| envelope.metadata.name == model.name)
        {
            return Err(ApiError::Conflict(format!(
                "the carried organization model '{}' can neither be mapped, since this \
                 organization's differs, nor land, since the project holds a model '{}' already \
                 (MF-50)",
                model.name, model.name
            )));
        }
    }

    let landed: Vec<(String, u32)> = landing
        .iter()
        .map(|model| (model.name.clone(), model.version.major()))
        .collect();
    let mut uploads = Vec::new();
    for model in &landing {
        let manifest = files
            .get(&model.manifest)
            .and_then(|bytes| serde_norway::from_slice::<ResourceEnvelope>(bytes).ok())
            .filter(|envelope| envelope.kind == "DataModel")
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "{} is not a DataModel manifest (MF-49)",
                    model.manifest
                ))
            })?;
        let mut manifest = manifest;
        manifest.metadata.name = model.name.clone();
        manifest.metadata.namespace = Some(project.to_owned());
        manifest.strip_status();
        let compiled = artifacts
            .get(&model.name)
            .ok_or_else(|| ApiError::Internal(format!("'{}' lands uncompiled", model.name)))?;
        let (generated, artifact_writes) =
            artifact_files(&model.name, model.version.major(), compiled)?;
        if let Some(spec) = manifest.spec.as_object_mut() {
            spec.remove("contextSpaceRef");
            // The organization's artifacts stay there; the project commits the ones compiled
            // from the source as it lands.
            spec.insert(
                "artifacts".into(),
                serde_json::to_value(&generated)
                    .map_err(|err| ApiError::Internal(format!("the artifacts: {err}")))?,
            );
        }
        let linkml = manifest
            .spec
            .get("linkml")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ApiError::BadRequest(format!("{} names no linkml source", model.manifest))
            })?;
        let linkml = confine_linkml_path(linkml)?;
        let spec: DataModelSpec = serde_json::from_value(manifest.spec.clone()).map_err(|err| {
            ApiError::BadRequest(format!("{} does not read: {err}", model.manifest))
        })?;
        spec.validate()
            .map_err(|err| ApiError::BadRequest(format!("{}: {err}", model.manifest)))?;
        let folder = format!("projects/{project}/datamodels/{}", model.name);
        let text = serde_norway::to_string(&manifest)
            .map_err(|err| ApiError::Internal(format!("the landed manifest: {err}")))?;
        uploads.push((format!("{folder}/{}.yaml", model.name), text));
        let source = String::from_utf8(files[&model.file].clone())
            .map_err(|_| ApiError::BadRequest(format!("{} is not UTF-8 text", model.file)))?;
        // A landed model that imports another landed one imports it in the project now.
        uploads.push((
            format!("{folder}/{linkml}"),
            rewrite_imports(&source, &landed),
        ));
        uploads.extend(
            artifact_writes
                .into_iter()
                .map(|(file, content)| (format!("{folder}/{file}"), content)),
        );
    }
    for (path, envelope) in &own {
        let Some(linkml) = envelope.spec.get("linkml").and_then(Value::as_str) else {
            continue;
        };
        let Ok(linkml) = confine_linkml_path(linkml) else {
            continue;
        };
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let source_path = format!("{folder}/{linkml}");
        let Some(file) = mounted.get_file(&source_path, "main").await? else {
            continue;
        };
        let rewritten = rewrite_imports(&file.content, &landed);
        if rewritten != file.content {
            uploads.push((source_path, rewritten));
        }
    }

    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    let names: Vec<&str> = landing.iter().map(|model| model.name.as_str()).collect();
    let commit = mounted
        .change_files(
            "main",
            &format!(
                "Land the organization models {} as models of {project} (MF-50)",
                names.join(", ")
            ),
            crate::git::Author {
                name: &author_name,
                email: &author_email,
            },
            &uploads,
            &[],
        )
        .await?;
    Ok(Some(commit))
}

#[cfg(test)]
mod tests {
    use super::rewrite_imports;

    // MF-50: only the whole import name of a landed model is rewritten.
    #[test]
    fn a_landed_models_import_is_rewritten_and_nothing_else() {
        let landed = [("stations".to_owned(), 1)];
        let source = "imports:\n  - linkml:types\n  - org.stations.v1\n  - org.stations.v10\n  - my-org.stations.v1\n  - org.bikes.v1\n";
        assert_eq!(
            rewrite_imports(source, &landed),
            "imports:\n  - linkml:types\n  - project.stations.v1\n  - org.stations.v10\n  - my-org.stations.v1\n  - org.bikes.v1\n"
        );
        assert_eq!(
            rewrite_imports("imports: [linkml:types, org.stations.v1]", &landed),
            "imports: [linkml:types, project.stations.v1]"
        );
        assert_eq!(rewrite_imports("", &landed), "");
        assert_eq!(rewrite_imports(source, &[]), source);
    }
}
