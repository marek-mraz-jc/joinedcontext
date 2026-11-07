use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{self, API_VERSION};
use crate::state::AppState;

/// The projects the configuration repository holds (PF-05): one per `projects/<slug>/`
/// directory the mirror has a manifest from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectList {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<ProjectSummary>,
    /// The Organizations that own a listed project, by name, when the installation holds more
    /// than one (PF-124, ADR-N-058 section 2.7): what the organization switcher offers. Empty
    /// with one Organization, so a person who sees one gets no switcher.
    pub organizations: Vec<OrganizationSummary>,
}

/// One Organization a caller sees (PF-124): its name, the `?org=` value, and its title.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationSummary {
    pub name: String,
    /// The `Organization` manifest's `metadata.title`, absent when it names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(schema_with = crate::openapi::text_ref)]
    pub title: Option<jc_core::i18n::Text>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    /// The project slug, the `{project}` segment of every other path.
    pub name: String,
    /// Whether its `Project` manifest carries [`SAMPLE_LABEL`]: real open data a newcomer may look
    /// around in (PF-109).
    pub sample: bool,
    /// The `Project` manifest's `metadata.title`: one string, or the legacy map per locale;
    /// absent when it names none (T-4243). The switcher shows it, the slug beside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(schema_with = crate::openapi::text_ref)]
    pub title: Option<jc_core::i18n::Text>,
    /// The Organization that owns the project, its `spec.organizationRef` (PF-122).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
}

/// The label that marks a project as the sample of real open data the first-run checklist offers
/// (PF-109, T-3234). It grants nothing: who may read the project is their grants' question alone.
pub const SAMPLE_LABEL: &str = "joinedcontext.com/sample";

/// Whether `name` is a project the caller may read: the one rule of the project list and of every
/// field that names a project (T-4948). An unknown project reads like an unreadable one.
pub fn readable(state: &AppState, identity: &crate::auth::session::Identity, name: &str) -> bool {
    state.mirror.namespaces().iter().any(|known| known == name)
        && crate::permissions::for_request(state, identity, name).may_read_project()
}

/// The projects this caller may read, and no others (PF-59, T-0974).
///
/// A project a caller has no grant in answers `404` everywhere else, so naming it here would
/// hand out the organization's internal project and department list to anyone with a session.
/// The filter is the same question the resource lists ask, `may_read_project`, asked once per
/// project the mirror holds.
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    tag = "resources",
    responses(
        (status = 200, description = "Projects present in the configuration repository", body = ProjectList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_projects(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ProjectList>, ApiError> {
    let items: Vec<ProjectSummary> = state
        .mirror
        .namespaces()
        .into_iter()
        .filter(|name| readable(&state, &user.0.identity, name))
        .map(|name| {
            let manifest = state
                .mirror
                .get(crate::permissions::ORG_NAMESPACE, "Project", &name);
            let sample = manifest.as_ref().is_some_and(|project| {
                project
                    .metadata
                    .labels
                    .get(SAMPLE_LABEL)
                    .map(String::as_str)
                    == Some("true")
            });
            let title = manifest.and_then(|project| project.metadata.title);
            let organization = crate::permissions::organization_of(&state.mirror, &name);
            ProjectSummary {
                name,
                sample,
                title,
                organization,
            }
        })
        .collect();
    let organizations = organizations_seen(&state, &items);
    Ok(Json(ProjectList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        items,
        organizations,
    }))
}

/// The Organizations owning one of `items`, in name order, when there are two or more (PF-124).
/// Only what the caller reads already: an Organization none of whose projects they read stays
/// unnamed, as the project list keeps an unreadable project unnamed.
fn organizations_seen(state: &AppState, items: &[ProjectSummary]) -> Vec<OrganizationSummary> {
    let names: std::collections::BTreeSet<&str> = items
        .iter()
        .filter_map(|item| item.organization.as_deref())
        .collect();
    if names.len() < 2 {
        return Vec::new();
    }
    names
        .into_iter()
        .map(|name| OrganizationSummary {
            name: name.to_owned(),
            title: state
                .mirror
                .get(crate::permissions::ORG_NAMESPACE, "Organization", name)
                .and_then(|organization| organization.metadata.title),
        })
        .collect()
}

/// One quota dimension of a project: what it holds and what it may (PF-75).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub used: u32,
    /// Absent when no quota limits this dimension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    /// Every countable dimension by its manifest field name (`contextSpaces`,
    /// `residentPipelines`, `publicEndpoints`, `apps`).
    #[schema(value_type = Object)]
    pub usage: std::collections::BTreeMap<String, Usage>,
}

/// One project as the Portal holds it, with what it is using of its quota (PF-75).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub api_version: String,
    pub kind: String,
    #[schema(value_type = Object)]
    pub metadata: Value,
    #[schema(value_type = Object)]
    pub spec: Value,
    pub status: ProjectStatus,
}

