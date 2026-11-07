//! Pipeline access requests (API/01 §47, PL-90…PL-93, T-4863): a pipeline's ask for its
//! ServiceAccount's read or write on an Endpoint of another Context Space.
//!
//! A request grants nothing. It waits in the owning project; an owner of the Endpoint's space
//! approves it, and the approval proposes the grant as a Change of the owning project, authored
//! by the approver and judged by that project's lane: the Endpoint admitting the requesting
//! project and one Policy naming exactly the requested types and attributes for the pipeline's
//! own account. Revoking proposes that Policy ended now.
//!
//! A person asks the same way on an Endpoint they read but get nothing through (EP-55, PF-58,
//! T-5615): a read of named types of its space, for themself only. Approving proposes one Policy
//! assigned to that person, until the request's end.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use time::OffsetDateTime;
use utoipa::ToSchema;

/// At most this many entity types in one request (PL-90).
pub const MAX_TYPES: usize = 20;
/// At most this many attributes per type (PL-90).
pub const MAX_ATTRS: usize = 50;
/// The longest reason a request or a decision carries.
pub const MAX_REASON: usize = 500;

/// What the pipeline asks to do through the Endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Operations {
    Read,
    Write,
    // A read and its history: what a KPI's `prev` term reads (PL-117). A plain comment: a doc
    // comment on one variant turns the schema's enum into a oneOf.
    History,
}

impl Operations {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::History => "history",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "write" => Self::Write,
            "history" => Self::History,
            _ => Self::Read,
        }
    }

    /// The Policy operations: a read is the retrieve group, a write the pipeline's own write
    /// grant (PL-20).
    pub fn policy_operations(self) -> Value {
        match self {
            Self::Read => json!(["retrieveOps"]),
            // The retrieve group holds no temporal row (CIM 009 Table 4.20-2).
            Self::History => json!(["retrieveOps", "queryTemporal"]),
            Self::Write => json!(["upsertBatch", "createBatch", "queryBatch"]),
        }
    }
}

/// One entity type and the attributes asked for on it, each written out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeGrant {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub attrs: Vec<String>,
}

/// The Endpoint a request names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EndpointName {
    pub project: String,
    pub name: String,
}

/// The body a requester sends.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewRequest {
    pub endpoint: EndpointName,
    pub operations: Operations,
    pub types: Vec<TypeGrant>,
    /// The grant's end, RFC 3339; none is no end.
    #[serde(default)]
    pub until: Option<String>,
    pub reason: String,
}

/// Who a request asks for: a pipeline of `project`, or the person who filed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requester {
    Pipeline(String),
    /// The person's username, the id a Policy's `user` assignee names.
    Person(String),
}

/// A stored request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRequest {
    pub id: String,
    /// The pipeline's project; for a person's request the Endpoint's own.
    pub project: String,
    pub requester: Requester,
    pub endpoint_project: String,
    pub endpoint: String,
    pub space: String,
    pub operations: Operations,
    pub types: Vec<TypeGrant>,
    pub until: Option<OffsetDateTime>,
    pub reason: String,
    pub state: String,
    pub requested_by: String,
    pub requested_at: OffsetDateTime,
    pub decided_by: Option<String>,
    pub decided_at: Option<OffsetDateTime>,
    pub decision_reason: Option<String>,
    pub change: Option<String>,
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl AccessRequest {
    /// The account a grant names: the pipeline's own, qualified by its project (PL-19, PL-92),
    /// or the person's Keycloak user id (ADR-N-061; a request filed before it holds a username).
    pub fn account(&self) -> String {
        match &self.requester {
            Requester::Pipeline(pipeline) => format!("{}/pl-{pipeline}", self.project),
            Requester::Person(person) => person.clone(),
        }
    }

    /// The Policy that carries the grant (PL-91): a person's is named by the request, since a
    /// username is no manifest name.
    pub fn policy_name(&self) -> String {
        match &self.requester {
            Requester::Pipeline(pipeline) => {
                format!("{}-{}-{pipeline}", self.endpoint, self.project)
            }
            // `ar-` and 12 hex digits leave 47 characters of the 63 for the Endpoint's name.
            Requester::Person(_) if self.endpoint.len() <= 47 => {
                format!("{}-{}", self.endpoint, self.id)
            }
            Requester::Person(_) => format!("access-{}", self.id),
        }
    }

    /// The pipeline's name, `None` for a person's request.
    pub fn pipeline(&self) -> Option<&str> {
        match &self.requester {
            Requester::Pipeline(pipeline) => Some(pipeline),
            Requester::Person(_) => None,
        }
    }

    /// The state a reader sees: an approved grant whose end has passed is `ended`.
    pub fn shown_state(&self, now: OffsetDateTime) -> &str {
        match self.until {
            Some(until) if self.state == "approved" && until <= now => "ended",
            _ => self.state.as_str(),
        }
    }

    /// The request as the API answers it: names and the asked-for grant, never the space's data.
    pub fn view(&self, now: OffsetDateTime) -> Value {
        json!({
            "id": self.id,
            "pipeline": self.pipeline().map(|name| json!({ "project": self.project, "name": name })),
            "person": match &self.requester {
                Requester::Person(person) => Some(person.as_str()),
                Requester::Pipeline(_) => None,
            },
            "account": self.account(),
            "endpoint": { "project": self.endpoint_project, "name": self.endpoint },
            "space": self.space,
            "operations": self.operations.as_str(),
            "types": self.types,
            "until": self.until.map(rfc3339),
            "reason": self.reason,
            "state": self.shown_state(now),
            "requestedBy": self.requested_by,
            "requestedAt": rfc3339(self.requested_at),
            "decidedBy": self.decided_by,
            "decidedAt": self.decided_at.map(rfc3339),
            "decisionReason": self.decision_reason,
            "change": self.change,
        })
    }
}

