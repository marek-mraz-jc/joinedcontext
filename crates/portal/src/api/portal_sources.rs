//! The sources of one open data portal (ODP-15, T-6883): test a source's connection, run it now,
//! list its last runs, and list the portal's datasets, under
//! `/api/v1/projects/org/portals/{name}/...`.
//!
//! The Portal holds no handle on the catalogue store: the host that serves the catalogues (jc)
//! hands it a [`SourceDesk`] at start, which runs each call as that portal's catalogue's own
//! principal. Every route reads the `Portal` manifest first: one of another Organization, or one
//! that does not exist, is the same 404; a member of its Organization without the right is 403.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::{PortalSourceType, PortalSpec, Verb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::auth::CurrentUser;
use crate::auth::session::Identity;
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// A boxed future of a [`SourceDesk`] call.
pub type DeskFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DeskError>> + Send + 'a>>;

/// Why a desk call did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskError {
    /// The portal's catalogue or the source is not served here.
    NotFound,
    /// A run of the source is active.
    Busy,
    /// The catalogue refused or failed; the detail is for the log, never the answer.
    Failed(String),
}

/// The catalogue side of a portal's sources, run as the portal's catalogue's own principal.
pub trait SourceDesk: Send + Sync {
    /// One guarded read of `source`'s first page: `ok`, `unreachable`, `refused host`,
    /// `not the expected format` or `too large`.
    fn test<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, String>;
    /// Starts a run of `source` and answers at once; [`DeskError::Busy`] while one is active.
    fn start<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, ()>;
    /// The last runs of `source`, newest first, at most 50.
    fn runs<'a>(&'a self, portal: &'a PortalSpec, source: &'a str) -> DeskFuture<'a, Vec<Value>>;
    /// Every dataset of the portal's catalogue, by name.
    fn datasets<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, Vec<DeskDataset>>;
}

/// One dataset of a portal's catalogue, as the desk reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskDataset {
    pub name: String,
    pub title: String,
    /// The harvest source that wrote it; `None` for an Endpoint listed in it.
    pub source: Option<String>,
}

static DESK: RwLock<Option<Arc<dyn SourceDesk>>> = RwLock::new(None);

/// Hands the catalogues' desk to the Portal from now on. One Portal runs per process; a host that
/// starts a new one replaces the desk of the stopped one.
pub fn hand_desk_to(desk: Arc<dyn SourceDesk>) {
    *DESK.write().unwrap_or_else(|e| e.into_inner()) = Some(desk);
}

fn desk() -> Result<Arc<dyn SourceDesk>, ApiError> {
    DESK.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| {
            ApiError::Unavailable(
                "this installation serves no open data catalogue, so a source cannot be tested \
                 or run here"
                    .into(),
            )
        })
}

/// Tests one person may start in a minute, and at once (Decided, supervisor: constants per
/// deployment).
const TESTS_PER_MINUTE: f64 = 6.0;
const TEST_BURST: f64 = 3.0;
/// Most buckets held: past it the idle ones go.
const MAX_BUCKETS: usize = 10_000;

struct Bucket {
    tokens: f64,
    at: Instant,
}

// ponytail: one replica's memory, so N Portal replicas allow N times the rate; jc runs one.
static TEST_BUCKETS: LazyLock<Mutex<HashMap<String, Bucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `None` when `person` may test now, else the seconds until the next test is allowed.
fn take_test(person: &str, now: Instant) -> Option<u64> {
    let mut map = TEST_BUCKETS.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= MAX_BUCKETS && !map.contains_key(person) {
        map.retain(|_, b| now.duration_since(b.at).as_secs() < 60);
    }
    let bucket = map.entry(person.to_owned()).or_insert(Bucket {
        tokens: TEST_BURST,
        at: now,
    });
    let rate = TESTS_PER_MINUTE / 60.0;
    bucket.tokens = (bucket.tokens + now.duration_since(bucket.at).as_secs_f64() * rate)
        .min(TEST_BURST);
    bucket.at = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        None
    } else {
        Some(((1.0 - bucket.tokens) / rate).ceil() as u64)
    }
}

