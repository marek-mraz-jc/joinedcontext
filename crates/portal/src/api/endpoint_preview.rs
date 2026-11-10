//! Previewing an Endpoint's filter (API/01 §40, EP-85, T-3802).
//!
//! The filter editor's left side: one page of what the Endpoint would serve with the draft in
//! the body saved. The Portal compiles the draft into the Endpoint's candidate broker view the
//! way the reconciler compiles the saved filter, and the broker's view preview runs the page
//! through it under the person's own subject headers. Nothing is stored, so a draft never
//! serves past the Endpoint's Policies, and the same draft saved answers the same entities.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::{ProjectedClass, ProjectionFilter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use axum::http::HeaderMap;
use context_gateway::auth::subject::{anonymous_with, person, with_endpoint_roles};
use context_gateway::auth::token::{Claims, RealmAccess};
use jc_core::kinds::Audience;

use crate::auth::{CurrentUser, Session};
use crate::error::ApiError;
use crate::reconciler::views::Draft;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The largest page, and the most ids one page may be restricted to (the broker's bound).
const MAX_PAGE: u32 = 100;
const DEFAULT_PAGE: u32 = 50;

/// The body of `POST …/endpoints/{name}/preview`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreviewRequest {
    /// The draft `ModelProjection`'s classes and filter; absent names none.
    #[serde(default)]
    pub projection: Option<DraftProjection>,
    /// The draft `spec.projection.hiddenAttributes`.
    #[serde(default)]
    pub hidden_attributes: Vec<String>,
    /// The entity type of the page.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// At most 100 entity ids the page is restricted to.
    #[serde(default)]
    pub id: Vec<String>,
    /// 1 to 100, default 50.
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// The part of a `ModelProjection` the editor changes.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftProjection {
    /// `{name, slots}` per class, as `ModelProjection.spec.classes`.
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub classes: Vec<ProjectedClass>,
    /// `q`, `scopeQ`, `geoQ`, `temporalQ`, as `ModelProjection.spec.filter`.
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub filter: Option<ProjectionFilter>,
}

/// One page under the draft.
#[derive(Debug, Serialize, ToSchema)]
pub struct Previewed {
    /// How many entities the draft serves of the type.
    pub count: u64,
    /// The page, normalized as `application/json` returns it.
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub entities: Vec<Value>,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/endpoints/{name}/preview",
    summary = "Preview An Endpoint's Filter",
    description = "One page of what the Endpoint would serve with the draft projection and hidden attributes saved, read through its candidate broker view as the person (or the anonymous caller on a public Endpoint). Members of the Endpoint's Context Space only (API/01 §40, EP-85). Changes no data.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Endpoint name"),
    ),
    request_body(
        content = PreviewRequest,
        description = "The draft and the page.",
        content_type = "application/json",
        example = json!({
            "projection": {
                "classes": [{ "name": "Event", "slots": ["name", "startDate", "location"] }],
                "filter": { "q": "endDate>=\"2026-09-25T00:00:00Z\"" }
            },
            "hiddenAttributes": ["source"],
            "type": "Event",
            "limit": 50,
            "offset": 0
        })
    ),
    responses(
        (status = 200, description = "The count and the page under the draft", body = Previewed),
        (status = 400, description = "A refused draft, naming the member at fault", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A workload token, or the broker refused the reading", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint in a space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker, or the Endpoint's view is not compiled yet", body = crate::error::ProblemDetails),
    )
)]
pub async fn preview_endpoint(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Result<Json<PreviewRequest>, JsonRejection>,
) -> Result<Json<Previewed>, ApiError> {
    let identity = &user.0.identity;
    let unknown = || ApiError::NotFound(format!("no Endpoint {name} in project {project}"));
    let endpoint = state
        .mirror
        .get(&project, "Endpoint", &name)
        .filter(|_| is_dns1123(&project))
        .ok_or_else(unknown)?;
    let space = crate::permissions::space_ref(&serde_json::json!({ "spec": endpoint.spec }))
        .ok_or_else(unknown)?;
    let effective = crate::permissions::for_request(&state, identity, &project);
    // A draft can name what the saved projection does not publish yet: a member's view (SP-06,
    // SP-11), and nobody else learns the Endpoint is there.
    if !(effective.may_read_project() && effective.may_read_in("ContextSpace", Some(&space))) {
        return Err(unknown());
    }
    // Decided (worker-1, owner may override): the preview is a person's editor; a workload's
    // reading is its ServiceAccount's grants, which this route does not resolve.
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "the filter preview is for a person, not a workload token".into(),
        ));
    }
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let query = query_of(&request).map_err(ApiError::BadRequest)?;
    let views = state.views.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no broker address or admin token".into())
    })?;
    let draft = Draft {
        projection: request
            .projection
            .map(|projection| (projection.classes, projection.filter)),
        hidden_attributes: request.hidden_attributes,
    };
    let (id, view) = views
        .draft_view(&project, &name, &draft)
        .map_err(ApiError::BadRequest)?
        .ok_or_else(|| {
            ApiError::Unavailable(
                "the Endpoint's view is not compiled yet; try again shortly".into(),
            )
        })?;
    let headers = subject(&state, views, &user.0, &project, &name)?;
    let (status, answer) = views
        .preview(&id, &view, &query, &headers)
        .await
        .map_err(|reason| {
            tracing::warn!(%reason, "the broker did not preview a draft");
            ApiError::Unavailable("the broker did not answer; try again shortly".into())
        })?;
    let detail = || {
        answer
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no reason given")
            .to_owned()
    };
    match status {
        200 => Ok(Json(Previewed {
            count: answer["count"].as_u64().unwrap_or_default(),
            entities: answer["entities"].as_array().cloned().unwrap_or_default(),
        })),
        400 => Err(ApiError::BadRequest(detail())),
        403 => Err(ApiError::Denied(detail())),
        _ => {
            tracing::warn!(status, "the broker refused a preview");
            Err(ApiError::Unavailable(format!(
                "the broker refused the preview ({status})"
            )))
        }
    }
}