/// `GET /api/v1/projects/{project}`: the project and what it holds of each quota, so a person
/// sees the limit before the verdict does (PF-75). A project no binding of the caller covers is
/// `404`, like every other read of it (PF-59, R20).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}",
    summary = "Read A Project",
    description = "The project and what it holds of each quota: context spaces, resident pipelines, public endpoints and apps.",
    tag = "resources",
    params(("project" = String, Path, description = "Project slug")),
    responses(
        (status = 200, description = "The project and its quota usage", body = ProjectDetail),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No binding of the caller covers the project", body = ProblemDetails)
    )
)]
pub async fn get_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Result<Json<ProjectDetail>, ApiError> {
    let identity = &user.0.identity;
    let not_found = || ApiError::NotFound(format!("project '{project}' not found"));
    if !crate::permissions::for_request(&state, identity, &project).may_read_project() {
        return Err(not_found());
    }
    let manifest = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", &project);
    // A project directory the repository holds without a Project manifest is still a project:
    // its usage is real and the page should show it (PF-05).
    if manifest.is_none()
        && !state
            .mirror
            .namespaces()
            .iter()
            .any(|held| held == &project)
    {
        return Err(not_found());
    }

    let quotas = crate::quotas::effective(&state.mirror, &project);
    let limits = crate::quotas::limits(&quotas);
    let usage = crate::quotas::usage(&state.mirror, &project)
        .into_iter()
        .map(|(dimension, used)| {
            let limit = limits.get(&dimension).copied();
            (dimension, Usage { used, limit })
        })
        .collect();

    Ok(Json(ProjectDetail {
        api_version: API_VERSION.to_string(),
        kind: "Project".to_string(),
        metadata: manifest
            .as_ref()
            .and_then(|env| serde_json::to_value(&env.metadata).ok())
            .unwrap_or_else(
                || json!({ "name": project, "namespace": crate::permissions::ORG_NAMESPACE }),
            ),
        spec: manifest.map(|env| env.spec).unwrap_or(Value::Null),
        status: ProjectStatus { usage },
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects).post(open_project))
        .route(
            "/projects/{project}",
            get(get_project).delete(delete_project),
        )
        .route(
            "/projects/{project}/duplicate",
            axum::routing::post(duplicate_project),
        )
        .route(
            "/projects/{project}/registry",
            get(get_registry_entry).put(repoint_project),
        )
}

// ---------------------------------------------------------------------------
// Opening a project (PF-65, PF-66, PF-67, T-0869)
// ---------------------------------------------------------------------------

/// What the organization's own manifest says about who may open a project (PF-65).
///
/// `anyone` is every signed-in person, `group:<name>` the members of one `Group`, and
/// `org-admin` — the default — whoever holds `propose` on `Project`. The setting is read
/// before a `Change` exists, because a person who may not open a project must not be able to
/// open a merge request that says they did (PF-65).
pub fn may_open(
    state: &AppState,
    identity: &crate::auth::session::Identity,
) -> Result<(), ApiError> {
    match creation(state).as_str() {
        "anyone" => Ok(()),
        group if group.starts_with("group:") => {
            let name = group.trim_start_matches("group:");
            if in_group(state, identity, name) {
                Ok(())
            } else {
                Err(ApiError::Denied(format!(
                    "opening a project here is for the members of group '{name}' (PF-65)"
                )))
            }
        }
        _ => crate::permissions::for_request(state, identity, crate::permissions::ORG_NAMESPACE)
            .check("Project", jc_core::kinds::Verb::Propose, None)
            .map_err(|_| {
                ApiError::Denied(
                    "opening a project here needs propose on Project, which org-admin holds \
                     (PF-65)"
                        .to_owned(),
                )
            }),
    }
}

/// The same answer as [`may_open`], in the shape `permissions/me` carries to the UI: the "New
/// project" control is enabled or disabled with this reason, and never hidden (UI-44, PF-65).
pub fn creation_affordance(
    state: &AppState,
    identity: &crate::auth::session::Identity,
) -> crate::permissions::Affordance {
    match may_open(state, identity) {
        Ok(()) => crate::permissions::Affordance {
            allowed: true,
            reason: None,
        },
        Err(err) => crate::permissions::Affordance {
            allowed: false,
            reason: Some(err.to_string()),
        },
    }
}

/// Whether the caller is a member of the named group: the `Group` manifest first, which is the
/// configuration (PF-62), and the identity provider's own groups as long as no manifest names
/// them (PF-63 is what makes the two agree).
fn in_group(state: &AppState, identity: &crate::auth::session::Identity, name: &str) -> bool {
    if let Some(group) = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Group", name)
    {
        let members = group
            .spec
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return members.iter().any(|member| {
            let named = member
                .get("user")
                .and_then(Value::as_str)
                .or_else(|| member.as_str())
                .unwrap_or_default();
            !named.is_empty() && crate::permissions::names_user(identity, named)
        });
    }
    identity.groups.iter().any(|held| held == name)
}

/// What a person fills in to open a project.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OpenProject {
    /// The slug: the `{project}` segment of every path of it (PF-67).
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /api/v1/projects`: opens a project, with the opener's steward binding in the same
/// change (PF-65, PF-66, PF-67).
#[utoipa::path(
    post,
    path = "/api/v1/projects",
    summary = "Open A Project",
    description = "Opens a project, with the opener's steward binding in the same change; the organization's own setting says who may.",
    tag = "resources",
    request_body(
        content = OpenProject,
        example = json!({ "name": "helsinki", "displayName": "Helsinki", "description": "The city's open data" })
    ),
    responses(
        (status = 202, description = "The change that opens the project", body = Change),
        (status = 400, description = "The name is not a DNS-1123 label", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The organization does not let this caller open a project", body = ProblemDetails),
        (status = 409, description = "A project of that name exists or is already proposed", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn open_project(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<OpenProject>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let identity = &user.0.identity;
    may_open(&state, identity)?;

    let name = request.name.trim().to_owned();
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    new_project_name(&state, gitea, &name).await?;

    let mut files = open_project_files(&state, identity, &request, &name)?;
    // Layout 2 (CC-85, PF-86): the project gets a repository of its own, seeded here, and the
    // organization's Change carries its registry entry in place of the project's own file.
    let own_repository = if state.mirror.layout() == 2 {
        let repository = gitea.for_project(&name, &name);
        seed_project_repository(&repository, identity, &name, &mut files).await?;
        Some(repository)
    } else {
        None
    };
    // Yellow: a project and its opener's own steward binding are reviewed, not confirmed by
    // typing a name back (PF-66).
    let report = crate::api::import::ImportReport {
        created: files.iter().map(|(path, _)| path.clone()).collect(),
        replaced: Vec::new(),
        skipped: Vec::new(),
        renamed: Default::default(),
        reassigned: Default::default(),
        native_files: 0,
        lane: crate::change::Lane::Yellow,
        source: None,
        verified: Vec::new(),
        needs: Vec::new(),
    };
    let proposed = crate::api::import::propose_bundle(
        &state,
        identity,
        &name,
        report,
        files,
        Some(("Project", &name)),
    )
    .await;
    let mut change = match (proposed, own_repository) {
        (Ok(change), _) => change,
        (Err(error), None) => return Err(error),
        // Both or neither: a repository nobody registered is removed again (CC-85).
        (Err(error), Some(repository)) => {
            if let Err(cleanup) = repository.delete_repository().await {
                tracing::error!(project = %name, error = %cleanup, "the repository of a project that did not open is left behind");
            }
            return Err(error);
        }
    };

    // The organization that lets anyone open a project, on an installation that does not hold
    // every change for a person, has nobody to wait for: the platform merges it and the merge
    // message says so (PF-66, PF-57).
    if lets_anyone_open(&state) && state.branding().validation == crate::branding::Validation::Lax {
        let (_, number) = crate::api::changes::parse_change_ref(&change.metadata.name)?;
        gitea
            .merge(
                number,
                crate::git::MergeStyle::Squash,
                &format!(
                    "Merge change proposal {}: open project {name}\n\nMerged by the platform: \
                     the organization lets anyone open a project and this installation runs \
                     platform.validation: lax (PF-65, PF-66, PF-57)",
                    change.metadata.name
                ),
                // Nothing was reviewed: the Portal wrote this bundle and merges it in the same
                // call, so there is no commit an approver read to pin it to (T-1683).
                None,
            )
            .await?;
        // Nothing is waiting for a person, so the answer says so and the UI opens the project
        // itself instead of a change nobody will approve (PF-66).
        change.status.phase = crate::change::ChangePhase::Merged;
        if let Some(syncer) = state.syncer.as_ref() {
            let syncer = syncer.clone();
            tokio::spawn(async move {
                if let Err(err) = syncer.sync_once().await {
                    tracing::warn!(error = %err, "sync after opening a project failed");
                }
            });
        }
    }

    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// The checks a new project's slug passes, whether it is opened or duplicated (PF-67, PF-78):
/// a DNS-1123 label, not the organization's namespace, not held, not reserved by a deleted
/// project's cooling period, not proposed by an open change.
pub(crate) async fn new_project_name(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    name: &str,
) -> Result<(), ApiError> {
    if !resource::is_dns1123(name) {
        return Err(ApiError::BadRequest(format!(
            "project name '{name}' is not a DNS-1123 label: lowercase letters, digits and \
             hyphens, starting and ending with a letter or a digit (PF-67)"
        )));
    }
    if name == crate::permissions::ORG_NAMESPACE {
        return Err(ApiError::BadRequest(format!(
            "'{name}' is the organization's own namespace and is not a project (PF-67)"
        )));
    }
    if state.mirror.namespaces().iter().any(|held| held == name)
        || state
            .mirror
            .get(crate::permissions::ORG_NAMESPACE, "Project", name)
            .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "project '{name}' already exists (PF-67)"
        )));
    }
    // A name a deleted project held stays reserved for the organization's cooling period, so
    // nobody opens a project that inherits another one's URNs, dashboards and links (PF-78).
    if let Some(free) = reserved_until(state, gitea, name).await {
        return Err(ApiError::Conflict(format!(
            "project '{name}' was deleted and its name stays reserved until {} (PF-78)",
            free.format("%Y-%m-%d")
        )));
    }
    // A name reserved by an open change is taken, even though nothing is merged yet (PF-67).
    let open = gitea.list_pull_requests("open").await?;
    let reserved = format!("portal/create-project-{name}-");
    if open.iter().any(|pr| pr.head_branch.starts_with(&reserved)) {
        return Err(ApiError::Conflict(format!(
            "project '{name}' is already proposed and waiting for approval (PF-67)"
        )));
    }
    Ok(())
}

fn lets_anyone_open(state: &AppState) -> bool {
    creation(state) == "anyone"
}

/// Who may open a project: the installation Organization's `projects.creation`, which a new
/// project belongs to (ADR-N-058 section 2.2); another Organization's setting opens nothing
/// here. `org-admin` when it sets none.
fn creation(state: &AppState) -> String {
    crate::permissions::installation_organization_manifest(&state.mirror)
        .and_then(|org| {
            org.spec
                .pointer("/projects/creation")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "org-admin".to_owned())
}

/// The two files a new project is: its manifest, and the binding that makes its opener the
/// steward of it and of nothing else (PF-66).
pub(crate) fn open_project_files(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    request: &OpenProject,
    name: &str,
) -> Result<Vec<(String, String)>, ApiError> {
    // A new project belongs to the installation's Organization (ADR-N-058 section 2.2); moving
    // it to another is a change of its `organizationRef`.
    let organization =
        crate::permissions::installation_organization(&state.mirror).ok_or_else(|| {
            ApiError::Unavailable(
                "the organization's own manifest is not in the mirror yet; a project belongs to \
                 one (PF-66)"
                    .to_owned(),
            )
        })?;

    let mut metadata = json!({ "name": name, "namespace": crate::permissions::ORG_NAMESPACE });
    if let Some(title) = request
        .display_name
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        metadata["labels"] = json!({ "joinedcontext.com/display-name": title });
    }
    if let Some(about) = request
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        metadata["annotations"] = json!({ "joinedcontext.com/description": about });
    }
    let project = json!({
        "apiVersion": API_VERSION,
        "kind": "Project",
        "metadata": metadata,
        // A bare name: jc-core's `Ref` takes a name or a `{kind, name}` pair, and a `{name}`
        // alone made every project this door opened a manifest the loader refuses (T-2643).
        "spec": { "organizationRef": organization },
    });

    // The opener gets `steward` on their own project and nothing anywhere else (PF-66, PF-52).
    let who = identity
        .email
        .clone()
        .unwrap_or_else(|| identity.username.clone());
    let binding = json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": {
            "name": format!("{name}-creator"),
            "namespace": crate::permissions::ORG_NAMESPACE,
        },
        "spec": {
            "subjects": [{ "user": who }],
            "role": "steward",
            "scope": { "project": name },
        },
    });

    let yaml = |value: &Value| -> Result<String, ApiError> {
        serde_norway::to_string(value)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))
    };
    Ok(vec![
        (format!("projects/{name}/project.yaml"), yaml(&project)?),
        (
            format!("users/assignments/{name}-creator.yaml"),
            yaml(&binding)?,
        ),
    ])
}

/// Replaces the project's own file in `files` with its registry entry `projects/{name}.yaml`:
/// the same manifest naming the repository `repo` at `main`, with `parameters` when given
/// (PF-86, CC-88). Answers the project's own manifest it took out.
pub(crate) fn into_registry_entry(
    files: &mut Vec<(String, String)>,
    name: &str,
    repo: &str,
    parameters: Option<&serde_json::Map<String, Value>>,
) -> Result<Value, ApiError> {
    let own_path = format!("projects/{name}/project.yaml");
    let position = files
        .iter()
        .position(|(path, _)| *path == own_path)
        .ok_or_else(|| ApiError::Internal("the project's own manifest is missing".into()))?;
    let (_, own) = files.remove(position);
    let project: Value = serde_norway::from_str(&own)
        .map_err(|e| ApiError::Internal(format!("the project manifest did not parse: {e}")))?;
    let mut entry = project.clone();
    entry["spec"]["repository"] = json!({ "name": repo });
    entry["spec"]["ref"] = json!("main");
    if let Some(parameters) = parameters.filter(|p| !p.is_empty()) {
        entry["spec"]["parameters"] = Value::Object(parameters.clone());
    }
    let entry = serde_norway::to_string(&entry)
        .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
    files.insert(position, (format!("projects/{name}.yaml"), entry));
    Ok(project)
}

/// The CI of every project repository (CC-90), in its first commit.
const VALIDATE_WORKFLOW: &str = include_str!("project-validate.yml");

/// Creates the project repository of layout 2 and commits its first files to `main`: `.jc/layout`,
/// the project's own `project.yaml` at version `0.1.0`, `CODEOWNERS` and the CI workflow (CC-85,
/// CC-90, PF-86, PF-87, PF-88).
/// `main` then takes no direct push. `files` loses the project's own file and gains the
/// registry entry `projects/{name}.yaml`, which the organization's Change carries.
///
/// A repository of that name that is already there is refused, never adopted: somebody else's
/// history would become the project's. A failure after the repository exists removes it.
async fn seed_project_repository(
    repository: &crate::git::GiteaClient,
    identity: &crate::auth::session::Identity,
    name: &str,
    files: &mut Vec<(String, String)>,
) -> Result<(), ApiError> {
    let own_path = format!("projects/{name}/project.yaml");
    let mut project = into_registry_entry(files, name, &repository.repo, None)?;
    project["spec"]["version"] = json!("0.1.0");
    let yaml = |value: &Value| -> Result<String, ApiError> {
        serde_norway::to_string(value)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))
    };

    if !repository
        .ensure_repository(&format!("The configuration of the project {name}"))
        .await?
    {
        return Err(ApiError::Conflict(format!(
            "a repository named '{}' is already in the forge; a project opens only into a new one \
             (PF-86)",
            repository.repo
        )));
    }
    let seed = vec![
        (
            format!("projects/{name}/{}", jc_core::project::LAYOUT_FILE),
            "2\n".to_owned(),
        ),
        (own_path, yaml(&project)?),
        (
            format!("projects/{name}/CODEOWNERS"),
            format!("* @{}/{name}-writers\n", repository.owner),
        ),
        (
            format!("projects/{name}/.gitea/workflows/validate.yml"),
            VALIDATE_WORKFLOW.to_owned(),
        ),
    ];
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, name);
    let author = crate::git::Author {
        name: &author_name,
        email: &author_email,
    };
    let seeded = async {
        repository
            .change_files(
                "main",
                &format!("Open the project {name} (PF-86)"),
                author,
                &seed,
                &[],
            )
            .await?;
        repository.protect_branch("main").await?;
        repository.let_the_reader_in().await
    }
    .await;
    if let Err(error) = seeded {
        if let Err(cleanup) = repository.delete_repository().await {
            tracing::error!(project = %name, error = %cleanup, "the repository of a project that did not open is left behind");
        }
        return Err(error.into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Duplicating a project (PF-89)
// ---------------------------------------------------------------------------

/// The new slug of a duplicate and this deployment's own values.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DuplicateProject {
    /// The copy's slug: the `{project}` segment of every path of it (PF-67).
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// Values for the parameters the origin's `project.yaml` declares (CC-88).
    #[serde(default)]
    #[schema(value_type = Object)]
    pub parameters: serde_json::Map<String, Value>,
}

/// `POST /api/v1/projects/{project}/duplicate`: copies a project of layout 2 into a new
/// repository with its history and proposes its registry entry under a new slug (PF-89).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/duplicate",
    summary = "Duplicate A Project",
    description = "Copies the project's repository with its history under a new slug and proposes its registry entry and the caller's steward binding; the copy's endpoints get slugs of their own.",
    tag = "resources",
    params(("project" = String, Path, description = "Project slug of the origin")),
    request_body(
        content = DuplicateProject,
        example = json!({ "name": "helsinki-test", "displayName": "Helsinki (test)", "parameters": {} })
    ),
    responses(
        (status = 202, description = "The change that registers the copy", body = Change),
        (status = 400, description = "The name is not a DNS-1123 label, or a parameter does not fit the origin's declarations", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The organization does not let this caller open a project", body = ProblemDetails),
        (status = 404, description = "No binding of the caller covers the origin", body = ProblemDetails),
        (status = 409, description = "The name is taken, or the organization is not of layout 2", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn duplicate_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(origin): axum::extract::Path<String>,
    Json(request): Json<DuplicateProject>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let identity = &user.0.identity;
    if !crate::permissions::for_request(&state, identity, &origin).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{origin}' not found")));
    }
    may_open(&state, identity)?;
    if state.mirror.layout() != 2 {
        return Err(ApiError::Conflict(
            "a project of layout 1 has no repository of its own to copy; import its export \
             under the new name instead (PF-89, MF-45)"
                .into(),
        ));
    }
    let origin_repo = state
        .mirror
        .repository_of(&origin)
        .ok_or_else(|| ApiError::NotFound(format!("project '{origin}' not found")))?;
    let name = request.name.trim().to_owned();
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    new_project_name(&state, gitea, &name).await?;

    let source = gitea.for_project(&origin_repo, &origin);
    let mut files = open_project_files(
        &state,
        identity,
        &OpenProject {
            name: name.clone(),
            display_name: request.display_name.clone(),
            description: None,
        },
        &name,
    )?;
    into_registry_entry(&mut files, &name, &name, Some(&request.parameters))?;
    check_parameters(&source, &origin, &name, &files).await?;

    let copy = gitea.for_project(&name, &name);
    if !copy.migrate_repository(&source).await? {
        return Err(ApiError::Conflict(format!(
            "a repository named '{name}' is already in the forge; a duplicate goes only into a \
             new one (PF-89)"
        )));
    }
    let prepared = async {
        // A copy in the same organization never answers at the origin's URLs, so every slug is
        // drawn anew (EP-02).
        remount_copy(
            &copy,
            identity,
            &Remount {
                origin: &origin,
                name: &name,
                fresh_slug: &|_| true,
                applications: &BTreeMap::new(),
                keep_secrets: false,
                message: format!("Duplicate {origin} as {name} (PF-89)"),
            },
        )
        .await?;
        copy.protect_branch("main").await?;
        copy.let_the_reader_in().await?;
        let report = crate::api::import::ImportReport {
            created: files.iter().map(|(path, _)| path.clone()).collect(),
            replaced: Vec::new(),
            skipped: Vec::new(),
            renamed: Default::default(),
            reassigned: Default::default(),
            native_files: 0,
            lane: crate::change::Lane::Yellow,
            source: None,
            verified: Vec::new(),
            needs: Vec::new(),
        };
        crate::api::import::propose_bundle(
            &state,
            identity,
            &name,
            report,
            files,
            Some(("Project", &name)),
        )
        .await
    }
    .await;
    match prepared {
        Ok(change) => Ok((StatusCode::ACCEPTED, Json(change))),
        // Both or neither: a copy no registry entry names is removed again (CC-85).
        Err(error) => {
            if let Err(cleanup) = copy.delete_repository().await {
                tracing::error!(project = %name, error = %cleanup, "the copy of a project that was not duplicated is left behind");
            }
            Err(error)
        }
    }
}

/// The registry entry in `files` against what the origin's `project.yaml` declares: a value
/// for an undeclared parameter, or one that does not fit, is the caller's `400` before anything
/// is copied (CC-88).
async fn check_parameters(
    source: &crate::git::GiteaClient,
    origin: &str,
    name: &str,
    files: &[(String, String)],
) -> Result<(), ApiError> {
    let own = source
        .get_file(&format!("projects/{origin}/project.yaml"), "main")
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "the repository of '{origin}' has no project.yaml on main to copy (PF-86)"
            ))
        })?;
    let own = jc_core::kinds::Project::from_yaml(&own.content).map_err(|e| {
        ApiError::Conflict(format!("the project.yaml of '{origin}' does not load: {e}"))
    })?;
    let entry_path = format!("projects/{name}.yaml");
    let entry = files
        .iter()
        .find(|(path, _)| *path == entry_path)
        .ok_or_else(|| ApiError::Internal("the registry entry is missing".into()))?;
    let entry = jc_core::kinds::Project::from_yaml(&entry.1)
        .map_err(|e| ApiError::BadRequest(format!("the registry entry does not load: {e}")))?;
    jc_core::project::resolve_parameters(&own.spec, &entry.spec)
        .map(|_| ())
        .map_err(|e| ApiError::BadRequest(format!("{e} (CC-88)")))
}

// ---------------------------------------------------------------------------
// A project's registry entry: what this deployment runs (PF-86, CC-88)
// ---------------------------------------------------------------------------

/// One tag of a project repository: a release a registry entry may pin (CC-88).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegistryTag {
    pub name: String,
    pub commit: String,
}

/// What the registry entry `projects/{slug}.yaml` says this deployment runs, with what the form
/// that changes it needs (PF-86, CC-88).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    /// `{name}` of a repository of the local forge, or `{url, secretRef}` of an external one.
    #[schema(value_type = Object)]
    pub repository: Value,
    /// The tag, branch or commit this deployment runs.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// This deployment's values.
    #[schema(value_type = Object)]
    pub parameters: Value,
    /// The parameters `project.yaml` declares at `ref`; empty for an external repository.
    #[schema(value_type = Object)]
    pub declarations: Value,
    /// The project repository's tags; empty for an external repository.
    pub tags: Vec<RegistryTag>,
}

/// Another ref to run, or another set of values (PF-86, CC-88).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Repoint {
    /// A tag, a branch or a commit of the project repository.
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// The whole set of values; an empty object returns every parameter to its default.
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub parameters: Option<serde_json::Map<String, Value>>,
}

/// The registry entry of `project` as `main` of the organization repository holds it: its path,
/// its blob, the manifest as written and its spec. A caller who may not read the project meets a
/// project that is not there (R20).
struct Registered {
    path: String,
    sha: String,
    manifest: Value,
    spec: jc_core::kinds::ProjectSpec,
}

async fn registered(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<(std::sync::Arc<crate::git::GiteaClient>, String, Registered), ApiError> {
    let missing = || ApiError::NotFound(format!("project '{project}' not found"));
    if !crate::permissions::for_request(state, identity, project).may_read_project() {
        return Err(missing());
    }
    if state.mirror.layout() != 2 {
        return Err(ApiError::Conflict(
            "a project of layout 1 has no registry entry: its files are in the organization \
             repository, and it runs what main holds (PF-86)"
                .into(),
        ));
    }
    let gitea = state
        .gitea
        .clone()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let base = gitea.default_branch().await?;
    let path = format!("projects/{project}.yaml");
    let file = gitea.get_file(&path, &base).await?.ok_or_else(missing)?;
    let manifest: Value = serde_norway::from_str(&file.content).map_err(|e| {
        ApiError::Internal(format!("the registry entry {path} does not parse: {e}"))
    })?;
    let spec: jc_core::kinds::ProjectSpec = serde_json::from_value(
        manifest.get("spec").cloned().unwrap_or(Value::Null),
    )
    .map_err(|e| ApiError::Internal(format!("the registry entry {path} does not load: {e}")))?;
    if !spec.is_registry_entry() {
        return Err(ApiError::Internal(format!(
            "{path} names no repository, so it is no registry entry (PF-86)"
        )));
    }
    Ok((
        gitea,
        base,
        Registered {
            path,
            sha: file.sha,
            manifest,
            spec,
        },
    ))
}

/// The project's own `project.yaml` at `git_ref` of its local repository, `None` when the
/// repository holds no such ref or no such file there.
async fn project_file_at(
    project_repository: &crate::git::GiteaClient,
    project: &str,
    git_ref: &str,
) -> Result<Option<jc_core::kinds::ProjectSpec>, ApiError> {
    let Some(file) = project_repository
        .get_file(&format!("projects/{project}/project.yaml"), git_ref)
        .await?
    else {
        return Ok(None);
    };
    let own = jc_core::kinds::Project::from_yaml(&file.content).map_err(|e| {
        ApiError::BadRequest(format!(
            "the project.yaml of '{project}' at '{git_ref}' does not load: {e}"
        ))
    })?;
    Ok(Some(own.spec))
}

fn json_of<T: Serialize>(value: &T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::Internal(e.to_string()))
}

/// `GET /api/v1/projects/{project}/registry`: what the registry entry says this deployment
/// runs, the declarations at that ref and the releases a pin may name (PF-86, CC-88).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/registry",
    summary = "Read A Project's Registry Entry",
    description = "The repository, pinned ref and parameter values of the project's registry entry, the parameters project.yaml declares at that ref, and the repository's tags.",
    tag = "resources",
    params(("project" = String, Path, description = "Project slug")),
    responses(
        (status = 200, description = "The registry entry", body = RegistryEntry),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project, or none this caller may read", body = ProblemDetails),
        (status = 409, description = "The organization is of layout 1", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn get_registry_entry(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Result<Json<RegistryEntry>, ApiError> {
    let (gitea, _, entry) = registered(&state, &user.0.identity, &project).await?;
    let git_ref = entry.spec.git_ref.clone().unwrap_or_default();
    let repository = json_of(&entry.spec.repository)?;
    let local_name = entry
        .spec
        .repository
        .as_ref()
        .and_then(|repository| repository.name.as_deref());
    let (declarations, tags) = match local_name {
        Some(repo) => {
            let local = gitea.for_project(repo, &project);
            let declared = project_file_at(&local, &project, &git_ref)
                .await?
                .map(|own| own.parameters)
                .unwrap_or_default();
            let tags = local
                .list_tags()
                .await?
                .into_iter()
                .map(|(name, commit)| RegistryTag { name, commit })
                .collect();
            (declared, tags)
        }
        None => (BTreeMap::new(), Vec::new()),
    };
    Ok(Json(RegistryEntry {
        repository,
        git_ref,
        parameters: json_of(&entry.spec.parameters)?,
        declarations: json_of(&declarations)?,
        tags,
    }))
}

/// `PUT /api/v1/projects/{project}/registry`: proposes the registry entry pinned to another ref
/// or carrying other values, as one red-lane Change on the organization repository (PF-86,
/// CC-88). The ref is read in the project repository before anything is proposed.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/registry",
    summary = "Repoint A Project",
    description = "Proposes the project's registry entry pinned to another tag, branch or commit, or with other parameter values, as one red-lane change on the organization repository.",
    tag = "resources",
    params(("project" = String, Path, description = "Project slug")),
    request_body(
        content = Repoint,
        example = json!({ "ref": "v0.2.0", "parameters": { "audience": "public" } })
    ),
    responses(
        (status = 202, description = "The change that repoints the project", body = Change),
        (status = 400, description = "No ref and no parameters, a ref the repository does not hold, or a value that does not fit the declarations", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose a change to this project", body = ProblemDetails),
        (status = 404, description = "No such project, or none this caller may read", body = ProblemDetails),
        (status = 409, description = "Layout 1, a repoint already open, or nothing would change", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn repoint_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(request): Json<Repoint>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let identity = &user.0.identity;
    if request.git_ref.is_none() && request.parameters.is_none() {
        return Err(ApiError::BadRequest(
            "name the ref to run, the parameter values, or both (PF-86)".into(),
        ));
    }
    let (gitea, base, entry) = registered(&state, identity, &project).await?;
    // The registry entry is the organization's manifest of the project: the same rule as editing
    // its title from the organization (PF-86, PF-52).
    crate::permissions::for_request(&state, identity, crate::permissions::ORG_NAMESPACE).check(
        "Project",
        jc_core::kinds::Verb::Propose,
        Some(&entry.manifest),
    )?;

    let mut manifest = entry.manifest.clone();
    let spec = manifest
        .get_mut("spec")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::Internal(format!("{} has no spec", entry.path)))?;
    if let Some(git_ref) = &request.git_ref {
        spec.insert("ref".into(), Value::String(git_ref.trim().to_owned()));
    }
    match &request.parameters {
        Some(values) if values.is_empty() => {
            spec.remove("parameters");
        }
        Some(values) => {
            spec.insert("parameters".into(), Value::Object(values.clone()));
        }
        None => {}
    }
    if manifest == entry.manifest {
        return Err(ApiError::Conflict(format!(
            "project '{project}' already runs that: nothing to propose (PF-86)"
        )));
    }
    let proposed: jc_core::kinds::ProjectSpec = serde_json::from_value(manifest["spec"].clone())
        .map_err(|e| ApiError::BadRequest(format!("{e} (CC-88)")))?;
    proposed
        .validate()
        .map_err(|e| ApiError::BadRequest(format!("{e} (PF-86, CC-88)")))?;

    // A local repository is read at the ref the entry will pin: the ref must be there, and the
    // values must fit that release's declarations. An external one is the checkouts' to fetch
    // (CC-86, CC-89).
    if let Some(repo) = proposed.repository.as_ref().and_then(|r| r.name.as_deref()) {
        let git_ref = proposed.git_ref.as_deref().unwrap_or_default();
        let local = gitea.for_project(repo, &project);
        let own = project_file_at(&local, &project, git_ref)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "'{git_ref}' is no tag, branch or commit of the repository '{repo}' that \
                     holds a project.yaml (PF-86)"
                ))
            })?;
        jc_core::project::resolve_parameters(&own, &proposed)
            .map_err(|e| ApiError::BadRequest(format!("{e} (CC-88)")))?;
    }

    let branch = format!("portal/update-project-{project}-registry");
    let held = crate::api::mutate::turn(&gitea, &branch).await;
    if let Some(pending) =
        crate::api::mutate::open_change_on(&state, &gitea, &branch, &project).await?
    {
        return Err(ApiError::Conflict(format!(
            "a repoint of project '{project}' is already proposed: {}; approve or reject it first",
            pending.name
        )));
    }
    let content = serde_norway::to_string(&manifest)
        .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
    let branch = crate::api::mutate::create_or_reuse_branch(&gitea, &held, &base).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, &project);
    gitea
        .put_file(&crate::git::FileWrite {
            path: &entry.path,
            branch: &branch,
            message: &format!("update {}", entry.path),
            content: &content,
            sha: Some(&entry.sha),
            author: crate::git::Author {
                name: &author_name,
                email: &author_email,
            },
        })
        .await?;

    let was = |spec: &jc_core::kinds::ProjectSpec| spec.git_ref.clone().unwrap_or_default();
    let mut lines = Vec::new();
    if was(&entry.spec) != was(&proposed) {
        lines.push(format!(
            "- runs `{}` instead of `{}`",
            was(&proposed),
            was(&entry.spec)
        ));
    }
    if entry.spec.parameters != proposed.parameters {
        lines.push(format!("- parameters: {}", json_of(&proposed.parameters)?));
    }
    let body = format!(
        "Repoints project `{project}`: the registry entry `{}` decides what this deployment runs \
         (PF-86, CC-88).\n\n{}\n\nOnce merged, the checkouts fetch the ref and the next render \
         runs it (CC-86).",
        entry.path,
        lines.join("\n")
    );
    let pull = gitea
        .create_pull_request(&branch, &base, &format!("repoint project {project}"), &body)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(Change::new(
            crate::api::changes::change_meta(&state, &gitea, pull.number, &project),
            crate::change::ChangeStatus::new(
                crate::change::Lane::Red,
                crate::change::ChangePhase::PendingApproval,
                crate::change::PlanSummary::new(0, 1, 0),
            )
            .in_repository(&pull.repository)
            .proposed_through(&pull.body)
            .with_merge_request(pull.url),
        )),
    ))
}

/// How a project repository is remounted from the slug it left onto the one it lands under
/// (PF-89, MF-45).
pub(crate) struct Remount<'a> {
    pub origin: &'a str,
    pub name: &'a str,
    /// Whether an Endpoint's slug is drawn anew: always for a copy beside its origin, for an
    /// import only when another project of this organization serves it (EP-02).
    pub fresh_slug: &'a (dyn Fn(&str) -> bool + Sync),
    /// Application repositories by the name the origin gave them, to the clone URL each one
    /// lands at: an App's `source.git.url` follows its repository (AP-75).
    pub applications: &'a BTreeMap<String, String>,
    /// Whether the encrypted secrets files go with it. A copy beside its origin leaves them
    /// behind: they hold the origin's credentials, which a reader of the origin who opens the
    /// copy must not get to use (CC-06); an import carries the bundle its owner brought.
    pub keep_secrets: bool,
    pub message: String,
}

/// One commit on the repository's `main` that makes it the project `remount.name` (PF-89, PF-83,
/// EP-02): every manifest that names the origin names it, the Endpoint slugs `fresh_slug` asks
/// for are drawn anew (a slug is a capability, and two projects must not answer at one URL),
/// every App builds from its own repository, and CODEOWNERS names this project's writers. A file
/// that is not YAML, or does not parse, stays as it is. Answers the commit, `None` when nothing
/// changed.
pub(crate) async fn remount_copy(
    copy: &crate::git::GiteaClient,
    identity: &crate::auth::session::Identity,
    remount: &Remount<'_>,
) -> Result<Option<String>, ApiError> {
    let name = remount.name;
    let owners = format!("* @{}/{name}-writers\n", copy.owner);
    let mut uploads = Vec::new();
    let mut deletes = Vec::new();
    for (path, sha) in copy.list_tree_blobs("main").await? {
        if !remount.keep_secrets
            && jcctl::secrets::sops::is_encrypted_file(path.rsplit('/').next().unwrap_or_default())
        {
            deletes.push((path, sha));
            continue;
        }
        let yaml = path.ends_with(".yaml") || path.ends_with(".yml");
        let codeowners = path == format!("projects/{name}/CODEOWNERS");
        if !yaml && !codeowners {
            continue;
        }
        let Some(file) = copy.get_file(&path, "main").await? else {
            continue;
        };
        let text = if codeowners {
            if file.content == owners {
                continue;
            }
            owners.clone()
        } else {
            match remount_manifests(&file.content, remount) {
                Some(text) => text,
                None => continue,
            }
        };
        uploads.push((path, text));
    }
    if uploads.is_empty() && deletes.is_empty() {
        return Ok(None);
    }
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, name);
    let commit = copy
        .change_files(
            "main",
            &remount.message,
            crate::git::Author {
                name: &author_name,
                email: &author_email,
            },
            &uploads,
            &deletes,
        )
        .await?;
    Ok(Some(commit))
}

/// The documents of one YAML file remounted as `remount` says, or `None` when nothing in it
/// changes or it does not parse.
fn remount_manifests(text: &str, remount: &Remount<'_>) -> Option<String> {
    use serde::Deserialize as _;
    let mut documents = Vec::new();
    let mut changed = false;
    for document in serde_norway::Deserializer::from_str(text) {
        let mut manifest = Value::deserialize(document).ok()?;
        let kind = manifest
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(metadata) = manifest.get_mut("metadata").and_then(Value::as_object_mut) {
            let field = if kind.as_deref() == Some("Project") {
                "name"
            } else {
                "namespace"
            };
            if remount.origin != remount.name
                && metadata.get(field).and_then(Value::as_str) == Some(remount.origin)
            {
                metadata.insert(field.to_owned(), Value::String(remount.name.to_owned()));
                changed = true;
            }
        }
        let spec = manifest.get_mut("spec").and_then(Value::as_object_mut);
        match (kind.as_deref(), spec) {
            (Some("Endpoint"), Some(spec)) => {
                let fresh = spec
                    .get("slug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| (remount.fresh_slug)(slug));
                if fresh {
                    spec.insert(
                        "slug".to_owned(),
                        Value::String(crate::apps::reconciler::generate_slug().as_str().to_owned()),
                    );
                    changed = true;
                }
            }
            (Some("App"), Some(spec)) => {
                let url = spec
                    .get_mut("source")
                    .and_then(|source| source.get_mut("git"))
                    .and_then(|git| git.get_mut("url"));
                if let Some(url) = url {
                    let repository = url
                        .as_str()
                        .and_then(|url| url.trim_end_matches('/').rsplit('/').next())
                        .map(|last| last.trim_end_matches(".git").to_owned());
                    if let Some(new) = repository.and_then(|r| remount.applications.get(&r)) {
                        if url.as_str() != Some(new) {
                            *url = Value::String(new.clone());
                            changed = true;
                        }
                    }
                }
            }
            _ => {}
        }
        documents.push(manifest);
    }
    if !changed {
        return None;
    }
    let rendered: Result<Vec<String>, _> = documents.iter().map(serde_norway::to_string).collect();
    Some(rendered.ok()?.join("---\n"))
}

// ---------------------------------------------------------------------------
// Deleting a project (PF-77, PF-78)
// ---------------------------------------------------------------------------

/// Every file the deletion of `project` removes (PF-77).
///
/// Its own tree, and the role bindings of the organization whose scope names the project or one
/// of its spaces: those live under `users/`, so a cascade that only removed `projects/{name}/`
/// would leave grants behind pointing at a project that is gone.
pub(crate) async fn deletion_plan(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    project: &str,
    git_ref: &str,
) -> Result<Vec<String>, ApiError> {
    let tree = gitea.list_tree(git_ref).await?;
    let prefix = format!("projects/{project}/");
    // Layout 2 holds the project's registry entry instead of its tree (PF-86); the sync
    // archives the project's repository once the entry is gone.
    let entry = format!("projects/{project}.yaml");
    let mut files: Vec<String> = tree
        .iter()
        .filter(|path| path.starts_with(&prefix) || **path == entry)
        .cloned()
        .collect();

    let spaces: std::collections::HashSet<String> = state
        .mirror
        .list(
            project,
            "ContextSpace",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .map(|env| env.metadata.name)
        .collect();
    let info = resource::by_kind("RoleBinding")
        .ok_or_else(|| ApiError::Internal("RoleBinding is not a kind of this Portal".into()))?;
    for env in state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "RoleBinding",
            &crate::store::ListOptions::default(),
        )
        .items
    {
        let scope = env.spec.get("scope").cloned().unwrap_or(Value::Null);
        let names_it = scope.get("project").and_then(Value::as_str) == Some(project)
            || scope
                .get("contextSpace")
                .and_then(Value::as_str)
                .is_some_and(|space| spaces.contains(space));
        if !names_it {
            continue;
        }
        let path = resource::repository_path(
            info,
            crate::permissions::ORG_NAMESPACE,
            None,
            &env.metadata.name,
        )
        .map_err(ApiError::Internal)?;
        if tree.contains(&path) {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// The `SharedSpaceReference` manifests of other projects pointing at this project's Endpoints
/// (PF-77), as `{project}/{name}`.
///
/// A reference is another project's manifest: removing it is that project's own change, so the
/// deletion waits and names what it is waiting for rather than breaking a live share.
fn live_references(state: &AppState, project: &str) -> Vec<String> {
    let slugs: std::collections::HashSet<String> = state
        .mirror
        .list(project, "Endpoint", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| {
            env.spec
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut naming: Vec<String> = state
        .mirror
        .matching(|env| {
            env.kind == "SharedSpaceReference"
                && env.metadata.namespace.as_deref() != Some(project)
                && (env
                    .spec
                    .get("endpointSlug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| slugs.contains(slug))
                    // By name inside the organization (EP-77).
                    || env
                        .spec
                        .pointer("/endpointRef/project")
                        .and_then(Value::as_str)
                        == Some(project))
        })
        .into_iter()
        .map(|env| {
            format!(
                "{}/{}",
                env.metadata.namespace.unwrap_or_default(),
                env.metadata.name
            )
        })
        .collect();
    naming.sort();
    naming
}

/// How long a deleted project's name stays reserved (PF-78): the organization's setting, else
/// the 30 days jc-core ships.
fn cooldown_days(state: &AppState) -> u64 {
    state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .iter()
        .find_map(|env| {
            env.spec
                .pointer("/projects/nameCooldownDays")
                .and_then(Value::as_u64)
        })
        .unwrap_or(u64::from(jc_core::kinds::DEFAULT_NAME_COOLDOWN_DAYS))
}

/// When the name of a project that was deleted becomes free again, if it is still reserved
/// (PF-78). `None` means nobody deleted a project of this name, or the period has passed.
///
/// The commit that removed `projects/{name}/project.yaml` is the start of the period: the forge
/// history is the record, so the reservation survives a restart and a re-sync of the mirror.
async fn reserved_until(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    name: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let days = cooldown_days(state);
    if days == 0 {
        return None;
    }
    let branch = gitea.default_branch().await.ok()?;
    // The project's own file in layout 1, its registry entry in layout 2 (PF-86).
    let mut removed = None;
    for path in [
        format!("projects/{name}/project.yaml"),
        format!("projects/{name}.yaml"),
    ] {
        let history = gitea
            .list_commits(&branch, &path, 1)
            .await
            .unwrap_or_default();
        if let Some(date) = history
            .first()
            .and_then(|last| chrono::DateTime::parse_from_rfc3339(&last.date).ok())
        {
            removed = removed.max(Some(date));
        }
    }
    let removed = removed?;
    let free = removed.with_timezone(&chrono::Utc) + chrono::Duration::days(days as i64);
    (chrono::Utc::now() < free).then_some(free)
}

/// `DELETE /api/v1/projects/{project}`: proposes the one red-lane change that removes a project
/// and everything written for it (PF-77).
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}",
    summary = "Delete A Project",
    description = "Proposes the one red-lane change that removes a project and every space, endpoint, app, service account, role and binding written for it.",
    tag = "resources",
    params(("project" = String, Path, description = "The project to delete")),
    responses(
        (status = 202, description = "The change that deletes the project", body = Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not delete this project", body = ProblemDetails),
        (status = 404, description = "No such project, or none this caller may read", body = ProblemDetails),
        (status = 409, description = "A share points at it, or a deletion is already open", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn delete_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let change = delete_project_for(&state, &user.0.identity, &project).await?;
    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// The deletion itself, so the route and the operations registry propose the same change.
pub async fn delete_project_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<Change, ApiError> {
    let missing = || ApiError::NotFound(format!("project '{project}' not found"));
    let effective = crate::permissions::for_request(state, identity, project);
    // A project the caller may not read answers as a project that is not there (R20).
    if !effective.may_read_project() {
        return Err(missing());
    }
    let manifest = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)
        .ok_or_else(missing)?;
    let target = serde_json::to_value(&manifest).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check("Project", jc_core::kinds::Verb::Delete, Some(&target))?;

    let referenced = live_references(state, project);
    if !referenced.is_empty() {
        return Err(ApiError::Conflict(format!(
            "project '{project}' is shared with {}: remove the SharedSpaceReference there first \
             (PF-77)",
            referenced.join(", ")
        )));
    }

    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let files = deletion_plan(state, gitea, project, &default_branch).await?;
    if files.is_empty() {
        return Err(missing());
    }

    let branch = format!("portal/delete-project-{project}");
    let held = crate::api::mutate::turn(gitea, &branch).await;
    if let Some(pending) =
        crate::api::mutate::open_change_on(state, gitea, &branch, project).await?
    {
        return Err(ApiError::Conflict(format!(
            "deleting project '{project}' is already proposed: {}; approve or reject it first",
            pending.name
        )));
    }
    let branch = crate::api::mutate::create_or_reuse_branch(gitea, &held, &default_branch).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    // One commit for every file, not a read and a commit per file (F-R2POR3-019). The blob
    // shas are the base's: the forge refuses the commit if the branch holds another version.
    let planned: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let deletes: Vec<(String, String)> = gitea
        .list_tree_blobs(&default_branch)
        .await?
        .into_iter()
        .filter(|(path, _)| planned.contains(path.as_str()))
        .collect();
    if !deletes.is_empty() {
        gitea
            .change_files(
                &branch,
                &format!("delete project {project} (PF-77)"),
                crate::git::Author {
                    name: &author_name,
                    email: &author_email,
                },
                &[],
                &deletes,
            )
            .await?;
    }

    let title = format!("delete project {project}");
    let listed = files
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = format!(
        "Deleting project `{project}` removes {} files, and with them every space, endpoint, \
         pipeline, app, service account, role and binding written for it (PF-77).\n\n{listed}\n\n\
         Export each space's data before approving — `GET /api/v1/projects/{project}/export` \
         while the project is still here — because the broker tenants are dropped when this \
         merges (CC-07). The name stays reserved afterwards (PF-78).",
        files.len()
    );
    let pull = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;

    let summary = crate::change::PlanSummary::new(0, 0, files.len());
    Ok(Change::new(
        crate::api::changes::change_meta(state, gitea, pull.number, project),
        crate::change::ChangeStatus::new(
            crate::change::Lane::Red,
            crate::change::ChangePhase::PendingApproval,
            summary,
        )
        .in_repository(&pull.repository)
        .proposed_through(&pull.body)
        .with_merge_request(pull.url),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        }
    }

    fn person(subject: &str) -> crate::auth::session::Identity {
        crate::auth::session::Identity {
            client: None,
            subject: subject.into(),
            username: "jana".into(),
            email: Some("jana@hel.fi".into()),
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        }
    }

    /// F-R2POR3-004: who opens a project is the installation Organization's to say; a second
    /// Organization's `anyone` opens nothing and merges nothing.
    #[test]
    fn only_the_installation_organization_says_who_opens_a_project() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "Organization",
            "aa",
            json!({ "domain": "aa.sk", "projects": { "creation": "anyone" } }),
        ));
        state.mirror.upsert(envelope(
            "Organization",
            "bb",
            json!({ "domain": "bb.sk", "installation": true, "projects": { "creation": "org-admin" } }),
        ));
        assert_eq!(creation(&state), "org-admin");
        assert!(!lets_anyone_open(&state));
        assert!(may_open(&state, &person("sub-jana")).is_err());
    }

    /// F-R2POR3-007: a `group:` member written as a Keycloak id (ADR-N-061) is its person.
    #[test]
    fn a_group_member_written_as_an_id_may_open_a_project() {
        const ID: &str = "0b6f0e4c-1d2e-4f00-9a7b-3c4d5e6f7a8b";
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "Organization",
            "bb",
            json!({ "domain": "bb.sk", "projects": { "creation": "group:openers" } }),
        ));
        state.mirror.upsert(envelope(
            "Group",
            "openers",
            json!({ "members": [{ "user": ID }] }),
        ));
        assert!(in_group(&state, &person(ID), "openers"));
        assert!(!in_group(&state, &person("sub-other"), "openers"));
    }
}
