//! The flow gallery and the flow it starts (T-0207, T-0208, CC-24, CC-30, CC-32, CC-59).
//!
//! A blueprint is an organization-level manifest, so it is not under
//! `/api/v1/projects/{project}/…` like the rest; API/01 §4 lists it among the kinds served at
//! `/api/v1/{plural}` and §13 describes the flow. Only the blueprint plural is routed here
//! rather than a wildcard `{plural}`: a wildcard segment at the root of `/api/v1` sits beside
//! `/preferences`, `/sync` and `/health`, and the gallery does not need that risk.

use std::hash::{Hash, Hasher};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use jc_core::kinds::RiskClass;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::mutate::{
    author_credentials, create_or_reuse_branch, find_literal_secret, resolve_repo_path,
};
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::{CurrentUser, Identity};
use crate::change::{self, Change, ChangePhase, ChangeStatus, Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite};
use crate::plan;
use crate::resource::{self, ResourceEnvelope, API_VERSION};
use crate::state::AppState;
use crate::store::ListOptions;

/// Blueprints live in the organization namespace, like `org.yaml` (see `sync.rs`).
pub const ORG_NAMESPACE: &str = "org";
const BLUEPRINT_KIND: &str = "Blueprint";

/// `spec.allowedRoles`: the roles that may see and run a blueprint (CC-59).
fn allowed_roles(envelope: &ResourceEnvelope) -> Vec<&str> {
    envelope
        .spec
        .get("allowedRoles")
        .and_then(|value| value.as_array())
        .map(|roles| roles.iter().filter_map(|role| role.as_str()).collect())
        .unwrap_or_default()
}

/// Whether this caller may run this blueprint (CC-59).
///
/// Fail-closed: `spec.allowedRoles` must name at least one role, which `jc_core::BlueprintSpec`
/// already refuses to accept empty, so a blueprint that reaches the mirror without one is
/// malformed rather than public. Reading it as "everyone" would turn a broken manifest into an
/// open door.
fn may_run(identity: &Identity, envelope: &ResourceEnvelope) -> bool {
    allowed_roles(envelope)
        .iter()
        .any(|role| identity.roles.iter().any(|r| r == role))
}

/// The lane a blueprint declares for its own changes (CC-59, CC-63).
fn declared_lane(risk: RiskClass) -> Lane {
    match risk {
        RiskClass::Green => Lane::Green,
        RiskClass::Yellow => Lane::Yellow,
        RiskClass::Red => Lane::Red,
    }
}

/// The stricter of two lanes.
///
/// A blueprint declares a lane, but a template can render a kind that is Red on its own terms
/// (a Policy, a federation edge, anything `change::classify` calls Red). Taking the stricter of
/// the two stops a Green blueprint from being a way to auto-approve a Red manifest (CC-63).
fn stricter(a: Lane, b: Lane) -> Lane {
    match (a, b) {
        (Lane::Red, _) | (_, Lane::Red) => Lane::Red,
        (Lane::Yellow, _) | (_, Lane::Yellow) => Lane::Yellow,
        _ => Lane::Green,
    }
}

/// One branch per (project, blueprint, version, parameters), so a retry of the same submission
/// reuses its branch instead of opening a second merge request (same rule as `mutate::branch_name`).
fn flow_branch(
    project: &str,
    blueprint: &str,
    version: &str,
    parameters: &serde_json::Value,
) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    (project, blueprint, version, parameters.to_string()).hash(&mut hasher);
    format!("portal/flow-{blueprint}-{:016x}", hasher.finish())[..]
        .chars()
        .take(120)
        .collect()
}