/// The page as a Query Entities query: the type, the ids that align it, the window.
fn query_of(request: &PreviewRequest) -> Result<String, String> {
    jc_core::names::validate_entity_type(&request.entity_type)
        .map_err(|err| format!("type: {err}"))?;
    let limit = request.limit.unwrap_or(DEFAULT_PAGE);
    if limit == 0 || limit > MAX_PAGE {
        return Err(format!("limit: a page holds 1 to {MAX_PAGE} entities"));
    }
    if request.id.len() > MAX_PAGE as usize {
        return Err(format!("id: at most {MAX_PAGE} ids restrict one page"));
    }
    if let Some(bad) = request
        .id
        .iter()
        .find(|id| id.is_empty() || id.contains(',') || id.chars().any(char::is_whitespace))
    {
        return Err(format!("id: '{bad}' is not an entity id"));
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("type", &request.entity_type);
    if !request.id.is_empty() {
        query.append_pair("id", &request.id.join(","));
    }
    query.append_pair("limit", &limit.to_string());
    query.append_pair("offset", &request.offset.unwrap_or(0).to_string());
    Ok(query.finish())
}

/// Whose reading the page is: the `x-jc-*` headers the gateway's hop to the broker carries for
/// the same caller on the same Endpoint, built by the gateway's own rules (context-gateway
/// `auth::subject`, `tenancy::caller_headers`), never from the request's headers. On a public
/// Endpoint the anonymous caller with the roles the Endpoint hands everyone (EP-16); on the
/// others the person by Keycloak id, with their standing in the project's Organization and the
/// Endpoint roles they hold (EP-14, ADR-N-067 2.3, AP-97).
pub(crate) fn subject(
    state: &AppState,
    views: &crate::reconciler::views::ViewSync,
    session: &Session,
    project: &str,
    name: &str,
) -> Result<HeaderMap, ApiError> {
    let (roles, audience) = views.caller_roles(project, name).ok_or_else(|| {
        ApiError::Unavailable("the Endpoint's view is not compiled yet; try again shortly".into())
    })?;
    if audience == Audience::Public {
        return Ok(context_gateway::middleware::tenancy::caller_headers(
            &anonymous_with(&roles),
        ));
    }
    let claims = claims_of(state, session);
    let organization = organization_of(state, project);
    let mut caller = person(&claims, audience, organization.as_deref()).ok_or_else(|| {
        ApiError::Denied("the gateway names nobody by this session's username".into())
    })?;
    with_endpoint_roles(&mut caller, &roles, Some(&claims));
    Ok(context_gateway::middleware::tenancy::caller_headers(
        &caller,
    ))
}

/// The claims of the session's verified ID token, which the realm gives `jc_org` and `jc_groups`
/// on `portal-ui` as on the access token the gateway reads. Without it (a restart, another
/// replica) the session's own identity with no Organization standing and no grant groups: fewer
/// facts than the gateway hears, never more.
fn claims_of(state: &AppState, session: &Session) -> Claims {
    let identity = &session.identity;
    state
        .session_id_token(session)
        .and_then(|token| crate::auth::oidc::jwt_payload::<Claims>(&token))
        .filter(|claims| claims.sub == identity.subject)
        .unwrap_or_else(|| Claims {
            sub: identity.subject.clone(),
            iss: String::new(),
            azp: None,
            preferred_username: Some(identity.username.clone()),
            realm_access: Some(RealmAccess {
                roles: identity.roles.clone(),
            }),
            groups: identity.groups.clone(),
            jc_org: Vec::new(),
            jc_groups: Some(Vec::new()),
            resource_access: Default::default(),
            exp: 0,
            iat: None,
            agreement_id: None,
            participant: None,
            aud: Vec::new(),
            scope: None,
        })
}

/// The Organization the gateway gives `project`: its Project manifest's `organizationRef` when
/// the spec parses, else none (context-gateway `store::provenance_of`). Unlike
/// [`crate::permissions::organization_of`], no installation fallback: that would make a member of
/// someone the gateway does not.
fn organization_of(state: &AppState, project: &str) -> Option<String> {
    let envelope = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)?;
    serde_json::from_value::<jc_core::kinds::ProjectSpec>(envelope.spec)
        .ok()
        .map(|spec| spec.organization_ref.name().to_owned())
}

