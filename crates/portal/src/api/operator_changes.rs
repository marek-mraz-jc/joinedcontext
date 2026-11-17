//! Operator Changes (evidence/T-7129/design.md §4, T-7139): what an installation operator does
//! without the cluster, asked by one operator and applied only once a second one approves.
//!
//! - `POST /operator/changes` asks for one: `HostAdd` (a host of an Organization, into the host
//!   registry as `approved`), `HostRemove` (the host's row turns `removed`), `OperatorAdd` and
//!   `OperatorRemove` (a person into or out of the operators' group, `JC_PORTAL_BOOTSTRAP_ADMINS`,
//!   in the installation realm). The rules the write needs are checked then and again at approval.
//! - `POST /operator/changes/{id}/approve` applies it, in one transaction with its audit row:
//!   only an operator other than the proposer, whatever the kind (`403 self-approval`), whose
//!   membership the current session carries. A `HostAdd` under a verified domain asks DNS for
//!   the domain's TXT record again, so a lapsed record refuses the approval.
//! - `POST /operator/changes/{id}/reject` closes it; any operator may.
//! - `GET /operator/changes` lists them, newest first.
//!
//! Every door is for a person of the operators' group with a second factor in the session,
//! never a service account, and the second factor is asked even where
//! `JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR=false` turns it off elsewhere. Each proposal, approval
//! and rejection writes a row of the append-only `operator_audit` (migration 0062) and the same
//! row as a log line. With one operator a Change waits: there is no bypass here; jc.toml's seed
//! is the break-glass, and it needs the cluster.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use jc_router::registry::Rules;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::session::{CurrentUser, Identity};
use crate::domain_verification::{record, txt_name, TxtRecords, Unanswered};
use crate::error::{ApiError, ProblemDetails};
use crate::host_registry::{self, Proposal};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// The longest one TXT lookup of an approval may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
/// The roles a host takes in the registry, as jc's router and seed name them.
pub const HOST_ROLES: &[&str] = &[
    "working",
    "opendata",
    "api",
    "signin",
    "apps",
    "portal-site",
    "host",
];

/// What an operator Change does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum OperatorKind {
    /// A host of an Organization enters the host registry (approved, then served once its DNS
    /// and certificate are in place).
    HostAdd,
    /// A host leaves: its row turns `removed`, which jc and the edge stop serving.
    HostRemove,
    /// A person joins the operators' group.
    OperatorAdd,
    /// A person leaves the operators' group.
    OperatorRemove,
}

impl OperatorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::HostAdd => "HostAdd",
            Self::HostRemove => "HostRemove",
            Self::OperatorAdd => "OperatorAdd",
            Self::OperatorRemove => "OperatorRemove",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::HostAdd,
            Self::HostRemove,
            Self::OperatorAdd,
            Self::OperatorRemove,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// An operator Change as it is asked for. Each kind takes its own fields and no other.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(example = json!({ "kind": "HostAdd", "host": "smartcity.bbsk.sk", "organization": "bbsk", "organizationId": "sk_bbsk", "role": "working" }))]
pub struct ChangeRequest {
    pub kind: OperatorKind,
    /// `HostAdd`, `HostRemove`: the host, lowercase, without a port, e.g. `smartcity.bbsk.sk`.
    pub host: Option<String>,
    /// `HostAdd`: the Organization's name, e.g. `bbsk`.
    pub organization: Option<String>,
    /// `HostAdd`: the minted Organization id the host is proved by, e.g. `sk_bbsk`; needed for a
    /// host outside the installation's domain.
    pub organization_id: Option<String>,
    /// `HostAdd`: what answers on the host: `working`, `opendata`, `api`, `signin`, `apps`,
    /// `portal-site` or `host`.
    pub role: Option<String>,
    /// `OperatorAdd`, `OperatorRemove`: the person's user id in the installation realm, e.g.
    /// `5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23`.
    pub subject: Option<String>,
}

/// The rows a Change writes, as it was asked for and checked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// One operator Change.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorChange {
    pub id: String,
    pub kind: OperatorKind,
    pub payload: ChangePayload,
    /// `proposed`, `applied` or `rejected`.
    pub state: String,
    /// The user id of the operator who asked for it.
    pub proposed_by: String,
    #[schema(value_type = String, format = DateTime)]
    pub proposed_at: DateTime<Utc>,
    /// The name of the operator who asked for it, when the realm answers it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_by_name: Option<String>,
    /// The operator who approved or rejected it.
    pub decided_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by_name: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub decided_at: Option<DateTime<Utc>>,
}

/// The operator Changes, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct OperatorChangeList {
    pub items: Vec<OperatorChange>,
}

