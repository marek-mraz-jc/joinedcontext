//! Who may do what on one Endpoint (API/01 §57, EP-103, T-5531).
//!
//! The Endpoint page's Access tab: every principal a Policy of the Endpoint's space names (and
//! the project's ServiceAccounts and the public) against each type and read, write, delete and
//! history, with the attributes and the constraint the grant carries. The Portal decides
//! nothing: it asks the gateway's `access/simulate` with no action once per principal, with its
//! own token as the Policy simulator does (§39), and lays the grant documents side by side.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

use crate::api::access_explore::EXPLORE_AT_ONCE;
use crate::api::access_simulate::{ask_gateway, Resolved};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::permissions::Effective;
use crate::state::AppState;
use crate::store::ListOptions;

/// The most principals one matrix asks the gateway about; more is a `422` asking for `?kind=`.
pub const MAX_PRINCIPALS: usize = 50;

/// The principal kinds a matrix row can be, the values `?kind=` takes.
const KINDS: [&str; 5] = ["person", "group", "role", "serviceAccount", "public"];

/// The four columns per type, in the order a row reads.
const ACTIONS: [&str; 4] = ["read", "write", "delete", "history"];

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct MatrixQuery {
    /// Only principals of this kind: `person`, `group`, `role`, `serviceAccount` or `public`.
    pub kind: Option<String>,
}

/// How a principal reaches the Endpoint: a Policy that names it, or the project's
/// ServiceAccount manifest it is.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatrixVia {
    /// `policy` or `serviceAccount`.
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatrixPrincipal {
    /// `person`, `group`, `role`, `serviceAccount` or `public`.
    pub kind: String,
    pub name: String,
    pub via: Vec<MatrixVia>,
}

/// What one principal may do on one type for one action.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCell {
    /// `{kind}:{name}` of the principal.
    pub principal: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    /// `read`, `write`, `delete` or `history`.
    pub action: String,
    /// `permit`, or `prohibit` where a prohibition takes it away.
    pub effect: String,
    /// `"*"` for every attribute, else the attribute names.
    #[schema(value_type = Value, example = json!(["status"]))]
    pub attributes: Value,
    /// The constraints the grant narrows the rows by (`q`, `scopeQ`, `geoQ`, `temporalQ`).
    pub constraint: Option<String>,
    /// The Policy that grants or prohibits this, when one Policy in force covers the type.
    pub decided_by: Option<String>,
}

