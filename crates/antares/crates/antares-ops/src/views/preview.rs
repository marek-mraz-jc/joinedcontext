// SPDX-License-Identifier: EUPL-1.2
//! `POST /q/views/{id}/preview` (gap G13): what a draft view would serve,
//! before anyone stores it. The body is the candidate view document, checked
//! as a PUT checks it; the request's query string is a Query Entities query
//! (5.7.2) run through the candidate's narrowing in its Tenant, under the
//! subject headers of the `/q` request (T-3813): the admin caller names whose
//! reading it previews, and a request naming nobody is the cluster's own, as
//! on every path. The answer is the count and one bounded page. Nothing is
//! stored and the table is not reloaded. The narrowing is previewed whether
//! or not the draft lists `query_entities` among its operations.

use super::{candidate, problem, serve, Reloader, MAX_VIEW_BYTES};
use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use std::sync::{Arc, Weak};

/// The largest page a preview answers; `limit` defaults to 20 (6.3.10).
pub const MAX_PREVIEW_LIMIT: u64 = 100;

const ENTITIES: &str = "/ngsi-ld/v1/entities";

/// `/q/views/{id}/preview`.
pub(super) fn routes(r: &Weak<Reloader>) -> Router {
    let r = r.clone();
    Router::new().route(
        "/views/{id}/preview",
        post(
            move |Path(id): Path<String>,
                  RawQuery(q): RawQuery,
                  headers: HeaderMap,
                  body: Bytes| {
                serve(r.clone(), |r| preview(r, id, q, headers, body))
            },
        ),
    )
}

/// The query as it is run: the caller's pairs without `count`, then
/// `count=true`; or why its page is over the bound.
fn query(raw: Option<&str>) -> Result<String, String> {
    let mut pairs = Vec::new();
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // as the API reads them, so an escaped `limit` meets the bound too
        let dec = |s: &str| antares_ql::percent_decode(s.replace('+', " ").as_bytes());
        let (key, value) = (dec(key), dec(value));
        match key.as_str() {
            "count" => continue,
            "limit" => match value.parse::<u64>() {
                Ok(n) if n <= MAX_PREVIEW_LIMIT => {}
                _ => {
                    return Err(format!(
                        "a preview's limit is a number up to {MAX_PREVIEW_LIMIT}"
                    ))
                }
            },
            _ => {}
        }
        pairs.push(pair);
    }
    pairs.push("count=true");
    Ok(pairs.join("&"))
}

async fn preview(
    r: Arc<Reloader>,
    id: String,
    raw: Option<String>,
    caller: HeaderMap,
    body: Bytes,
) -> Result<Response, String> {
    if body.len() > MAX_VIEW_BYTES {
        return Ok(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "BadRequestData",
            &format!("a view document is at most {MAX_VIEW_BYTES} bytes"),
        ));
    }
    let checked = candidate(&id, &body).and_then(|(_, l)| query(raw.as_deref()).map(|q| (l, q)));
    let (loaded, q) = match checked {
        Ok(c) => c,
        Err(why) => return Ok(problem(StatusCode::BAD_REQUEST, "BadRequestData", &why)),
    };
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("{ENTITIES}?{q}"))
        .header("NGSILD-Tenant", &loaded.tenant)
        .header("Accept", "application/json")
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let answer = r.state.with_view(loaded.view).call(&caller, req).await;
    // The admin caller reads a refusal as the broker gave it.
    if !answer.status().is_success() {
        return Ok(answer);
    }
    let count = answer
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or_default();
    let bytes = match r.state.read_answer(answer.into_body(), "views").await {
        Ok(bytes) => bytes,
        Err(antares_api::bounds::AnswerRefusal::OverBudget) => {
            return Ok(antares_api::negotiate::ApiError::from(
                antares_api::bounds::AnswerRefusal::OverBudget,
            )
            .into_response())
        }
        Err(_) => {
            return Ok(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BadRequestData",
                antares_api::bounds::ANSWER_TOO_LARGE,
            ))
        }
    };
    let entities: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(axum::Json(serde_json::json!({"count": count, "entities": entities})).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_counts_and_bounds_its_page() {
        assert_eq!(query(None).as_deref(), Ok("count=true"));
        assert_eq!(
            query(Some("type=Car&count=false&limit=100")).as_deref(),
            Ok("type=Car&limit=100&count=true")
        );
        assert!(query(Some("limit=101")).is_err());
        assert!(query(Some("limit=x")).is_err());
        assert!(query(Some("limit")).is_err());
        // escaped as the API reads it: the bound and the count hold
        assert!(query(Some("li%6Dit=1000")).is_err());
        assert!(query(Some("limit=%31%30%31")).is_err());
        assert_eq!(
            query(Some("%63ount=false&type=Car")).as_deref(),
            Ok("type=Car&count=true")
        );
    }
}