/// Percent-encoded UTF-8 with the RFC 3986 unreserved characters kept, so a diacritic travels
/// and a comma never splits a list member.
pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/endpoints/{name}/preview",
        post(preview_endpoint),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Identity;

    fn request(id: Vec<&str>, limit: Option<u32>) -> PreviewRequest {
        PreviewRequest {
            projection: None,
            hidden_attributes: Vec::new(),
            entity_type: "Event".into(),
            id: id.into_iter().map(str::to_owned).collect(),
            limit,
            offset: None,
        }
    }

    #[test]
    fn the_page_is_a_bounded_query() {
        assert_eq!(
            query_of(&request(vec!["urn:a", "urn:b"], None)).as_deref(),
            Ok("type=Event&id=urn%3Aa%2Curn%3Ab&limit=50&offset=0")
        );
        assert!(query_of(&request(vec![], Some(0))).is_err());
        assert!(query_of(&request(vec![], Some(101))).is_err());
        assert!(query_of(&request(vec!["a,b"], None)).is_err());
        assert!(query_of(&request(vec![""], None)).is_err());
        assert!(query_of(&request(vec!["urn:a"; 101], None)).is_err());
        let mut bad = request(vec![], None);
        bad.entity_type = "Ev ent".into();
        assert!(query_of(&bad).is_err());
    }

    /// A session whose ID token this process does not keep is named by its Keycloak id with no
    /// Organization standing and no grant groups, and a project without a parsing Project
    /// manifest has no Organization, installation or not (the gateway's rule).
    #[test]
    fn without_a_kept_id_token_a_person_is_named_narrower() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let session = Session {
            identity: Identity {
                client: None,
                subject: "id".into(),
                username: "jana@hel.fi".into(),
                email: None,
                name: None,
                roles: vec!["viewer".into()],
                groups: vec!["helsinki".into()],
                second_factor: false,
            },
            expires_at: i64::MAX,
            issued_at: 1,
            signed_in_at: None,
            id_token: String::new(),
            access_expires_at: i64::MAX,
            refresh_token: None,
        };
        let claims = claims_of(&state, &session);
        let caller = person(&claims, Audience::Organization, Some("helsinki")).expect("a person");
        assert_eq!(caller.user_principal(), Some("id"));
        assert!(!caller.member_of_organization);
        assert!(caller.grant_groups().is_empty());
        assert_eq!(organization_of(&state, "city"), None);
    }
}