/// One edge of the graph principal → Policy → Endpoint → type; node ids are
/// `principal:{kind}:{name}`, `policy:{name}`, `endpoint:{name}` and `type:{name}`.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatrixEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Matrix {
    pub endpoint: String,
    pub space: String,
    pub principals: Vec<MatrixPrincipal>,
    pub types: Vec<String>,
    pub cells: Vec<MatrixCell>,
    pub edges: Vec<MatrixEdge>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/endpoints/{name}/access/matrix",
    summary = "Who May Do What On An Endpoint",
    description = "Every principal a Policy of the Endpoint's space names, the project's ServiceAccounts the caller may read and the public, against each type and read, write, delete and history, from the gateway's own grant document per principal (API/01 §57, EP-103). Organization administrators and whoever reads Policies in the Endpoint's space; recorded as an `access.simulated` activity event. Changes no data.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Endpoint name"),
        MatrixQuery,
    ),
    responses(
        (status = 200, description = "The matrix and its graph", body = Matrix),
        (status = 400, description = "`kind` is not a principal kind", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Neither an organization administrator nor a reader of Policies in the Endpoint's space", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint, or the gateway does not serve it yet", body = crate::error::ProblemDetails),
        (status = 422, description = "More than 50 principals: narrow with `kind`", body = crate::error::ProblemDetails),
        (status = 503, description = "No gateway, or it did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn access_matrix(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<MatrixQuery>,
) -> Result<Json<Matrix>, ApiError> {
    let (identity, kind) = (&user.0.identity, query.kind.as_deref());
    let (slug, space, rights) = admitted(&state, identity, &project, &name, kind)?;
    let policies = policies_of(&state, &project, &space);
    let candidates = candidates(&state, &project, &policies, rights.as_ref(), kind)?;
    let answers = ask_each(&state, &slug, candidates.keys().cloned().collect()).await;
    let matrix = assemble(&name, space, candidates, answers, &policies)?;
    record(&state, identity, &project, &name, matrix.principals.len()).await;
    Ok(Json(matrix))
}

/// The Endpoint's slug and space, once the caller may read its matrix: an organization
/// administrator (`None` rights: they read every manifest), or a reader of Policies in the
/// Endpoint's space (their rights); everybody else is `403` before the gateway is asked.
fn admitted(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    name: &str,
    kind: Option<&str>,
) -> Result<(String, String, Option<Effective>), ApiError> {
    if kind.is_some_and(|kind| !KINDS.contains(&kind)) {
        return Err(ApiError::BadRequest(format!(
            "kind must be one of {}",
            KINDS.join(", ")
        )));
    }
    let admin = crate::permissions::for_request(state, identity, crate::permissions::ORG_NAMESPACE)
        .administers_organization();
    let rights = crate::permissions::for_request(state, identity, project);
    let refused = || {
        ApiError::Denied(
            "the access matrix is for organization administrators and for whoever reads Policies in the Endpoint's space".into(),
        )
    };
    if !admin && !rights.may_read_some("Policy") {
        return Err(refused());
    }
    let endpoint = state
        .mirror
        .get(project, "Endpoint", name)
        .ok_or_else(|| ApiError::NotFound(format!("no Endpoint {name} in project {project}")))?;
    let space = space_of(&endpoint.spec)
        .ok_or_else(|| ApiError::NotFound(format!("the Endpoint {name} names no Context Space")))?;
    if !admin && !rights.may_read_in("Policy", Some(&space)) {
        return Err(refused());
    }
    let slug = endpoint.spec["slug"]
        .as_str()
        .ok_or_else(|| ApiError::NotFound(format!("the Endpoint {name} has no slug yet")))?;
    Ok((slug.to_owned(), space, (!admin).then_some(rights)))
}

type Candidates = BTreeMap<(String, String), BTreeSet<MatrixVia>>;

/// The principals to ask about, of `kind` when given; more than [`MAX_PRINCIPALS`] is a `422`.
fn candidates(
    state: &AppState,
    project: &str,
    policies: &[(String, Value)],
    rights: Option<&Effective>,
    kind: Option<&str>,
) -> Result<Candidates, ApiError> {
    let mut found = principals(state, project, policies, rights);
    if let Some(kind) = kind {
        found.retain(|key, _| key.0 == kind);
    }
    if found.len() > MAX_PRINCIPALS {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "{} principals reach this Endpoint's space, more than {MAX_PRINCIPALS} at once: pick a kind",
                found.len()
            ),
            errors: vec![format!("kind: one of {}", KINDS.join(", "))],
        });
    }
    Ok(found)
}

/// The gateway's grant document for each principal, at most [`EXPLORE_AT_ONCE`] in flight,
/// in the principals' order.
async fn ask_each(
    state: &AppState,
    slug: &str,
    keys: Vec<(String, String)>,
) -> Vec<(String, String, Result<Value, ApiError>)> {
    futures_util::stream::iter(keys)
        .map(|key| async move {
            let question = json!({ "subject": subject(&key) });
            let answer = ask_gateway(state, slug, &question).await;
            (key.0, key.1, answer)
        })
        .buffered(EXPLORE_AT_ONCE)
        .collect()
        .await
}

