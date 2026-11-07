//! One application exported from a project and imported into another (UI-87, MF-42, MF-46,
//! AP-72, AP-75, T-2879).
//!
//! The export is the App repository's `git bundle` beside the App manifest as the project's
//! repository holds it, under the same `kind: Bundle` index a `format=git` project export
//! writes. The import lands the bundle in a new repository `{project}_{name}` and proposes the
//! App, building from it, as the project's red-lane `Change`. Both are an organization
//! administrator's, on the Administration page.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use jc_core::kinds::{Bundle, BundleFile, BundleRepository, BundleRole, BundleSpec};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::export_git::{application_repository, stored_zip, take};
use crate::api::import::{ImportReport, Verified};
use crate::api::import_git::{unpack, verify_files, GitImport, GitImportPlan, PlannedRepository};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// The App manifest's name inside the archive.
const MANIFEST: &str = "app.yaml";

/// The App manifest at the head of the project's repository, stripped as every export strips it:
/// no status, no digest this environment built, no secret value (MF-17, AP-13a).
async fn manifest(
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(ResourceEnvelope, String), ApiError> {
    let forge = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let info = resource::by_kind("App")
        .ok_or_else(|| ApiError::Internal("the App kind is not registered".into()))?;
    let path = info.repo_path(project, "", name);
    let branch = forge.default_branch().await?;
    let head = forge.branch_head(&branch).await?;
    let file = forge.get_file(&path, &head).await?.ok_or_else(|| {
        ApiError::NotFound(format!(
            "the App '{name}' is not in the project's repository"
        ))
    })?;
    let mut envelope: ResourceEnvelope = serde_norway::from_str(&file.content)
        .map_err(|err| ApiError::Conflict(format!("{path} is not a manifest: {err}")))?;
    envelope.strip_status();
    for key in crate::apps::converge::BUILT_ANNOTATIONS {
        envelope.metadata.annotations.remove(key);
    }
    crate::api::export::strip_secret_values(&mut envelope.spec);
    let text = serde_norway::to_string(&envelope)
        .map_err(|err| ApiError::Internal(format!("the App did not serialise: {err}")))?;
    Ok((envelope, text))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/export",
    summary = "Export App",
    description = "One App as its repository's git bundle beside its manifest, for an organization administrator (UI-87).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The archive: the App's bundle, its tags, app.yaml and bundle.yaml"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization (UI-87)", body = ProblemDetails),
        (status = 404, description = "No such project or App, or not one the caller may read", body = ProblemDetails),
        (status = 409, description = "The App builds from outside the forge's applications organization, or its repository moved during the export", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails)
    )
)]
pub async fn export_app(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::NotFound(format!("App '{name}' not found in '{project}'"));
    if !resource::is_dns1123(&project) || !resource::is_dns1123(&name) {
        return Err(not_found());
    }
    let identity = &user.0.identity;
    if !crate::permissions::for_request(&state, identity, &project).may_read_project() {
        return Err(not_found());
    }
    crate::permissions::require_organization_admin(&state, identity, "exporting an application")?;
    let app = state
        .mirror
        .get(&project, "App", &name)
        .ok_or_else(not_found)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let repository = application_repository(&app, gitea)?.ok_or_else(|| {
        ApiError::Conflict(format!(
            "the App '{name}' names no source.git repository; an App export carries its \
             repository (AP-72)"
        ))
    })?;

    let (envelope, text) = manifest(&state, &project, &name).await?;
    let (bundle, head, tags) = take(&name, &gitea.for_application(repository)).await?;
    let file = format!("{name}.bundle");
    let mut entries = vec![(file.clone(), bundle)];
    if !tags.is_empty() {
        entries.push((format!("{name}.tags"), tags.into_bytes()));
    }
    entries.push((MANIFEST.to_owned(), text.into_bytes()));

    let index = Bundle {
        api_version: jc_core::API_VERSION.to_owned(),
        kind: "Bundle".to_owned(),
        metadata: jc_core::ObjectMeta {
            name: name.clone(),
            // A Bundle is the organization's; the App's project is its item's namespace.
            namespace: Some(crate::permissions::ORG_NAMESPACE.to_owned()),
            ..Default::default()
        },
        spec: BundleSpec {
            exported_at: chrono::Utc::now(),
            exported_by: identity.username.clone(),
            source_instance: None,
            source_revision: head.clone(),
            items: vec![crate::api::export::bundle_item(&envelope, MANIFEST)],
            native_files: Vec::new(),
            files: entries
                .iter()
                .map(|(path, bytes)| BundleFile {
                    path: path.clone(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                })
                .collect(),
            omitted: 0,
            readme: None,
            schemas: None,
            repositories: vec![BundleRepository {
                name: name.clone(),
                role: BundleRole::Application,
                file,
                head: head.clone(),
            }],
            // An App export carries no data model; a project export does (MF-49).
            models: Vec::new(),
        },
        status: None,
    };
    index
        .validate()
        .map_err(|err| ApiError::Internal(format!("the App's bundle index: {err}")))?;
    let index = serde_norway::to_string(&index)
        .map_err(|err| ApiError::Internal(format!("the bundle index did not serialise: {err}")))?;
    entries.push(("bundle.yaml".to_owned(), index.into_bytes()));

    Ok((
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}-app-{}.zip\"", &head[..7]),
            ),
        ],
        stored_zip(&entries)?,
    )
        .into_response())
}