/// The Portal manifest `name` and its spec, when the caller belongs to its Organization and holds
/// `verb` on it: 404 for one of another Organization or none, 403 for a member without the right.
fn portal_for(
    state: &AppState,
    identity: &Identity,
    name: &str,
    verb: Verb,
) -> Result<PortalSpec, ApiError> {
    let not_found = || ApiError::NotFound(format!("portal '{name}' not found"));
    let envelope = state
        .mirror
        .get(ORG_NAMESPACE, "Portal", name)
        .ok_or_else(not_found)?;
    let spec: PortalSpec =
        serde_json::from_value(envelope.spec.clone()).map_err(|_| not_found())?;
    if !crate::permissions::belongs_to_organization(
        &state.mirror,
        &state.config.bootstrap_admins,
        identity,
        &spec.organization_ref,
    ) {
        return Err(not_found());
    }
    let effective = crate::permissions::for_request(state, identity, ORG_NAMESPACE);
    let allowed = match verb {
        Verb::Read => effective.may_read_envelope("Portal", &envelope),
        _ => {
            let manifest = serde_json::to_value(&envelope).map_err(|_| not_found())?;
            effective.check("Portal", verb, Some(&manifest)).is_ok()
        }
    };
    if !allowed {
        return Err(ApiError::Denied(format!(
            "{} on Portal '{name}' is for its Organization's administrators (ODP-12)",
            crate::permissions::verb_name(verb)
        )));
    }
    Ok(spec)
}

/// The harvested source `source` of `spec`: 404 for a name it lacks, 422 for an `endpoints` source,
/// which lists published Endpoints and reads no remote address.
fn harvested<'a>(spec: &'a PortalSpec, name: &str, source: &str) -> Result<&'a str, ApiError> {
    let found = spec
        .sources
        .iter()
        .find(|s| s.name == source)
        .ok_or_else(|| ApiError::NotFound(format!("portal '{name}' has no source '{source}'")))?;
    if found.kind == PortalSourceType::Endpoints {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "source '{source}' lists this Organization's published Endpoints; it reads no \
                 remote address, so there is nothing to test or run"
            ),
            errors: vec!["source".into()],
        });
    }
    Ok(found.name.as_str())
}

fn desk_error(e: DeskError, source: &str) -> ApiError {
    match e {
        DeskError::NotFound => ApiError::NotFound(format!(
            "source '{source}' is not served yet; the catalogue takes an approved change within a \
             minute, try again then"
        )),
        DeskError::Busy => ApiError::Conflict(format!(
            "a run of source '{source}' is active; wait for it to finish (see its runs)"
        )),
        DeskError::Failed(detail) => {
            tracing::warn!(source = %source, error = %detail, "portal source call failed");
            ApiError::Internal("the open data catalogue did not answer; try again later".into())
        }
    }
}

/// What a source test found.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceTestAnswer {
    /// One word: `ok`, `unreachable`, `refused host`, `not the expected format` or `too large`.
    pub outcome: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/org/portals/{name}/sources/{source}/test",
    summary = "Test Portal Source",
    description = "Reads the source's first page once through the harvest guard (https, allowed hosts, public addresses only, every redirect hop checked), at most its byte budget and 10 s, and writes nothing (ODP-15). The answer is one word; no remote address, status, header or body is echoed. Limited to 6 tests a minute per person, 3 at once.",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("source" = String, Path, description = "A `spec.sources[].name` of it, e.g. `kosice`")
    ),
    responses(
        (status = 200, description = "The outcome, one word", body = SourceTestAnswer),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal or source in the caller's Organization", body = ProblemDetails),
        (status = 422, description = "An `endpoints` source, which reads no remote address", body = ProblemDetails),
        (status = 429, description = "More than 6 tests a minute, or 3 at once, by this person", body = ProblemDetails),
        (status = 503, description = "This installation serves no open data catalogue", body = ProblemDetails)
    )
)]
pub async fn test_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, source)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    let spec = portal_for(&state, identity, &name, Verb::Propose)?;
    let source = harvested(&spec, &name, &source)?.to_owned();
    let desk = desk()?;
    if let Some(wait) = take_test(&identity.subject, Instant::now()) {
        let mut answer = ApiError::TooManyRequests(format!(
            "6 source tests a minute, 3 at once; try again in {wait} s"
        ))
        .into_response();
        if let Ok(value) = wait.to_string().parse() {
            answer.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return Ok(answer);
    }
    let outcome = desk
        .test(&spec, &source)
        .await
        .map_err(|e| desk_error(e, &source))?;
    Ok(Json(SourceTestAnswer { outcome }).into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/org/portals/{name}/sources/{source}/runs",
    summary = "Run Portal Source Now",
    description = "Starts a harvest run of the source now (ODP-15); its counts and error word appear in the source's runs when it ends.",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("source" = String, Path, description = "A `spec.sources[].name` of it, e.g. `kosice`")
    ),
    responses(
        (status = 202, description = "The run started"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization without propose on it", body = ProblemDetails),
        (status = 404, description = "No such portal or source in the caller's Organization", body = ProblemDetails),
        (status = 409, description = "A run of this source is active", body = ProblemDetails),
        (status = 422, description = "An `endpoints` source, which reads no remote address", body = ProblemDetails),
        (status = 503, description = "This installation serves no open data catalogue", body = ProblemDetails)
    )
)]
pub async fn run_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, source)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Propose)?;
    let source = harvested(&spec, &name, &source)?.to_owned();
    desk()?
        .start(&spec, &source)
        .await
        .map_err(|e| desk_error(e, &source))?;
    tracing::info!(portal = %name, source = %source, by = %user.0.identity.subject, "portal source run started");
    Ok(StatusCode::ACCEPTED)
}