/// The matrix of the answers: a principal the gateway grants nothing is left out, and the graph
/// runs principal → each Policy in force naming it → the Endpoint → each type a cell names.
fn assemble(
    name: &str,
    space: String,
    mut candidates: Candidates,
    answers: Vec<(String, String, Result<Value, ApiError>)>,
    policies: &[(String, Value)],
) -> Result<Matrix, ApiError> {
    let mut matrix = Matrix {
        endpoint: name.to_owned(),
        space,
        principals: Vec::new(),
        types: Vec::new(),
        cells: Vec::new(),
        edges: Vec::new(),
    };
    let mut edges = BTreeSet::new();
    let endpoint = format!("endpoint:{name}");
    for (kind, principal, answer) in answers {
        let answer = answer?;
        let id = format!("{kind}:{principal}");
        let in_force: Vec<&str> = answer["policies"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let cells = cells(&id, &answer, &in_force, policies);
        if cells.is_empty() {
            continue;
        }
        let via = candidates
            .remove(&(kind.clone(), principal.clone()))
            .unwrap_or_default();
        let node = format!("principal:{id}");
        let through: Vec<String> = via
            .iter()
            .filter(|step| step.kind == "policy" && in_force.contains(&step.name.as_str()))
            .map(|step| format!("policy:{}", step.name))
            .collect();
        for policy in &through {
            edges.insert(edge(&node, policy));
            edges.insert(edge(policy, &endpoint));
        }
        if through.is_empty() {
            edges.insert(edge(&node, &endpoint));
        }
        matrix.cells.extend(cells);
        matrix.principals.push(MatrixPrincipal {
            kind,
            name: principal,
            via: via.into_iter().collect(),
        });
    }
    let types: BTreeSet<String> = matrix.cells.iter().map(|c| c.entity_type.clone()).collect();
    for entity_type in &types {
        edges.insert(edge(&endpoint, &format!("type:{entity_type}")));
    }
    matrix.types = types.into_iter().collect();
    matrix.edges = edges.into_iter().collect();
    Ok(matrix)
}

fn edge(from: &str, to: &str) -> MatrixEdge {
    MatrixEdge {
        from: from.to_owned(),
        to: to.to_owned(),
    }
}

/// `contextSpaceRef` of a spec, written bare or as `{ name }`.
fn space_of(spec: &Value) -> Option<String> {
    crate::permissions::space_ref(
        &json!({ "spec": { "contextSpaceRef": spec["contextSpaceRef"] } }),
    )
}

/// The Policies of `space` in `project`, by name, with their manifests' specs.
fn policies_of(state: &AppState, project: &str, space: &str) -> Vec<(String, Value)> {
    state
        .mirror
        .list(project, "Policy", &ListOptions::default())
        .items
        .into_iter()
        .filter(|policy| space_of(&policy.spec).as_deref() == Some(space))
        .map(|policy| (policy.metadata.name, policy.spec))
        .collect()
}

/// Each principal the space's Policies name (the Policy's assignee, or each of its `rules`'),
/// the project's ServiceAccounts the caller may read (PF-59: a kind they cannot read stays
/// invisible; `None` rights read all) and the public, keyed by (kind, name). A `did` assignee is no
/// token the gateway can be asked about and is left out.
fn principals(
    state: &AppState,
    project: &str,
    policies: &[(String, Value)],
    rights: Option<&Effective>,
) -> Candidates {
    let mut found = Candidates::new();
    found.entry(("public".into(), "public".into())).or_default();
    for (name, spec) in policies {
        let rules = spec["rules"].as_array().cloned().unwrap_or_default();
        let assignees =
            std::iter::once(&spec["assignee"]).chain(rules.iter().map(|r| &r["assignee"]));
        for assignee in assignees {
            let (Some(kind), Some(id)) = (assignee["kind"].as_str(), assignee["id"].as_str())
            else {
                continue;
            };
            let key = match kind {
                "role" if id == "public" => ("public".to_owned(), "public".to_owned()),
                "user" => ("person".to_owned(), id.to_owned()),
                "group" | "role" | "serviceAccount" => (kind.to_owned(), id.to_owned()),
                _ => continue,
            };
            found.entry(key).or_default().insert(MatrixVia {
                kind: "policy".into(),
                name: name.clone(),
            });
        }
    }
    for account in state
        .mirror
        .list(project, "ServiceAccount", &ListOptions::default())
        .items
        .into_iter()
        .filter(|account| {
            rights.is_none_or(|rights| {
                serde_json::to_value(account)
                    .is_ok_and(|manifest| rights.may_read_manifest("ServiceAccount", &manifest))
            })
        })
    {
        // The Keycloak client id is derived, never chosen (Architecture/12 §3).
        let client = format!("{project}-{}", account.metadata.name);
        found
            .entry(("serviceAccount".into(), client))
            .or_default()
            .insert(MatrixVia {
                kind: "serviceAccount".into(),
                name: account.metadata.name,
            });
    }
    found
}

/// The subject a token of this principal carries, as the gateway reads it.
fn subject(key: &(String, String)) -> Resolved {
    let (kind, name) = key;
    let mut subject = Resolved::default();
    match kind.as_str() {
        "person" => subject.user = Some(name.clone()),
        "group" => subject.groups = vec![name.clone()],
        "role" => subject.roles = vec![name.clone()],
        "serviceAccount" => subject.service_account = Some(name.clone()),
        _ => {}
    }
    subject
}

/// The column a CIM 009 operation falls into, the UI's `areaOf` (AccessWords.tsx) folded into
/// four: creating and changing are both `write`; subscriptions, registrations and maps none.
fn action_of(operation: &str) -> Option<&'static str> {
    if operation.contains("EntityMap")
        || operation.contains("CSourceRegistration")
        || operation.contains("Subscription")
    {
        return None;
    }
    if operation.contains("Temporal") {
        return Some("history");
    }
    if operation.starts_with("retrieve") || operation.starts_with("query") {
        return Some("read");
    }
    if operation.starts_with("delete") || operation.starts_with("purge") {
        return Some("delete");
    }
    [
        "create", "update", "replace", "append", "merge", "upsert", "batch",
    ]
    .iter()
    .any(|verb| operation.starts_with(verb))
    .then_some("write")
}

