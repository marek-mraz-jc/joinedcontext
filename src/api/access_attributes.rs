//! The Policy behind a refused attribute, for the entity grid's cell inspector (API/01 §58, T-5528).
//!
//! The per-attribute decisions are the gateway's: the browser reads `access/attributes` with the
//! person's own session, so the Portal never computes or relays a grant. What the gateway never
//! tells a caller is which Policy refused them (GW6). This route asks the gateway's
//! `access/simulate` about the caller themself, as their verified session names them, and passes
//! the deciding Policy on only to a caller who may read that Policy.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use crate::api::access_simulate::{ask_gateway, Resolved};
use crate::auth::{CurrentUser, Identity};
use crate::error::ApiError;
use crate::state::AppState;

/// The longest type or attribute name the route passes on.
const MAX_NAME: usize = 255;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct Asked {
    /// The entity type, a class of the Endpoint's space model.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// The attribute's short name or IRI.
    pub attribute: String,
    /// `read`, `write` (default) or `delete`.
    #[serde(default)]
    pub action: Option<String>,
}

/// What decided one attribute for the caller.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttributeExplained {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attribute: String,
    pub action: String,
    /// The gateway's decision; `None` when it was not asked (the caller may read no Policy).
    pub decision: Option<bool>,
    /// `policy_grant_matched`, `prohibited`, `no_grant` or `not_admitted`, as the gateway says.
    pub reason: Option<String>,
    /// The deciding Policy's manifest name, only for a caller who may read it.
    pub policy: Option<String>,
    pub policy_title: Option<String>,
    /// A Policy decided and the caller may not read it.
    pub policy_hidden: bool,
}

/// The CIM 009 operation a chip stands for.
fn operation_of(action: Option<&str>) -> Result<(&'static str, &'static str), ApiError> {
    match action.unwrap_or("write") {
        "read" => Ok(("read", "queryEntity")),
        "write" => Ok(("write", "updateAttrs")),
        "delete" => Ok(("delete", "deleteAttrs")),
        _ => Err(ApiError::BadRequest(
            "action must be read, write or delete".into(),
        )),
    }
}

fn named<'a>(what: &str, value: &'a str) -> Result<&'a str, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_NAME || value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(format!(
            "{what} must be 1 to {MAX_NAME} printable characters"
        )));
    }
    Ok(value)
}

/// The caller as a token of theirs carries them: never a subject the request names.
fn caller(identity: &Identity) -> Resolved {
    Resolved {
        user: Some(identity.username.clone()),
        groups: identity
            .groups
            .iter()
            .map(|group| group.trim_start_matches('/').to_owned())
            .collect(),
        roles: identity.roles.clone(),
        service_account: None,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/endpoints/{name}/access/attributes",
    summary = "Why An Attribute Is Refused",
    description = "What decided one attribute for the caller and, for a caller who may read it, the Policy that decided (API/01 §58). The decisions table itself is the gateway's `access/attributes`, read with the person's session. Whoever may read the Endpoint. Changes no data.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Endpoint name"),
        Asked,
    ),
    responses(
        (status = 200, description = "The decision and, where the caller may read it, the deciding Policy", body = AttributeExplained),
        (status = 400, description = "A missing or malformed type, attribute or action", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not read the Endpoint", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint, or no such type in its space model", body = crate::error::ProblemDetails),
        (status = 503, description = "No gateway, or it did not answer", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn explain_attribute(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    asked: Result<Query<Asked>, QueryRejection>,
) -> Result<Json<AttributeExplained>, ApiError> {
    let identity = &user.0.identity;
    // An unknown Endpoint and an unknown type read the same: the answer names neither.
    let not_found = || {
        ApiError::NotFound(format!(
            "no Endpoint {name} with that type in project {project}"
        ))
    };
    let effective = crate::permissions::for_request(&state, identity, &project);
    if !effective.may_read_project() {
        return Err(not_found());
    }
    let endpoint = state
        .mirror
        .get(&project, "Endpoint", &name)
        .and_then(|endpoint| serde_json::to_value(endpoint).ok())
        .ok_or_else(not_found)?;
    if !effective.may_read_manifest("Endpoint", &endpoint) {
        return Err(ApiError::Denied(format!(
            "reading Endpoint {name} needs read on Endpoint in its Context Space"
        )));
    }
    let Query(asked) = asked.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let entity_type = named("type", &asked.entity_type)?;
    let attribute = named("attribute", &asked.attribute)?;
    let (action, operation) = operation_of(asked.action.as_deref())?;
    let spec = &endpoint["spec"];
    let classes = crate::resource::reference_name(&spec["contextSpaceRef"])
        .map(|space| crate::api::assistant_context::model_classes(&state.mirror, &project, space))
        .unwrap_or_default();
    // A model that lists no classes (a LinkML-only one) cannot tell a type from a typo: the
    // gateway's answer then decides alone.
    if !classes.is_empty() && !classes.iter().any(|class| class == entity_type) {
        return Err(not_found());
    }
    let slug = spec["slug"].as_str().ok_or_else(not_found)?;
    let mut explained = AttributeExplained {
        entity_type: entity_type.to_owned(),
        attribute: attribute.to_owned(),
        action: action.to_owned(),
        decision: None,
        reason: None,
        policy: None,
        policy_title: None,
        policy_hidden: false,
    };
    // Someone who may read no Policy here learns nothing a question would add: the gateway's
    // own table already gave them the decision and its reason.
    if !effective.may_read_some("Policy") {
        explained.policy_hidden = true;
        return Ok(Json(explained));
    }
    let question = json!({
        "subject": caller(identity),
        "action": { "name": operation },
        "resource": { "type": entity_type, "properties": { "attribute": attribute } },
    });
    let answer = ask_gateway(&state, slug, &question).await?;
    explained.decision = Some(answer["decision"] == json!(true));
    explained.reason = Some(
        answer["context"]["reason"]
            .as_str()
            .unwrap_or("no_grant")
            .to_owned(),
    );
    if let Some(policy) = answer["context"]["policy"].as_str() {
        let readable = state
            .mirror
            .get(&project, "Policy", policy)
            .and_then(|manifest| serde_json::to_value(manifest).ok())
            .filter(|manifest| effective.may_read_manifest("Policy", manifest));
        match readable {
            Some(manifest) => {
                explained.policy = Some(policy.to_owned());
                explained.policy_title =
                    crate::resource::title_text(&manifest["metadata"]["title"]);
            }
            None => explained.policy_hidden = true,
        }
    }
    Ok(Json(explained))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/endpoints/{name}/access/attributes",
        get(explain_attribute),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_chip_is_one_operation_and_nothing_else_passes() {
        assert_eq!(operation_of(None).ok(), Some(("write", "updateAttrs")));
        assert_eq!(
            operation_of(Some("read")).ok(),
            Some(("read", "queryEntity"))
        );
        assert_eq!(
            operation_of(Some("delete")).ok(),
            Some(("delete", "deleteAttrs"))
        );
        assert!(operation_of(Some("purge")).is_err());
    }
}
