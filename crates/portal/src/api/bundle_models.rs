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
use crate::forge::ShakoClient;
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
    mounted: &ShakoClient,
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
                serde_json::to_value(&generated).map_err(ApiError::internal)?,
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
        let text = serde_norway::to_string(&manifest).map_err(ApiError::internal)?;
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
            crate::forge::Author {
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
    use base64::Engine;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// An export of a project with no model still asks the organization's forge for its models'
    /// index: with no forge configured it answers 503 naming the forge.
    #[tokio::test]
    async fn an_export_without_a_forge_carries_no_model() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let refused = super::carry(&state, "helsinki", "helsinki")
            .await
            .expect_err("no forge");
        assert!(
            matches!(&refused, crate::error::ApiError::Unavailable(m) if m.contains("forge")),
            "{refused:?}"
        );
    }

    /// A carried model whose source the archive lacks, or whose source is not the one the
    /// index's SHA-256 names, refuses the import before anything else is read (MF-42, MF-49).
    #[tokio::test]
    async fn a_missing_or_altered_model_source_is_refused() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let index: super::Bundle = serde_json::from_value(serde_json::json!({
            "apiVersion": jc_core::API_VERSION,
            "kind": "Bundle",
            "metadata": { "name": "air", "namespace": crate::permissions::ORG_NAMESPACE },
            "spec": {
                "exportedAt": "2026-11-25T00:00:00Z",
                "exportedBy": "jana",
                "sourceRevision": "abc1234",
                "items": [],
                "files": [],
                "omitted": 0,
                "models": [{
                    "name": "stations", "version": "1.0.0",
                    "manifest": "models/stations.yaml", "file": "models/stations.linkml.yaml",
                    "sha256": "0".repeat(64),
                    "origin": { "organization": "bbsk", "name": "stations" }
                }]
            }
        }))
        .expect("a Bundle");
        let mut files = std::collections::BTreeMap::new();
        let missing = super::plan(&state, &index, &files)
            .await
            .expect_err("refused");
        assert!(
            matches!(&missing, crate::error::ApiError::BadRequest(m) if m.contains("has no models/stations.linkml.yaml")),
            "{missing:?}"
        );
        files.insert(
            "models/stations.linkml.yaml".to_owned(),
            b"id: other".to_vec(),
        );
        let altered = super::plan(&state, &index, &files)
            .await
            .expect_err("refused");
        assert!(
            matches!(&altered, crate::error::ApiError::BadRequest(m) if m.contains("SHA-256 differs")),
            "{altered:?}"
        );
    }

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

    const REPO: &str = "/odata/Repositories(Owner='bb',Name='org')";

    async fn forge() -> (MockServer, crate::forge::ShakoClient) {
        let server = MockServer::start().await;
        let client = crate::forge::ShakoClient::new(
            server.uri().parse().expect("a url"),
            "bb",
            "org",
            "token",
        )
        .expect("a client");
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })),
            )
            .mount(&server)
            .await;
        (server, client)
    }

    /// The repository holds `content` at `at`; every other file is not there.
    async fn holds(server: &MockServer, at: &str, content: &str) {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{at}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "blob",
                "Content": base64::engine::general_purpose::STANDARD.encode(content),
            })))
            .mount(server)
            .await;
    }

    async fn tree(server: &MockServer, paths: &[&str]) {
        Mock::given(method("GET"))
            .and(path(format!(
                "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "tree",
                "Truncated": false,
                "Tree": paths.iter().map(|p| json!({ "Path": p, "Type": "blob", "Sha": "b" })).collect::<Vec<_>>(),
            })))
            .mount(server)
            .await;
    }

    fn identity() -> crate::auth::session::Identity {
        crate::auth::session::Identity {
            client: None,
            subject: "sub-jana".into(),
            username: "jana".into(),
            email: Some("jana@hel.fi".into()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        }
    }

    fn index(names: &[&str]) -> super::Bundle {
        let models: Vec<Value> = names
            .iter()
            .map(|name| {
                json!({
                    "name": name, "version": "1.0.0",
                    "manifest": format!("models/{name}.v1.yaml"),
                    "file": format!("models/{name}.v1.linkml.yaml"),
                    "sha256": "0".repeat(64),
                    "origin": { "organization": "bbsk", "name": name }
                })
            })
            .collect();
        serde_json::from_value(json!({
            "apiVersion": jc_core::API_VERSION,
            "kind": "Bundle",
            "metadata": { "name": "air", "namespace": crate::permissions::ORG_NAMESPACE },
            "spec": {
                "exportedAt": "2026-11-25T00:00:00Z",
                "exportedBy": "jana",
                "sourceRevision": "abc1234",
                "items": [],
                "files": [],
                "omitted": 0,
                "models": models
            }
        }))
        .expect("a Bundle")
    }

    fn planned(name: &str, action: super::ModelAction) -> super::PlannedModel {
        super::PlannedModel {
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            action,
        }
    }

    fn manifest(kind: &str, name: &str, spec: Value) -> String {
        serde_norway::to_string(&json!({
            "apiVersion": jc_core::API_VERSION,
            "kind": kind,
            "metadata": { "name": name, "namespace": "org" },
            "spec": spec,
        }))
        .expect("yaml")
    }

    fn model_spec(name: &str) -> Value {
        json!({
            "linkml": format!("./{name}.linkml.yaml"),
            "version": "1.0.0",
            "lifecycle": "published",
            "classes": ["Station"]
        })
    }

    fn stations_files(manifest: &[u8], source: &[u8]) -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("models/stations.v1.yaml".to_owned(), manifest.to_vec()),
            ("models/stations.v1.linkml.yaml".to_owned(), source.to_vec()),
        ])
    }

    /// MF-49, MF-50: a carried manifest that is no DataModel, names no source or does not read,
    /// a model compiled for nobody, and a source that is not text each refuse the landing by
    /// the file, and nothing is committed.
    #[tokio::test]
    async fn a_carried_model_that_cannot_land_refuses_the_landing() {
        let (server, mounted) = forge().await;
        tree(&server, &[]).await;
        let compiled = BTreeMap::from([(
            "stations".to_owned(),
            crate::tools::model_tools::Artifacts::default(),
        )]);
        let lands = [planned("stations", super::ModelAction::Land)];
        let good = manifest("DataModel", "stations", model_spec("stations"));
        let mut unread = model_spec("stations");
        unread["notAField"] = json!(1);
        let mut invalid = model_spec("stations");
        invalid["classes"] = json!(["station"]);
        for (files, artifacts, needle) in [
            (
                stations_files(manifest("App", "stations", json!({})).as_bytes(), b"id: s"),
                &compiled,
                "is not a DataModel manifest (MF-49)",
            ),
            (
                stations_files(good.as_bytes(), b"id: s"),
                &BTreeMap::new(),
                "'stations' lands uncompiled",
            ),
            (
                stations_files(
                    manifest("DataModel", "stations", json!("text")).as_bytes(),
                    b"id: s",
                ),
                &compiled,
                "names no linkml source",
            ),
            (
                stations_files(
                    manifest("DataModel", "stations", unread).as_bytes(),
                    b"id: s",
                ),
                &compiled,
                "does not read",
            ),
            (
                stations_files(
                    manifest("DataModel", "stations", invalid).as_bytes(),
                    b"id: s",
                ),
                &compiled,
                "PascalCase",
            ),
            (
                stations_files(good.as_bytes(), &[0xff, 0xfe]),
                &compiled,
                "models/stations.v1.linkml.yaml is not UTF-8 text",
            ),
        ] {
            let refused = super::land(
                &mounted,
                &identity(),
                "p",
                &index(&["stations"]),
                &files,
                &lands,
                artifacts,
            )
            .await
            .expect_err(needle);
            assert!(
                format!("{refused:?}").contains(needle),
                "{needle}: {refused:?}"
            );
        }
        let commits = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST")
            .count();
        assert_eq!(commits, 0);
    }

    /// MF-50: the landing reads past what is not one of the project's models (a file gone, one
    /// that does not parse, another kind, a model with no source, one outside its folder, one
    /// whose source is gone) and rewrites only the sources that import the landed model.
    #[tokio::test]
    async fn the_landing_rewrites_only_the_sources_that_import_it() {
        let (server, mounted) = forge().await;
        let folder = "projects/p/datamodels";
        tree(
            &server,
            &[
                "projects/p/README.md",
                &format!("{folder}/gone.yaml"),
                &format!("{folder}/garbled.yaml"),
                &format!("{folder}/app.yaml"),
                &format!("{folder}/nolink.yaml"),
                &format!("{folder}/outside.yaml"),
                &format!("{folder}/lost.yaml"),
                &format!("{folder}/plain.yaml"),
                &format!("{folder}/parking.yaml"),
            ],
        )
        .await;
        holds(&server, &format!("{folder}/garbled.yaml"), "kind: [").await;
        holds(
            &server,
            &format!("{folder}/app.yaml"),
            &manifest("App", "app", json!({})),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/nolink.yaml"),
            &manifest("DataModel", "nolink", json!({})),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/outside.yaml"),
            &manifest(
                "DataModel",
                "outside",
                json!({ "linkml": "../outside.linkml.yaml" }),
            ),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/lost.yaml"),
            &manifest("DataModel", "lost", model_spec("lost")),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/plain.yaml"),
            &manifest("DataModel", "plain", model_spec("plain")),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/plain.linkml.yaml"),
            "id: plain\n",
        )
        .await;
        holds(
            &server,
            &format!("{folder}/parking.yaml"),
            &manifest("DataModel", "parking", model_spec("parking")),
        )
        .await;
        holds(
            &server,
            &format!("{folder}/parking.linkml.yaml"),
            "id: parking\nimports: [linkml:types, org.stations.v1]\n",
        )
        .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c0mm1t" })))
            .mount(&server)
            .await;

        let files = stations_files(
            manifest("DataModel", "stations", model_spec("stations")).as_bytes(),
            b"id: stations\n",
        );
        let compiled = BTreeMap::from([(
            "stations".to_owned(),
            crate::tools::model_tools::Artifacts::default(),
        )]);
        let commit = super::land(
            &mounted,
            &identity(),
            "p",
            &index(&["stations"]),
            &files,
            &[planned("stations", super::ModelAction::Land)],
            &compiled,
        )
        .await
        .expect("landed");
        assert_eq!(commit.as_deref(), Some("c0mm1t"));

        let commits: Vec<Value> = server
            .received_requests()
            .await
            .expect("requests")
            .into_iter()
            .filter(|r| r.method.as_str() == "POST")
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(commits.len(), 1);
        let written: BTreeMap<String, String> = commits[0]["Files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|f| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(f["content"].as_str().expect("content"))
                    .expect("base64");
                (
                    f["path"].as_str().expect("path").to_owned(),
                    String::from_utf8(bytes).expect("utf-8"),
                )
            })
            .collect();
        assert!(
            written.contains_key(&format!("{folder}/stations/stations.yaml")),
            "{written:?}"
        );
        assert!(
            written[&format!("{folder}/parking.linkml.yaml")].contains("project.stations.v1"),
            "{written:?}"
        );
        for untouched in ["plain", "lost", "outside", "nolink"] {
            assert!(
                !written
                    .keys()
                    .any(|p| p.contains(&format!("/{untouched}."))),
                "{untouched}: {written:?}"
            );
        }
    }

    /// DM-02: a landing source that is not text is refused before Model Tools is asked.
    #[tokio::test]
    async fn a_landing_source_that_is_not_text_is_refused() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let files = stations_files(b"", &[0xff, 0xfe]);
        let refused = super::compile(
            &state,
            &index(&["stations"]),
            &files,
            &[planned("stations", super::ModelAction::Land)],
        )
        .await
        .expect_err("not text");
        assert!(
            matches!(&refused, crate::error::ApiError::BadRequest(m) if m == "models/stations.v1.linkml.yaml is not UTF-8 text"),
            "{refused:?}"
        );
    }

    /// DM-76: a landing model compiles with every carried model it may import, a mapped one
    /// under its organization name, and only the landing one is compiled.
    #[tokio::test]
    async fn a_landing_model_compiles_with_the_mapped_ones_as_imports() {
        let tools = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "jsonSchema": { "title": "Station" } })),
            )
            .mount(&tools)
            .await;
        let mut config = crate::config::Config::for_tests();
        config.model_tools_url = Some(tools.uri());
        let state = crate::state::AppState::new(config, None);
        let mut files = stations_files(b"", b"id: stations\nimports: [org.bikes.v1]\n");
        files.insert(
            "models/bikes.v1.linkml.yaml".to_owned(),
            b"id: bikes\n".to_vec(),
        );
        let compiled = super::compile(
            &state,
            &index(&["stations", "bikes"]),
            &files,
            &[
                planned("stations", super::ModelAction::Land),
                planned("bikes", super::ModelAction::Map),
            ],
        )
        .await
        .expect("compiled");
        assert_eq!(compiled.keys().collect::<Vec<_>>(), ["stations"]);
        let asked = tools.received_requests().await.expect("requests");
        assert_eq!(asked.len(), 1);
        let body: Value = serde_json::from_slice(&asked[0].body).expect("json");
        assert_eq!(body["imports"], json!({ "org.bikes.v1": "id: bikes\n" }));
    }

    /// The project `p` holding each `(name, source)` as a model, on a forge, and the
    /// organization holding `stations` with `spec`.
    async fn carrying(
        models: &[(&str, &str)],
        stations: Option<Value>,
    ) -> (MockServer, crate::state::AppState) {
        let (server, client) = forge().await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(client));
        for (name, source) in models {
            let spec = json!({ "linkml": format!("./{name}.linkml.yaml") });
            let folder = super::model_folder("p", &spec, name);
            holds(&server, &format!("{folder}/{name}.linkml.yaml"), source).await;
            state.mirror.upsert(
                serde_json::from_value(json!({
                    "apiVersion": jc_core::API_VERSION,
                    "kind": "DataModel",
                    "metadata": { "name": name, "namespace": "p" },
                    "spec": spec,
                }))
                .expect("a model"),
            );
        }
        if let Some(spec) = stations {
            state.mirror.upsert(
                serde_json::from_value(json!({
                    "apiVersion": jc_core::API_VERSION,
                    "kind": "DataModel",
                    "metadata": { "name": "stations", "namespace": crate::permissions::ORG_NAMESPACE },
                    "spec": spec,
                }))
                .expect("an organization model"),
            );
        }
        (server, state)
    }

    /// MF-49: a source that does not parse, or imports no organization model, carries nothing.
    #[tokio::test]
    async fn sources_that_import_no_organization_model_carry_nothing() {
        let (_server, state) = carrying(
            &[
                ("garbled", "imports: ["),
                ("local", "imports: [linkml:types, project.other.v1]\n"),
            ],
            None,
        )
        .await;
        let (models, files) = super::carry(&state, "p", "bbsk").await.expect("carried");
        assert!(models.is_empty() && files.is_empty());
    }

    /// MF-49: a malformed import, and an organization model whose manifest does not read, refuse
    /// the export.
    #[tokio::test]
    async fn a_malformed_import_or_an_unreadable_organization_model_refuses_the_export() {
        let (_server, state) = carrying(&[("parking", "imports: [org.Bad.v1]\n")], None).await;
        let refused = super::carry(&state, "p", "bbsk")
            .await
            .expect_err("malformed");
        assert!(
            matches!(&refused, crate::error::ApiError::Conflict(m) if m.contains("the project's models") && m.contains("MF-49")),
            "{refused:?}"
        );

        let (_server, state) = carrying(
            &[("parking", "imports: [org.stations.v1]\n")],
            Some(json!({ "linkml": "./stations.linkml.yaml" })),
        )
        .await;
        let refused = super::carry(&state, "p", "bbsk")
            .await
            .expect_err("unreadable");
        assert!(
            matches!(&refused, crate::error::ApiError::Internal(m) if m.starts_with("stations: ")),
            "{refused:?}"
        );
    }

    /// MF-49: an organization model whose manifest is not in the organization repository refuses
    /// the export by its path; once it is there, two models importing it carry it once.
    #[tokio::test]
    async fn an_organization_model_is_carried_once_and_only_with_its_manifest() {
        let spec = json!({
            "linkml": "./stations.linkml.yaml",
            "version": "1.3.0",
            "lifecycle": "published",
            "classes": ["Station"],
            "artifacts": {
                "jsonSchema": "./s.json", "context": "./s.jsonld",
                "docs": "./s.md", "example": "./s.example.json"
            }
        });
        let (server, state) = carrying(
            &[
                ("parking", "imports: [org.stations.v1]\n"),
                ("bikes", "imports: [org.stations.v1]\n"),
            ],
            Some(spec.clone()),
        )
        .await;
        let folder = super::model_folder(crate::permissions::ORG_NAMESPACE, &spec, "stations");
        holds(
            &server,
            &format!("{folder}/stations.linkml.yaml"),
            "id: stations\n",
        )
        .await;
        let refused = super::carry(&state, "p", "bbsk")
            .await
            .expect_err("no manifest");
        assert!(
            matches!(&refused, crate::error::ApiError::Conflict(m) if m == &format!("{folder}/stations.yaml is not in the organization repository")),
            "{refused:?}"
        );

        holds(
            &server,
            &format!("{folder}/stations.yaml"),
            "kind: DataModel\n",
        )
        .await;
        let (models, files) = super::carry(&state, "p", "bbsk").await.expect("carried");
        assert_eq!(models.len(), 1, "{models:?}");
        assert_eq!(models[0].name, "stations");
        assert_eq!(
            files
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["models/stations.v1.yaml", "models/stations.v1.linkml.yaml"]
        );
    }
}