/// The constraints of one grant as one line: `q`, `scopeQ` and the others, objects as JSON.
fn constraint(constraints: &Value) -> Option<String> {
    let members = constraints.as_object()?;
    let parts: Vec<String> = members
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(member, value)| match value.as_str() {
            Some(text) => format!("{member}: {text}"),
            None => format!("{member}: {value}"),
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The cells of one principal's grant document: one per type and column a permission or a
/// prohibition names. A subject the audience refuses has none.
fn cells(
    principal: &str,
    answer: &Value,
    in_force: &[&str],
    policies: &[(String, Value)],
) -> Vec<MatrixCell> {
    if answer["admitted"] != json!(true) {
        return Vec::new();
    }
    let mut cells: Vec<MatrixCell> = Vec::new();
    for (member, effect) in [("permissions", "permit"), ("prohibitions", "prohibit")] {
        for grant in answer[member].as_array().into_iter().flatten() {
            let entity_type = grant["resource"]["type"].as_str().unwrap_or("*").to_owned();
            let operations: Vec<&str> = grant["actions"]
                .as_array()
                .map(|actions| actions.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let columns: BTreeSet<&str> = if operations.contains(&"*") {
                ACTIONS.into_iter().collect()
            } else {
                operations.iter().filter_map(|op| action_of(op)).collect()
            };
            for action in ACTIONS.iter().filter(|a| columns.contains(**a)) {
                if cells.iter().any(|c| {
                    c.entity_type == entity_type && c.action == *action && c.effect == effect
                }) {
                    continue;
                }
                cells.push(MatrixCell {
                    principal: principal.to_owned(),
                    entity_type: entity_type.clone(),
                    action: (*action).to_owned(),
                    effect: effect.to_owned(),
                    attributes: match &grant["attributes"] {
                        Value::Array(names) => Value::Array(names.clone()),
                        _ => json!("*"),
                    },
                    constraint: constraint(&grant["constraints"]),
                    decided_by: decided_by(&entity_type, effect, in_force, policies),
                });
            }
        }
    }
    cells
}

/// The first Policy in force for the subject, of the cell's effect, whose `information` names
/// the type or names no type at all.
fn decided_by(
    entity_type: &str,
    effect: &str,
    in_force: &[&str],
    policies: &[(String, Value)],
) -> Option<String> {
    let prohibition = effect == "prohibit";
    policies
        .iter()
        .filter(|(name, _)| in_force.contains(&name.as_str()))
        .find(|(_, spec)| {
            let is_prohibition = spec["effect"]
                .as_str()
                .is_some_and(|e| e.starts_with("prohibit"));
            let types: Vec<&str> = spec["information"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|info| info["entities"].as_array().into_iter().flatten())
                .filter_map(|entity| entity["type"].as_str())
                .collect();
            is_prohibition == prohibition && (types.is_empty() || types.contains(&entity_type))
        })
        .map(|(name, _)| name.clone())
}

/// Who read the matrix of which Endpoint (EP-103): one `access.simulated` event per read. A
/// record that cannot be written is logged, not a failure of the answer.
async fn record(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    endpoint: &str,
    principals: usize,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "access.simulated".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} read the access matrix of {endpoint}: {principals} principals",
            identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "actor": identity.username,
            "endpoint": endpoint,
            "matrix": true,
            "principals": principals,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the access matrix read was not recorded");
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/endpoints/{name}/access/matrix",
        get(access_matrix),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four columns follow the UI's operation areas: a temporal read is history, not read.
    #[test]
    fn operations_fall_into_the_four_columns() {
        assert_eq!(action_of("queryEntity"), Some("read"));
        assert_eq!(action_of("retrieveTemporalEntity"), Some("history"));
        assert_eq!(action_of("queryTemporalEntities"), Some("history"));
        assert_eq!(action_of("batchUpsert"), Some("write"));
        assert_eq!(action_of("createEntity"), Some("write"));
        assert_eq!(action_of("deleteEntity"), Some("delete"));
        assert_eq!(action_of("createSubscription"), None);
        assert_eq!(action_of("retrieveCSourceRegistration"), None);
    }

    #[test]
    fn constraints_read_as_one_line() {
        assert_eq!(constraint(&json!({})), None);
        assert_eq!(
            constraint(&json!({ "q": "status==\"open\"", "scopeQ": "/city/#" })),
            Some("q: status==\"open\"; scopeQ: /city/#".to_owned())
        );
    }
}
