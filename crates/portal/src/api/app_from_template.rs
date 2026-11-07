//! `POST /api/v1/projects/{project}/app-templates/{name}/apps`: an App started from a template as
//! the gallery ships it, without the model (T-5617, AP-141). The template's source and the
//! sample's own overlay land in a new repository `{project}_{app}`, and the App building from it
//! is proposed as the project's Change with the chosen Endpoint as its data need, under the
//! caller's `propose App`. Nothing here calls the agent proxy, so the path works while the
//! model key is out (AG-96).

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::agents::endpoints::RunEndpoint;
use crate::agents::samples::{self, Template};
use crate::agents::{preview, repository};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::change::{Change, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, GiteaClient};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// What the person chooses: the App's name and the Endpoint it reads.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(example = json!({ "name": "bike-finder", "endpointName": "bikes" }))]
pub struct TemplateApp {
    /// The App's name, a DNS-1123 label: `kpi-board`.
    pub name: String,
    /// The project's Endpoint the App reads, one the caller may read.
    pub endpoint_name: String,
}

fn field(name: &str, detail: String) -> ApiError {
    ApiError::Unprocessable {
        errors: vec![format!("{name}: {detail}")],
        detail: format!("{name}: {detail}"),
    }
}

/// The App's name as a person fixes it: a label, short enough for its repository, not taken.
fn checked_name(state: &AppState, project: &str, name: &str) -> Result<(), ApiError> {
    if !is_dns1123(name) {
        return Err(field(
            "name",
            format!(
                "'{name}' is not an App name: lowercase letters, digits and '-', such as kpi-board"
            ),
        ));
    }
    if let Some(refusal) = repository::name_refusal(project, name) {
        return Err(field("name", refusal));
    }
    if state.mirror.get(project, "App", name).is_some() {
        return Err(field(
            "name",
            format!(
                "the project '{project}' has an App named '{name}' already; choose another name"
            ),
        ));
    }
    Ok(())
}

/// The Endpoint the caller may read, or a 422 naming the field: an Endpoint they may not read
/// answers as one that is not there.
fn readable_endpoint(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
) -> Result<RunEndpoint, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    let readable = state
        .mirror
        .get(project, "Endpoint", name)
        .filter(|endpoint| {
            serde_json::to_value(endpoint)
                .is_ok_and(|value| effective.may_read_manifest("Endpoint", &value))
        });
    if readable.is_none() {
        return Err(field(
            "endpointName",
            format!(
                "'{name}' is not an Endpoint of '{project}' you may read; choose one from the list"
            ),
        ));
    }
    crate::agents::endpoints::resolve(&state.mirror, project, &[name.to_owned()])?
        .pop()
        .ok_or_else(|| ApiError::Internal("an Endpoint resolved to nothing".into()))
}

/// A refusal of the template's needs on the Endpoint, as the field the person picked it in.
fn on_endpoint(endpoint: &str, error: ApiError) -> ApiError {
    let reasons = match error {
        ApiError::Invalid { errors, .. } => errors,
        ApiError::BadRequest(reason) => vec![reason],
        other => return other,
    };
    ApiError::Unprocessable {
        detail: format!("endpointName: '{endpoint}' does not serve what the template reads"),
        errors: reasons
            .into_iter()
            .map(|reason| format!("endpointName: {reason}"))
            .collect(),
    }
}

/// The template's data needs, each in the Endpoint's context space.
pub fn needs_on(template: &Template, endpoint: &RunEndpoint) -> Vec<Value> {
    template
        .data_needs
        .iter()
        .cloned()
        .map(|mut need| {
            need["contextSpaceRef"] = json!({ "kind": "ContextSpace", "name": endpoint.space });
            need
        })
        .collect()
}

/// The path a relative import of `from` names in `tree`: `../App` from
/// `src/pages/TypePage.test.tsx` is `src/App.tsx` when the tree holds it.
fn imported<'a>(from: &str, import: &str, tree: &'a BTreeMap<String, String>) -> Option<&'a str> {
    let mut path: Vec<&str> = from.split('/').collect();
    path.pop();
    for part in import.split('/') {
        match part {
            "." => {}
            ".." => {
                path.pop()?;
            }
            part => path.push(part),
        }
    }
    let base = path.join("/");
    ["", ".ts", ".tsx"]
        .iter()
        .find_map(|extension| tree.get_key_value(&format!("{base}{extension}")))
        .map(|(key, _)| key.as_str())
}