/// The checked archive: its index, its files and the App manifest it carries.
struct Checked {
    index: Bundle,
    files: std::collections::BTreeMap<String, Vec<u8>>,
    app: ResourceEnvelope,
}

/// Every check that runs before anything is created (MF-42, MF-46, MF-47).
fn check(bytes: &[u8]) -> Result<Checked, ApiError> {
    let files = unpack(bytes)?;
    let index = files.get("bundle.yaml").ok_or_else(|| {
        ApiError::BadRequest(
            "the archive has no bundle.yaml; import the archive of an App export (UI-87)".into(),
        )
    })?;
    let index: Bundle = serde_norway::from_slice(index)
        .map_err(|err| ApiError::BadRequest(format!("bundle.yaml is not a Bundle: {err}")))?;
    index
        .validate()
        .map_err(|err| ApiError::BadRequest(format!("bundle.yaml: {err}")))?;
    if index.spec.repositories.len() != 1
        || index.spec.repositories[0].role != BundleRole::Application
    {
        return Err(ApiError::BadRequest(
            "bundle.yaml lists one application repository and nothing else in an App export; \
             a project's export is imported with format=git (MF-45)"
                .into(),
        ));
    }
    if index.spec.items.len() != 1 || index.spec.items[0].kind != "App" {
        return Err(ApiError::BadRequest(
            "bundle.yaml lists one App as its item in an App export (UI-87)".into(),
        ));
    }
    verify_files(&index, &files)?;
    if !index.spec.files.iter().any(|f| f.path == MANIFEST) {
        return Err(ApiError::BadRequest(
            "bundle.yaml carries no checksum of app.yaml; export again (MF-42)".into(),
        ));
    }
    let text = files.get(MANIFEST).ok_or_else(|| {
        ApiError::BadRequest("the archive has no app.yaml beside its bundle".into())
    })?;
    let text = String::from_utf8_lossy(text);
    let app: ResourceEnvelope = serde_norway::from_str(&text)
        .map_err(|err| ApiError::BadRequest(format!("app.yaml is not a manifest: {err}")))?;
    if app.api_version != jc_core::API_VERSION || app.kind != "App" {
        return Err(ApiError::BadRequest(format!(
            "app.yaml is {} {}, and this release imports App at {}: migrate an older one with \
             `jcctl migrate` and export it again (MF-47)",
            app.api_version,
            app.kind,
            jc_core::API_VERSION
        )));
    }
    if let Some(checked) = jc_core::registry::validate_yaml("App", &text) {
        checked
            .map_err(|err| ApiError::BadRequest(format!("app.yaml is not a valid App: {err}")))?;
    }
    Ok(Checked { index, files, app })
}

