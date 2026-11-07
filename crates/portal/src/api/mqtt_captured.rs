//! `GET` and `DELETE /api/v1/projects/{project}/mqttbrokers/{name}/captured`: what a `capture`
//! broker kept (API/01 §47, ADR-N-048 §2.10, PL-87, T-4880).
//!
//! Only a person whose binding holds `approve` on `MqttBroker` in the project reads or deletes a
//! capture; anybody else, a reader of the manifest included, gets the `404` of a broker the
//! project does not declare (PF-59). A delete is recorded in the activity log with the count of
//! messages, never a payload.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use chrono::{DateTime, Utc};
use jc_core::kinds::mqtt_broker::MqttMode;
use jc_core::kinds::MqttBrokerSpec;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::mqtt_capture::Message;
use crate::state::AppState;
use jc_core::kinds::Verb;

/// The most messages one read answers.
const MAX_LIMIT: usize = 1000;
/// The longest topic MQTT carries.
const MAX_TOPIC: usize = 65_535;

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct CapturedQuery {
    /// One exact topic, no wildcard, e.g. `cams/cam-west/frame-meta`.
    pub topic: Option<String>,
    /// How many messages, 1 to 1000; default 100.
    pub limit: Option<usize>,
}

/// One kept message.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapturedMessage {
    pub topic: String,
    #[schema(value_type = String, format = DateTime)]
    pub received_at: DateTime<Utc>,
    /// The device, where a publish filter of the broker names it as `${clientid}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub bytes: usize,
    /// UTF-8 text when the payload decodes, else base64.
    pub payload: String,
    /// `utf-8` or `base64`.
    pub encoding: &'static str,
}

/// A broker's capture, newest first.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Captured {
    pub broker: String,
    /// `ingest`, `capture` or `relay`; only `capture` keeps messages.
    pub mode: String,
    /// The capture's window, an ISO 8601 duration; absent outside `capture`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_per_topic: Option<u32>,
    pub messages: Vec<CapturedMessage>,
    /// More messages are kept than `limit` answered.
    pub truncated: bool,
}

fn shown(message: Message) -> CapturedMessage {
    let bytes = message.payload.len();
    let (payload, encoding) = match String::from_utf8(message.payload) {
        Ok(text) => (text, "utf-8"),
        Err(raw) => (
            base64::engine::general_purpose::STANDARD.encode(raw.into_bytes()),
            "base64",
        ),
    };
    CapturedMessage {
        topic: message.topic,
        received_at: message.received_at,
        client_id: message.client_id,
        bytes,
        payload,
        encoding,
    }
}

/// The broker's spec, for a caller who may approve brokers in `project`; `404` for everybody
/// else, the answer for a broker that does not exist (PF-59).
fn approvable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
) -> Result<MqttBrokerSpec, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "MQTT broker '{name}' not found in project '{project}'"
        ))
    };
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if !effective.may_read_project() || !effective.may_in("MqttBroker", Verb::Approve, None) {
        return Err(not_found());
    }
    let manifest = state
        .mirror
        .get(project, "MqttBroker", name)
        .ok_or_else(not_found)?;
    serde_json::from_value(manifest.spec).map_err(|_| not_found())
}

fn unavailable(error: sqlx::Error) -> ApiError {
    tracing::warn!(%error, "the MQTT capture store did not answer");
    ApiError::Unavailable("the capture store did not answer; try again in a minute".to_owned())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/captured",
    summary = "Read An MQTT Broker's Capture",
    description = "The newest messages a capture broker kept; 404 to whoever may not approve MqttBrokers.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
        CapturedQuery,
    ),
    responses(
        (status = 200, description = "The kept messages; empty outside capture mode", body = Captured),
        (status = 400, description = "A topic with a wildcard, or a limit outside 1…1000", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or the caller may not approve brokers here", body = ProblemDetails),
        (status = 503, description = "The capture store did not answer", body = ProblemDetails),
    )
)]
pub(crate) async fn get_captured(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<CapturedQuery>,
) -> Result<Json<Captured>, ApiError> {
    let spec = approvable(&state, &user, &project, &name)?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "limit {limit} is outside 1…{MAX_LIMIT}; for example ?limit=100"
        )));
    }
    let topic = query.topic.filter(|topic| !topic.is_empty());
    if let Some(topic) = &topic {
        if topic.contains(['+', '#']) || topic.len() > MAX_TOPIC {
            return Err(ApiError::BadRequest(
                "topic must be one exact topic without + or #, for example cams/cam-west/frame-meta"
                    .to_owned(),
            ));
        }
    }
    let mode = spec.mode.unwrap_or(MqttMode::Ingest);
    let capture = (mode == MqttMode::Capture).then(|| spec.capture.clone().unwrap_or_default());
    let (messages, truncated) = if capture.is_some() {
        state
            .mqtt_capture
            .read(&project, &name, topic.as_deref(), limit, Utc::now())
            .await
            .map_err(unavailable)?
    } else {
        (Vec::new(), false)
    };
    Ok(Json(Captured {
        broker: name,
        mode: mode.word().to_owned(),
        window: capture.as_ref().map(|c| c.window.clone()),
        messages_per_topic: capture.as_ref().map(|c| c.messages_per_topic),
        messages: messages.into_iter().map(shown).collect(),
        truncated,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/captured",
    summary = "Delete An MQTT Broker's Capture",
    description = "Deletes every message the broker kept at once and records who did it in the activity log, without the payloads (API/01 §47). Only a person who may approve MqttBrokers in the project; anybody else gets 404.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    responses(
        (status = 204, description = "The capture is gone"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or the caller may not approve brokers here", body = ProblemDetails),
        (status = 503, description = "The capture store did not answer", body = ProblemDetails),
    )
)]
pub(crate) async fn delete_captured(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    approvable(&state, &user, &project, &name)?;
    let gone = state
        .mqtt_capture
        .purge(&project, &name)
        .await
        .map_err(unavailable)?;
    let actor = &user.0.identity.username;
    let event = crate::activity::ActivityEvent {
        time: Utc::now(),
        project: project.clone(),
        space: None,
        kind: "mqtt.capture.deleted".to_owned(),
        source: "portal".to_owned(),
        summary: format!("{actor} deleted the capture of MQTT broker {name}: {gone} messages"),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "object": format!("mqttbrokers/{name}"), "actor": actor, "messages": gone }),
    };
    if let Err(error) = state.activity.append(&[event]).await {
        tracing::warn!(%error, "the capture deletion was not recorded");
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/mqttbrokers/{name}/captured",
        get(get_captured).delete(delete_captured),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_text_when_it_decodes_and_base64_otherwise() {
        let message = |payload: &[u8]| Message {
            topic: "t".to_owned(),
            client_id: None,
            received_at: DateTime::from_timestamp(0, 0).unwrap_or_default(),
            expires_at: DateTime::from_timestamp(60, 0).unwrap_or_default(),
            payload: payload.to_vec(),
        };
        let text = shown(message(br#"{"seq": 1}"#));
        assert_eq!(
            (text.payload.as_str(), text.encoding, text.bytes),
            (r#"{"seq": 1}"#, "utf-8", 10)
        );
        let raw = shown(message(&[0xff, 0x00, 0x01]));
        assert_eq!(
            (raw.payload.as_str(), raw.encoding, raw.bytes),
            ("/wAB", "base64", 3)
        );
    }
}
