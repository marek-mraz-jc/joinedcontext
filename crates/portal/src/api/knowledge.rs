//! The knowledge assistant's administration in the Portal (API/01 §34, T-3057, AG-113): what a
//! project's `KnowledgeSource`s hold once crawled, read and steered here, answered by
//! `jc-assistant`'s internal paths. The caller's permission is checked first and nothing reaches
//! the assistant for a caller who lacks it; the assistant is asked with the Portal's own token.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use jc_core::kinds::assistant::{KnowledgeSourceSpec, SourceType};
use jc_core::kinds::Verb;
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::state::AppState;
use crate::store::ListOptions;

/// The most ids one inclusion call may name.
pub const MAX_IDS: usize = 500;

/// One level of the page tree.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct Level {
    /// The page whose children to list; none lists the roots.
    pub parent: Option<i64>,
    /// Instead of one level: at most 100 pages of the whole source whose title or address holds
    /// it, case and diacritics ignored; 2 to 100 characters (UI-99).
    pub q: Option<String>,
}

/// A trial of the search `search_documents` runs, over one source (UI-99, T-4659).
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct Tried {
    /// 1 to 300 characters.
    pub q: String,
    /// 1 to 10 hits, 5 when absent.
    pub limit: Option<u8>,
}

/// The page or the document whose passages to show; exactly one.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct PassagesOf {
    pub page: Option<i64>,
    pub document: Option<i64>,
}

/// Include or exclude pages, their subtrees, and documents (API/01 §34).
#[derive(Debug, Deserialize, serde::Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InclusionRequest {
    /// Page ids of this source.
    #[serde(default)]
    pub pages: Vec<i64>,
    /// Document ids of this source.
    #[serde(default)]
    pub documents: Vec<i64>,
    /// Whether every page below the named ones, and the documents they link, change too.
    #[serde(default)]
    pub subtree: bool,
    /// `false` excludes: their passages go at once and the choice holds over later crawls.
    pub included: bool,
}

/// One turn of what the person's chat holds (API/05 §1.1).
#[derive(Debug, Deserialize, serde::Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatTurn {
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
}

/// A question to an assistant (API/05 §1.1); `jc-assistant` holds every bound.
#[derive(Debug, Deserialize, serde::Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    /// The id the first answer gave, to continue its conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// 1 to 4,000 characters.
    pub message: String,
    /// At most 6 earlier turns.
    #[serde(default)]
    pub history: Vec<ChatTurn>,
    /// The connectors switched on; none sent leaves every one on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connectors: Option<Vec<String>>,
}

/// The largest question body (API/05 §1.1).
const MAX_CHAT_BODY: usize = 64 * 1024;
/// The largest upload: the most any source's `upload.maxBytes` allows (T-4184).
const MAX_UPLOAD_BODY: usize = jc_core::kinds::assistant::MAX_UPLOAD_BYTES as usize;
/// The longest a list of stored conversations may take.
pub(super) const LIST_TIME: Duration = Duration::from_secs(20);
/// The longest an answer may take: six model calls and their tools (AG-107).
const CHAT_TIMEOUT: Duration = Duration::from_secs(300);

/// Read access to the project's sources, or not there at all (R20).
fn readable(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if is_dns1123(project) && effective.may_read_project() && effective.may_read("KnowledgeSource")
    {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("project '{project}' not found")))
    }
}

/// Write access: proposing a `KnowledgeSource` is what steering one takes (AG-113).
fn writable(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    readable(state, user, project)?;
    crate::permissions::for_request(state, &user.0.identity, project).check(
        "KnowledgeSource",
        Verb::Propose,
        None,
    )
}

fn named(what: &str, name: &str) -> Result<(), ApiError> {
    if is_dns1123(name) {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("{what} '{name}' not found")))
    }
}

/// One client for the assistant, without redirects: its address is configuration, and the
/// Portal's token goes nowhere else.
fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// Asks `jc-assistant` at `path` (under `/internal/v1/projects/`) with the Portal's own token and
/// returns its status and body. Its `401` is this Portal's misconfiguration, not the caller's.
pub(crate) async fn ask(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), ApiError> {
    let with = move |request: reqwest::RequestBuilder| match body {
        Some(body) => request.json(&body),
        None => request,
    };
    ask_with(state, method, path, Duration::from_secs(20), with).await
}

/// A request to the assistant's administration routes with the Portal's own token.
async fn request(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    timeout: Duration,
) -> Result<reqwest::RequestBuilder, ApiError> {
    let base = state.config.knowledge_url.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "this Portal has no knowledge assistant address (JC_PORTAL_KNOWLEDGE_URL)".into(),
        )
    })?;
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no Keycloak client to ask the assistant with".into())
    })?;
    let token = oidc.service_token().await.map_err(|err| {
        ApiError::Unavailable(format!("no token for the knowledge assistant: {err}"))
    })?;
    Ok(http()
        .request(method, format!("{base}/internal/v1/projects/{path}"))
        .bearer_auth(token)
        .timeout(timeout))
}

