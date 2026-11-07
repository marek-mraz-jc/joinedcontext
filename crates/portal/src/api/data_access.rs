//! The Organization's "Data access" page (ADR-N-067 §2.5, T-6412).
//!
//! A matrix of the installation's non-public Endpoints against "Everyone in the organization"
//! and each group a Policy grants something to, each cell naming the Policy rules that grant it.
//! The Portal decides nothing: per Endpoint and column it asks the gateway's `access/simulate`
//! with no action for a token that names only that Organization (and that group), the same
//! question the signed-in catalogue asks for a person (T-6411), and reads the rules out of the
//! permitting Policies the gateway named. The same answer, narrowed by `?group=` or
//! `?endpoint=`, is a group's "Datasets this group reads" and an Endpoint's "Who can read this".
//! "What this person can read" is the catalogue as that person's realm groups would name them.
//! Every read here is for `org-admin` and `people-admin` (read on Person); a change goes through
//! a proposal of the Policy, never through this module.

use std::collections::BTreeSet;
use std::future::Future;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

use crate::api::access_explore::EXPLORE_AT_ONCE;
use crate::api::access_simulate::ask_gateway;
use crate::api::catalogue_readable::{readable, ReadableCatalogue};
use crate::auth::{CurrentUser, Identity};
use crate::error::ApiError;
use crate::permissions::organization_of;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;
use jc_core::kinds::Verb;

/// The most gateway questions one matrix asks (rows times columns); more is a `422` asking for
/// `?group=` or `?endpoint=`.
pub const MAX_QUESTIONS: usize = 600;

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DataAccessQuery {
    /// Only this group's column: the group page's "Datasets this group reads".
    pub group: Option<String>,
    /// Only this Endpoint's row, by slug: the Endpoint page's "Who can read this".
    pub endpoint: Option<String>,
}