/// The import behind `POST /api/v1/projects/{project}/import?format=app`. The caller is an
/// organization administrator, which the route asked before the body was read.
pub async fn import(
    state: &AppState,
    identity: &Identity,
    project: &str,
    bytes: &[u8],
    input: GitImport,
) -> Result<(StatusCode, Value), ApiError> {
    if !input.parameters.is_empty() || input.display_name.is_some() {
        return Err(ApiError::BadRequest(
            "an App import takes file, name and dryRun; parameters and displayName belong to a \
             project import"
                .into(),
        ));
    }
    if state.mirror.layout() != 2 {
        return Err(ApiError::Conflict(
            "an App lands in a repository of its own beside a project of layout 2, and this \
             organization is of layout 1 (AP-75)"
                .into(),
        ));
    }
    if state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)
        .is_none()
    {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let Checked { index, files, app } = check(bytes)?;
    let name = input
        .name
        .clone()
        .unwrap_or_else(|| app.metadata.name.clone());
    if !resource::is_dns1123(&name) {
        return Err(ApiError::BadRequest(format!(
            "'{name}' is not an App name: lowercase letters, digits and '-', at most 63"
        )));
    }
    if let Some(refusal) = crate::agents::repository::name_refusal(project, &name) {
        return Err(ApiError::BadRequest(refusal));
    }
    if state.mirror.get(project, "App", &name).is_some() {
        return Err(ApiError::Conflict(format!(
            "the project '{project}' has an App named '{name}' already; import it under another \
             name"
        )));
    }
    let bundled = &index.spec.repositories[0];
    let target = crate::agents::repository::name(project, &name);
    let client = gitea.for_application(target.clone());
    if client.default_branch().await.is_ok() {
        return Err(ApiError::Conflict(format!(
            "a repository named '{target}' is already in the forge; an import lands only in a \
             new one (PF-86)"
        )));
    }

    // PF-57: the dry run is the check, over the archive as sent, the project and the name.
    let subject = serde_json::json!({
        "bundle": format!("{:x}", Sha256::digest(bytes)),
        "format": "app",
        "targetNamespace": project,
        "name": name,
    });
    let planned = PlannedRepository {
        name: bundled.name.clone(),
        role: "application".to_owned(),
        repository: target.clone(),
        head: bundled.head.clone(),
    };
    if input.dry_run {
        crate::ops::record_import_check(state, identity, project, &subject).await;
        let plan = GitImportPlan {
            repositories: vec![planned],
            parameters: Value::Object(Default::default()),
            models: Vec::new(),
        };
        return Ok((
            StatusCode::OK,
            serde_json::to_value(plan).map_err(|e| ApiError::Internal(e.to_string()))?,
        ));
    }
    crate::ops::verdict_for_import(state, identity, project, &subject).await?;

    if !client
        .create_empty_repository(&format!("application {name} of the project {project}"))
        .await?
    {
        return Err(ApiError::Conflict(format!(
            "a repository named '{target}' appeared in the forge during the import"
        )));
    }
    let landing = Landing {
        project,
        name: &name,
        origin: &format!(
            "{}/{}",
            index.spec.items[0].namespace.as_deref().unwrap_or_default(),
            index.spec.items[0].name
        ),
        index: &index,
        files: &files,
        app,
        client: &client,
    };
    match land(state, identity, landing).await {
        Ok(change) => {
            crate::ops::forget_import_check(state, identity, project).await;
            Ok((
                StatusCode::ACCEPTED,
                serde_json::to_value(change).map_err(|e| ApiError::Internal(e.to_string()))?,
            ))
        }
        // Both or neither (CC-85): the repository this import created goes again.
        Err(error) => {
            if let Err(cleanup) = client.delete_repository().await {
                tracing::error!(repository = %client.repo, error = %cleanup, "the repository of an App import that failed is left behind");
            }
            Err(error)
        }
    }
}

/// What one App import lands.
struct Landing<'a> {
    project: &'a str,
    name: &'a str,
    origin: &'a str,
    index: &'a Bundle,
    files: &'a std::collections::BTreeMap<String, Vec<u8>>,
    app: ResourceEnvelope,
    client: &'a crate::git::GiteaClient,
}

/// Fills and verifies the new repository and proposes the App that builds from it.
async fn land(
    state: &AppState,
    identity: &Identity,
    landing: Landing<'_>,
) -> Result<crate::change::Change, ApiError> {
    let Landing {
        project,
        name,
        origin,
        index,
        files,
        mut app,
        client,
    } = landing;
    let bundled = &index.spec.repositories[0];
    let mut refs = vec![(bundled.head.clone(), "refs/heads/main".to_owned())];
    if let Some(listed) = files.get(&format!("{}.tags", bundled.name)) {
        refs.extend(crate::api::import_git::tags(listed, &bundled.name)?);
    }
    let mut refused_tags = Vec::new();
    for refused in client.push_bundle(&files[&bundled.file], &refs).await? {
        if refused.starts_with("refs/heads/") {
            return Err(ApiError::Conflict(format!(
                "the forge refused {refused} of '{}'",
                client.repo
            )));
        }
        refused_tags.push(format!("{}: {refused}", client.repo));
    }
    // MF-46: the head the forge now holds is the head the index lists.
    let head = client.branch_head("main").await?;
    if head != bundled.head {
        return Err(ApiError::Conflict(format!(
            "the repository '{}' ends at {head} after the push, and bundle.yaml lists {} (MF-46)",
            client.repo, bundled.head
        )));
    }
    client.protect_branch("main").await?;

    // The App as exported, under its name here, building from its new repository.
    app.metadata.name = name.to_owned();
    app.metadata.namespace = Some(project.to_owned());
    let source = app
        .spec
        .pointer_mut("/source/git")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::BadRequest("app.yaml names no source.git".into()))?;
    source.insert("url".to_owned(), Value::String(client.clone_url()));
    let info = resource::by_kind("App")
        .ok_or_else(|| ApiError::Internal("the App kind is not registered".into()))?;
    let path = info.repo_path(project, "", name);
    let text = serde_norway::to_string(&app)
        .map_err(|err| ApiError::Internal(format!("the App did not serialise: {err}")))?;

    let report = ImportReport {
        created: vec![path.clone()],
        replaced: Vec::new(),
        skipped: refused_tags,
        renamed: Default::default(),
        reassigned: Default::default(),
        native_files: 0,
        // An import is red lane, whatever it carries (UI-87).
        lane: crate::change::Lane::Red,
        source: Some(format!("{origin} at {}", index.spec.source_revision)),
        verified: vec![Verified {
            path: bundled.file.clone(),
            equal: true,
        }],
        needs: Vec::new(),
    };
    crate::api::import::propose_bundle(
        state,
        identity,
        project,
        report,
        vec![(path, text)],
        Some(("App", name)),
    )
    .await
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/apps/{name}/export", get(export_app))
}