/// Whether a template test imports a file the sample replaces (its `App`, its `fixtures`): it
/// tests the template's version of that file, which the App no longer holds.
fn tests_what_was_replaced(
    path: &str,
    text: &str,
    overlay: &BTreeMap<String, String>,
    tree: &BTreeMap<String, String>,
) -> bool {
    path.contains(".test.")
        && !overlay.contains_key(path)
        && text
            .split("from \"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|import| import.starts_with("./") || import.starts_with("../"))
            .filter_map(|import| imported(path, import, tree))
            .any(|target| overlay.contains_key(target))
}

/// The template, the sample's overlay over it and a README: the App's first commit. The
/// template's tests of what the sample replaced go; the sample's own tests cover that.
pub fn files(
    template: &Template,
    project: &str,
    app: &str,
    clone_url: &str,
) -> Vec<(String, String)> {
    let overlay = samples::overlay(&template.name);
    let mut files: BTreeMap<String, String> = preview::template_files();
    files.extend(overlay.clone());
    let tree = files.clone();
    files.retain(|path, text| !tests_what_was_replaced(path, text, &overlay, &tree));
    files.insert(
        repository::README.to_owned(),
        format!(
            "# {title}\n\nThe application `{app}` of project `{project}`, started from the Portal's \
             template `{template}` as it ships. Clone it with `git clone {clone_url}`, then \
             `pnpm install`, `pnpm test` and `pnpm dev`. It reads only through the project's \
             endpoints, with the signed-in person's own grants, and holds no credential.\n",
            title = template.title,
            template = template.name,
        ),
    );
    files.into_iter().collect()
}

/// The App the Change proposes: a `ui` App building from `source`, reading `needs`.
pub fn manifest(project: &str, app: &str, source: Value, needs: &[Value]) -> Value {
    json!({
        "apiVersion": crate::resource::API_VERSION,
        "kind": "App",
        "metadata": { "name": app, "namespace": project },
        "spec": {
            "kind": jc_core::kinds::AppClass::Ui.to_string(),
            "source": source,
            "build": crate::api::agent_runs::build_toolchains("ui"),
            "visibility": "project",
            "lifecycle": "published",
            "dataNeeds": needs,
        },
    })
}

async fn propose(
    state: &AppState,
    identity: &Identity,
    project: &str,
    app: &str,
    dry_run: bool,
    manifest: Value,
) -> Result<Option<Change>, ApiError> {
    let outcome = crate::api::mutate::propose_with_identity(
        identity,
        state,
        project,
        "apps",
        Some(app),
        Operation::Create,
        dry_run,
        manifest,
    )
    .await?;
    Ok(match outcome {
        crate::api::mutate::ProposeOutcome::Change(change) => Some(change),
        _ => None,
    })
}

/// Fills the new repository, protects `main` and proposes the App at the commit it holds.
async fn land(
    state: &AppState,
    identity: &Identity,
    repo: &GiteaClient,
    project: &str,
    app: &str,
    files: &[(String, String)],
    needs: &[Value],
) -> Result<Change, ApiError> {
    let sha = repo
        .change_files(
            "main",
            "Start the application from its template",
            Author {
                name: &identity.username,
                email: "portal@joinedcontext.local",
            },
            files,
            &[],
        )
        .await?;
    repo.protect_branch("main").await?;
    let source = json!({ "git": { "url": repo.clone_url(), "ref": sha } });
    propose(
        state,
        identity,
        project,
        app,
        false,
        manifest(project, app, source, needs),
    )
    .await?
    .ok_or_else(|| ApiError::Internal("the proposal answered a dry run".into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/app-templates/{name}/apps",
    summary = "Create App From Template",
    description = "Lands the template's source in a new repository `{project}_{app}` and proposes the App building from it, reading the chosen Endpoint; no model is asked (T-5617).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "A template's name"),
    ),
    request_body = TemplateApp,
    responses(
        (status = 202, description = "The Change that proposes the App", body = Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose an App in the project", body = ProblemDetails),
        (status = 404, description = "No such project or template", body = ProblemDetails),
        (status = 409, description = "A repository of that name is already in the forge", body = ProblemDetails),
        (status = 422, description = "The name or the Endpoint is refused; errors names the field", body = ProblemDetails),
        (status = 503, description = "No forge configured", body = ProblemDetails)
    )
)]
pub async fn create_from_template(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<TemplateApp>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let token = crate::api::agent_runs::caller_token(&state, &headers);
    let (template, needs) =
        checked(&state, &user, &project, &name, &request, token.as_deref()).await?;
    let change = create(
        &state,
        &user.0.identity,
        &project,
        &request.name,
        &template,
        &needs,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// Every check before the forge is touched, in the order a person fixes them: the right, the
/// template, the name, the Endpoint, then the caller's own grants on it (AP-132).
async fn checked(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
    request: &TemplateApp,
    token: Option<&str>,
) -> Result<(Template, Vec<Value>), ApiError> {
    let identity = &user.0.identity;
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // The right a run asks for (AP-10, PF-50): the Change at the end is reviewed like any other.
    crate::permissions::for_request(state, identity, project).check(
        "App",
        jc_core::kinds::Verb::Propose,
        None,
    )?;
    let template = samples::templates()
        .into_iter()
        .find(|template| template.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("no App template named '{name}'")))?;
    checked_name(state, project, &request.name)?;
    let endpoint = readable_endpoint(state, identity, project, &request.endpoint_name)?;
    let needs = needs_on(&template, &endpoint);
    let endpoints = std::slice::from_ref(&endpoint);
    crate::agents::needs::validate_data_needs(&state.mirror, project, endpoints, "project", &needs)
        .map_err(|err| on_endpoint(&endpoint.name, err))?;
    crate::agents::held::check(
        crate::agents::held::client(),
        state.config.gateway_url.as_deref(),
        token,
        endpoints,
        &needs,
        &crate::agents::held::portal_reads(state, identity, project),
    )
    .await?;
    Ok((template, needs))
}

/// The proposal as a dry run, then the new repository, its first commit and the Change; a
/// repository already there is never adopted, and one this request created goes again when
/// anything after it fails (CC-85).
async fn create(
    state: &AppState,
    identity: &Identity,
    project: &str,
    app: &str,
    template: &Template,
    needs: &[Value],
) -> Result<Change, ApiError> {
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let repo = gitea.for_application(repository::name(project, app));
    let planned = json!({ "git": { "url": repo.clone_url(), "ref": "main" } });
    propose(
        state,
        identity,
        project,
        app,
        true,
        manifest(project, app, planned, needs),
    )
    .await?;
    let taken = || {
        ApiError::Conflict(format!(
            "name: a repository named '{}' is already in the forge; choose another name",
            repo.repo
        ))
    };
    let description = format!(
        "Application {app} of the project {project}, from the template {}",
        template.name
    );
    if repo.default_branch().await.is_ok() || !repo.ensure_repository(&description).await? {
        return Err(taken());
    }
    let files = files(template, project, app, &repo.clone_url());
    let landed = land(state, identity, &repo, project, app, &files, needs).await;
    if landed.is_err() {
        if let Err(cleanup) = repo.delete_repository().await {
            tracing::error!(repository = %repo.repo, error = %cleanup, "the repository of a template App that failed is left behind");
        }
    }
    landed
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/app-templates/{name}/apps",
        post(create_from_template),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> BTreeMap<String, String> {
        paths
            .iter()
            .map(|path| ((*path).to_owned(), String::new()))
            .collect()
    }

    #[test]
    fn a_relative_import_names_the_file_the_tree_holds() {
        let files = tree(&["src/App.tsx", "src/fixtures.ts", "src/pages/shape.ts"]);
        assert_eq!(
            imported("src/pages/TypePage.test.tsx", "../App", &files),
            Some("src/App.tsx")
        );
        assert_eq!(
            imported("src/pages/TypePage.test.tsx", "./shape", &files),
            Some("src/pages/shape.ts")
        );
        assert_eq!(
            imported("src/App.test.tsx", "./fixtures", &files),
            Some("src/fixtures.ts")
        );
        assert_eq!(imported("src/App.test.tsx", "./missing", &files), None);
        assert_eq!(imported("App.test.tsx", "../../x", &files), None);
    }

    /// Every template lands its own App and tests over the template, keeps the Portal's build,
    /// and leaves no template test of a file it replaced: those failed 9 tests per template in
    /// the build lane's `pnpm test` (T-5617).
    #[test]
    fn every_template_lands_a_tree_whose_tests_test_what_it_holds() {
        let templates = samples::templates();
        assert!(!templates.is_empty(), "the gallery is embedded");
        for template in templates {
            let landed: BTreeMap<String, String> =
                files(&template, "sharing", "app", "https://forge.example/o/r.git")
                    .into_iter()
                    .collect();
            let overlay = samples::overlay(&template.name);
            assert!(overlay.contains_key("src/App.tsx"), "{}", template.name);
            for (path, text) in &overlay {
                assert_eq!(landed.get(path), Some(text), "{} {path}", template.name);
            }
            assert_eq!(
                landed.get(repository::WORKFLOW).map(String::as_str),
                Some(repository::WORKFLOW_TEXT)
            );
            assert!(
                !landed.contains_key("src/pages/TypePage.test.tsx"),
                "{}",
                template.name
            );
            assert!(
                !landed.contains_key("src/components/FeatureMap.test.tsx"),
                "{}",
                template.name
            );
            assert!(
                landed.contains_key("src/endpoints.test.ts"),
                "{}",
                template.name
            );
            for (path, text) in &landed {
                assert!(
                    !tests_what_was_replaced(path, text, &overlay, &landed),
                    "{} {path}",
                    template.name
                );
            }
        }
    }

    #[test]
    fn the_needs_read_the_endpoints_space() {
        let template = samples::templates()
            .into_iter()
            .find(|template| template.name == "citizen-map")
            .expect("the citizen map");
        let endpoint = RunEndpoint {
            name: "bikes".into(),
            slug: "bikes".into(),
            space: "mobility".into(),
        };
        let needs = needs_on(&template, &endpoint);
        assert_eq!(needs.len(), template.data_needs.len());
        assert_eq!(
            needs[0]["contextSpaceRef"],
            json!({ "kind": "ContextSpace", "name": "mobility" })
        );
        serde_json::from_value::<jc_core::kinds::DataNeed>(needs[0].clone())
            .expect("the App kind reads it");
    }
}