/// One column: everyone in the Endpoint's Organization, or one of its groups.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq, PartialOrd, Ord)]
pub struct AccessColumn {
    /// `organization` or `group`.
    pub kind: String,
    /// The group's name for a `group` column, as a Policy assignee names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// One rule of a Policy that grants a column: the Policy, the rule's place (1 is the first of
/// `rules`, 0 the Policy's own assignee) and the types it names.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq)]
pub struct GrantingRule {
    pub policy: String,
    pub rule: usize,
    pub types: Vec<String>,
}

/// What one column reads on one Endpoint.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccessCell {
    pub granted: bool,
    /// The rules that grant it, empty when nothing does.
    pub rules: Vec<GrantingRule>,
    /// A group reads exactly what everyone in the organization reads here: its members get it
    /// through the organization's grant, and no rule of the group adds to it.
    pub through_organization: bool,
    /// The gateway did not answer for this cell: nothing is claimed either way.
    pub unanswered: bool,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct AccessRow {
    pub project: String,
    /// The Endpoint's manifest name.
    pub name: String,
    pub slug: String,
    pub title: Option<String>,
    /// `organization` or `project-list`.
    pub audience: String,
    /// The Organization that owns the project; `None` for a project without one.
    pub organization: Option<String>,
    /// One per column, in the order of `columns`.
    pub cells: Vec<AccessCell>,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct DataAccess {
    pub columns: Vec<AccessColumn>,
    pub rows: Vec<AccessRow>,
    /// Cells the gateway did not answer for.
    pub unanswered: usize,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/data-access",
    summary = "Data Access Matrix",
    description = "Every non-public Endpoint against everyone in its Organization and each group a Policy grants something to, each cell naming the Policy rules that grant it (ADR-N-067 §2.5). The gateway decides each cell for a token naming only that Organization or group. `?group=` keeps one column, `?endpoint=` one row. For org-admin and people-admin; changes no data.",
    tag = "people",
    params(DataAccessQuery),
    responses(
        (status = 200, description = "The matrix", body = DataAccess),
        (status = 403, description = "The caller lacks read on Person", body = crate::error::ProblemDetails),
        (status = 404, description = "No non-public Endpoint with that slug", body = crate::error::ProblemDetails),
        (status = 422, description = "Too many cells to ask about; narrow by group or endpoint", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_data_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<DataAccessQuery>,
) -> Result<Json<DataAccess>, ApiError> {
    admins_only(&state, &user.0.identity)?;
    let ask = |slug: String, question: Value| {
        let state = state.clone();
        async move { ask_gateway(&state, &slug, &question).await }
    };
    Ok(Json(matrix(&state, &query, ask).await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/people/{id}/readable",
    summary = "What This Person Can Read",
    description = "The catalogue as the person would see it signed in: every public Endpoint and each other Endpoint the gateway lets their realm groups and roles read, by name only, never a data row (ADR-N-067 §2.5). For org-admin and people-admin; changes no data.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 200, description = "The datasets the person may read", body = ReadableCatalogue),
        (status = 403, description = "The caller lacks read on Person", body = crate::error::ProblemDetails),
        (status = 404, description = "No such person", body = crate::error::ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_person_readable(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ReadableCatalogue>, ApiError> {
    admins_only(&state, &user.0.identity)?;
    let (admin, found) =
        crate::api::people::target(&state, &user.0.identity, Verb::Read, &id).await?;
    let (groups, roles) = admin.groups_and_realm_roles(&found.id).await?;
    let paths = admin.group_paths(&found.id).await?;
    let subject = person_subject(&found.id, groups, roles, &paths);
    let ask = |slug: String, question: Value| {
        let state = state.clone();
        async move { ask_gateway(&state, &slug, &question).await }
    };
    Ok(Json(readable(&state, Some(&subject), ask).await))
}

/// Read on Person at the Organization, as `org-admin` and `people-admin` hold it.
fn admins_only(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    crate::api::people::allow(state, identity, Verb::Read)
}

/// The subject a token of this person would carry: their groups and roles, and `jc_org` and
/// `jc_groups` as the realm's mappers write them from the `/{organization}/{group}` groups they
/// are in (ADR-N-067 §2.2).
fn person_subject(id: &str, groups: Vec<String>, roles: Vec<String>, paths: &[String]) -> Value {
    let nested: Vec<&String> = paths
        .iter()
        .filter(|path| path.trim_start_matches('/').contains('/'))
        .collect();
    let organizations: BTreeSet<&str> = nested
        .iter()
        .filter_map(|path| path.trim_start_matches('/').split('/').next())
        .chain(paths.iter().filter_map(|path| {
            crate::reconciler::groups::organization_of_group(path.strip_prefix('/')?)
        }))
        .collect();
    json!({
        "user": id,
        "groups": groups,
        "roles": roles,
        "organizations": organizations,
        "organizationGroups": nested,
    })
}

/// The matrix: one gateway question per row and column, at most [`EXPLORE_AT_ONCE`] in flight.
async fn matrix<F, Fut>(
    state: &AppState,
    query: &DataAccessQuery,
    ask: F,
) -> Result<DataAccess, ApiError>
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ApiError>>,
{
    let endpoints = closed_endpoints(state, query.endpoint.as_deref());
    if let Some(slug) = &query.endpoint {
        if endpoints.is_empty() {
            return Err(ApiError::NotFound(format!(
                "no non-public endpoint '{slug}'"
            )));
        }
    }
    let columns = columns(state, query.group.as_deref());
    let questions = endpoints.len().saturating_mul(columns.len());
    if questions > MAX_QUESTIONS {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "{questions} cells are more than {MAX_QUESTIONS}; choose a group or an endpoint"
            ),
            errors: vec!["group: choose a group or an endpoint to narrow the matrix".into()],
        });
    }
    let asks: Vec<_> = endpoints
        .iter()
        .flat_map(|(project, _, slug)| {
            let organization = organization_of(&state.mirror, project);
            columns.iter().map(move |column| {
                let question =
                    json!({ "subject": column_subject(organization.as_deref(), column) });
                (project.clone(), slug.clone(), column.clone(), question)
            })
        })
        .collect();
    let answers: Vec<_> = futures_util::stream::iter(asks)
        .map(|(project, slug, column, question)| {
            let answer = ask(slug.clone(), question);
            async move { (project, column, answer.await) }
        })
        .buffered(EXPLORE_AT_ONCE)
        .collect()
        .await;
    let mut unanswered = 0;
    let mut answers = answers.into_iter();
    let mut rows = Vec::new();
    for (project, endpoint, slug) in &endpoints {
        let row: Vec<_> = answers.by_ref().take(columns.len()).collect();
        // The organization's own answer, which a group column is compared with.
        let everyone = row.iter().find_map(|(_, column, answer)| match answer {
            Ok(answer) if column.kind == "organization" && granted(answer) => Some(answer.clone()),
            _ => None,
        });
        let cells = row
            .into_iter()
            .map(|(project, column, answer)| {
                let cell = cell(state, &project, &column, answer, everyone.as_ref());
                unanswered += usize::from(cell.unanswered);
                cell
            })
            .collect();
        rows.push(AccessRow {
            project: project.clone(),
            name: endpoint.metadata.name.clone(),
            slug: slug.clone(),
            title: endpoint.spec["title"].as_str().map(str::to_owned),
            audience: endpoint.spec["audience"]
                .as_str()
                .unwrap_or("organization")
                .to_owned(),
            organization: organization_of(&state.mirror, project),
            cells,
        });
    }
    Ok(DataAccess {
        columns,
        rows,
        unanswered,
    })
}

/// One cell from the gateway's answer. A group column granted exactly what `everyone` (the
/// organization column's answer) is granted reads through the organization: the gateway names
/// Policies, not rules, so a group rule beside the organization's in one Policy is claimed only
/// when the group's members get more than everyone.
fn cell(
    state: &AppState,
    project: &str,
    column: &AccessColumn,
    answer: Result<Value, ApiError>,
    everyone: Option<&Value>,
) -> AccessCell {
    let empty = |unanswered| AccessCell {
        granted: false,
        rules: Vec::new(),
        through_organization: false,
        unanswered,
    };
    match answer {
        Ok(answer) if granted(&answer) => {
            let through = column.kind == "group"
                && everyone.is_some_and(|all| all["permissions"] == answer["permissions"]);
            AccessCell {
                granted: true,
                rules: if through {
                    Vec::new()
                } else {
                    rules_for(state, project, &answer, column)
                },
                through_organization: through,
                unanswered: false,
            }
        }
        // Not served by the gateway yet: nobody reads it.
        Ok(_) | Err(ApiError::NotFound(_)) => empty(false),
        Err(err) => {
            tracing::warn!(error = %err, "the gateway did not answer for the data access matrix");
            empty(true)
        }
    }
}

/// Every Endpoint with a slug whose audience is not `public`, sorted by project and name; only
/// the one with `slug` when given.
fn closed_endpoints(
    state: &AppState,
    slug: Option<&str>,
) -> Vec<(String, ResourceEnvelope, String)> {
    let mut found = Vec::new();
    for project in state.mirror.namespaces() {
        for endpoint in state
            .mirror
            .list(&project, "Endpoint", &ListOptions::default())
            .items
        {
            let Some(own) = endpoint.spec["slug"].as_str().map(str::to_owned) else {
                continue;
            };
            if endpoint.spec["audience"].as_str() == Some("public")
                || slug.is_some_and(|wanted| wanted != own)
            {
                continue;
            }
            found.push((project.clone(), endpoint, own));
        }
    }
    found.sort_by(|a, b| (&a.0, &a.1.metadata.name).cmp(&(&b.0, &b.1.metadata.name)));
    found
}

/// "Everyone in the organization", then each group a Policy rule names as its assignee, by
/// name; with `group`, everyone and that group alone (a group no Policy names still gets its
/// column, read empty), so its cells can say what it reads through everyone.
fn columns(state: &AppState, group: Option<&str>) -> Vec<AccessColumn> {
    let column = |name: &str| AccessColumn {
        kind: "group".into(),
        group: Some(name.to_owned()),
    };
    let everyone = AccessColumn {
        kind: "organization".into(),
        group: None,
    };
    if let Some(name) = group {
        return vec![everyone, column(name)];
    }
    let mut groups = BTreeSet::new();
    for project in state.mirror.namespaces() {
        for policy in state
            .mirror
            .list(&project, "Policy", &ListOptions::default())
            .items
        {
            for assignee in assignees(&policy.spec).map(|(_, assignee)| assignee) {
                if assignee["kind"] == "group" {
                    if let Some(id) = assignee["id"].as_str().filter(|id| !id.contains('/')) {
                        groups.insert(id.to_owned());
                    }
                }
            }
        }
    }
    std::iter::once(everyone)
        .chain(groups.iter().map(|name| column(name)))
        .collect()
}

/// A token naming only the Organization, and for a group column that group too.
fn column_subject(organization: Option<&str>, column: &AccessColumn) -> Value {
    let organizations: Vec<&str> = organization.into_iter().collect();
    let groups: Vec<String> = match (organization, &column.group) {
        (Some(org), Some(group)) => vec![format!("/{org}/{group}")],
        _ => Vec::new(),
    };
    json!({
        "groups": [],
        "roles": [],
        "organizations": organizations,
        "organizationGroups": groups,
    })
}

/// The gateway admits the subject and grants at least one permission.
fn granted(answer: &Value) -> bool {
    answer["admitted"] == json!(true)
        && answer["permissions"]
            .as_array()
            .is_some_and(|permissions| !permissions.is_empty())
}

/// A Policy's own assignee (place 0) and each rule's (place 1..), with their parts.
fn assignees(spec: &Value) -> impl Iterator<Item = (usize, &Value)> {
    let rules = spec["rules"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    std::iter::once(spec)
        .chain(rules)
        .enumerate()
        .filter(|(_, part)| part["assignee"].is_object())
        .map(|(place, part)| (place, &part["assignee"]))
}

/// The permitting rules of the Policies the gateway named whose assignee is this column.
fn rules_for(
    state: &AppState,
    project: &str,
    answer: &Value,
    column: &AccessColumn,
) -> Vec<GrantingRule> {
    let mut found = Vec::new();
    for name in answer["policies"].as_array().into_iter().flatten() {
        let Some(name) = name.as_str() else { continue };
        let Some(policy) = state.mirror.get(project, "Policy", name) else {
            continue;
        };
        let spec = &policy.spec;
        let rules = spec["rules"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (place, assignee) in assignees(spec) {
            let part = place
                .checked_sub(1)
                .and_then(|i| rules.get(i))
                .unwrap_or(spec);
            let effect = part["effect"].as_str().or(spec["effect"].as_str());
            let matches = assignee["kind"] == column.kind.as_str()
                && (column.group.is_none() || assignee["id"].as_str() == column.group.as_deref());
            if effect != Some("prohibition") && matches {
                found.push(GrantingRule {
                    policy: name.to_owned(),
                    rule: place,
                    types: types_of(part),
                });
            }
        }
    }
    found
}

/// The types a rule names under `information`, or `*`.
fn types_of(part: &Value) -> Vec<String> {
    let types: Vec<String> = part["information"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|info| info["type"].as_str().map(str::to_owned))
        .collect();
    if types.is_empty() {
        vec!["*".to_owned()]
    } else {
        types
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/data-access", get(get_data_access))
        .route(
            "/organization/people/{id}/readable",
            get(get_person_readable),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(state: &AppState, kind: &str, project: &str, name: &str, spec: Value) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, project),
            spec,
            status: None,
        });
    }

    fn endpoint(state: &AppState, name: &str, audience: &str) {
        put(
            state,
            "Endpoint",
            "helsinki",
            name,
            json!({
            "slug": format!("{name}zzzzzzzzzzzzzzzzzzzzzz"), "audience": audience,
            "title": name, "contextSpaceRef": "traffic" }),
        );
    }

    /// helsinki (of the Organization helsinki): a public Endpoint, one shared with the
    /// Organization, one with traffic-analysts, one with nobody.
    fn world() -> AppState {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        put(
            &state,
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "helsinki",
            json!({ "organizationRef": { "name": "helsinki" } }),
        );
        endpoint(&state, "bikes", "public");
        endpoint(&state, "counts", "organization");
        endpoint(&state, "vehicles", "organization");
        endpoint(&state, "closed", "organization");
        put(
            &state,
            "Policy",
            "helsinki",
            "staff-traffic",
            json!({
            "contextSpaceRef": "traffic",
            "rules": [
                { "assignee": { "kind": "organization" },
                  "information": [{ "type": "TrafficFlowObserved", "operations": ["retrieveOps"] }] },
                { "assignee": { "kind": "group", "id": "traffic-analysts" },
                  "information": [{ "type": "Vehicle", "operations": ["retrieveOps"] }] },
            ] }),
        );
        state
    }

    /// A stub of the gateway: counts reads for the Organization, vehicles for its analysts.
    fn gateway(subject: &Value, slug: &str) -> Result<Value, ApiError> {
        let org = subject["organizations"] == json!(["helsinki"]);
        let analysts = subject["organizationGroups"] == json!(["/helsinki/traffic-analysts"]);
        let yes = match slug.trim_end_matches('z') {
            "counts" => org,
            "vehicles" => org && analysts,
            "bikes" => panic!("the public Endpoint is never asked about"),
            _ => false,
        };
        Ok(if yes {
            json!({ "admitted": true, "policies": ["staff-traffic"], "permissions": [{ "types": ["*"] }] })
        } else {
            json!({ "admitted": true, "policies": [], "permissions": [] })
        })
    }

    async fn of(
        state: &AppState,
        group: Option<&str>,
        endpoint: Option<&str>,
    ) -> Result<DataAccess, ApiError> {
        let query = DataAccessQuery {
            group: group.map(str::to_owned),
            endpoint: endpoint.map(str::to_owned),
        };
        let ask =
            |slug: String, question: Value| async move { gateway(&question["subject"], &slug) };
        matrix(state, &query, ask).await
    }

    /// One line per row: name, then per cell the granting `policy#rule` places or `-`.
    fn lines(matrix: &DataAccess) -> Vec<String> {
        matrix
            .rows
            .iter()
            .map(|row| {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .map(|cell| match (cell.granted, cell.through_organization) {
                        (true, true) => "everyone".to_owned(),
                        (true, false) => cell
                            .rules
                            .iter()
                            .map(|r| format!("{}#{}", r.policy, r.rule))
                            .collect::<Vec<_>>()
                            .join("+"),
                        (false, _) => "-".to_owned(),
                    })
                    .collect();
                format!("{} {}", row.name, cells.join(" "))
            })
            .collect()
    }

    /// ADR-N-067 §2.5: non-public Endpoints against everyone and each granted group, each cell
    /// naming the rule that grants it.
    #[tokio::test]
    async fn the_matrix_names_the_rule_behind_each_cell() {
        let state = world();
        let matrix = of(&state, None, None).await.expect("matrix");
        assert_eq!(
            matrix.columns,
            [
                AccessColumn {
                    kind: "organization".into(),
                    group: None
                },
                AccessColumn {
                    kind: "group".into(),
                    group: Some("traffic-analysts".into())
                },
            ]
        );
        assert_eq!(
            lines(&matrix),
            [
                "closed - -",
                "counts staff-traffic#1 everyone",
                "vehicles - staff-traffic#2"
            ]
        );
        let vehicles = &matrix.rows[2].cells[1].rules[0];
        assert_eq!(vehicles.types, ["Vehicle"]);
        assert_eq!(matrix.rows[2].organization.as_deref(), Some("helsinki"));
        assert_eq!(matrix.unanswered, 0);
    }

    /// A group page reads its own column, an Endpoint page its own row; an unknown or public
    /// slug is 404, never a row of another Endpoint.
    #[tokio::test]
    async fn a_group_and_an_endpoint_narrow_the_matrix() {
        let state = world();
        let group = of(&state, Some("traffic-analysts"), None)
            .await
            .expect("group");
        assert_eq!(group.columns.len(), 2);
        assert_eq!(
            lines(&group),
            [
                "closed - -",
                "counts staff-traffic#1 everyone",
                "vehicles - staff-traffic#2"
            ]
        );
        let nobody = of(&state, Some("planners"), None)
            .await
            .expect("empty group");
        assert_eq!(
            lines(&nobody),
            [
                "closed - -",
                "counts staff-traffic#1 everyone",
                "vehicles - -"
            ]
        );
        let slug = |name: &str| format!("{name}zzzzzzzzzzzzzzzzzzzzzz");
        let row = of(&state, None, Some(&slug("counts"))).await.expect("row");
        assert_eq!(lines(&row), ["counts staff-traffic#1 everyone"]);
        for slug in [slug("bikes"), "nonexistent".to_owned()] {
            assert!(
                matches!(
                    of(&state, None, Some(&slug)).await,
                    Err(ApiError::NotFound(_))
                ),
                "{slug}"
            );
        }
    }

    /// A cell the gateway did not answer claims nothing and is counted.
    #[tokio::test]
    async fn an_unanswered_cell_claims_nothing() {
        let state = world();
        let query = DataAccessQuery {
            group: None,
            endpoint: None,
        };
        let ask =
            |_: String, _: Value| async { Err::<Value, _>(ApiError::Unavailable("down".into())) };
        let matrix = matrix(&state, &query, ask).await.expect("matrix");
        assert_eq!(matrix.unanswered, 6);
        assert!(matrix
            .rows
            .iter()
            .flat_map(|r| &r.cells)
            .all(|c| !c.granted && c.unanswered));
    }

    /// A person's subject: `jc_org` and `jc_groups` from their `/{organization}/{group}` groups,
    /// root groups left as plain groups.
    #[test]
    fn a_person_is_asked_about_as_their_realm_groups_name_them() {
        let paths = [
            "/stewards".to_owned(),
            "/helsinki/members".to_owned(),
            "/helsinki/traffic-analysts".to_owned(),
            "/espoo_bound".to_owned(),
            "/doprava-readers".to_owned(),
        ];
        let subject = person_subject("u1", vec!["stewards".into()], vec!["viewer".into()], &paths);
        assert_eq!(subject["organizations"], json!(["espoo", "helsinki"]));
        assert_eq!(
            subject["organizationGroups"],
            json!(["/helsinki/members", "/helsinki/traffic-analysts"])
        );
        assert_eq!(subject["groups"], json!(["stewards"]));
    }

    /// A member holds no read on Person: every route of this page is 403 for them, 200 for an
    /// org-admin.
    #[tokio::test]
    async fn only_admins_read_the_data_access_page() {
        let state = world();
        let org = crate::permissions::ORG_NAMESPACE;
        put(
            &state,
            "Role",
            org,
            "people-admin",
            json!({ "rules": [{ "kinds": ["Person"], "verbs": ["read"] }] }),
        );
        put(
            &state,
            "RoleBinding",
            org,
            "people",
            json!({ "subjects": [{ "user": "owner@hel.fi" }], "role": "people-admin",
                    "scope": { "organization": "helsinki" } }),
        );
        let person = |email: &str, groups: &[&str]| Identity {
            client: None,
            subject: format!("sub-{email}"),
            username: email.to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            second_factor: false,
        };
        let member = person("member@hel.fi", &["helsinki/members"]);
        assert!(matches!(
            admins_only(&state, &member),
            Err(ApiError::Denied(_))
        ));
        assert!(admins_only(&state, &person("owner@hel.fi", &[])).is_ok());
    }
}