/// The caller, when it is a person of the operators' group with a second factor in this
/// session; `403` for anybody else, a service account of that group included.
pub(crate) fn operator(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    if !crate::permissions::in_group(identity, &state.config.bootstrap_admins)
        || crate::permissions::is_service_account(&state.mirror, identity)
        || identity.client.is_some()
    {
        return Err(ApiError::Denied(
            "operator Changes are for a person of the installation's operators' group (T-7129 §4)"
                .into(),
        ));
    }
    // Always: the installation-wide switch that turns the second factor off elsewhere does not
    // reach the operators' door.
    if !identity.second_factor {
        return Err(ApiError::SecondFactorRequired);
    }
    Ok(())
}

fn database(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "operator Changes are kept in the database, and this Portal has none".into(),
        )
    })
}

fn rules(state: &AppState) -> Result<Rules<'_>, ApiError> {
    let (base_domain, login_host) = state.config.installation_hosts.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "this Portal does not know the installation's domain and sign-in host \
             (JC_PORTAL_INSTALLATION_DOMAIN, JC_PORTAL_LOGIN_HOST), which jc sets when it runs \
             the Portal; no host Change is taken"
                .into(),
        )
    })?;
    Ok(Rules {
        base_domain,
        login_host,
        reserved: jc_core::names::RESERVED_LABELS,
    })
}

fn db(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "operator changes");
    ApiError::Unavailable("operator Changes could not be read or written; try again".into())
}

fn invalid(field: &str, message: String) -> ApiError {
    ApiError::Unprocessable {
        detail: format!("the Change cannot be taken: {message}"),
        errors: vec![format!("{field}: {message}")],
    }
}

fn required(field: &str, value: Option<String>, example: &str) -> Result<String, ApiError> {
    value
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid(field, format!("{field} is required, e.g. {example}")))
}

fn absent(kind: OperatorKind, fields: &[(&str, &Option<String>)]) -> Result<(), ApiError> {
    match fields.iter().find(|(_, v)| v.is_some()) {
        Some((field, _)) => Err(invalid(
            field,
            format!("{} takes no {field}", kind.as_str()),
        )),
        None => Ok(()),
    }
}

/// The request checked field by field: the payload and the Change's target (its host or person).
fn payload(request: ChangeRequest) -> Result<(OperatorKind, ChangePayload, String), ApiError> {
    let kind = request.kind;
    match kind {
        OperatorKind::HostAdd => {
            absent(kind, &[("subject", &request.subject)])?;
            let host = required("host", request.host, "smartcity.bbsk.sk")?.to_ascii_lowercase();
            let organization = required("organization", request.organization, "bbsk")?;
            let role = required("role", request.role, "working")?;
            if !HOST_ROLES.contains(&role.as_str()) {
                return Err(invalid(
                    "role",
                    format!(
                        "'{role}' is no host role; give one of {}",
                        HOST_ROLES.join(", ")
                    ),
                ));
            }
            let organization_id = request
                .organization_id
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty());
            Ok((
                kind,
                ChangePayload {
                    host: Some(host.clone()),
                    organization: Some(organization),
                    organization_id,
                    role: Some(role),
                    subject: None,
                },
                host,
            ))
        }
        OperatorKind::HostRemove => {
            absent(
                kind,
                &[
                    ("organization", &request.organization),
                    ("organizationId", &request.organization_id),
                    ("role", &request.role),
                    ("subject", &request.subject),
                ],
            )?;
            let host = required("host", request.host, "smartcity.bbsk.sk")?.to_ascii_lowercase();
            Ok((
                kind,
                ChangePayload {
                    host: Some(host.clone()),
                    ..ChangePayload::default()
                },
                host,
            ))
        }
        OperatorKind::OperatorAdd | OperatorKind::OperatorRemove => {
            absent(
                kind,
                &[
                    ("host", &request.host),
                    ("organization", &request.organization),
                    ("organizationId", &request.organization_id),
                    ("role", &request.role),
                ],
            )?;
            let subject = required(
                "subject",
                request.subject,
                "5f1c2b9e-0d3a-4c7e-9a41-2b6f8e0c1d23",
            )?
            .to_ascii_lowercase();
            if !crate::permissions::is_person_id(&subject) {
                return Err(invalid(
                    "subject",
                    format!("'{subject}' is no user id of the installation realm"),
                ));
            }
            Ok((
                kind,
                ChangePayload {
                    subject: Some(subject.clone()),
                    ..ChangePayload::default()
                },
                subject,
            ))
        }
    }
}

fn proposal<'a>(payload: &'a ChangePayload, by: &'a str) -> Proposal<'a> {
    Proposal {
        host: payload.host.as_deref().unwrap_or_default(),
        organization: payload.organization.as_deref().unwrap_or_default(),
        organization_id: payload.organization_id.as_deref(),
        role: payload.role.as_deref().unwrap_or_default(),
        by,
    }
}

