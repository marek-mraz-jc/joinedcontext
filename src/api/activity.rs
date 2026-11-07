//! What is happening in a project: one paged list, one live tail, and the collector's ingest
//! (UI-31, OPS-48, OPS-49).
//!
//! - `GET /api/v1/projects/{project}/activity` — the list, newest first, paged
//! - `GET /api/v1/projects/{project}/activity/stream` — the same filter as Server-Sent Events
//! - `POST /api/v1/activity` — OTLP log records from the OpenTelemetry Collector
//!
//! The ingest route sits outside the project API on purpose: a record names its own project in
//! its attributes, and the collector is a member of no project and must not be given one.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use utoipa::{IntoParams, ToSchema};

use crate::activity::{ActivityEvent, ActivityFilter, Cursor, DEFAULT_LIMIT, MAX_LIMIT};
use crate::auth::bearer::TokenClient;
use crate::auth::session::{CurrentUser, Front};
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

const API_VERSION: &str = "joinedcontext.com/v1alpha1";

/// The parameters the list and the stream share, so a view switches between them without
/// rewriting anything.
#[derive(Debug, Clone, Default, Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ActivityQuery {
    pub space: Option<String>,
    /// One or more kinds, repeated or comma-separated.
    pub kind: Option<String>,
    pub source: Option<String>,
    /// `info`, `warning` or `error`; a value includes everything above it.
    pub severity: Option<String>,
    /// RFC 3339 instant, the oldest event to return.
    pub since: Option<String>,
    /// One object the events belong to, as `{plural}/{name}`.
    pub object: Option<String>,
    /// Only the calls and changes of one connector, by its id (T-5973).
    pub connector: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ActivityQuery {
    /// The filter this query names; the operation `jc_activity_list` builds the same one (T-0840).
    pub fn into_filter(self) -> Result<ActivityFilter, ApiError> {
        let since = match &self.since {
            Some(text) => Some(
                DateTime::parse_from_rfc3339(text)
                    .map_err(|_| {
                        ApiError::BadRequest(format!("'{text}' is not an RFC 3339 instant"))
                    })?
                    .with_timezone(&Utc),
            ),
            None => None,
        };
        if let Some(severity) = &self.severity {
            if !["info", "warning", "error"].contains(&severity.as_str()) {
                return Err(ApiError::BadRequest(format!(
                    "'{severity}' is not a severity: info, warning or error"
                )));
            }
        }
        if let Some(connector) = &self.connector {
            if connector.len() != 26
                || !connector
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
            {
                return Err(ApiError::BadRequest(format!(
                    "connector '{connector}' is not a connector id: 26 characters of a-z and 2-7, \
                     as My connectors shows it"
                )));
            }
        }
        let cursor = match &self.cursor {
            Some(text) => Some(Cursor::decode(text).ok_or_else(|| {
                ApiError::BadRequest("that cursor is not one this Portal handed out".into())
            })?),
            None => None,
        };
        Ok(ActivityFilter {
            space: self.space,
            kinds: self
                .kind
                .iter()
                .flat_map(|text| text.split(','))
                .map(str::trim)
                .filter(|kind| !kind.is_empty())
                .map(str::to_owned)
                .collect(),
            source: self.source,
            severity: self.severity,
            since,
            object: self.object,
            connector: self.connector,
            limit: self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
            cursor,
            reach: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityList {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<ActivityEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// A project the caller is a member of, or `404`. Never `403`: an activity route that tells the
/// two apart says which projects exist (R20).
fn member_of(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    if crate::permissions::for_request(state, &user.0.identity, project).may_read_project() {
        return Ok(());
    }
    Err(ApiError::NotFound(format!("project '{project}' not found")))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/activity",
    summary = "List Activity",
    description = "What happened in the project: the same events, filters and paging the activity page reads.",
    tag = "activity",
    params(
        ("project" = String, Path, description = "Project name"),
        ActivityQuery,
    ),
    responses(
        (status = 200, description = "What happened, newest first", body = ActivityList),
        (status = 400, description = "A parameter is not one of this route's", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project for this caller", body = ProblemDetails)
    )
)]
pub async fn list_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<ActivityList>, ApiError> {
    member_of(&state, &user, &project)?;
    let mut filter = query.into_filter()?;
    filter.reach =
        crate::permissions::for_request(&state, &user.0.identity, &project).spaces_reached();
    let page = state
        .activity
        .list(&project, &filter)
        .await
        .map_err(|err| ApiError::Unavailable(err.to_string()))?;
    Ok(Json(ActivityList {
        api_version: API_VERSION.to_owned(),
        kind: "List".to_owned(),
        items: page.items,
        next: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/activity/stream",
    summary = "Follow Activity",
    description = "The project's activity as Server-Sent Events, with the same filters as the list. Readers of the project only; the stream carries what the list would.",
    tag = "activity",
    params(
        ("project" = String, Path, description = "Project name"),
        ActivityQuery,
    ),
    responses(
        (status = 200, description = "The same filter, as Server-Sent Events"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project for this caller", body = ProblemDetails)
    )
)]
pub async fn stream_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ActivityQuery>,
) -> Result<Response, ApiError> {
    member_of(&state, &user, &project)?;
    let filter = query.into_filter()?;
    let project_of_stream = project.clone();
    let identity = user.0.identity;

    let live = state.activity_events.subscribe(&project).await;
    let stream = BroadcastStream::new(live)
        .map_while(
            move |item| -> Option<Option<Result<Event, std::convert::Infallible>>> {
                // Checked against the bindings of this moment, as the draft stream is: a revoked
                // space binding stops that space's events without a reconnect (T-4876), and a
                // person no longer in the project gets no event at all, the stream ends
                // (F-POR1-009).
                let effective =
                    crate::permissions::for_request(&state, &identity, &project_of_stream);
                if !effective.may_read_project() {
                    return None;
                }
                // A lagging browser misses events rather than the rest of the tail: it
                // reconnects with `since` and the store replays what it lost.
                let Ok(event) = item else {
                    return Some(None);
                };
                if event.project != project_of_stream
                    || !filter.matches(&event)
                    || !(ActivityFilter {
                        reach: effective.spaces_reached(),
                        ..Default::default()
                    })
                    .matches(&event)
                {
                    return Some(None);
                }
                Some(Some(Ok(Event::default()
                    .event(event.kind.clone())
                    .json_data(&event)
                    .unwrap_or_else(|_| {
                        Event::default().comment("unserializable")
                    }))))
            },
        )
        .filter_map(|event| event);

    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(20))
            .text("keep-alive"),
    );
    let mut response = sse.into_response();
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

/// What the collector reads: how many records the Portal refused, and why the first one was.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartialSuccess {
    pub rejected_log_records: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_message: String,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsServiceResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_success: Option<PartialSuccess>,
}

/// One OTLP attribute value, flattened to what an event field holds. A kvlist or an array is not
/// an event field, so it is dropped rather than stringified into something a filter cannot use.
fn attribute_value(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if let Some(text) = object.get("stringValue").and_then(Value::as_str) {
        return Some(json!(text));
    }
    if let Some(text) = object.get("intValue").and_then(Value::as_str) {
        return text.parse::<i64>().ok().map(|n| json!(n));
    }
    if let Some(number) = object.get("intValue").and_then(Value::as_i64) {
        return Some(json!(number));
    }
    if let Some(number) = object.get("doubleValue").and_then(Value::as_f64) {
        return Some(json!(number));
    }
    if let Some(flag) = object.get("boolValue").and_then(Value::as_bool) {
        return Some(json!(flag));
    }
    None
}

/// One log record as the event it carries. Everything the vocabulary does not name is `details`.
fn event_of(record: &Value) -> Result<ActivityEvent, String> {
    let mut named: HashMap<String, Value> = HashMap::new();
    for attribute in record
        .get("attributes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(key), Some(value)) = (
            attribute.get("key").and_then(Value::as_str),
            attribute.get("value").and_then(attribute_value),
        ) else {
            continue;
        };
        named.insert(key.to_owned(), value);
    }
    let text = |key: &str| -> Option<String> {
        named
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.is_empty())
    };

    // The collector sends the instant as nanoseconds; a record without one happened when it
    // arrived, which is closer to the truth than a zero epoch.
    let time = record
        .get("timeUnixNano")
        .and_then(|value| {
            value
                .as_str()
                .and_then(|text| text.parse::<i64>().ok())
                .or_else(|| value.as_i64())
        })
        .map(DateTime::from_timestamp_nanos)
        .unwrap_or_else(Utc::now);

    let summary = text("summary")
        .or_else(|| {
            record
                .get("body")
                .and_then(|body| body.get("stringValue"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();

    let details: Map<String, Value> = named
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "project" | "space" | "kind" | "source" | "summary" | "severity" | "correlationId"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let event = ActivityEvent {
        time,
        project: text("project").unwrap_or_default(),
        space: text("space"),
        kind: text("kind").unwrap_or_default(),
        source: text("source").unwrap_or_default(),
        summary,
        severity: text("severity").unwrap_or_else(|| "info".to_owned()),
        correlation_id: text("correlationId"),
        details: Value::Object(details),
    };
    event.validate()?;
    Ok(event)
}

/// The OpenTelemetry Collector's Keycloak client
/// (`joinedcontext-deployment/components/observability/keycloak-clients.yaml`).
pub const COLLECTOR_CLIENT: &str = "activity-ingest";

#[utoipa::path(
    post,
    path = "/api/v1/activity",
    tag = "activity",
    request_body(
        content = Object,
        description = "An OTLP/HTTP JSON `ExportLogsServiceRequest`: one log record per event, \
                       its fields as attributes",
        content_type = "application/json",
        example = json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": [{
            "timeUnixNano": "1789314063000000000",
            "attributes": [
                { "key": "project", "value": { "stringValue": "helsinki" } },
                { "key": "kind", "value": { "stringValue": "access.denied" } },
                { "key": "source", "value": { "stringValue": "gateway" } },
                { "key": "severity", "value": { "stringValue": "warning" } },
                { "key": "summary", "value": { "stringValue": "An anonymous caller was refused a write." } }
            ]
        }] }] }] })
    ),
    responses(
        (status = 200, description = "The OTLP export response, naming what was rejected", body = ExportLogsServiceResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Not the collector's own service account", body = ProblemDetails)
    )
)]
/// The collector's route. One batch a request; a record with an unknown project or a kind outside
/// the vocabulary is rejected on its own, because one malformed record must not cost the other
/// four hundred.
pub async fn ingest_activity(
    _user: CurrentUser,
    client: Option<axum::Extension<TokenClient>>,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<ExportLogsServiceResponse>, ApiError> {
    // The collector is a ServiceAccount client, not a person: a browser session or an edge token
    // is refused here whatever roles it carries (OPS-48).
    if Front::of(&headers, state.config.trust_edge_token) != Front::Bearer {
        return Err(ApiError::Denied(
            "the activity ingest route is the collector's, and a human session is not it".into(),
        ));
    }
    // And not any service account either (T-1402): every workload's token carries the Portal's
    // audience, so the audience says nothing about who writes the audit trail. The client that
    // obtained the token does, and only the collector's own may append here.
    if client
        .as_ref()
        .map(|axum::Extension(TokenClient(id))| id.as_str())
        != Some(COLLECTOR_CLIENT)
    {
        return Err(ApiError::Denied(format!(
            "the activity ingest route is the collector's (Keycloak client '{COLLECTOR_CLIENT}'), \
             and this token was obtained by another client"
        )));
    }
    let known: Vec<String> = state.mirror.namespaces();

    let mut accepted: Vec<ActivityEvent> = Vec::new();
    let mut rejected = 0_i64;
    let mut first_error = String::new();
    let mut reject = |reason: String| {
        rejected += 1;
        if first_error.is_empty() {
            first_error = reason;
        }
    };

    for record in body
        .get("resourceLogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|resource| {
            resource
                .get("scopeLogs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .flat_map(|scope| {
            scope
                .get("logRecords")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
    {
        match event_of(record) {
            Ok(event) if !known.contains(&event.project) => {
                reject(format!("project '{}' is not a project here", event.project));
            }
            Ok(event) => accepted.push(event),
            Err(reason) => reject(reason),
        }
    }

    if !accepted.is_empty() {
        state
            .activity
            .append(&accepted)
            .await
            .map_err(|err| ApiError::Unavailable(err.to_string()))?;
    }

    Ok(Json(ExportLogsServiceResponse {
        partial_success: (rejected > 0).then_some(PartialSuccess {
            rejected_log_records: rejected,
            error_message: first_error,
        }),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/activity", get(list_activity))
        .route("/projects/{project}/activity/stream", get(stream_activity))
        .route("/activity", post(ingest_activity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribute(key: &str, value: &str) -> Value {
        json!({ "key": key, "value": { "stringValue": value } })
    }

    fn record(extra: Vec<Value>) -> Value {
        let mut attributes = vec![
            attribute("project", "helsinki"),
            attribute("kind", "access.denied"),
            attribute("source", "gateway"),
            attribute("severity", "warning"),
            attribute("summary", "An anonymous caller was refused a write."),
        ];
        attributes.extend(extra);
        json!({ "timeUnixNano": "1789314063000000000", "attributes": attributes })
    }

    #[test]
    fn a_log_record_becomes_the_event_its_attributes_name() {
        let event = event_of(&record(vec![
            attribute("space", "air-quality"),
            attribute("correlationId", "4bf92f3577b34da6a3ce929d0e0e4736"),
            attribute("endpoint", "public-air"),
            json!({ "key": "requests", "value": { "intValue": "42" } }),
        ]))
        .expect("an event");

        assert_eq!(event.project, "helsinki");
        assert_eq!(event.kind, "access.denied");
        assert_eq!(event.space.as_deref(), Some("air-quality"));
        assert_eq!(
            event.correlation_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(event.time.timestamp(), 1_789_314_063);
        // Everything the vocabulary does not name is details, with its own type kept.
        assert_eq!(event.details["endpoint"], json!("public-air"));
        assert_eq!(event.details["requests"], json!(42));
        assert!(
            event.details.get("project").is_none(),
            "a field of the record is not repeated in details"
        );
    }

    #[test]
    fn a_kind_outside_the_vocabulary_and_a_record_with_no_project_are_each_refused() {
        let invented = event_of(&record(vec![attribute("kind", "something.happened")]));
        assert!(invented.is_err());

        let nameless = event_of(&json!({ "attributes": [attribute("kind", "access.denied")] }));
        assert!(nameless.is_err(), "a record names its own project");
    }

    #[test]
    fn the_gateways_rate_limited_row_is_an_event_of_its_kind() {
        let mut row = record(vec![
            attribute("space", "walk"),
            attribute("endpoint", "walk-read"),
            json!({ "key": "count", "value": { "intValue": "12" } }),
        ]);
        row["attributes"][1] = attribute("kind", "endpoint.rate-limited");
        row["attributes"][4] = attribute(
            "summary",
            "serviceAccount:walk-caller was refused 12 times on walk-read: its rate limit was spent",
        );
        let event = event_of(&row).expect("the gateway's T-5624 kind is in the vocabulary");
        assert_eq!(event.kind, "endpoint.rate-limited");
        assert_eq!(event.details["count"], json!(12));
        assert!(event.summary.starts_with("serviceAccount:walk-caller"));
    }

    #[test]
    fn the_body_is_the_summary_when_no_attribute_carries_one() {
        let mut spoken = record(vec![]);
        spoken["attributes"] = json!(spoken["attributes"]
            .as_array()
            .expect("attributes")
            .iter()
            .filter(|a| a["key"] != json!("summary"))
            .cloned()
            .collect::<Vec<_>>());
        spoken["body"] = json!({ "stringValue": "The PDP refused a write." });

        let event = event_of(&spoken).expect("an event");
        assert_eq!(event.summary, "The PDP refused a write.");
    }

    #[test]
    fn a_query_reads_its_kinds_comma_separated_and_refuses_a_severity_that_is_not_one() {
        let filter = ActivityQuery {
            kind: Some("access.denied, pipeline.error".into()),
            ..Default::default()
        }
        .into_filter()
        .expect("a filter");
        assert_eq!(filter.kinds, vec!["access.denied", "pipeline.error"]);
        assert_eq!(filter.limit, DEFAULT_LIMIT);

        assert!(ActivityQuery {
            severity: Some("loud".into()),
            ..Default::default()
        }
        .into_filter()
        .is_err());
        assert!(ActivityQuery {
            since: Some("yesterday".into()),
            ..Default::default()
        }
        .into_filter()
        .is_err());
    }

    #[test]
    fn a_connector_filter_takes_only_a_connector_id() {
        let filter = ActivityQuery {
            connector: Some("abcdefghijklmnopqrstuvwxyz".into()),
            ..Default::default()
        }
        .into_filter()
        .expect("a filter");
        assert_eq!(
            filter.connector.as_deref(),
            Some("abcdefghijklmnopqrstuvwxyz")
        );
        for bad in [
            "doprava-in-claude",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "' OR 1=1 --",
        ] {
            let refused = ActivityQuery {
                connector: Some(bad.into()),
                ..Default::default()
            }
            .into_filter();
            assert!(
                matches!(&refused, Err(ApiError::BadRequest(why)) if why.starts_with("connector")),
                "{bad}: {refused:?}"
            );
        }
    }

    #[test]
    fn a_page_larger_than_the_ceiling_is_the_ceiling() {
        let filter = ActivityQuery {
            limit: Some(10_000),
            ..Default::default()
        }
        .into_filter()
        .expect("a filter");
        assert_eq!(filter.limit, MAX_LIMIT);
    }
}