/// An NGSI-LD attribute name as a grant may hold it: a term, never a wildcard or an IRI.
fn is_attr_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A reason as stored: trimmed, one to 500 characters.
pub fn reason(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_REASON {
        return Err(format!(
            "reason is required, at most {MAX_REASON} characters (reason-required)"
        ));
    }
    Ok(trimmed.to_owned())
}

/// The types and attributes of a request, refused when one asks for more than it names
/// (PL-90): one to twenty distinct types, each with one to fifty distinct attribute names.
pub fn checked_types(types: &[TypeGrant]) -> Result<Vec<TypeGrant>, String> {
    if types.is_empty() || types.len() > MAX_TYPES {
        return Err(format!(
            "name one to {MAX_TYPES} entity types; a request never asks for every type"
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(types.len());
    for grant in types {
        let entity_type = grant.entity_type.trim();
        jc_core::names::validate_entity_type(entity_type).map_err(|_| {
            format!("'{entity_type}' is not an entity type name (PascalCase, PF-42)")
        })?;
        if !seen.insert(entity_type.to_owned()) {
            return Err(format!("the type {entity_type} is named twice"));
        }
        if grant.attrs.is_empty() || grant.attrs.len() > MAX_ATTRS {
            return Err(format!(
                "name one to {MAX_ATTRS} attributes of {entity_type}; a request never asks for all of them"
            ));
        }
        let mut attrs = Vec::with_capacity(grant.attrs.len());
        for attr in &grant.attrs {
            let attr = attr.trim();
            if !is_attr_name(attr) {
                return Err(format!(
                    "'{attr}' is not an attribute name of {entity_type}: a letter, then letters, digits or _"
                ));
            }
            if attrs.iter().any(|known: &String| known == attr) {
                return Err(format!(
                    "the attribute {attr} of {entity_type} is named twice"
                ));
            }
            attrs.push(attr.to_owned());
        }
        out.push(TypeGrant {
            entity_type: entity_type.to_owned(),
            attrs,
        });
    }
    Ok(out)
}

/// The types a person asks for (EP-55, T-5615): one to twenty distinct type names, each one the
/// Endpoint serves (`served`), each for all its attributes. The error names the field.
pub fn checked_person_types(
    types: &[String],
    served: &[String],
) -> Result<Vec<TypeGrant>, Vec<String>> {
    if types.is_empty() || types.len() > MAX_TYPES {
        return Err(vec![format!(
            "types: choose one to {MAX_TYPES} entity types"
        )]);
    }
    let mut problems = Vec::new();
    let mut out: Vec<TypeGrant> = Vec::with_capacity(types.len());
    for (at, name) in types.iter().enumerate() {
        let name = name.trim();
        if !served.iter().any(|one| one == name) {
            problems.push(format!(
                "types[{at}]: '{name}' is no type this endpoint serves"
            ));
        } else if out.iter().any(|known| known.entity_type == name) {
            problems.push(format!("types[{at}]: the type {name} is named twice"));
        } else {
            out.push(TypeGrant {
                entity_type: name.to_owned(),
                attrs: Vec::new(),
            });
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// The end date of a request: in the future, or none.
pub fn checked_until(
    until: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<OffsetDateTime>, String> {
    let Some(text) = until.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let at = OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).map_err(
        |_| format!("until '{text}' is not an RFC 3339 time, e.g. 2027-01-31T23:59:59Z"),
    )?;
    if at <= now {
        return Err("until has passed; choose a later end or none".into());
    }
    Ok(Some(at))
}

/// Whether the Policy `policy` already grants `account` the operations on every requested type,
/// so a request would add nothing (PL-90).
pub fn already_granted(
    policies: &[Value],
    account: &str,
    request: &[TypeGrant],
    ops: Operations,
) -> bool {
    let wanted = ops.policy_operations();
    let wanted: Vec<&str> = wanted
        .as_array()
        .map(|ops| ops.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    request.iter().all(|grant| {
        policies.iter().any(|policy| {
            let spec = &policy["spec"];
            let ops: Vec<&str> = spec["operations"]
                .as_array()
                .map(|ops| ops.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            spec["assignee"]["kind"] == "serviceAccount"
                && spec["assignee"]["id"] == account
                && spec["validity"]["to"].is_null()
                && wanted.iter().all(|op| ops.contains(op))
                && spec["information"].as_array().is_some_and(|infos| {
                    infos.iter().any(|info| {
                        info["entities"].as_array().is_some_and(|entities| {
                            entities
                                .iter()
                                .any(|e| e["type"] == grant.entity_type.as_str())
                        })
                    })
                })
        })
    })
}

/// A manifest as the import takes it back: without the status the platform computed.
fn writable(manifest: &Value) -> Value {
    let mut copy = manifest.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("status");
    }
    copy
}

/// What approving `request` proposes (PL-91): the Endpoint admitting the requesting project when
/// its audience is a list, and the Policy granting exactly what was asked to the pipeline's
/// account until the request's end.
/// A person's grant is the Policy alone: the person reads the Endpoint in its own project already.
pub fn grant_bundle(request: &AccessRequest, endpoint: &Value) -> Vec<Value> {
    let mut manifests = Vec::with_capacity(2);
    let audience = endpoint["spec"]["audience"].as_str().unwrap_or("project");
    let (assignee_kind, person) = match &request.requester {
        Requester::Pipeline(_) => ("serviceAccount", false),
        Requester::Person(_) => ("user", true),
    };
    if !person && audience != "organization" && audience != "public" {
        let mut updated = writable(endpoint);
        let mut allowed: Vec<String> = updated["spec"]["allowedProjects"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if !allowed.contains(&request.project) {
            allowed.push(request.project.clone());
        }
        updated["spec"]["audience"] = json!("project-list");
        updated["spec"]["allowedProjects"] = json!(allowed);
        manifests.push(updated);
    }
    let information: Vec<Value> = request
        .types
        .iter()
        .map(|grant| {
            let mut info = json!({ "entities": [{ "type": grant.entity_type }] });
            // A person asks for whole types; a pipeline names each attribute (PL-90).
            if !grant.attrs.is_empty() {
                info["propertyNames"] = json!(grant.attrs);
            }
            info
        })
        .collect();
    let mut spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": request.space },
        // Rendered by the loader, so a copied organization is its own assigner (CC-82).
        "assigner": "did:web:{orgDomain}",
        "assignee": { "kind": assignee_kind, "id": request.account() },
        "operations": request.operations.policy_operations(),
        "information": information,
    });
    if let Some(until) = request.until {
        spec["validity"] = json!({ "to": rfc3339(until) });
    }
    manifests.push(json!({
        "apiVersion": crate::resource::API_VERSION,
        "kind": "Policy",
        "metadata": {
            "name": request.policy_name(),
            "namespace": request.endpoint_project,
            // Nobody's name or address goes into Git with the grant (ADR-N-061).
            "description": { "en": format!("Access request {}: {}", request.id, request.reason) },
        },
        "spec": spec,
    }));
    manifests
}

/// What revoking proposes (PL-93): the grant's Policy ended `now`, and the Endpoint without the
/// requesting project when no other Policy of the Endpoint still names that project.
pub fn revoke_bundle(
    request: &AccessRequest,
    endpoint: &Value,
    policies: &[Value],
    now: OffsetDateTime,
) -> Vec<Value> {
    let mut manifests = Vec::with_capacity(2);
    let own = request.policy_name();
    let share = format!("{}-{}", request.endpoint, request.project);
    let others_remain = policies.iter().any(|policy| {
        let name = policy["metadata"]["name"].as_str().unwrap_or_default();
        name != own
            && (name == share || name.starts_with(&format!("{share}-")))
            && policy["spec"]["validity"]["to"]
                .as_str()
                .and_then(|to| {
                    OffsetDateTime::parse(to, &time::format_description::well_known::Rfc3339).ok()
                })
                .is_none_or(|to| to > now)
    });
    // A person's grant never admitted a project, so its revoke leaves the Endpoint alone.
    let listed = request.pipeline().is_some()
        && endpoint["spec"]["allowedProjects"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == request.project.as_str()));
    if listed && !others_remain {
        let mut updated = writable(endpoint);
        if let Some(list) = updated["spec"]["allowedProjects"].as_array_mut() {
            list.retain(|p| p != request.project.as_str());
        }
        manifests.push(updated);
    }
    if let Some(policy) = policies
        .iter()
        .find(|policy| policy["metadata"]["name"] == own.as_str())
    {
        let mut ended = writable(policy);
        ended["spec"]["validity"]["to"] = json!(rfc3339(now));
        manifests.push(ended);
    }
    manifests
}

/// Why the gateway now refuses a pipeline one of whose grants ended (PL-93): "access revoked by
/// {owner} on {date}" for a revoked grant, "access ended on {date}" for one whose end passed, the
/// newest of them; `None` when no grant ended, or while another grant on the same Endpoint holds.
pub fn refusal_reason(requests: &[AccessRequest], now: OffsetDateTime) -> Option<String> {
    let in_force =
        |r: &AccessRequest| r.state == "approved" && r.until.is_none_or(|until| until > now);
    let held = |r: &AccessRequest| {
        requests.iter().any(|other| {
            in_force(other)
                && other.endpoint_project == r.endpoint_project
                && other.endpoint == r.endpoint
        })
    };
    requests
        .iter()
        .filter(|r| !held(r))
        .filter_map(|r| match (r.state.as_str(), r.decided_at, r.until) {
            ("revoked", Some(at), _) => Some((
                at,
                format!(
                    "access revoked by {} on {}",
                    r.decided_by.as_deref().unwrap_or("an owner of the space"),
                    at.date()
                ),
            )),
            ("approved", _, Some(until)) if until <= now => {
                Some((until, format!("access ended on {}", until.date())))
            }
            _ => None,
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, text)| text)
}

/// Whether a Policy of another project grants the pipeline's own account now (PL-92, PL-93): an
/// approval after a revocation brings a pipeline paused for its refusals back.
pub fn grant_in_force(
    mirror: &crate::store::Mirror,
    project: &str,
    pipeline: &str,
    now: OffsetDateTime,
) -> bool {
    let account = format!("{project}/pl-{pipeline}");
    mirror
        .matching(|env| {
            env.kind == "Policy"
                && env.metadata.namespace.as_deref() != Some(project)
                && env.spec["assignee"]["id"] == account.as_str()
        })
        .iter()
        .any(|policy| {
            policy.spec["validity"]["to"]
                .as_str()
                .and_then(|to| {
                    OffsetDateTime::parse(to, &time::format_description::well_known::Rfc3339).ok()
                })
                .is_none_or(|to| to > now)
        })
}

/// Whether a run log line is a refusal [`refusal_reason`] wrote (PL-93).
pub fn is_refusal(message: &str) -> bool {
    message.starts_with("access revoked by ") || message.starts_with("access ended on ")
}

/// A new request id: `ar-` and twelve hex digits from the CSPRNG.
pub fn new_id() -> String {
    use sha2::{Digest, Sha256};
    let drawn = Sha256::digest(crate::auth::csrf::new_token().as_bytes());
    let hex: String = drawn[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!("ar-{hex}")
}

/// Whether `id` has the shape of a request id, so a malformed one never reaches the database.
pub fn is_id(id: &str) -> bool {
    id.len() == 15
        && id.starts_with("ar-")
        && id[3..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

// --- the store -----------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    project: String,
    pipeline: String,
    person: Option<String>,
    endpoint_project: String,
    endpoint: String,
    space: String,
    operations: String,
    types: Value,
    until: Option<OffsetDateTime>,
    reason: String,
    state: String,
    requested_by: String,
    requested_at: OffsetDateTime,
    decided_by: Option<String>,
    decided_at: Option<OffsetDateTime>,
    decision_reason: Option<String>,
    change: Option<String>,
}

impl From<Row> for AccessRequest {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            project: row.project,
            requester: match row.person {
                Some(person) => Requester::Person(person),
                None => Requester::Pipeline(row.pipeline),
            },
            endpoint_project: row.endpoint_project,
            endpoint: row.endpoint,
            space: row.space,
            operations: Operations::parse(&row.operations),
            types: serde_json::from_value(row.types).unwrap_or_default(),
            until: row.until,
            reason: row.reason,
            state: row.state,
            requested_by: row.requested_by,
            requested_at: row.requested_at,
            decided_by: row.decided_by,
            decided_at: row.decided_at,
            decision_reason: row.decision_reason,
            change: row.change,
        }
    }
}

/// A `SELECT` of every column, with the rest of the statement: one static string, as sqlx asks.
macro_rules! select {
    ($rest:literal) => {
        concat!(
            "SELECT id, project, pipeline, person, endpoint_project, endpoint, space, operations, types, until, \
             reason, state, requested_by, requested_at, decided_by, decided_at, decision_reason, change \
             FROM access_requests ",
            $rest
        )
    };
}

/// Stores a new request; `Ok(false)` when the pipeline or the person already has an open or
/// granted one for that Endpoint (PL-90).
pub async fn insert(pool: &PgPool, request: &AccessRequest) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO access_requests (id, project, pipeline, person, endpoint_project, endpoint, space, \
           operations, types, until, reason, state, requested_by, requested_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'pending', $12, $13) ON CONFLICT DO NOTHING",
    )
    .bind(&request.id)
    .bind(&request.project)
    .bind(request.pipeline().unwrap_or_default())
    .bind(match &request.requester {
        Requester::Person(person) => Some(person.as_str()),
        Requester::Pipeline(_) => None,
    })
    .bind(&request.endpoint_project)
    .bind(&request.endpoint)
    .bind(&request.space)
    .bind(request.operations.as_str())
    .bind(json!(request.types))
    .bind(request.until)
    .bind(&request.reason)
    .bind(&request.requested_by)
    .bind(request.requested_at)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// One request into `endpoint_project`, or `None`.
pub async fn get(
    pool: &PgPool,
    endpoint_project: &str,
    id: &str,
) -> Result<Option<AccessRequest>, sqlx::Error> {
    sqlx::query_as::<_, Row>(select!("WHERE id = $1 AND endpoint_project = $2"))
        .bind(id)
        .bind(endpoint_project)
        .fetch_optional(pool)
        .await
        .map(|row| row.map(AccessRequest::from))
}

/// The requests into `endpoint_project`'s spaces, newest first.
pub async fn incoming(
    pool: &PgPool,
    endpoint_project: &str,
) -> Result<Vec<AccessRequest>, sqlx::Error> {
    sqlx::query_as::<_, Row>(select!(
        "WHERE endpoint_project = $1 ORDER BY requested_at DESC LIMIT 200"
    ))
    .bind(endpoint_project)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(AccessRequest::from).collect())
}

/// The requests of one pipeline, newest first.
pub async fn outgoing(
    pool: &PgPool,
    project: &str,
    pipeline: &str,
) -> Result<Vec<AccessRequest>, sqlx::Error> {
    sqlx::query_as::<_, Row>(select!(
        "WHERE project = $1 AND pipeline = $2 AND person IS NULL ORDER BY requested_at DESC LIMIT 200"
    ))
    .bind(project)
    .bind(pipeline)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(AccessRequest::from).collect())
}

/// A person's own requests on one Endpoint, newest first.
pub async fn of_person(
    pool: &PgPool,
    person: &str,
    (endpoint_project, endpoint): (&str, &str),
) -> Result<Vec<AccessRequest>, sqlx::Error> {
    sqlx::query_as::<_, Row>(select!(
        "WHERE person = $1 AND endpoint_project = $2 AND endpoint = $3 ORDER BY requested_at DESC LIMIT 50"
    ))
    .bind(person)
    .bind(endpoint_project)
    .bind(endpoint)
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(AccessRequest::from).collect())
}

/// Moves a request from `from` to `to`, recording who decided, why and the Change; `Ok(false)`
/// when it was no longer in `from` (another owner decided first).
pub(crate) async fn decide(
    pool: &PgPool,
    id: &str,
    (from, to): (&str, &str),
    by: &str,
    reason: Option<&str>,
    change: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE access_requests SET state = $3, decided_by = $4, decided_at = now(), \
           decision_reason = COALESCE($5, decision_reason), change = COALESCE($6, change) \
         WHERE id = $1 AND state = $2",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .bind(by)
    .bind(reason)
    .bind(change)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Takes back a decision `by` claimed as `claimed` whose Change was never proposed: the request
/// as it was before the claim, its earlier decider, time, reason and Change included.
pub(crate) async fn take_back(
    pool: &PgPool,
    request: &AccessRequest,
    claimed: &str,
    by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE access_requests SET state = $2, decided_by = $3, decided_at = $4, \
           decision_reason = $5 \
         WHERE id = $1 AND state = $6 AND decided_by = $7 AND change IS NOT DISTINCT FROM $8",
    )
    .bind(&request.id)
    .bind(&request.state)
    .bind(&request.decided_by)
    .bind(request.decided_at)
    .bind(&request.decision_reason)
    .bind(claimed)
    .bind(by)
    .bind(&request.change)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records the Change that grants or revokes a decided request.
pub(crate) async fn record_change(
    pool: &PgPool,
    id: &str,
    change: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE access_requests SET change = $2 WHERE id = $1")
        .bind(id)
        .bind(change)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AccessRequest {
        AccessRequest {
            id: "ar-0a1b2c3d4e5f".into(),
            project: "city-kpis".into(),
            requester: Requester::Pipeline("air-index".into()),
            endpoint_project: "helsinki".into(),
            endpoint: "air-quality".into(),
            space: "air".into(),
            operations: Operations::Read,
            types: vec![TypeGrant {
                entity_type: "AirQualityObserved".into(),
                attrs: vec!["pm10".into(), "pm25".into()],
            }],
            until: Some(time::macros::datetime!(2027-01-31 23:59:59 UTC)),
            reason: "air index for the city dashboard".into(),
            state: "pending".into(),
            requested_by: "jana".into(),
            requested_at: time::macros::datetime!(2026-10-20 19:40 UTC),
            decided_by: None,
            decided_at: None,
            decision_reason: None,
            change: None,
        }
    }

    fn endpoint(audience: &str, allowed: &[&str]) -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Endpoint",
            "metadata": { "name": "air-quality", "namespace": "helsinki" },
            "spec": { "contextSpaceRef": "air", "audience": audience, "allowedProjects": allowed, "slug": "s" },
            "status": { "ready": true },
        })
    }

    /// PL-90: a request names what it asks for; no wildcard, no empty list, no duplicates.
    #[test]
    fn a_request_names_every_type_and_attribute_it_asks_for() {
        let grant = |t: &str, attrs: &[&str]| TypeGrant {
            entity_type: t.into(),
            attrs: attrs.iter().map(|a| (*a).to_owned()).collect(),
        };
        assert!(
            checked_types(&[grant("AirQualityObserved", &["pm10", " no2 "])])
                .is_ok_and(|t| t[0].attrs == ["pm10", "no2"])
        );
        for refused in [
            vec![],
            vec![grant("AirQualityObserved", &[])],
            vec![grant("AirQualityObserved", &["*"])],
            vec![grant(
                "AirQualityObserved",
                &["https://uri.etsi.org/ngsi-ld/x"],
            )],
            vec![grant("*", &["pm10"])],
            vec![grant("airQuality", &["pm10"])],
            vec![grant("AirQualityObserved", &["pm10", "pm10"])],
            vec![
                grant("AirQualityObserved", &["pm10"]),
                grant("AirQualityObserved", &["no2"]),
            ],
        ] {
            assert!(checked_types(&refused).is_err(), "accepted {refused:?}");
        }
        let many: Vec<TypeGrant> = (0..21)
            .map(|i| grant(&format!("Type{i}"), &["a"]))
            .collect();
        assert!(checked_types(&many).is_err());
    }

    #[test]
    fn an_end_date_is_in_the_future_or_absent() {
        let now = time::macros::datetime!(2026-10-20 20:00 UTC);
        assert_eq!(checked_until(None, now), Ok(None));
        assert_eq!(checked_until(Some(" "), now), Ok(None));
        assert!(checked_until(Some("2027-01-31T23:59:59Z"), now).is_ok_and(|u| u.is_some()));
        assert!(checked_until(Some("2026-10-20T19:00:00Z"), now).is_err());
        assert!(checked_until(Some("tomorrow"), now).is_err());
        assert!(reason("  ").is_err());
        assert!(reason(&"x".repeat(501)).is_err());
    }

    /// PL-91: approval admits the project on a listed Endpoint and grants exactly what was asked
    /// to the pipeline's qualified account, until the request's end.
    #[test]
    fn the_approval_grants_exactly_the_requested_types_to_the_pipelines_own_account() {
        let bundle = grant_bundle(&request(), &endpoint("project-list", &["traffic"]));
        assert_eq!(bundle.len(), 2);
        assert_eq!(
            bundle[0]["spec"]["allowedProjects"],
            json!(["traffic", "city-kpis"])
        );
        assert!(
            bundle[0].get("status").is_none(),
            "the computed status is not written back"
        );
        let policy = &bundle[1];
        assert_eq!(
            policy["metadata"]["name"],
            "air-quality-city-kpis-air-index"
        );
        assert_eq!(policy["metadata"]["namespace"], "helsinki");
        assert_eq!(
            policy["spec"]["assignee"],
            json!({ "kind": "serviceAccount", "id": "city-kpis/pl-air-index" })
        );
        assert_eq!(policy["spec"]["operations"], json!(["retrieveOps"]));
        assert_eq!(
            policy["spec"]["information"],
            json!([{ "entities": [{ "type": "AirQualityObserved" }], "propertyNames": ["pm10", "pm25"] }])
        );
        assert_eq!(policy["spec"]["validity"]["to"], "2027-01-31T23:59:59Z");
        assert_eq!(policy["spec"]["contextSpaceRef"]["name"], "air");

        // An organization-wide Endpoint keeps its audience: only the Policy is proposed.
        let open = grant_bundle(&request(), &endpoint("organization", &[]));
        assert_eq!(open.len(), 1);
        assert_eq!(open[0]["kind"], "Policy");

        let mut write = request();
        write.operations = Operations::Write;
        write.until = None;
        let policy = &grant_bundle(&write, &endpoint("project", &[]))[1];
        assert_eq!(
            policy["spec"]["operations"],
            json!(["upsertBatch", "createBatch", "queryBatch"])
        );
        assert!(policy["spec"].get("validity").is_none());
    }

    /// PL-93 (T-4882): the run log names who revoked the grant and when, or when it ended; a
    /// grant on the same Endpoint still in force refuses nothing.
    #[test]
    fn a_refusal_names_the_revoking_owner_or_the_end() {
        let now = OffsetDateTime::parse(
            "2026-10-22T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .expect("now");
        let mut revoked = request();
        revoked.state = "revoked".into();
        revoked.decided_by = Some("anna@helsinki.example".into());
        revoked.decided_at = Some(now - time::Duration::days(1));
        assert_eq!(
            refusal_reason(std::slice::from_ref(&revoked), now).as_deref(),
            Some("access revoked by anna@helsinki.example on 2026-10-21")
        );
        let mut ended = request();
        ended.state = "approved".into();
        ended.until = Some(now - time::Duration::days(3));
        let reason = refusal_reason(&[revoked.clone(), ended.clone()], now);
        assert_eq!(
            reason.as_deref(),
            Some("access revoked by anna@helsinki.example on 2026-10-21")
        );
        assert!(is_refusal(reason.as_deref().unwrap_or_default()));
        assert_eq!(
            refusal_reason(std::slice::from_ref(&ended), now).as_deref(),
            Some("access ended on 2026-10-19")
        );
        let mut renewed = request();
        renewed.state = "approved".into();
        assert_eq!(
            refusal_reason(&[revoked, ended, renewed.clone()], now),
            None
        );
        assert_eq!(refusal_reason(&[renewed], now), None);
        assert!(!is_refusal(
            "HTTP request returned unexpected response code (403)"
        ));
    }

    /// PL-93: revoking ends the Policy now and takes the project off the list unless another
    /// live grant of the Endpoint still names it.
    #[test]
    fn revoking_ends_the_grant_and_unlists_the_project_when_nothing_else_names_it() {
        let now = time::macros::datetime!(2026-11-01 08:00 UTC);
        let request = request();
        let ours = grant_bundle(&request, &endpoint("project-list", &["city-kpis"]))[1].clone();
        let bundle = revoke_bundle(
            &request,
            &endpoint("project-list", &["city-kpis", "traffic"]),
            std::slice::from_ref(&ours),
            now,
        );
        assert_eq!(bundle.len(), 2);
        assert_eq!(bundle[0]["spec"]["allowedProjects"], json!(["traffic"]));
        assert_eq!(bundle[1]["spec"]["validity"]["to"], "2026-11-01T08:00:00Z");

        let share = json!({ "metadata": { "name": "air-quality-city-kpis" }, "spec": {} });
        let kept = revoke_bundle(
            &request,
            &endpoint("project-list", &["city-kpis"]),
            &[ours, share],
            now,
        );
        assert_eq!(kept.len(), 1, "the project's share still needs the listing");
        assert_eq!(kept[0]["kind"], "Policy");
    }

    #[test]
    fn a_grant_already_held_makes_the_request_pointless() {
        let request = request();
        let policy = grant_bundle(&request, &endpoint("organization", &[]))[0].clone();
        let mut open = policy.clone();
        open["spec"].as_object_mut().map(|s| s.remove("validity"));
        assert!(already_granted(
            &[open],
            &request.account(),
            &request.types,
            Operations::Read
        ));
        assert!(
            !already_granted(
                std::slice::from_ref(&policy),
                &request.account(),
                &request.types,
                Operations::Read
            ),
            "an ending grant is renewed by a request"
        );
        assert!(!already_granted(
            &[],
            &request.account(),
            &request.types,
            Operations::Read
        ));
    }

    #[test]
    fn a_request_reads_as_ended_once_its_end_passed_and_ids_are_checked() {
        let mut approved = request();
        approved.state = "approved".into();
        assert_eq!(
            approved.shown_state(time::macros::datetime!(2026-12-01 0:00 UTC)),
            "approved"
        );
        assert_eq!(
            approved.shown_state(time::macros::datetime!(2027-02-01 0:00 UTC)),
            "ended"
        );
        assert!(is_id(&new_id()));
        assert!(!is_id("ar-0A1B2C3D4E5F") && !is_id("ar-1") && !is_id("er-0a1b2c3d4e5f"));
        let view = approved.view(time::macros::datetime!(2026-12-01 0:00 UTC));
        assert_eq!(
            view["endpoint"],
            json!({ "project": "helsinki", "name": "air-quality" })
        );
        assert_eq!(view["account"], "city-kpis/pl-air-index");
    }

    fn person_request() -> AccessRequest {
        AccessRequest {
            project: "helsinki".into(),
            requester: Requester::Person("2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e".into()),
            types: vec![TypeGrant {
                entity_type: "AirQualityObserved".into(),
                attrs: Vec::new(),
            }],
            ..request()
        }
    }

    /// PL-115 (T-5615): approving a person's request proposes one Policy for that person alone,
    /// on the asked-for types until the end, and leaves the Endpoint's audience as it is.
    #[test]
    fn a_persons_approval_proposes_one_policy_for_that_person() {
        let request = person_request();
        let bundle = grant_bundle(&request, &endpoint("project-list", &["traffic"]));
        assert_eq!(bundle.len(), 1, "{bundle:?}");
        let policy = &bundle[0];
        assert_eq!(policy["kind"], "Policy");
        assert_eq!(policy["metadata"]["name"], "air-quality-ar-0a1b2c3d4e5f");
        assert_eq!(policy["metadata"]["namespace"], "helsinki");
        assert_eq!(
            policy["spec"]["assignee"],
            json!({ "kind": "user", "id": "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" })
        );
        assert_eq!(policy["spec"]["operations"], json!(["retrieveOps"]));
        assert_eq!(
            policy["spec"]["information"],
            json!([{ "entities": [{ "type": "AirQualityObserved" }] }])
        );
        assert_eq!(policy["spec"]["validity"]["to"], "2027-01-31T23:59:59Z");
        assert_eq!(request.account(), "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e");
        assert_eq!(request.view(request.requested_at)["pipeline"], Value::Null);
        assert_eq!(
            request.view(request.requested_at)["person"],
            "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e"
        );
    }

    /// Revoking a person's grant ends the Policy and never takes a project off the Endpoint.
    #[test]
    fn a_persons_revoke_ends_only_the_policy() {
        let request = person_request();
        let policy = grant_bundle(&request, &endpoint("project-list", &["helsinki"]))[0].clone();
        let now = time::macros::datetime!(2026-11-01 10:00 UTC);
        let ended = revoke_bundle(
            &request,
            &endpoint("project-list", &["helsinki"]),
            &[policy],
            now,
        );
        assert_eq!(ended.len(), 1, "{ended:?}");
        assert_eq!(ended[0]["kind"], "Policy");
        assert_eq!(ended[0]["spec"]["validity"]["to"], "2026-11-01T10:00:00Z");
    }

    /// PL-115: a person asks only for types the Endpoint serves, each once; each refusal names its field.
    #[test]
    fn a_person_names_only_served_types() {
        let served = ["AirQualityObserved".to_owned(), "Device".to_owned()];
        let asked = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            checked_person_types(&asked(&[" Device "]), &served),
            Ok(vec![TypeGrant {
                entity_type: "Device".into(),
                attrs: Vec::new()
            }])
        );
        assert_eq!(
            checked_person_types(&asked(&["Device", "Vehicle"]), &served),
            Err(vec![
                "types[1]: 'Vehicle' is no type this endpoint serves".to_owned()
            ])
        );
        assert!(checked_person_types(&asked(&["Device", "Device"]), &served).is_err());
        assert!(checked_person_types(&[], &served).is_err());
        assert!(checked_person_types(&asked(&["Device"]), &[]).is_err());
    }
}
