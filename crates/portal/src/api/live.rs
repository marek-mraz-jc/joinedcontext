//! `GET /api/v1/projects/{project}/spaces/{space}/live?type=…` and `POST /live-notify/{key}`:
//! the live updates of open data views (API/01 §32, T-3105).

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use utoipa::IntoParams;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

/// The largest notification passed on.
pub const MAX_NOTIFICATION: usize = 1024 * 1024;

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct LiveQuery {
    /// The NGSI-LD type the view shows.
    #[serde(rename = "type")]
    pub entity_type: String,
}

fn type_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/live",
    summary = "Watch A Type Of A Space",
    description = "Server-sent `changed` events saying that entities of one type changed, with no entity id, attribute name or value (API/01 §32, T-4876). The view reads the rows again with the caller's session, which the gateway narrows to the caller's grants.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        LiveQuery,
    ),
    responses(
        (status = 200, description = "The event stream", content_type = "text/event-stream"),
        (status = 400, description = "No NGSI-LD type name", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn watch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    Query(query): Query<LiveQuery>,
) -> Result<Response, ApiError> {
    crate::api::entity_trash::readable(&state, &user, &project, &space)?;
    if !type_name(&query.entity_type) {
        return Err(ApiError::BadRequest(
            "type must be an NGSI-LD type name".into(),
        ));
    }
    let segment = crate::spaces::segment(&state.mirror, &project, &space);
    let receiver = state
        .live
        .watch(&project, &space, &segment, &query.entity_type);
    // A window that fell behind misses changes rather than the stream: its next event reads again.
    let stream = BroadcastStream::new(receiver).filter_map(|item| {
        let changed = item.ok()?;
        Some(Ok::<_, std::convert::Infallible>(
            Event::default()
                .event("changed")
                .json_data(changed.as_ref())
                .unwrap_or_else(|_| Event::default().comment("unserializable")),
        ))
    });
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(20))
                .text("keep-alive"),
        )
        .into_response();
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

/// `POST /live-notify/{key}`: the gateway's delivery of one notification. Outside `/api/v1`, with
/// no session: the key is the credential, and a notification only makes a view read again.
pub async fn notify(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> StatusCode {
    if body.len() > MAX_NOTIFICATION {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    if serde_json::from_slice::<serde::de::IgnoredAny>(&body).is_err() {
        return StatusCode::BAD_REQUEST;
    }
    if state.live.deliver(&key) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/spaces/{space}/live", get(watch))
}