/// Turns one rendered manifest into an envelope this project may actually receive.
///
/// The checks are `mutate::propose`'s, for the same reason: expansion is not a way past them.
/// A template that renders a foreign namespace, an organization-level kind, a `status` block or
/// a literal secret is refused here rather than committed (MF-04, MF-24).
fn accept_rendered(
    yaml: &str,
    template: &str,
    project: &str,
) -> Result<(ResourceEnvelope, &'static resource::KindInfo), ApiError> {
    let body: serde_json::Value = serde_norway::from_str(yaml).map_err(|e| {
        ApiError::Internal(format!("template `{template}` rendered invalid yaml: {e}"))
    })?;
    let mut envelope: ResourceEnvelope = serde_json::from_value(body.clone()).map_err(|e| {
        ApiError::Internal(format!(
            "template `{template}` did not render a manifest: {e}"
        ))
    })?;

    if envelope.api_version != API_VERSION {
        return Err(ApiError::Internal(format!(
            "template `{template}` rendered apiVersion '{}' (expected '{API_VERSION}')",
            envelope.api_version
        )));
    }
    let kind_info = resource::by_kind(&envelope.kind).ok_or_else(|| {
        ApiError::Internal(format!(
            "template `{template}` rendered unknown kind '{}'",
            envelope.kind
        ))
    })?;

    match envelope.metadata.namespace.as_deref() {
        None | Some("") => envelope.metadata.namespace = Some(project.to_string()),
        Some(ns) if ns == project => {}
        Some(foreign) => {
            return Err(ApiError::BadRequest(format!(
                "template `{template}` renders into namespace '{foreign}', not project '{project}'"
            )))
        }
    }
    resource::validate_meta(&envelope.metadata).map_err(ApiError::BadRequest)?;

    if body.get("status").is_some() || envelope.status.is_some() {
        return Err(ApiError::Internal(format!(
            "template `{template}` rendered a status block, which the platform computes (MF-04)"
        )));
    }
    if let Some(key) = find_literal_secret(&body) {
        return Err(ApiError::BadRequest(format!(
            "template `{template}` rendered a literal secret in field '{key}'; use secretRef (MF-24)"
        )));
    }

    Ok((envelope, kind_info))
}