/// What the kind needs to hold when it is asked for and again when it is approved, before
/// anything is written.
async fn holds(
    state: &AppState,
    pool: &sqlx::PgPool,
    kind: OperatorKind,
    payload: &ChangePayload,
    by: &str,
) -> Result<(), ApiError> {
    match kind {
        OperatorKind::HostAdd => {
            let organization = payload.organization.as_deref().unwrap_or_default();
            if state
                .mirror
                .get(ORG_NAMESPACE, "Organization", organization)
                .is_none()
            {
                return Err(invalid(
                    "organization",
                    format!("the installation has no Organization '{organization}'"),
                ));
            }
            let rules = rules(state)?;
            host_registry::check_proposal(pool, &state.mirror, &rules, &proposal(payload, by)).await
        }
        OperatorKind::HostRemove => {
            let host = payload.host.as_deref().unwrap_or_default();
            let served = host_registry::rows(pool)
                .await
                .map_err(db)?
                .into_iter()
                .any(|row| row.host == host && row.state != "removed");
            if served {
                Ok(())
            } else {
                Err(invalid(
                    "host",
                    format!("'{host}' is no host of the registry, or it is removed already"),
                ))
            }
        }
        OperatorKind::OperatorAdd | OperatorKind::OperatorRemove => {
            let subject = payload.subject.as_deref().unwrap_or_default();
            let found = people(state)?
                .admin()
                .await?
                .get(subject)
                .await
                .map_err(|e| match e {
                    crate::people::PeopleError::NotFound => invalid(
                        "subject",
                        format!("the installation realm has no person '{subject}'"),
                    ),
                    other => other.into(),
                })?;
            if kind == OperatorKind::OperatorAdd && found.is_service_account() {
                return Err(invalid(
                    "subject",
                    format!(
                        "'{subject}' is a client's service account; an operator is a person who signs in"
                    ),
                ));
            }
            Ok(())
        }
    }
}

/// The names behind the user ids an operator read shows, at most `LAST_SEEN_AT_ONCE` realm reads
/// in flight. Best effort: a read still answers with the ids when the realm does not, and an id it
/// does not know (or `reconciler`) keeps no name.
async fn names_of(
    state: &AppState,
    ids: std::collections::BTreeSet<String>,
) -> std::collections::BTreeMap<String, String> {
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| crate::permissions::is_person_id(id))
        .collect();
    if ids.is_empty() {
        return Default::default();
    }
    let Ok(people) = people(state) else {
        return Default::default();
    };
    let Ok(admin) = people.admin().await else {
        return Default::default();
    };
    let lookups = ids.into_iter().map(|id| {
        let admin = &admin;
        async move {
            let found = admin.get(&id).await;
            (id, found)
        }
    });
    let answers: Vec<_> = futures_util::StreamExt::collect(futures_util::StreamExt::buffered(
        futures_util::stream::iter(lookups),
        super::people::LAST_SEEN_AT_ONCE,
    ))
    .await;
    answers
        .into_iter()
        .filter_map(|(id, found)| Some((id, super::people_ids::name_of(&found.ok()?))))
        .collect()
}

fn people(state: &AppState) -> Result<&crate::people::People, ApiError> {
    state.people.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "no Keycloak admin client is configured, so the operators' group cannot be changed here"
                .into(),
        )
    })
}

/// The fewest operators the installation keeps: a Change needs a second operator to approve it,
/// so with one left nothing could be approved again without the cluster.
const FEWEST_OPERATORS: usize = 2;

/// Operator-group Changes of every Portal replica apply one at a time (a transaction-scoped
/// advisory lock), so two removals approved at once cannot both pass the count below.
const OPERATOR_LOCK: i64 = 0x6a63_6f70_7273; // "jcoprs"

/// The proposer is still in the operators' group (or holds its realm role) when a second
/// operator approves: one who left takes their open Changes' four eyes with them.
async fn proposer_holds(
    admin: &crate::people::Admin,
    group: &str,
    proposer: &str,
) -> Result<(), ApiError> {
    if admin
        .groups_and_roles(proposer)
        .await?
        .iter()
        .any(|name| name == group)
    {
        Ok(())
    } else {
        Err(ApiError::Conflict(
            "the operator who asked for this Change is no operator any more; reject it and ask again"
                .into(),
        ))
    }
}

/// What an approval of an operator-group Change needs from the realm, read under
/// [`OPERATOR_LOCK`]: the proposer is still an operator, and a removal leaves
/// [`FEWEST_OPERATORS`].
async fn operators_hold(
    tx: &mut sqlx::PgConnection,
    admin: &crate::people::Admin,
    group: &str,
    found: &OperatorChange,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(OPERATOR_LOCK)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    proposer_holds(admin, group, &found.proposed_by).await?;
    if found.kind == OperatorKind::OperatorRemove {
        let subject = found.payload.subject.as_deref().unwrap_or_default();
        let left = admin
            .group_members(group)
            .await?
            .unwrap_or_default()
            .iter()
            .filter(|m| m.enabled && !m.is_service_account() && m.id != subject)
            .count();
        if left < FEWEST_OPERATORS {
            return Err(ApiError::Conflict(format!(
                "subject: removing '{subject}' would leave {left} operator(s); the installation keeps at least {FEWEST_OPERATORS}, so add another operator first"
            )));
        }
    }
    Ok(())
}

