//! The internal listener's capture route of a pipeline test (AG-52, T-2271): what the harness
//! produced, posted back by the project's pipeline runner.

use crate::api::pipeline_test::{BODY_LIMIT, RUNNING};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use jcctl::pipeline_test::Captured;

/// `POST /internal/pipeline-tests/{id}`: what the harness produced, one message per call.
///
/// The id is 130 random bits minted for this test and known to the harness alone, so it is a
/// capability of its own; a message for a test that is not running is dropped with a 404. Since
/// T-2271 the caller is named as well: the project's pipeline runner presents its own ServiceAccount
/// token, audience-bound to this listener, and a call with no identity gets the 401 it deserves
/// rather than a 404 that only says "no such test" (AG-52).
pub async fn capture(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // The test's own project, when one of that id is running: its harness presents that
    // project's client (T-3193). An id nobody runs names no project, and only the runner's
    // configured client is told it is not running.
    let project = RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|running| running.project.clone());
    if crate::auth::internal::authenticate_pipeline_runner(&state, &headers, project.as_deref())
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED;
    }
    captured(&id, &body)
}

/// What the capture does once the caller is known: the message, or why it went nowhere.
pub(crate) fn captured(id: &str, body: &Bytes) -> StatusCode {
    let Ok(message) = serde_json::from_slice::<Captured>(body) else {
        return StatusCode::BAD_REQUEST;
    };
    let sent = RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|running| running.sender.send(message).is_ok());
    match sent {
        Some(true) => StatusCode::NO_CONTENT,
        _ => StatusCode::NOT_FOUND,
    }
}

pub fn router() -> Router<AppState> {
    // The harness posts a whole fetched feed back as one message, so the capture route takes
    // what the test route takes; axum's default two mebibytes turned a 1.2 MB feed into 413.
    Router::new()
        .route("/internal/pipeline-tests/{id}", post(capture))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}