/// A source's last runs, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceRuns {
    /// At most 50: `id`, `state`, `startedAt`, `finishedAt`, the `created`, `updated`,
    /// `unchanged`, `withdrawn` and `skipped` counts, and `error`, one word.
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<Value>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/org/portals/{name}/sources/{source}/runs",
    summary = "List Portal Source Runs",
    description = "The source's last harvest runs, newest first, at most 50, each with its counts and one error word (ODP-15).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        ("source" = String, Path, description = "A `spec.sources[].name` of it, e.g. `kosice`")
    ),
    responses(
        (status = 200, description = "The runs, newest first", body = SourceRuns),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization who may not read it", body = ProblemDetails),
        (status = 404, description = "No such portal or source in the caller's Organization", body = ProblemDetails),
        (status = 503, description = "This installation serves no open data catalogue", body = ProblemDetails)
    )
)]
pub async fn list_runs(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((name, source)): Path<(String, String)>,
) -> Result<Json<SourceRuns>, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Read)?;
    let source = harvested(&spec, &name, &source)?.to_owned();
    let items = desk()?
        .runs(&spec, &source)
        .await
        .map_err(|e| desk_error(e, &source))?;
    Ok(Json(SourceRuns { items }))
}

/// A page of a portal's datasets.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DatasetPage {
    /// Datasets to skip, e.g. `50`.
    #[serde(default)]
    pub offset: usize,
    /// Datasets in the page, 1 to 200, e.g. `50` (the default).
    pub limit: Option<usize>,
}

/// One dataset of a portal.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct PortalDataset {
    /// Its name, e.g. `ovzdusie-kosice`.
    pub name: String,
    /// Its title in the catalogue's first language.
    pub title: String,
    /// The source that wrote it: a harvest source's name, or `endpoints`.
    pub source: String,
    /// Whether `spec.hidden` of the portal names it.
    pub hidden: bool,
}

/// A page of a portal's datasets, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct PortalDatasets {
    pub total: usize,
    pub items: Vec<PortalDataset>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/org/portals/{name}/datasets",
    summary = "List Portal Datasets",
    description = "The datasets of the portal's catalogue by name, each with the source that wrote it and whether the portal hides it (ODP-15).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The Portal manifest's name, e.g. `bbsk-skoly`"),
        DatasetPage
    ),
    responses(
        (status = 200, description = "A page of datasets", body = PortalDatasets),
        (status = 400, description = "A limit outside 1 to 200", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A member of the portal's Organization who may not read it", body = ProblemDetails),
        (status = 404, description = "No such portal in the caller's Organization", body = ProblemDetails),
        (status = 503, description = "This installation serves no open data catalogue", body = ProblemDetails)
    )
)]
pub async fn list_datasets(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
    page: Result<Query<DatasetPage>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<PortalDatasets>, ApiError> {
    let spec = portal_for(&state, &user.0.identity, &name, Verb::Read)?;
    let Query(page) = page.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let limit = page.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "limit must be 1 to 200, e.g. 50; got {limit}"
        )));
    }
    let all = desk()?
        .datasets(&spec)
        .await
        .map_err(|e| desk_error(e, "datasets"))?;
    let total = all.len();
    let items = all
        .into_iter()
        .skip(page.offset)
        .take(limit)
        .map(|d| PortalDataset {
            hidden: spec.hidden.contains(&d.name),
            source: d.source.unwrap_or_else(|| "endpoints".into()),
            name: d.name,
            title: d.title,
        })
        .collect();
    Ok(Json(PortalDatasets { total, items }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/org/portals/{name}/sources/{source}/test",
            post(test_source),
        )
        .route(
            "/projects/org/portals/{name}/sources/{source}/runs",
            post(run_source).get(list_runs),
        )
        .route("/projects/org/portals/{name}/datasets", get(list_datasets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_person_tests_three_at_once_and_six_a_minute() {
        let start = Instant::now();
        let person = "sub-bucket-test";
        for _ in 0..3 {
            assert_eq!(take_test(person, start), None);
        }
        assert_eq!(take_test(person, start), Some(10));
        assert_eq!(take_test("sub-someone-else", start), None, "per person");
        assert_eq!(take_test(person, start + Duration::from_secs(10)), None);
        assert!(take_test(person, start + Duration::from_secs(10)).is_some());
    }
}