/// A Change as it is read: times in epoch seconds (sqlx here speaks `time`, the answer chrono).
type Row = (
    String,
    String,
    Value,
    String,
    String,
    i64,
    Option<String>,
    Option<i64>,
);

const COLUMNS: &str = "id::text, kind, payload, state, proposed_by, \
     extract(epoch FROM proposed_at)::bigint, decided_by, extract(epoch FROM decided_at)::bigint";

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap_or_else(Utc::now)
}

fn change(row: Row) -> Result<OperatorChange, ApiError> {
    let (id, kind, payload, state, proposed_by, proposed_at, decided_by, decided_at) = row;
    let kind = OperatorKind::parse(&kind)
        .ok_or_else(|| ApiError::Internal(format!("operator change {id}: unknown kind {kind}")))?;
    let payload = serde_json::from_value(payload)
        .map_err(|e| ApiError::Internal(format!("operator change {id}: {e}")))?;
    Ok(OperatorChange {
        id,
        kind,
        payload,
        state,
        proposed_by,
        proposed_by_name: None,
        proposed_at: at(proposed_at),
        decided_by,
        decided_by_name: None,
        decided_at: decided_at.map(at),
    })
}

/// One audit row and its log line.
async fn audit(
    conn: &mut sqlx::PgConnection,
    actor: &str,
    change: &str,
    action: &str,
    before: Option<&Value>,
    after: Option<&Value>,
    source: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO operator_audit (actor, change_id, action, before, after, source_ip) \
         VALUES ($1, $2::uuid, $3, $4, $5, $6)",
    )
    .bind(actor)
    .bind(change)
    .bind(action)
    .bind(before)
    .bind(after)
    .bind(source)
    .execute(conn)
    .await
    .map_err(db)?;
    tracing::info!(
        target: "operator_audit",
        actor,
        change = %change,
        action,
        before = %before.map(serde_json::Value::to_string).unwrap_or_default(),
        after = %after.map(serde_json::Value::to_string).unwrap_or_default(),
        source = source.unwrap_or_default(),
        "operator audit"
    );
    Ok(())
}

fn source(client: Option<Extension<jc_router::ClientAddr>>) -> Option<String> {
    client.map(|Extension(addr)| addr.0.to_string())
}

/// A Change id: a lowercase hyphenated UUID, `404` for anything else.
fn parse_id(id: &str) -> Result<String, ApiError> {
    let id = id.to_ascii_lowercase();
    if crate::permissions::is_person_id(&id) {
        Ok(id)
    } else {
        Err(ApiError::NotFound(format!(
            "operator Change '{id}' does not exist"
        )))
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/operator/changes",
    operation_id = "operator_change_list",
    summary = "List Operator Changes",
    description = "Every operator Change of the installation, newest first (T-7129 §4). For a person of the operators' group with a second factor.",
    tag = "system",
    responses(
        (status = 200, description = "The Changes", body = OperatorChangeList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn list(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OperatorChangeList>, ApiError> {
    operator(&state, &user.0.identity)?;
    let pool = database(&state)?;
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM operator_changes ORDER BY proposed_at DESC LIMIT 500"
    )))
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let mut items: Vec<OperatorChange> = rows.into_iter().map(change).collect::<Result<_, _>>()?;
    let ids = items
        .iter()
        .flat_map(|c| std::iter::once(&c.proposed_by).chain(c.decided_by.as_ref()))
        .cloned()
        .collect();
    let names = names_of(&state, ids).await;
    for item in &mut items {
        item.proposed_by_name = names.get(&item.proposed_by).cloned();
        item.decided_by_name = item
            .decided_by
            .as_ref()
            .and_then(|id| names.get(id).cloned());
    }
    Ok(Json(OperatorChangeList { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/operator/changes",
    operation_id = "operator_change_propose",
    summary = "Propose An Operator Change",
    description = "Asks for a host added to or removed from an Organization, or a person added to or removed from the operators' group (T-7129 §4). Nothing is written until a second operator approves. 409 when the host or person already has an open Change, or the host is taken.",
    tag = "system",
    request_body = ChangeRequest,
    responses(
        (status = 201, description = "The Change waits for a second operator", body = OperatorChange),
        (status = 400, description = "The body does not parse", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 409, description = "An open Change for the same host or person, or the host is taken", body = ProblemDetails),
        (status = 422, description = "A field breaks a rule; the error names it", body = ProblemDetails),
        (status = 503, description = "No database, the installation's hosts are unknown, or no realm admin client for an operator Change", body = ProblemDetails)
    )
)]
pub async fn propose(
    user: CurrentUser,
    State(state): State<AppState>,
    client: Option<Extension<jc_router::ClientAddr>>,
    body: Result<Json<ChangeRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<OperatorChange>), ApiError> {
    let identity = &user.0.identity;
    operator(&state, identity)?;
    let Json(body) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let (kind, payload, target) = payload(body)?;
    let pool = database(&state)?;
    holds(&state, pool, kind, &payload, &identity.subject).await?;
    let stored = serde_json::to_value(&payload)
        .map_err(|e| ApiError::Internal(format!("operator change: {e}")))?;
    let mut tx = pool.begin().await.map_err(db)?;
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO operator_changes (kind, target, payload, proposed_by) \
         VALUES ($1, $2, $3, $4) RETURNING {COLUMNS}"
    )))
    .bind(kind.as_str())
    .bind(&target)
    .bind(&stored)
    .bind(&identity.subject)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::Conflict(format!(
            "target: '{target}' has an open operator Change already; approve or reject that one first"
        )),
        _ => db(e),
    })?;
    let ip = source(client);
    audit(
        &mut tx,
        &identity.subject,
        &row.0,
        &format!("proposed {}", kind.as_str()),
        None,
        Some(&stored),
        ip.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((StatusCode::CREATED, Json(change(row)?)))
}