/// Sends `request`; the answer, or the refusal it carries as the Portal's own error.
async fn sent(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    let answer = request.send().await.map_err(|err| {
        tracing::warn!(error = %err.without_url(), "the knowledge assistant did not answer");
        ApiError::Unavailable("the knowledge assistant did not answer; try again shortly".into())
    })?;
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_success() {
        return Ok(answer);
    }
    let body: Value = crate::capped::json(answer, crate::capped::ANSWER_BYTES)
        .await
        .unwrap_or(Value::Null);
    let detail = || {
        body.get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no reason given")
            .to_owned()
    };
    Err(match status.as_u16() {
        400 => ApiError::BadRequest(detail()),
        404 => ApiError::NotFound(detail()),
        409 => ApiError::Conflict(detail()),
        413 => ApiError::TooLarge(detail()),
        429 => ApiError::TooManyRequests(detail()),
        415 => ApiError::UnsupportedMediaType(detail()),
        401 | 403 => {
            tracing::error!("the knowledge assistant refused the Portal's own token");
            ApiError::Unavailable(
                "the knowledge assistant does not accept this Portal's token".into(),
            )
        }
        _ => ApiError::Unavailable(format!("the knowledge assistant answered {status}")),
    })
}

/// [`ask`] with the request's body and time limit chosen by the caller: an upload's raw bytes.
pub(super) async fn ask_with(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    timeout: Duration,
    with: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<(StatusCode, Value), ApiError> {
    let answer = sent(with(request(state, method, path, timeout).await?)).await?;
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::OK);
    let body = crate::capped::json(answer, crate::capped::ANSWER_BYTES).await;
    Ok((status, body.unwrap_or(Value::Null)))
}

/// The assistant's answer to a GET of `path` with `query`, as text: a CSV export.
pub(super) async fn ask_text(
    state: &AppState,
    path: &str,
    timeout: Duration,
    query: &impl serde::Serialize,
) -> Result<String, ApiError> {
    let request = request(state, reqwest::Method::GET, path, timeout).await?;
    let answer = sent(request.query(query)).await?;
    crate::capped::text(answer, crate::capped::ANSWER_BYTES)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "the knowledge assistant's answer broke off");
            ApiError::Unavailable(
                "the knowledge assistant's answer broke off; try again shortly".into(),
            )
        })
}