#[utoipa::path(
    get,
    path = "/api/v1/blueprints",
    summary = "List Blueprints",
    description = "The organization's Blueprints this caller may run, for the flow gallery. A Blueprint is started in a project with POST /projects/{project}/flows.",
    tag = "blueprints",
    responses(
        (status = 200, description = "The blueprints this caller may run", body = ResourceList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_blueprints(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ResourceList>, ApiError> {
    // The gallery is a screen of cards, not a paged table: an organisation publishes tens of
    // blueprints, not thousands, and a card the filter removed must not leave a gap in a page.
    let page = state
        .mirror
        .list(ORG_NAMESPACE, BLUEPRINT_KIND, &ListOptions::default());
    let items = page
        .items
        .into_iter()
        .filter(|envelope| may_run(&user.0.identity, envelope))
        .collect();

    Ok(Json(ResourceList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        metadata: ListMeta {
            continue_token: None,
            remaining_item_count: None,
        },
        items,
        next_cursor: None,
        total_count: None,
    }))
}

/// Running a blueprint: the parameters the form collected (API/01 §13).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowRequest {
    /// The organization-level Blueprint to run.
    pub blueprint: String,
    /// The version the form was generated from. A mismatch is a conflict rather than an
    /// expansion against a schema the user never saw (CC-26).
    pub version: String,
    /// The values the user filled in, validated against `spec.parameterSchema` (CC-24).
    pub parameters: serde_json::Value,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/flows",
    summary = "Run A Blueprint",
    description = "Expands one of the organisation's blueprints with the parameters given, as one change: a green flow merges at once, anything stricter waits for a person's approval.",
    tag = "blueprints",
    params(("project" = String, Path, description = "Project the flow creates resources in")),
    request_body(
        content = FlowRequest,
        example = json!({ "blueprint": "cross-city-sharing", "version": "1.2.0", "parameters": {} })
    ),
    responses(
        (status = 202, description = "Change proposal opened", body = Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such blueprint for this caller", body = ProblemDetails),
        (status = 400, description = "The parameters do not satisfy the blueprint's schema", body = ProblemDetails),
        (status = 409, description = "The form was filled against another version", body = ProblemDetails),
        (status = 503, description = "The git forge is not configured", body = ProblemDetails)
    )
)]
pub async fn start_flow(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<FlowRequest>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    let planned = plan_flow(&state, identity, &project, &request)?;

    // One merge request for the whole expansion: the manifests of one flow are reviewed and
    // merged together or not at all (CC-32).
    // The project's own repository in layout 2, where its manifests live (F-POR1-016).
    let forge = state
        .forge_for(&project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea = forge.as_ref();
    let default_branch = gitea.default_branch().await?;
    let branch = flow_branch(
        &project,
        &request.blueprint,
        &request.version,
        &request.parameters,
    );
    let held = crate::api::mutate::turn(gitea, &branch).await;
    let branch = create_or_reuse_branch(gitea, &held, &default_branch).await?;

    let (author_name, author_email) = author_credentials(identity, &project);
    for (repo_path, yaml) in &planned.files {
        let existing_sha = gitea
            .get_file(repo_path, &branch)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);
        let message = format!(
            "run blueprint {} {} in {project}",
            request.blueprint, request.version
        );
        gitea
            .put_file(&FileWrite {
                path: repo_path,
                branch: &branch,
                message: &message,
                content: yaml,
                sha: existing_sha.as_deref(),
                author: Author {
                    name: &author_name,
                    email: &author_email,
                },
            })
            .await?;
    }

    let title = format!(
        "run blueprint {} {} in {project}",
        request.blueprint, request.version
    );
    let body = format!(
        "Blueprint `{}` version {} expanded into {} manifest(s) in project `{project}` via joinedcontext Portal.",
        request.blueprint,
        request.version,
        planned.files.len()
    );
    let pr = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;

    let change = Change::new(
        crate::api::changes::change_meta(&state, gitea, pr.number, &project),
        ChangeStatus::new(planned.lane, ChangePhase::PendingApproval, planned.summary)
            .in_repository(&pr.repository)
            .proposed_through(&pr.body)
            .with_merge_request(pr.url.clone()),
    );

    // CC-63, CC-65, AG-14: a flow that is green after the stricter-of-two rule is merged now,
    // for the person who started it, whose role the blueprint names (`may_run`) and who may
    // propose every kind it renders (`plan_flow`). Anything stricter waits for a person.
    let change = if planned.lane == Lane::Green {
        let paths: Vec<&str> = planned
            .files
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        let started_by = identity.email.as_deref().unwrap_or(&identity.username);
        let message = format!(
            "Merge change proposal {}: {}\n\nGreen lane: blueprint {} {}, started by {started_by} \
             (CC-63, CC-65, AG-14)",
            change.metadata.name, pr.title, request.blueprint, request.version
        );
        crate::api::changes::merge_if_only(&state, gitea, &pr, &paths, &message, change).await
    } else {
        change
    };
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// What a flow would write, and in which lane, before anything reaches the forge.
pub(crate) struct PlannedFlow {
    /// The stricter of the lane the blueprint declares and the lane of every manifest it renders.
    pub lane: Lane,
    summary: PlanSummary,
    /// Each rendered manifest: its path in the repository and its YAML.
    files: Vec<(String, String)>,
}

/// Every check a flow makes before it writes: the blueprint the caller may run, its version, the
/// expansion, and each rendered manifest through the gate a hand-written one passes. The lane is
/// the stricter of what the blueprint declares and what the kinds themselves are (CC-63).
/// The widget that names a parameter the Portal fills with an endpoint slug (Development/05
/// §2.1).
const MINTED_SLUG: &str = "endpointSlug";
/// The alphabet of an endpoint slug (EP-02): lowercase RFC 4648 base32.
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// What the slug HMAC is keyed for, so the key's other uses never produce the same bytes.
const SLUG_LABEL: &[u8] = b"jc-blueprint-endpoint-slug";

/// `parameters` with every `endpointSlug` parameter the caller left out filled (EP-02, CC-25).
///
/// An Endpoint a blueprint renders needs a slug nobody can guess, and nobody types one. The slug
/// is 160 bits of an HMAC-SHA-256 under the Portal's key, over the project, the
/// blueprint, its version, the parameter's name and the submission as it arrived, in base32: an
/// outsider who knows every parameter still cannot compute it, the same submission always yields
/// the same slug (so a retry lands on the same branch), and the value is recorded with the other
/// parameters, so a re-render is byte-identical (CC-27). A value the caller gave is kept; the
/// schema's pattern judges it like any other parameter.
fn with_minted_slugs(
    key: &[u8],
    project: &str,
    blueprint: &jc_core::kinds::Blueprint,
    parameters: &serde_json::Value,
) -> serde_json::Value {
    use hmac::{Hmac, Mac};
    let (Some(given), Some(properties)) = (
        parameters.as_object(),
        blueprint
            .spec
            .parameter_schema
            .get("properties")
            .and_then(serde_json::Value::as_object),
    ) else {
        return parameters.clone();
    };
    // serde_json keeps members sorted, so this is the canonical form of the submission.
    let submission = parameters.to_string();
    let mut filled = given.clone();
    for (name, property) in properties {
        if property
            .get("x-jc-widget")
            .and_then(serde_json::Value::as_str)
            != Some(MINTED_SLUG)
            || given.contains_key(name)
        {
            continue;
        }
        let Ok(mut mac) = Hmac::<sha2::Sha256>::new_from_slice(key) else {
            // HMAC takes a key of any length; there is no key this refuses.
            continue;
        };
        for part in [
            SLUG_LABEL,
            project.as_bytes(),
            blueprint.metadata.name.as_bytes(),
            blueprint.spec.version.as_str().as_bytes(),
            name.as_bytes(),
            submission.as_bytes(),
        ] {
            // Length-prefixed, so no two different tuples hash as the same byte string.
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part);
        }
        let digest = mac.finalize().into_bytes();
        // Five bits of each of the first 32 bytes: 160 bits, without bias.
        let slug: String = digest
            .iter()
            .take(32)
            .map(|byte| char::from(SLUG_ALPHABET[usize::from(byte & 0x1f)]))
            .collect();
        filled.insert(name.clone(), serde_json::Value::String(slug));
    }
    serde_json::Value::Object(filled)
}

pub(crate) fn plan_flow(
    state: &AppState,
    identity: &Identity,
    project: &str,
    request: &FlowRequest,
) -> Result<PlannedFlow, ApiError> {
    // A blueprint the caller may not run answers exactly like one that does not exist: the
    // gallery already hid it, and a different answer here would say which ones exist (R20).
    let missing = || ApiError::NotFound(format!("blueprint '{}' not found", request.blueprint));
    let envelope = state
        .mirror
        .get(ORG_NAMESPACE, BLUEPRINT_KIND, &request.blueprint)
        .filter(|envelope| may_run(identity, envelope))
        .ok_or_else(missing)?;

    // Hiding a card is not an authorisation, so the role check runs again here (CC-59); the
    // version check is next, before anything is rendered from parameters filled against a
    // schema that has since changed (CC-26).
    let current = envelope
        .spec
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if current != request.version {
        return Err(ApiError::Conflict(format!(
            "the form was filled against blueprint version {}, which is now {current}",
            request.version
        )));
    }

    // Expansion is `jcctl`'s, the same code the reconciler runs, so the manifests in the merge
    // request are byte-identical to the ones the reconciler would render (CC-25). A second
    // engine in the Portal is exactly what that requirement forbids.
    let mut typed = envelope.clone();
    typed.strip_status();
    let blueprint: jc_core::kinds::Blueprint = serde_json::to_value(&typed)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "blueprint '{}' in Git is not a valid Blueprint manifest",
                request.blueprint
            ))
        })?;

    let parameters = with_minted_slugs(
        state.config.cookie_key.signing(),
        project,
        &blueprint,
        &request.parameters,
    );
    let rendered = jcctl::blueprints::expand(&blueprint, &parameters).map_err(|e| match e {
        // CC-24: every violation at once, in `errors[]`, so the form marks all its bad fields
        // in one pass rather than sending the user round the loop once per mistake.
        jcctl::blueprints::ExpandError::Parameters(violations) => ApiError::Invalid {
            detail: format!(
                "the parameters do not match blueprint '{}': {}",
                request.blueprint,
                violations.join("; ")
            ),
            errors: violations,
        },
        // The blueprint itself is broken; the user filled in nothing wrong.
        other => ApiError::Internal(other.to_string()),
    })?;

    let mut lane = declared_lane(blueprint.spec.risk_class);
    let mut summary = PlanSummary::default();
    let mut files = Vec::with_capacity(rendered.len());
    for expanded in &rendered {
        let (mut manifest, kind_info) =
            accept_rendered(&expanded.manifest, &expanded.template, project)?;
        // The realm role on the card says which blueprints a person is offered (CC-59); what
        // they may propose in this project is the bindings of the organization repository, the
        // same gate a hand-written manifest passes, and it is read before the forge is touched
        // (PF-50, T-0799).
        let body = serde_json::to_value(&manifest)
            .map_err(|e| ApiError::Internal(format!("serialize rendered manifest: {e}")))?;
        crate::permissions::for_request(state, identity, project).check(
            kind_info.kind,
            jc_core::kinds::Verb::Propose,
            Some(&body),
        )?;
        // Nobody grants above their own rights, a blueprint's Role or RoleBinding included
        // (PF-52).
        crate::permissions::within_own_rights(state, identity, &body, None, "proposer")?;
        // A blueprint's Pipeline reaches only what its proposer reads and writes (PL-94).
        crate::pipeline_rights::check(
            state,
            identity,
            project,
            &body,
            jc_core::kinds::Verb::Propose,
        )?;
        // A name the organization holds once is refused as at every other door (PF-84, AP-14a,
        // AP-114, AP-115).
        let name = &manifest.metadata.name;
        crate::spaces::check(
            state,
            identity,
            project,
            kind_info.kind,
            name,
            &manifest.spec,
        )?;
        crate::apps::names::check(state, identity, project, kind_info.kind, name)?;
        crate::groups::check(state, identity, kind_info.kind, &manifest.metadata)?;
        let current = state
            .mirror
            .get(project, kind_info.kind, &manifest.metadata.name);
        let operation = if current.is_some() {
            Operation::Update
        } else {
            Operation::Create
        };
        lane = stricter(
            lane,
            change::classify(kind_info.kind, operation, &manifest.spec),
        );

        let diff = plan::diff(current.as_ref(), Some(&manifest));
        summary.create += diff.summary.create;
        summary.update += diff.summary.update;
        summary.delete += diff.summary.delete;

        let repo_path = resolve_repo_path(&manifest, kind_info, project)?;
        manifest.strip_status();
        let yaml = serde_norway::to_string(&manifest)
            .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;
        files.push((repo_path, yaml));
    }
    Ok(PlannedFlow {
        lane,
        summary,
        files,
    })
}