/// The TXT record of the minted id `id` still carries its challenge; `422` naming the record
/// otherwise.
async fn txt_holds(state: &AppState, pool: &sqlx::PgPool, id: &str) -> Result<(), ApiError> {
    let (domain, challenge): (String, String) = sqlx::query_as(
        "SELECT domain, challenge FROM organization_ids WHERE id = $1 AND minted_at IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db)?
    .ok_or_else(|| {
        invalid(
            "organizationId",
            format!("'{id}' is not a minted Organization id"),
        )
    })?;
    let records: Arc<dyn TxtRecords> = match &state.txt_records {
        Some(records) => Arc::clone(records),
        None => Arc::new(crate::domain_verification::NetLookup::new().map_err(|_| {
            ApiError::Unavailable("DNS cannot be asked from this Portal; try again".into())
        })?),
    };
    let name = txt_name(&domain);
    let wanted = format!("jc-verify={challenge}");
    let lapsed = || {
        invalid(
            "organizationId",
            format!(
                "the TXT record of {domain} no longer carries its challenge; publish {} again and approve once DNS answers it",
                record(&domain, &challenge)
            ),
        )
    };
    match tokio::time::timeout(LOOKUP_TIMEOUT, records.txt(&name)).await {
        Ok(Ok(found)) => {
            if found
                .iter()
                .any(|text| crate::auth::csrf::constant_time_eq(text.trim(), &wanted))
            {
                Ok(())
            } else {
                Err(lapsed())
            }
        }
        Ok(Err(Unanswered)) | Err(_) => Err(invalid(
            "organizationId",
            format!("the TXT lookup of {name} did not answer in time; try again in a minute"),
        )),
    }
}

/// The open Change `id`, locked for the rest of `tx`.
async fn open(tx: &mut sqlx::PgConnection, id: &str) -> Result<OperatorChange, ApiError> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM operator_changes WHERE id = $1::uuid FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let found =
        change(row.ok_or_else(|| {
            ApiError::NotFound(format!("operator Change '{id}' does not exist"))
        })?)?;
    if found.state != "proposed" {
        return Err(ApiError::Conflict(format!(
            "operator Change '{id}' is {} already",
            found.state
        )));
    }
    Ok(found)
}

async fn decide(
    tx: &mut sqlx::PgConnection,
    id: &str,
    state: &str,
    by: &str,
) -> Result<OperatorChange, ApiError> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE operator_changes SET state = $2, decided_by = $3, decided_at = now() \
         WHERE id = $1::uuid RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(state)
    .bind(by)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    change(row)
}