/// What a source is, from its manifest: for the list, beside what it holds.
fn summary(name: &str, spec: &KnowledgeSourceSpec) -> Value {
    json!({
        "source": name,
        "type": match spec.source {
            SourceType::Website => "website",
            SourceType::Ckan => "ckan",
            SourceType::Catalogue => "catalogue",
            SourceType::Guide => "guide",
            SourceType::Upload => "upload",
        },
        "startUrls": spec.start_urls,
        // The spaces a catalogue source reads; empty is every space of the project (AG-116).
        "contextSpaces": spec.context_spaces,
        "ckanInstanceRef": spec.ckan_instance_ref,
        // The CKAN organizations a ckan source narrows to; empty is every one (MF-51).
        "organizations": spec.organizations,
        "schedule": spec.schedule,
        "visibility": spec.visibility,
        "include": spec.include,
        "exclude": spec.exclude,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources",
    summary = "List Knowledge Sources",
    description = "Every KnowledgeSource the project declares, with what the assistant holds of it: pages, documents, passages, the last crawl and the last job; `state: not-crawled` before its first crawl (API/01 §34).",
    tag = "knowledge",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The sources", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project the caller may read the sources of", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_sources(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    let (_, held) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources"),
        None,
    )
    .await?;
    let held = held
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let items: Vec<Value> = state
        .mirror
        .list(&project, "KnowledgeSource", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| {
            let spec: KnowledgeSourceSpec = serde_json::from_value(env.spec).ok()?;
            let mut item = summary(&env.metadata.name, &spec);
            match held.iter().find(|h| {
                h.get("source").and_then(Value::as_str) == Some(env.metadata.name.as_str())
            }) {
                Some(found) => {
                    for key in [
                        "state",
                        "lastCrawl",
                        "pages",
                        "pagesIncluded",
                        "documents",
                        "passages",
                        "embedded",
                        "pending",
                        "failed",
                        "languages",
                        "job",
                    ] {
                        item[key] = found.get(key).cloned().unwrap_or(Value::Null);
                    }
                }
                None => item["state"] = json!("not-crawled"),
            }
            Some(item)
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

/// The pairs that have a value as a query, `?a=1&b=x`, encoded; nothing when none has one.
fn query_of(pairs: &[(&str, Option<String>)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        if let Some(value) = value {
            query.append_pair(name, value);
        }
    }
    let query = query.finish();
    if query.is_empty() {
        query
    } else {
        format!("?{query}")
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/pages",
    summary = "Read A Level Of The Page Tree",
    description = "The pages under `parent`, or the roots without it, or with `q` at most 100 pages of the whole source whose title or address holds it: depth, status, language, title, description, published and modified, whether included and who excluded it, and how many children, documents and passages each holds.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        Level,
    ),
    responses(
        (status = 200, description = "The pages", body = Object),
        (status = 400, description = "`q` is not 2 to 100 characters", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_pages(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
    Query(level): Query<Level>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let query = query_of(&[
        ("parent", level.parent.map(|p| p.to_string())),
        ("q", level.q),
    ]);
    let path = format!("{project}/knowledge/sources/{source}/pages{query}");
    let (_, body) = ask(&state, reqwest::Method::GET, &path, None).await?;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/search",
    summary = "Try A Source's Search",
    description = "The hits the MCP tool `search_documents` would answer from this one source (API/05 §4), each with its title, address, publisher, description, dates, language, passage and score; internal passages too, as an internal deployment would read them.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        Tried,
    ),
    responses(
        (status = 200, description = "The hits, best first", body = Object),
        (status = 400, description = "`q` is not 1 to 300 characters or `limit` not 1 to 10", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn try_search(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
    Query(tried): Query<Tried>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let query = query_of(&[
        ("q", Some(tried.q)),
        ("limit", tried.limit.map(|l| l.to_string())),
    ]);
    let path = format!("{project}/knowledge/sources/{source}/search{query}");
    let (_, body) = ask(&state, reqwest::Method::GET, &path, None).await?;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/documents",
    summary = "List A Source's Documents",
    description = "The documents (PDFs) the source's pages link: size, pages, whether on another host, included or who excluded it, passages.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
    ),
    responses(
        (status = 200, description = "The documents", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_documents(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let (_, body) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources/{source}/documents"),
        None,
    )
    .await?;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/errors",
    summary = "List A Source's Crawl Errors",
    description = "The pages and documents whose last read failed, newest first, at most 50: url, kind (`page` or `document`) and reason (`HTTP 404`, a network error). Needs `propose` on KnowledgeSource; a viewer sees the counts alone (API/01 §34, T-4197).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
    ),
    responses(
        (status = 200, description = "The errors", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not propose a KnowledgeSource", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_errors(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    writable(&state, &user, &project)?;
    named("source", &source)?;
    let (_, body) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources/{source}/errors"),
        None,
    )
    .await?;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/pages/{page}/links",
    summary = "List A Page's Links",
    description = "Every link the page carries: to another page of the site, to a document, or off the site.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        ("page" = i64, Path, description = "Page id"),
    ),
    responses(
        (status = 200, description = "The links", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project, source or page", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_links(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source, page)): Path<(String, String, i64)>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let (_, body) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources/{source}/pages/{page}/links"),
        None,
    )
    .await?;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/passages",
    summary = "Preview Passages",
    description = "The passages the assistant answers from, of one page (`?page=`) or one document (`?document=`), in order.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        PassagesOf,
    ),
    responses(
        (status = 200, description = "The passages", body = Object),
        (status = 400, description = "Neither or both of page and document", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_passages(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
    Query(of): Query<PassagesOf>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let query = match (of.page, of.document) {
        (Some(page), None) => format!("page={page}"),
        (None, Some(document)) => format!("document={document}"),
        _ => {
            return Err(ApiError::BadRequest(
                "name one page or one document: ?page= or ?document=".into(),
            ))
        }
    };
    let (_, body) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources/{source}/passages?{query}"),
        None,
    )
    .await?;
    Ok(Json(body))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/inclusion",
    summary = "Include Or Exclude Pages And Documents",
    description = "Excluding removes the passages of what it names at once and holds over every later crawl; including indexes it again at the next crawl. Needs `propose` on KnowledgeSource (AG-113).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
    ),
    request_body(
        content = InclusionRequest,
        description = "At most 500 page and document ids.",
        content_type = "application/json",
        example = json!({ "pages": [12], "documents": [], "subtree": true, "included": false })
    ),
    responses(
        (status = 200, description = "What changed: pages, documents, passagesRemoved", body = Object),
        (status = 400, description = "No id, or more than 500", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not propose a KnowledgeSource", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or crawled source", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn set_inclusion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
    Json(change): Json<InclusionRequest>,
) -> Result<Json<Value>, ApiError> {
    writable(&state, &user, &project)?;
    named("source", &source)?;
    let body = inclusion_body(&change)?;
    let path = format!("{project}/knowledge/sources/{source}/inclusion");
    let (_, counts) = ask(&state, reqwest::Method::POST, &path, Some(body)).await?;
    Ok(Json(counts))
}

/// The inclusion change as the assistant takes it: at least one and at most `MAX_IDS` ids.
fn inclusion_body(change: &InclusionRequest) -> Result<Value, ApiError> {
    let ids = change.pages.len() + change.documents.len();
    if ids == 0 {
        return Err(ApiError::BadRequest(
            "name at least one page or document".into(),
        ));
    }
    if ids > MAX_IDS {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_IDS} pages and documents in one call"
        )));
    }
    serde_json::to_value(change).map_err(|err| ApiError::Internal(err.to_string()))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/recrawl",
    summary = "Recrawl A Source Now",
    description = "Queues a crawl of the source now instead of at its schedule. Needs `propose` on KnowledgeSource (AG-113).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
    ),
    responses(
        (status = 202, description = "The queued job", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not propose a KnowledgeSource", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or declared source", body = crate::error::ProblemDetails),
        (status = 409, description = "A crawl is already queued or running", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn recrawl(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    writable(&state, &user, &project)?;
    named("source", &source)?;
    let (status, job) = ask(
        &state,
        reqwest::Method::POST,
        &format!("{project}/knowledge/sources/{source}/recrawl"),
        None,
    )
    .await?;
    Ok((status, Json(job)))
}

/// The file an upload names (API/01 §34): `jc-assistant` holds the rules for it.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct FileName {
    /// The file's name, its extension saying what it is: 1 to 200 characters, no `/` or `\\`.
    pub name: String,
}

/// The longest an upload may take: 50 MiB sent on and read into passages.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// One upload or removal in the project's activity, beside the crawls it replaces.
async fn record(state: &AppState, project: &str, kind: &str, summary: String, details: Value) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: kind.to_owned(),
        source: "portal".to_owned(),
        summary,
        severity: "info".to_owned(),
        correlation_id: None,
        details,
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, kind, "the knowledge file change was not recorded");
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/files",
    summary = "List A Source's Files",
    description = "The files uploaded to a `source: upload`, newest first, with the passages each was read into (API/01 §34, T-4184).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
    ),
    responses(
        (status = 200, description = "`items`: id, name, mime, bytes, passages, status", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or declared source", body = crate::error::ProblemDetails),
        (status = 409, description = "The source is not a `source: upload`", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_files(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    readable(&state, &user, &project)?;
    named("source", &source)?;
    let (_, files) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/sources/{source}/files"),
        None,
    )
    .await?;
    Ok(Json(files))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/files",
    summary = "Upload A File To A Source",
    description = "Reads the raw body, a PDF, DOCX, XLSX or UTF-8 text file (CSV, Markdown), into the source's passages; the assistant answers from it at once. At most the source's `upload.maxBytes` (50 MiB) and `upload.maxFiles`. Needs `propose` on KnowledgeSource (AG-113, T-4184).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        FileName,
    ),
    request_body(content = String, content_type = "application/octet-stream", description = "The file's bytes", example = json!("Obec,Obyvatelia\nBanská Bystrica,75000\n")),
    responses(
        (status = 201, description = "The file: id, name, mime, bytes, passages, status", body = Object),
        (status = 400, description = "No usable file name", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not propose a KnowledgeSource", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or declared source", body = crate::error::ProblemDetails),
        (status = 409, description = "Not a `source: upload`, or it holds its `maxFiles` already", body = crate::error::ProblemDetails),
        (status = 413, description = "Larger than the source's `upload.maxBytes`", body = crate::error::ProblemDetails),
        (status = 415, description = "Not a PDF, DOCX, XLSX or UTF-8 text file", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn upload_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source)): Path<(String, String)>,
    Query(file): Query<FileName>,
    request: axum::extract::Request,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    writable(&state, &user, &project)?;
    named("source", &source)?;
    // Read only once the caller may write here: a reader cannot make the Portal hold an upload
    // of the largest size a source allows (F-R2POR2-008, as F-POR2-005 at the import).
    let body = axum::body::to_bytes(request.into_body(), MAX_UPLOAD_BODY)
        .await
        .map_err(|_| {
            ApiError::TooLarge(format!(
                "the file is larger than {MAX_UPLOAD_BODY} bytes, the most a source takes"
            ))
        })?;
    let bytes = body.len();
    let name = file.name;
    let query = [("name", name.clone())];
    let (status, added) = ask_with(
        &state,
        reqwest::Method::POST,
        &format!("{project}/knowledge/sources/{source}/files"),
        UPLOAD_TIMEOUT,
        move |request| {
            request
                .query(&query)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
        },
    )
    .await?;
    record(
        &state,
        &project,
        "knowledge.file.uploaded",
        format!("{} uploaded {name} to {source}", user.0.identity.username),
        json!({
            "actor": user.0.identity.username,
            "source": source,
            "name": name,
            "bytes": bytes,
            "id": added.get("id"),
            "passages": added.get("passages"),
        }),
    )
    .await;
    Ok((status, Json(added)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/knowledge/sources/{source}/files/{id}",
    summary = "Remove A File From A Source",
    description = "Removes the uploaded file and its passages at once. Needs `propose` on KnowledgeSource (AG-113, T-4184).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("source" = String, Path, description = "KnowledgeSource name"),
        ("id" = i64, Path, description = "The file's id"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not propose a KnowledgeSource", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project, source or file", body = crate::error::ProblemDetails),
        (status = 409, description = "The source is not a `source: upload`", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn remove_file(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, source, id)): Path<(String, String, i64)>,
) -> Result<StatusCode, ApiError> {
    writable(&state, &user, &project)?;
    named("source", &source)?;
    let path = format!("{project}/knowledge/sources/{source}/files/{id}");
    ask(&state, reqwest::Method::DELETE, &path, None).await?;
    let actor = &user.0.identity.username;
    let message = format!("{actor} removed file {id} from {source}");
    let detail = json!({ "actor": actor, "source": source, "id": id });
    record(&state, &project, "knowledge.file.removed", message, detail).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/deployments/{deployment}/usage",
    summary = "Read An Assistant's Usage",
    description = "Requests and model tokens per day of one AssistantDeployment, the last 30 days.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("deployment" = String, Path, description = "AssistantDeployment name"),
    ),
    responses(
        (status = 200, description = "Usage per day", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or deployment the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn deployment_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, deployment)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !(is_dns1123(&project)
        && effective.may_read_project()
        && effective.may_read("AssistantDeployment"))
    {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    deployed(&state, &project, &deployment)?;
    let (_, body) = ask(
        &state,
        reqwest::Method::GET,
        &format!("{project}/knowledge/deployments/{deployment}/usage"),
        None,
    )
    .await?;
    Ok(Json(body))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/knowledge/deployments/{deployment}/answers",
    summary = "Forget An Assistant's Kept Answers",
    description = "Forgets the answers one AssistantDeployment keeps for repeated questions, so the next question is answered afresh (API/05 §3, T-4731). Needs `approve` on AssistantDeployment; every call is recorded.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("deployment" = String, Path, description = "AssistantDeployment name"),
    ),
    responses(
        (status = 200, description = "forgotten: how many kept answers were dropped", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or deployment", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn forget_answers(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, deployment)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    administers(&state, &user, &project, Verb::Approve)?;
    deployed(&state, &project, &deployment)?;
    let path = format!("{project}/knowledge/deployments/{deployment}/answers");
    let (_, body) = ask(&state, reqwest::Method::DELETE, &path, None).await?;
    let actor = &user.0.identity.username;
    let message = format!("{actor} forgot the kept answers of {deployment}");
    let detail =
        json!({ "actor": actor, "deployment": deployment, "forgotten": body["forgotten"] });
    let kind = "knowledge.answers.forgotten";
    record(&state, &project, kind, message, detail).await;
    Ok(Json(body))
}

/// `404` unless `deployment` names a mirrored AssistantDeployment of `project`.
fn deployed(state: &AppState, project: &str, deployment: &str) -> Result<(), ApiError> {
    named("deployment", deployment)?;
    match state.mirror.get(project, "AssistantDeployment", deployment) {
        Some(_) => Ok(()),
        None => Err(ApiError::NotFound(format!(
            "deployment '{deployment}' not found"
        ))),
    }
}

/// The filter of the stored conversations (API/05 §3.1); `jc-assistant` holds every bound.
#[derive(Debug, Default, Deserialize, serde::Serialize, IntoParams)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    /// RFC 3339, inclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// RFC 3339, exclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// `error`, `timeout` or `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `up`, `down` or `none`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Only conversations with a turn over 15 s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_cost_usd: Option<f64>,
    /// Words of a question or an answer, 2 to 100 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// `lastAt` (default), `costUsd`, `tokens` or `totalMs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 1 to 100, default 50.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The assistant's administrator: `approve` on `AssistantDeployment` (or `delete` to erase), else
/// `403` naming it (API/01 §34, T-4670).
pub(super) fn administers(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    verb: Verb,
) -> Result<(), ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    crate::permissions::for_request(state, &user.0.identity, project).check(
        "AssistantDeployment",
        verb,
        None,
    )
}

/// The assistant's path of one conversation of `project`, whose verb the caller already holds; a
/// conversation id is a UUID, or no conversation of this project.
fn conversation_path(project: &str, conversation: &str) -> Result<String, ApiError> {
    let uuid = conversation.len() == 36
        && conversation.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    if !uuid {
        return Err(ApiError::NotFound(format!(
            "conversation '{conversation}' not found"
        )));
    }
    Ok(format!("{project}/knowledge/conversations/{conversation}"))
}

/// The calls one person may make on the stored conversations in a minute (API/01 §34, T-4681).
const TRANSCRIPT_CALLS_PER_MINUTE: usize = 60;
/// The `404`s of one person in five minutes past which their probing is recorded (T-4681).
const TRANSCRIPT_MISSES: usize = 20;

/// How many of `key`'s times in `window` fall within `span` of `now`, `now` counted.
// ponytail: one window per replica, in memory, like the feedback route's; a shared counter if
// the Portal runs several.
pub(crate) fn counted(
    window: &OnceLock<Mutex<HashMap<String, Vec<Instant>>>>,
    key: &str,
    now: Instant,
    span: Duration,
) -> usize {
    let mut all = window
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let times = within_span(&mut all, key, now, span);
    times.push(now);
    times.len()
}

/// `key`'s times of `all` within `span` of `now`, every key with none left dropped first: a
/// window keeps the callers of the last `span` alone (F-POR4-030). The feedback limit shares it.
pub(crate) fn within_span<'a>(
    all: &'a mut HashMap<String, Vec<Instant>>,
    key: &str,
    now: Instant,
    span: Duration,
) -> &'a mut Vec<Instant> {
    all.retain(|_, times| {
        times
            .last()
            .is_some_and(|at| now.duration_since(*at) < span)
    });
    let times = all.entry(key.to_owned()).or_default();
    times.retain(|at| now.duration_since(*at) < span);
    times
}

/// `429` once `subject` made [`TRANSCRIPT_CALLS_PER_MINUTE`] calls on the transcripts this minute.
pub(super) fn admitted(subject: &str, now: Instant) -> Result<(), ApiError> {
    static CALLS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    if counted(&CALLS, subject, now, Duration::from_secs(60)) > TRANSCRIPT_CALLS_PER_MINUTE {
        return Err(ApiError::TooManyRequests(format!(
            "{TRANSCRIPT_CALLS_PER_MINUTE} calls on the stored conversations in a minute already; try again in a minute"
        )));
    }
    Ok(())
}

/// Whether this miss of `subject` is the one past [`TRANSCRIPT_MISSES`] in five minutes, which is
/// recorded once as a probe (T-4681).
fn probing(subject: &str, now: Instant) -> bool {
    static MISSES: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    counted(&MISSES, subject, now, Duration::from_secs(300)) == TRANSCRIPT_MISSES + 1
}

/// `result` as it is, a `404` counted toward [`probing`] and recorded when it is one.
pub(super) async fn missed<T>(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    result: Result<T, ApiError>,
) -> Result<T, ApiError> {
    if matches!(result, Err(ApiError::NotFound(_)))
        && probing(&user.0.identity.subject, Instant::now())
    {
        audited(
            state,
            user,
            project,
            "probe",
            json!({"misses": TRANSCRIPT_MISSES + 1, "minutes": 5}),
        )
        .await;
    }
    result
}

/// Who read or erased which transcripts (T-4670). A record that cannot be written is logged, not
/// a failure of the answer.
pub(super) async fn audited(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    what: &str,
    details: Value,
) {
    let actor = &user.0.identity.username;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "assistant.transcript".to_owned(),
        source: "portal".to_owned(),
        summary: format!("{actor} {what}"),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "actor": actor, "action": what, "of": details }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the transcript access was not recorded");
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/conversations",
    summary = "List Stored Conversations",
    description = "The project's stored assistant conversations, redacted, newest first or the most expensive first, with the cursor of the next page (API/05 §3.1). Needs `approve` on AssistantDeployment; every call is recorded (API/01 §34, AG-99).",
    tag = "knowledge",
    params(("project" = String, Path, description = "Project name"), ConversationFilter),
    responses(
        (status = 200, description = "items and nextCursor", body = Object),
        (status = 400, description = "A filter value out of its bounds", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_conversations(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(filter): Query<ConversationFilter>,
) -> Result<Json<Value>, ApiError> {
    administers(&state, &user, &project, Verb::Approve)?;
    admitted(&user.0.identity.subject, Instant::now())?;
    let path = format!("{project}/knowledge/conversations");
    let query = |r: reqwest::RequestBuilder| r.query(&filter);
    let (_, body) = ask_with(&state, reqwest::Method::GET, &path, LIST_TIME, query).await?;
    let what = "listed the stored conversations";
    audited(&state, &user, &project, what, json!(filter)).await;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/conversations/{conversation}",
    summary = "Read A Stored Conversation",
    description = "One stored conversation, every turn with its steps: route, searches, tool calls, model calls with tokens and cost, redacted (API/05 §3.1). Needs `approve` on AssistantDeployment; every call is recorded.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("conversation" = String, Path, description = "Conversation id (UUID)"),
    ),
    responses(
        (status = 200, description = "The conversation", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 404, description = "No such stored conversation", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_conversation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, conversation)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let call = (Verb::Approve, reqwest::Method::GET, Some(""));
    let what = ("read a stored conversation", json!(conversation));
    transcript_call(&state, &user, (&project, &conversation), call, what)
        .await
        .map(Json)
}

/// One call on a stored conversation, as each of these routes makes it: the verb on
/// AssistantDeployment, the per-person limit, the conversation's own path (`suffix` under it;
/// `None`, a turn that cannot be stored, is a `404` the assistant is never asked),
/// a `404` counted towards a probe, and the audit record of `what` once it answered.
async fn transcript_call(
    state: &AppState,
    user: &CurrentUser,
    (project, conversation): (&str, &str),
    (verb, method, suffix): (Verb, reqwest::Method, Option<&str>),
    (what, details): (&str, Value),
) -> Result<Value, ApiError> {
    administers(state, user, project, verb)?;
    admitted(&user.0.identity.subject, Instant::now())?;
    let call = async {
        let path = conversation_path(project, conversation)?;
        let suffix = suffix.ok_or_else(|| ApiError::NotFound("no such stored turn".into()))?;
        ask(state, method, &format!("{path}{suffix}"), None).await
    };
    let (_, body) = missed(state, user, project, call.await).await?;
    audited(state, user, project, what, details).await;
    Ok(body)
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/knowledge/conversations/{conversation}",
    summary = "Erase A Stored Conversation",
    description = "Erases one stored conversation with its steps, for a visitor's request (API/05 §3.1). Needs `delete` on AssistantDeployment; every call is recorded.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("conversation" = String, Path, description = "Conversation id (UUID)"),
    ),
    responses(
        (status = 204, description = "Erased"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not delete an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 404, description = "No such stored conversation", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn erase_conversation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, conversation)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let call = (Verb::Delete, reqwest::Method::DELETE, Some(""));
    let what = ("erased a stored conversation", json!(conversation));
    transcript_call(&state, &user, (&project, &conversation), call, what).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same",
    summary = "List The Turns Of The Same Question",
    description = "The project's stored turns whose question reads as this turn's (case, diacritics and spacing ignored), newest first, at most 50, for comparing two answers to one question (API/05 §3.1, T-4681). Needs `approve` on AssistantDeployment; every call is recorded.",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("conversation" = String, Path, description = "Conversation id (UUID)"),
        ("turn" = u32, Path, description = "Turn number, 1 to 200"),
    ),
    responses(
        (status = 200, description = "items: conversation, turn, at, deployment, totalMs, costUsd, steps", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 404, description = "No such stored turn", body = crate::error::ProblemDetails),
        (status = 429, description = "60 calls on the stored conversations in a minute already", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn same_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, conversation, turn)): Path<(String, String, u32)>,
) -> Result<Json<Value>, ApiError> {
    let suffix = format!("/turns/{turn}/same");
    let stored = (1..=200).contains(&turn).then_some(suffix.as_str());
    let call = (Verb::Approve, reqwest::Method::GET, stored);
    let what = (
        "listed the turns of one question",
        json!({"conversation": conversation, "turn": turn}),
    );
    transcript_call(&state, &user, (&project, &conversation), call, what)
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/knowledge/deployments/{deployment}/chat",
    summary = "Ask an Assistant",
    description = "One question to the project's AssistantDeployment as the signed-in person, answered as the Server-Sent Events of API/05 §1.3. An internal deployment reads its internal passages and the organization's Endpoints with the person's own token, which the Portal passes on and keeps no copy of; any other channel answers as it answers a visitor (API/05 §1.7, AG-115).",
    tag = "knowledge",
    params(
        ("project" = String, Path, description = "Project name"),
        ("deployment" = String, Path, description = "AssistantDeployment name"),
    ),
    request_body(
        content = ChatRequest,
        description = "API/05 §1.1: the question, the last turns the chat holds, the connectors switched on.",
        content_type = "application/json",
        example = json!({
            "message": "Where are the air quality stations?",
            "history": [{ "role": "user", "text": "Hello" }, { "role": "assistant", "text": "Hello! Ask me about the city's data." }],
            "connectors": ["helsinki-weather"]
        })
    ),
    responses(
        (status = 200, description = "The answer, as Server-Sent Events", content_type = "text/event-stream", body = String),
        (status = 400, description = "The question breaks a bound of API/05 §1.1", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project or deployment the caller may read", body = crate::error::ProblemDetails),
        (status = 409, description = "The deployment has no budget yet", body = crate::error::ProblemDetails),
        (status = 429, description = "The deployment's per-minute limit is reached", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn chat(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, deployment)): Path<(String, String)>,
    headers: HeaderMap,
    Json(question): Json<ChatRequest>,
) -> Result<Response, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !(is_dns1123(&project)
        && effective.may_read_project()
        && effective.may_read("AssistantDeployment"))
    {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    deployed(&state, &project, &deployment)?;
    let path = format!("{project}/knowledge/deployments/{deployment}/chat");
    let mut request = request(&state, reqwest::Method::POST, &path, CHAT_TIMEOUT)
        .await?
        .header("x-jc-person", &user.0.identity.username)
        .json(&question);
    // The person's own token, for an internal deployment's connectors (AG-115): passed on,
    // never kept or logged here.
    if let Some(person) =
        crate::agents::identity::persons_token(&headers, state.config.trust_edge_token)
    {
        request = request.header("x-jc-person-token", person);
    }
    let answer = sent(request).await?;
    // The events as they come, so the person sees each tool and the answer when it is there.
    let events = futures_util::stream::unfold(Some(answer), |answer| async move {
        let mut answer = answer?;
        match answer.chunk().await {
            Ok(Some(bytes)) => Some((Ok::<_, std::io::Error>(bytes), Some(answer))),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err.without_url(), "the assistant's answer broke off");
                Some((Err(std::io::Error::other("the answer broke off")), None))
            }
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        [("x-accel-buffering", "no")],
        Body::from_stream(events),
    )
        .into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/knowledge/sources", get(list_sources))
        .route(
            "/projects/{project}/knowledge/sources/{source}/pages",
            get(list_pages),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/documents",
            get(list_documents),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/search",
            get(try_search),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/errors",
            get(list_errors),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/pages/{page}/links",
            get(list_links),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/passages",
            get(list_passages),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/inclusion",
            post(set_inclusion),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/recrawl",
            post(recrawl),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/files",
            get(list_files)
                .post(upload_file)
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BODY)),
        )
        .route(
            "/projects/{project}/knowledge/sources/{source}/files/{id}",
            delete(remove_file),
        )
        .route(
            "/projects/{project}/knowledge/deployments/context",
            post(crate::api::assistant_context::preview_context),
        )
        .route(
            "/projects/{project}/knowledge/deployments/{deployment}/context",
            get(crate::api::assistant_context::deployment_context),
        )
        .route(
            "/projects/{project}/knowledge/deployments/{deployment}/usage",
            get(deployment_usage),
        )
        .route(
            "/projects/{project}/knowledge/deployments/{deployment}/answers",
            delete(forget_answers),
        )
        .route(
            "/projects/{project}/knowledge/deployments/{deployment}/chat",
            post(chat).layer(DefaultBodyLimit::max(MAX_CHAT_BODY)),
        )
        .route(
            "/projects/{project}/knowledge/conversations",
            get(list_conversations),
        )
        .route(
            "/projects/{project}/knowledge/conversations/{conversation}",
            get(get_conversation).delete(erase_conversation),
        )
        .route(
            "/projects/{project}/knowledge/conversations/{conversation}/turns/{turn}/same",
            get(same_question),
        )
}

#[cfg(test)]
mod tests {
    use super::{admitted, inclusion_body, probing, query_of, InclusionRequest, MAX_IDS};
    use super::{TRANSCRIPT_CALLS_PER_MINUTE, TRANSCRIPT_MISSES};
    use std::time::{Duration, Instant};

    /// F-R2POR2-008 (T-6226): who may not write to the project's knowledge sources is refused
    /// before a byte of the upload is read, so a reader cannot make the Portal hold one.
    #[tokio::test]
    async fn an_upload_by_a_caller_who_may_not_write_is_refused_before_its_body_is_read() {
        use crate::auth::session::{Identity, Session};
        use axum::extract::{Path, Query, State};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let read = Arc::new(AtomicBool::new(false));
        let flag = read.clone();
        let body = axum::body::Body::from_stream(futures_util::stream::poll_fn(move |_| {
            flag.store(true, Ordering::SeqCst);
            std::task::Poll::Ready(None::<Result<axum::body::Bytes, std::io::Error>>)
        }));
        let request = axum::http::Request::post("/").body(body).expect("request");
        let user = crate::auth::CurrentUser(Session {
            identity: Identity {
                client: None,
                subject: "sub-viewer".into(),
                username: "viewer@hel.fi".into(),
                email: Some("viewer@hel.fi".into()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        });
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let refused = super::upload_file(
            user,
            State(state),
            Path(("helsinki".to_owned(), "docs".to_owned())),
            Query(super::FileName {
                name: "a.txt".into(),
            }),
            request,
        )
        .await;
        assert!(refused.is_err(), "a caller with no grant uploaded");
        assert!(
            !read.load(Ordering::SeqCst),
            "the body was read before the refusal"
        );
    }

    /// F-POR4-030, F-R2POR2-017 (T-6226): a window drops every caller with no time left in its
    /// span, so the feedback limit's map no longer keeps a key per person who ever sent one.
    #[test]
    fn a_window_drops_the_callers_whose_times_all_passed() {
        let start = Instant::now();
        let hour = Duration::from_secs(3600);
        let mut all = std::collections::HashMap::new();
        super::within_span(&mut all, "ana", start, hour).push(start);
        let later = start + hour + Duration::from_secs(1);
        assert!(super::within_span(&mut all, "bob", later, hour).is_empty());
        assert_eq!(all.keys().collect::<Vec<_>>(), ["bob"]);
    }

    #[test]
    fn the_sixty_first_call_in_a_minute_is_refused_and_the_next_minute_admits() {
        let start = Instant::now();
        let who = "transcript-rate-subject";
        for _ in 0..TRANSCRIPT_CALLS_PER_MINUTE {
            assert!(admitted(who, start).is_ok());
        }
        assert!(matches!(
            admitted(who, start),
            Err(crate::error::ApiError::TooManyRequests(_))
        ));
        assert!(admitted(who, start + Duration::from_secs(61)).is_ok());
        assert!(admitted("transcript-rate-other", start).is_ok());
    }

    #[test]
    fn the_twenty_first_miss_in_five_minutes_is_a_probe_once() {
        let start = Instant::now();
        let who = "transcript-probe-subject";
        let seen: Vec<bool> = (0..TRANSCRIPT_MISSES + 3)
            .map(|_| probing(who, start))
            .collect();
        assert_eq!(seen.iter().filter(|p| **p).count(), 1);
        assert!(seen[TRANSCRIPT_MISSES]);
        assert!(
            !probing(who, start + Duration::from_secs(301)),
            "a new window starts again"
        );
    }

    #[test]
    fn a_query_names_the_pairs_with_a_value_encoded_and_none_is_empty() {
        assert_eq!(query_of(&[("parent", None), ("q", None)]), "");
        assert_eq!(
            query_of(&[("parent", Some("12".into())), ("q", None)]),
            "?parent=12"
        );
        assert_eq!(
            query_of(&[
                ("q", Some("Kirjasto & lainaus?".into())),
                ("limit", Some("3".into()))
            ]),
            "?q=Kirjasto+%26+lainaus%3F&limit=3"
        );
    }

    #[test]
    fn an_inclusion_names_one_to_max_ids() {
        let change = |pages: usize| InclusionRequest {
            pages: (0..pages as i64).collect(),
            documents: vec![],
            subtree: false,
            included: false,
        };
        assert!(inclusion_body(&change(0)).is_err());
        assert_eq!(
            inclusion_body(&change(1)).unwrap()["pages"],
            serde_json::json!([0])
        );
        assert!(inclusion_body(&change(MAX_IDS)).is_ok());
        assert!(inclusion_body(&change(MAX_IDS + 1)).is_err());
    }
}
