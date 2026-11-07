//! The internal listener's usage intake (T-5118): each gateway replica posts what every
//! Endpoint's surfaces answered since its last post, and the Portal adds it to the day's rows.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;

use crate::endpoint_usage::{parse, rows};
use crate::error::ApiError;
use crate::state::AppState;

/// 5,000 lines of about 60 bytes each, with room.
const BODY_LIMIT: usize = 1024 * 1024;

/// Adds one post, for the gateway's own client only (PF-46). The token is checked before the body
/// is read, so a caller without it learns nothing from a 400. A slug no Endpoint has is dropped.
pub async fn take_counts(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let post = parse(&body).map_err(ApiError::BadRequest)?;
    let slugs = endpoint_slugs(&state);
    let today = chrono::Utc::now().date_naive();
    let rows = rows(&post, today, |slug| slugs.get(slug).cloned());
    state
        .endpoint_usage
        .add(rows, today)
        .await
        .map_err(not_stored)?;
    Ok(StatusCode::NO_CONTENT)
}

fn not_stored(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "the usage counts were not stored");
    ApiError::Unavailable("the usage counts were not stored; the gateway sends them again".into())
}

/// Every Endpoint's slug with its project and name, from the mirror.
fn endpoint_slugs(state: &AppState) -> std::collections::HashMap<String, (String, String)> {
    state
        .mirror
        .matching(|env| env.kind == "Endpoint")
        .into_iter()
        .filter_map(|env| {
            let slug = env.spec.get("slug")?.as_str()?.to_owned();
            Some((slug, (env.metadata.namespace?, env.metadata.name)))
        })
        .collect()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/usage", post(take_counts))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}