#[utoipa::path(
    post,
    path = "/api/v1/operator/changes/{id}/approve",
    operation_id = "operator_change_approve",
    summary = "Approve An Operator Change",
    description = "Applies the Change, checked again against the registry and DNS, in one transaction with its audit row (T-7129 §4). Only an operator other than the proposer approves, whatever the kind.",
    tag = "system",
    params(("id" = String, Path, description = "The Change's id")),
    responses(
        (status = 200, description = "Applied", body = OperatorChange),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, no second factor, or the proposer", body = ProblemDetails),
        (status = 404, description = "No such Change", body = ProblemDetails),
        (status = 409, description = "Decided already, the host was taken since, the proposer is no operator any more, or a removal would leave fewer than two operators", body = ProblemDetails),
        (status = 422, description = "A rule no longer holds, e.g. the TXT record lapsed; the error names the field", body = ProblemDetails),
        (status = 503, description = "No database or no realm admin client", body = ProblemDetails)
    )
)]
pub async fn approve(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    client: Option<Extension<jc_router::ClientAddr>>,
) -> Result<Json<OperatorChange>, ApiError> {
    let identity = &user.0.identity;
    operator(&state, identity)?;
    let id = parse_id(&id)?;
    let pool = database(&state)?;
    let mut tx = pool.begin().await.map_err(db)?;
    let found = open(&mut tx, &id).await?;
    if found.proposed_by == identity.subject {
        return Err(ApiError::SelfApproval(
            "the operator who asked for a Change never approves it; a second operator does (T-7129 §4)"
                .into(),
        ));
    }
    holds(&state, pool, found.kind, &found.payload, &found.proposed_by).await?;
    // A host Change re-reads its proposer where the realm can be asked; an operator Change
    // always does, under its lock (`operators_hold`).
    if let (Some(people), OperatorKind::HostAdd | OperatorKind::HostRemove) =
        (state.people.as_deref(), found.kind)
    {
        proposer_holds(
            &people.admin().await?,
            &state.config.bootstrap_admins,
            &found.proposed_by,
        )
        .await?;
    }
    let payload = &found.payload;
    let (before, after) = match found.kind {
        OperatorKind::HostAdd => {
            if let Some(org_id) = payload.organization_id.as_deref() {
                txt_holds(&state, pool, org_id).await?;
            }
            let rules = rules(&state)?;
            host_registry::add_approved(
                &mut tx,
                &state.mirror,
                &rules,
                &proposal(payload, &found.proposed_by),
                &identity.subject,
            )
            .await?;
            (
                None,
                Some(json!({ "host": payload.host, "state": "approved" })),
            )
        }
        OperatorKind::HostRemove => {
            let host = payload.host.as_deref().unwrap_or_default();
            if !host_registry::remove_in(&mut tx, host).await.map_err(db)? {
                return Err(ApiError::Conflict(format!(
                    "host: '{host}' was removed since the Change was asked for"
                )));
            }
            (
                Some(json!({ "host": host })),
                Some(json!({ "host": host, "state": "removed" })),
            )
        }
        OperatorKind::OperatorAdd | OperatorKind::OperatorRemove => {
            return apply_operator(&state, tx, &found, &identity.subject, source(client)).await;
        }
    };
    let decided = decide(&mut tx, &id, "applied", &identity.subject).await?;
    let ip = source(client);
    audit(
        &mut tx,
        &identity.subject,
        &id,
        &format!("applied {}", found.kind.as_str()),
        before.as_ref(),
        after.as_ref(),
        ip.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(decided))
}

/// Applies an approved `OperatorAdd` or `OperatorRemove`. The decision and its audit row are
/// written first, the realm is changed next, and the transaction commits only once the realm
/// answered: a refused realm call leaves the Change open and unaudited. A commit that fails after
/// the realm changed is undone in the realm.
async fn apply_operator(
    state: &AppState,
    mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
    found: &OperatorChange,
    by: &str,
    ip: Option<String>,
) -> Result<Json<OperatorChange>, ApiError> {
    let subject = found.payload.subject.as_deref().unwrap_or_default();
    let group = &state.config.bootstrap_admins;
    let admin = people(state)?.admin().await?;
    operators_hold(&mut tx, &admin, group, found).await?;
    let member = found.kind == OperatorKind::OperatorAdd;
    let decided = decide(&mut tx, &found.id, "applied", by).await?;
    audit(
        &mut tx,
        by,
        &found.id,
        &format!("applied {}", found.kind.as_str()),
        Some(&json!({ "subject": subject, "operator": !member })),
        Some(&json!({ "subject": subject, "operator": member })),
        ip.as_deref(),
    )
    .await?;
    let changed = if member {
        admin.join_group(subject, group).await?
    } else {
        admin.leave_group(subject, group).await?
    };
    if !changed {
        return Err(ApiError::Conflict(format!(
            "the installation realm has no group '{group}': the operators are a realm \
             role there, which only the realm's administrator changes"
        )));
    }
    if let Err(err) = tx.commit().await {
        let undone = if member {
            admin.leave_group(subject, group).await
        } else {
            admin.join_group(subject, group).await
        };
        tracing::error!(
            change = %found.id,
            undone = undone.is_ok(),
            "operator change: the commit failed after the realm changed: {err}"
        );
        return Err(db(err));
    }
    Ok(Json(decided))
}

#[utoipa::path(
    post,
    path = "/api/v1/operator/changes/{id}/reject",
    operation_id = "operator_change_reject",
    summary = "Reject An Operator Change",
    description = "Closes the Change without writing anything it asked for (T-7129 §4). Any operator may, the proposer included.",
    tag = "system",
    params(("id" = String, Path, description = "The Change's id")),
    responses(
        (status = 200, description = "Rejected", body = OperatorChange),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 404, description = "No such Change", body = ProblemDetails),
        (status = 409, description = "Decided already", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn reject(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    client: Option<Extension<jc_router::ClientAddr>>,
) -> Result<Json<OperatorChange>, ApiError> {
    let identity = &user.0.identity;
    operator(&state, identity)?;
    let id = parse_id(&id)?;
    let pool = database(&state)?;
    let mut tx = pool.begin().await.map_err(db)?;
    let found = open(&mut tx, &id).await?;
    let decided = decide(&mut tx, &id, "rejected", &identity.subject).await?;
    let ip = source(client);
    audit(
        &mut tx,
        &identity.subject,
        &id,
        &format!("rejected {}", found.kind.as_str()),
        None,
        None,
        ip.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(decided))
}

/// One host of the registry as the operators' pages show it (T-7129 §3, T-7140).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorHost {
    pub host: String,
    pub organization: String,
    pub organization_id: Option<String>,
    pub role: String,
    /// `proposed`, `approved`, `dns-ok`, `cert-issued`, `served` or `removed`.
    pub state: String,
    /// What the host waits for on its way to `served`, e.g. `waiting for DNS: ...` or
    /// `waiting for a certificate slot until 2026-11-18 09:00 UTC`; none once served.
    pub detail: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub detail_at: Option<DateTime<Utc>>,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
}