/// The lane `jc_flow_start` runs in for this caller and input: the flow's own, so a green
/// blueprint is not asked about as if it were yellow (AG-14, AG-63). An input that does not plan
/// (unknown blueprint, bad parameters) answers `None`, and the operation's registered lane stands.
pub(crate) fn flow_lane(
    state: &AppState,
    identity: &Identity,
    project: &str,
    input: &serde_json::Value,
) -> Option<Lane> {
    let request: FlowRequest = serde_json::from_value(input.clone()).ok()?;
    plan_flow(state, identity, project, &request)
        .ok()
        .map(|planned| planned.lane)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/blueprints", get(list_blueprints))
        .route("/projects/{project}/flows", post(start_flow))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A library blueprint in miniature: one Endpoint whose slug the Portal fills.
    fn blueprint() -> jc_core::kinds::Blueprint {
        jc_core::kinds::Blueprint::from_yaml(
            r#"apiVersion: joinedcontext.com/v1alpha1
kind: Blueprint
metadata: { name: dataset-publication, namespace: org }
spec:
  version: 1.0.0
  riskClass: red
  allowedRoles: [portal-approver]
  parameterSchema:
    type: object
    required: [name, endpointSlug]
    additionalProperties: false
    properties:
      name: { type: string, pattern: "^[a-z][a-z0-9-]{1,40}$" }
      endpointSlug: { type: string, pattern: "^[a-z2-7]{26,64}$", x-jc-widget: endpointSlug }
  templates:
    - name: endpoint
      template: |
        apiVersion: joinedcontext.com/v1alpha1
        kind: Endpoint
        metadata: { name: "{{ name }}" }
        spec:
          contextSpaceRef: { kind: ContextSpace, name: air }
          slug: "{{ endpointSlug }}"
          audience: public
          enabledRepresentations: [ngsi-ld]
"#,
        )
        .expect("the blueprint parses")
    }

    fn slug_of(key: &[u8], project: &str, parameters: serde_json::Value) -> String {
        with_minted_slugs(key, project, &blueprint(), &parameters)["endpointSlug"]
            .as_str()
            .expect("a slug")
            .to_owned()
    }

    /// EP-02, CC-25 (T-1571, CC-28): a left-out slug is filled with 32 base32 characters, the same
    /// for the same submission and different for another project, other parameters or another key.
    #[test]
    fn a_left_out_endpoint_slug_is_minted_from_the_submission_under_the_portals_key() {
        let key = [7u8; 32];
        let parameters = json!({ "name": "air-open" });
        let slug = slug_of(&key, "helsinki", parameters.clone());
        assert_eq!(slug.len(), 32);
        assert!(slug.bytes().all(|b| SLUG_ALPHABET.contains(&b)), "{slug}");
        assert_eq!(slug, slug_of(&key, "helsinki", parameters.clone()));
        assert_ne!(slug, slug_of(&key, "espoo", parameters.clone()));
        assert_ne!(
            slug,
            slug_of(&key, "helsinki", json!({ "name": "air-open-2" }))
        );
        assert_ne!(slug, slug_of(&[8u8; 32], "helsinki", parameters));
    }

    /// A slug the caller gave is theirs, and the schema judges it; nothing else is touched.
    #[test]
    fn a_given_slug_and_the_other_parameters_are_kept() {
        let given = json!({ "name": "air-open", "endpointSlug": "short" });
        assert_eq!(
            with_minted_slugs(&[7u8; 32], "helsinki", &blueprint(), &given),
            given
        );
        assert!(matches!(
            jcctl::blueprints::expand(&blueprint(), &given),
            Err(jcctl::blueprints::ExpandError::Parameters(_))
        ));
        // Not an object: the schema refuses it, and the mint leaves it as it came.
        assert_eq!(
            with_minted_slugs(&[7u8; 32], "helsinki", &blueprint(), &json!([1])),
            json!([1])
        );
    }

    /// CC-27: the minted slug renders into the Endpoint and into the recorded parameters, so the
    /// Endpoint validates and a re-render from the annotation gives the same file.
    #[test]
    fn the_minted_slug_renders_a_valid_endpoint_and_is_recorded() {
        let filled = with_minted_slugs(
            &[7u8; 32],
            "helsinki",
            &blueprint(),
            &json!({ "name": "air-open" }),
        );
        let rendered = jcctl::blueprints::expand(&blueprint(), &filled).expect("expands");
        let slug = filled["endpointSlug"].as_str().expect("a slug");
        let mut manifest: serde_json::Value =
            serde_norway::from_str(&rendered[0].manifest).expect("a manifest");
        assert_eq!(manifest["spec"]["slug"], slug);
        let recorded: serde_json::Value = serde_json::from_str(
            manifest["metadata"]["annotations"][jcctl::blueprints::ANNOTATION_PARAMETERS]
                .as_str()
                .expect("the parameters are recorded"),
        )
        .expect("recorded as JSON");
        assert_eq!(recorded["endpointSlug"], slug);
        manifest["metadata"]["namespace"] = json!("helsinki");
        let yaml = serde_norway::to_string(&manifest).expect("serialises");
        assert!(
            matches!(
                jc_core::registry::validate_yaml("Endpoint", &yaml),
                Some(Ok(()))
            ),
            "{yaml}"
        );
    }
}
