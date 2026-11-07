//! `GET /api/v1/projects/{project}/mqtt/allowance`: what the project may run as MQTT edge brokers
//! and what its committed brokers use of it (API/01 §47, PF-117…PF-119, ADR-N-048 §2.9).
//!
//! There is no write route: the allowance is a field of the Organization or the Project manifest,
//! written through a Change only an org admin may propose (`crate::mqtt_allowance::check_project`).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use super::organization_limits::LimitOrigin;
use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// One summed limit: what the allowance gives, what the brokers use, and what is left.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct Meter {
    pub limit: u64,
    pub used: u64,
    /// Never below `0`; a project over a lowered allowance shows `used` above `limit` (PF-100).
    pub left: u64,
}

impl Meter {
    fn new(limit: u32, used: u64) -> Self {
        let limit = u64::from(limit);
        Self {
            limit,
            used,
            left: limit.saturating_sub(used),
        }
    }
}

/// A ceiling each broker is held to on its own.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct Ceiling {
    pub limit: u32,
}

/// A dedicated port of the project and the broker holding it.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct PortInUse {
    pub port: u16,
    pub broker: String,
}

/// The project's MQTT allowance and its usage (API/01 §47).
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Allowance {
    pub enabled: bool,
    /// `project` (its `spec.quotas.mqtt`), `organization` (`spec.limits.mqtt`) or `default`.
    pub origin: LimitOrigin,
    /// The sentence a refusal ends with: who can raise the allowance, and where (PF-101).
    pub raise: String,
    /// `ingest`, `capture`, `relay`.
    pub modes: Vec<String>,
    /// `platform`, `dedicatedPort`.
    pub addresses: Vec<String>,
    pub ports: Vec<u16>,
    pub brokers: Meter,
    pub connections: Meter,
    pub messages_per_second: Meter,
    pub max_message_bytes: Ceiling,
    pub ports_in_use: Vec<PortInUse>,
}

/// The words a list of manifest enums serializes to.
fn words<T: Serialize>(items: &[T]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match serde_json::to_value(item) {
            Ok(serde_json::Value::String(word)) => Some(word),
            _ => None,
        })
        .collect()
}

/// The allowance of `project` as the route answers it.
fn allowance(mirror: &crate::store::Mirror, project: &str) -> Allowance {
    let (allowance, origin) = crate::mqtt_allowance::effective(mirror, project);
    let brokers = crate::mqtt_allowance::brokers(mirror, project);
    let used = crate::mqtt_allowance::usage(&brokers, None);
    Allowance {
        enabled: allowance.enabled,
        origin,
        raise: jc_core::kinds::mqtt_allowance::raise(project),
        modes: if allowance.enabled {
            words(&allowance.modes())
        } else {
            Vec::new()
        },
        addresses: if allowance.enabled {
            words(&allowance.addresses())
        } else {
            Vec::new()
        },
        ports: allowance.ports.clone(),
        brokers: Meter::new(allowance.limit("brokers"), u64::from(used.brokers)),
        connections: Meter::new(allowance.limit("connections"), used.connections),
        messages_per_second: Meter::new(
            allowance.limit("messagesPerSecond"),
            used.messages_per_second,
        ),
        max_message_bytes: Ceiling {
            limit: allowance.limit("maxMessageBytes"),
        },
        ports_in_use: crate::mqtt_allowance::ports_in_use(&brokers)
            .into_iter()
            .map(|(port, broker)| PortInUse { port, broker })
            .collect(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqtt/allowance",
    summary = "The Project's MQTT Allowance",
    description = "What the project may run as MQTT edge brokers and what its committed brokers use of it (API/01 §47, PF-117, PF-118). Whoever may read the project. Written only as Organization spec.limits.mqtt or Project spec.quotas.mqtt, by an org admin.",
    tag = "resources",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The allowance and its usage", body = Allowance),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such project, or not readable by the caller", body = ProblemDetails),
    )
)]
pub async fn get_allowance(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<Allowance>, ApiError> {
    // PF-59: a project the caller may not read answers like one that does not exist.
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    Ok(Json(allowance(&state.mirror, &project)))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/mqtt/allowance", get(get_allowance))
}