/// The new-certificate slots of the instance (T-7129 §3 pacing).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CertificateSlots {
    /// New certificates requested in the last 24 hours.
    pub used_today: i64,
    /// The most a rolling 24 hours allows.
    pub per_day: i64,
    /// The most a rolling 7 days allows per registered domain (eTLD+1).
    pub per_domain_week: i64,
    /// When the next slot frees, while every slot of the day is taken.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub next_free_at: Option<DateTime<Utc>>,
}

/// Every host of the registry and the certificate slots.
#[derive(Debug, Serialize, ToSchema)]
pub struct OperatorHostList {
    pub items: Vec<OperatorHost>,
    pub certificates: CertificateSlots,
}

type HostReadRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<i64>,
    i64,
);

#[utoipa::path(
    get,
    path = "/api/v1/operator/hosts",
    operation_id = "operator_host_list",
    summary = "List The Host Registry",
    description = "Every host of every Organization with its state on the way to served and what it waits for, and the instance's new-certificate slots (T-7129 §3). For a person of the operators' group with a second factor.",
    tag = "system",
    responses(
        (status = 200, description = "The hosts", body = OperatorHostList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn hosts(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OperatorHostList>, ApiError> {
    operator(&state, &user.0.identity)?;
    let pool = database(&state)?;
    let rows: Vec<HostReadRow> = sqlx::query_as(
        "SELECT host, organization, organization_id, role, state, detail, \
         extract(epoch FROM detail_at)::bigint, extract(epoch FROM updated_at)::bigint \
         FROM organization_hosts ORDER BY organization, host",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let items = rows
        .into_iter()
        .map(
            |(host, organization, organization_id, role, state, detail, detail_at, updated_at)| {
                OperatorHost {
                    host,
                    organization,
                    organization_id,
                    role,
                    state,
                    detail,
                    detail_at: detail_at.map(at),
                    updated_at: at(updated_at),
                }
            },
        )
        .collect();
    let day: Vec<i64> = sqlx::query_scalar(
        "SELECT extract(epoch FROM requested_at)::bigint FROM organization_host_certificates \
         WHERE requested_at > now() - interval '24 hours' ORDER BY requested_at",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let next_free_at = crate::reconciler::org_hosts::wait_until(
        &day.iter().copied().map(at).collect::<Vec<_>>(),
        &[],
    );
    Ok(Json(OperatorHostList {
        items,
        certificates: CertificateSlots {
            used_today: i64::try_from(day.len()).unwrap_or(i64::MAX),
            per_day: crate::reconciler::org_hosts::PER_INSTANCE_DAY as i64,
            per_domain_week: crate::reconciler::org_hosts::PER_DOMAIN_WEEK as i64,
            next_free_at,
        },
    }))
}

/// One row of the operators' audit.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorAuditEntry {
    pub id: i64,
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    /// The operator's user id, or `reconciler` for a step on a host's objects.
    pub actor: String,
    /// The operator's name, when the realm answers it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    pub change_id: Option<String>,
    /// e.g. `proposed HostAdd`, `certificate.created`, `host.served`.
    pub action: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub source_ip: Option<String>,
}

/// The newest rows of the operators' audit, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct OperatorAuditList {
    pub items: Vec<OperatorAuditEntry>,
}

type AuditRow = (
    i64,
    i64,
    String,
    Option<String>,
    String,
    Option<Value>,
    Option<Value>,
    Option<String>,
);

#[utoipa::path(
    get,
    path = "/api/v1/operator/audit",
    operation_id = "operator_audit_list",
    summary = "List The Operators' Audit",
    description = "The newest 500 rows of the operators' append-only audit: every proposal, approval, rejection and applied write, and every reconciler step on a registry host (T-7129 §4). For a person of the operators' group with a second factor.",
    tag = "system",
    responses(
        (status = 200, description = "The audit rows", body = OperatorAuditList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn audit_list(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OperatorAuditList>, ApiError> {
    operator(&state, &user.0.identity)?;
    let pool = database(&state)?;
    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT id, extract(epoch FROM at)::bigint, actor, change_id::text, action, before, \
         after, source_ip FROM operator_audit ORDER BY id DESC LIMIT 500",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let names = names_of(&state, rows.iter().map(|row| row.2.clone()).collect()).await;
    let items = rows
        .into_iter()
        .map(
            |(id, when, actor, change_id, action, before, after, source_ip)| OperatorAuditEntry {
                id,
                at: at(when),
                actor_name: names.get(&actor).cloned(),
                actor,
                change_id,
                action,
                before,
                after,
                source_ip,
            },
        )
        .collect();
    Ok(Json(OperatorAuditList { items }))
}

/// One Organization id, minted or waiting for its TXT record (T-7012, T-7140).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorOrganizationId {
    /// e.g. `sk_bbsk`.
    pub id: String,
    /// e.g. `bbsk.sk`.
    pub domain: String,
    /// The TXT record's name, `_joinedcontext.{domain}`.
    pub record_name: String,
    /// The TXT record's value, `jc-verify={challenge}`: public, it is published in DNS.
    pub record_value: String,
    #[schema(value_type = String, format = DateTime)]
    pub requested_at: DateTime<Utc>,
    /// When the id was minted; none while the record is not checked yet.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub minted_at: Option<DateTime<Utc>>,
    /// The Organizations whose `spec.domain` is this domain.
    pub organizations: Vec<String>,
}

/// One Organization of the installation as the operators' list shows it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorOrganization {
    pub name: String,
    pub title: Option<String>,
    pub domain: Option<String>,
    /// Its hosts per registry state, e.g. `{"served": 3, "dns-ok": 1}`.
    pub hosts: std::collections::BTreeMap<String, i64>,
}

/// The Organizations and the Organization ids.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperatorOrganizationList {
    pub items: Vec<OperatorOrganization>,
    pub ids: Vec<OperatorOrganizationId>,
}

#[utoipa::path(
    get,
    path = "/api/v1/operator/organizations",
    operation_id = "operator_organization_list",
    summary = "List Organizations For Operators",
    description = "Every Organization of the installation with its domain and its hosts per state, and every Organization id with the TXT record that proves its domain (T-7129, T-7012). For a person of the operators' group with a second factor.",
    tag = "system",
    responses(
        (status = 200, description = "The Organizations and ids", body = OperatorOrganizationList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an operator, or no second factor", body = ProblemDetails),
        (status = 503, description = "No database", body = ProblemDetails)
    )
)]
pub async fn organizations(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OperatorOrganizationList>, ApiError> {
    operator(&state, &user.0.identity)?;
    let pool = database(&state)?;
    let counts: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT organization, state, count(*) FROM organization_hosts GROUP BY organization, state",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let manifests = state.mirror.matching(|env| {
        env.kind == "Organization" && env.metadata.namespace.as_deref() == Some(ORG_NAMESPACE)
    });
    let domain_of = |env: &crate::resource::ResourceEnvelope| {
        env.spec
            .get("domain")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut items: Vec<OperatorOrganization> = manifests
        .iter()
        .map(|env| OperatorOrganization {
            name: env.metadata.name.clone(),
            title: env
                .spec
                .get("title")
                .or_else(|| env.spec.get("displayName"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            domain: domain_of(env),
            hosts: counts
                .iter()
                .filter(|(org, _, _)| *org == env.metadata.name)
                .map(|(_, host_state, n)| (host_state.clone(), *n))
                .collect(),
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    let rows: Vec<(String, String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, domain, challenge, extract(epoch FROM requested_at)::bigint, \
         extract(epoch FROM minted_at)::bigint FROM organization_ids ORDER BY domain",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let ids = rows
        .into_iter()
        .map(
            |(id, domain, challenge, requested_at, minted_at)| OperatorOrganizationId {
                record_name: txt_name(&domain),
                record_value: format!("jc-verify={challenge}"),
                organizations: manifests
                    .iter()
                    .filter(|env| domain_of(env).as_deref() == Some(domain.as_str()))
                    .map(|env| env.metadata.name.clone())
                    .collect(),
                id,
                domain,
                requested_at: at(requested_at),
                minted_at: minted_at.map(at),
            },
        )
        .collect();
    Ok(Json(OperatorOrganizationList { items, ids }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/operator/changes", get(list).post(propose))
        .route("/operator/changes/{id}/approve", post(approve))
        .route("/operator/changes/{id}/reject", post(reject))
        .route("/operator/hosts", get(hosts))
        .route("/operator/audit", get(audit_list))
        .route("/operator/organizations", get(organizations))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-7129 §7 (critic row 10): a token in the operators' group, a service account's or an
    /// agent's, is no operator; neither is a person without a second factor.
    #[test]
    fn a_bearer_in_the_operators_group_is_no_operator() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let person = Identity {
            subject: "sub-ops".into(),
            username: "ops".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec![state.config.bootstrap_admins.clone()],
            client: None,
            second_factor: true,
        };
        assert!(operator(&state, &person).is_ok());
        let bearer = Identity {
            client: Some("ops-robot".into()),
            ..person.clone()
        };
        assert!(matches!(
            operator(&state, &bearer),
            Err(ApiError::Denied(_))
        ));
        let password_only = Identity {
            second_factor: false,
            ..person
        };
        assert!(matches!(
            operator(&state, &password_only),
            Err(ApiError::SecondFactorRequired)
        ));
    }
}
