//! A data subject's rights in a space (API/01 §46, DM-124, DM-126, DM-127, T-4668): the export of
//! every Entity naming the subject with its history, and the erasure that reaches every copy.
//!
//! The space's owner runs them (`approve` on its ContextSpace). The subject's value travels in the
//! request body, never in a URL an access log keeps; the log holds its keyed hash only. Every
//! call needs a `reason` and is audited with it.
//!
//! An erasure takes four eyes (DM-142, T-5356): the request is a proposal, and another person with
//! `approve` on the ContextSpace approves it by typing the subject again, or either owner declines
//! it. A bearer token never decides one.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::dry_run::{is_dry_run, DryRunQuery};
use crate::auth::CurrentUser;
use crate::erasure::{self, Group, SubjectClass};
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// A request about one data subject.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectRequest {
    /// The value of the class's `dataSubjectRef` slot.
    pub subject: String,
    /// Why, for the audit record: the request's reference, the legal ground.
    pub reason: String,
}

/// The approver's second look: the subject typed again, never kept.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproveRequest {
    /// The value of the class's `dataSubjectRef` slot, as the proposal named it.
    pub subject: String,
}

/// Why the proposal is declined, for the audit record.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclineRequest {
    pub reason: String,
}

/// The space's spec when the caller owns it; the 404 every unreadable space gets otherwise.
fn owned(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<Value, ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    let held = (is_dns1123(project) && is_dns1123(space))
        .then(|| state.mirror.get(project, "ContextSpace", space))
        .flatten()
        .filter(|_| {
            effective.may_read_project() && effective.may_read_in("ContextSpace", Some(space))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "context space '{space}' not found in project '{project}'"
            ))
        })?;
    if !effective.may_in("ContextSpace", Verb::Approve, Some(space)) {
        return Err(ApiError::Denied(format!(
            "only an owner of context space '{space}' runs a data subject's export and erasure (DM-124)"
        )));
    }
    Ok(held.spec)
}

/// The classes of the space's model that name a data subject.
async fn subject_classes(
    state: &AppState,
    (project, space): (&str, &str),
    spec: &Value,
) -> Result<Vec<SubjectClass>, ApiError> {
    let model = crate::resource::reference_name(&spec["dataModelRef"])
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::BadRequest(format!("context space '{space}' has no data model"))
        })?;
    let linkml = crate::api::datamodels::read_source(state, project, &model).await?;
    let classes = erasure::subject_classes(&linkml);
    if classes.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "the model '{model}' names no data subject: a class needs the annotation dataSubjectRef (Architecture/11 §9)"
        )));
    }
    Ok(classes)
}

fn checked(body: Result<Json<SubjectRequest>, JsonRejection>) -> Result<SubjectRequest, ApiError> {
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    reason_checked(&request.reason)?;
    if let Some(why) = erasure::subject_refusal(&request.subject) {
        return Err(ApiError::BadRequest(why));
    }
    Ok(request)
}

fn reason_checked(reason: &str) -> Result<(), ApiError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > erasure::MAX_REASON {
        return Err(ApiError::BadRequest(format!(
            "reason is required, at most {} characters (reason-required)",
            erasure::MAX_REASON
        )));
    }
    Ok(())
}

/// A person decides an erasure with their own Portal session; a service account's or an agent's
/// token never does, whatever its grants (DM-142, AG-133).
fn refuse_bearer(user: &CurrentUser) -> Result<(), ApiError> {
    if user.0.identity.client.is_some() {
        return Err(ApiError::Denied(
            "an erasure is approved or declined by a person in the Portal, never with a service \
             account's or an agent's token (DM-142, AG-133)"
                .into(),
        ));
    }
    Ok(())
}

fn erasure_log(state: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal keeps no erasure log (no database)".into())
    })
}

fn log_unread(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "the erasure log was not read");
    ApiError::Unavailable("the erasure log could not be read".into())
}

/// One erasure of the space, or the 404 an unknown or misshapen id gets.
async fn held(
    pool: &sqlx::PgPool,
    (project, space, id): (&str, &str, &str),
) -> Result<erasure::Erasure, ApiError> {
    let gone = || {
        ApiError::NotFound(format!(
            "erasure '{id}' not found in context space '{space}'"
        ))
    };
    if !erasure::is_id(id) {
        return Err(gone());
    }
    erasure::get(pool, project, space, id)
        .await
        .map_err(log_unread)?
        .ok_or_else(gone)
}

/// The 409 for a decision on an erasure that is no longer a proposal.
fn not_proposed(found: &erasure::Erasure) -> ApiError {
    ApiError::Conflict(format!(
        "erasure {} is {}: only a proposal is approved or declined (DM-142)",
        found.id,
        found.state()
    ))
}

fn actor_of(user: &CurrentUser) -> String {
    format!("user:{}", user.0.identity.username)
}

/// The Entities of each class naming the subject, read through the space surface as the
/// Portal's own account.
async fn named(
    state: &AppState,
    project: &str,
    space: &str,
    classes: &[SubjectClass],
    subject: &str,
) -> Result<(String, Vec<(SubjectClass, Vec<Value>)>), ApiError> {
    let queries = erasure::subject_queries(
        classes,
        subject,
        state.personal_keys.of(project, space).as_ref(),
    )
    .map_err(ApiError::Conflict)?;
    let watch = state.drift_watch.as_ref().ok_or_else(no_surface)?;
    let unavailable = |reason: String| {
        tracing::warn!(%reason, "a space was not read for a data subject");
        ApiError::Unavailable("the space did not answer; try again shortly".into())
    };
    let token = watch.scan_token().await.map_err(unavailable)?;
    let segment = crate::spaces::segment(&state.mirror, project, space);
    let mut found = Vec::new();
    for (class, q) in classes.iter().zip(queries) {
        let entities = watch
            .named(&token, &segment, &class.class, &q)
            .await
            .map_err(unavailable)?;
        found.push((class.clone(), entities));
    }
    Ok((token, found))
}

fn no_surface() -> ApiError {
    ApiError::Unavailable("this Portal reads no space surface (no gateway or realm client)".into())
}

async fn audited(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
    what: &str,
    details: Value,
) {
    let actor = &user.0.identity.username;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: Some(space.to_owned()),
        kind: "personal-data".to_owned(),
        source: "portal".to_owned(),
        summary: format!("{actor} {what}"),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "actor": actor, "action": what, "of": details }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "a data subject's request was not recorded");
    }
}

/// The erasure key, else the artifact store's root key the earlier hashes were made with.
fn subject_key(config: &crate::config::Config) -> Result<&str, ApiError> {
    config
        .erasure_key
        .as_deref()
        .or_else(|| {
            config
                .artifact_store
                .as_ref()
                .map(|settings| settings.root_secret_key.as_str())
        })
        .ok_or_else(|| {
            ApiError::Unavailable(
                "this Portal holds no key to hash a data subject with (JC_PORTAL_ERASURE_KEY)"
                    .into(),
            )
        })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/subjects/export",
    summary = "Export a Data Subject",
    description = "Every Entity of the space naming the data subject in its class's `dataSubjectRef` slot, current state and stored history, answered to the caller's session alone (DM-124). The subject is in the body, never the URL. For the space's owners; audited with the reason.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(content = SubjectRequest, example = json!({"subject": "r-42", "reason": "Art. 15 request 2026-10-19-07"})),
    responses(
        (status = 200, description = "The subject's Entities with their history", body = Object),
        (status = 400, description = "No reason, a subject that cannot be searched for, or a model that names no subject", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 409, description = "The subject slot is ingested as keyedHash and the Portal holds no key for the space (spec.personalData.keyRef)", body = crate::error::ProblemDetails),
        (status = 503, description = "The space or the key is not available", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn export_subject(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    body: Result<Json<SubjectRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let spec = owned(&state, &user, &project, &space)?;
    let request = checked(body)?;
    let hash = erasure::subject_hash(
        subject_key(&state.config)?,
        &project,
        &space,
        &request.subject,
    );
    let classes = subject_classes(&state, (&project, &space), &spec).await?;
    let (token, found) = named(&state, &project, &space, &classes, &request.subject).await?;
    let watch = state.drift_watch.as_ref().ok_or_else(no_surface)?;
    let segment = crate::spaces::segment(&state.mirror, &project, &space);
    let mut entities = Vec::new();
    for (class, held) in found {
        for entity in held {
            let id = entity["id"].as_str().unwrap_or_default().to_owned();
            let history = watch
                .history(&token, &segment, &id)
                .await
                .map_err(|reason| {
                    tracing::warn!(%reason, "an Entity's history was not read for an export");
                    ApiError::Unavailable(
                        "the space's history did not answer; try again shortly".into(),
                    )
                })?;
            entities.push(json!({ "class": class.class, "entity": entity, "history": history }));
        }
    }
    audited(
        &state,
        &user,
        &project,
        &space,
        "exported a data subject's records",
        json!({ "subject": hash, "reason": request.reason.trim(), "entities": entities.len() }),
    )
    .await;
    let body = json!({
        "space": space,
        "subject": hash,
        "exportedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "entities": entities,
    });
    // An answer to this session only: never kept by a cache on the way.
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/erasures",
    summary = "Propose a Data Subject's Erasure",
    description = "Proposes erasing the data subject from the space: once another person with `approve` on the ContextSpace approves it (DM-142), the personal Attributes (or the whole Entity of a personal class) of every Entity naming it go from the broker's current state and every version, the Portal's trash and comments and the assistant's stored conversations (DM-126). Answers with the proposal, `proposed` and nothing erased, which expires after 7 days; the erasure log keeps the subject's keyed hash, never the value (DM-127). For the space's owners; audited with the reason.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("dryRun" = Option<String>, Query, description = "`All` answers what would be erased and erases nothing"),
    ),
    request_body(content = SubjectRequest, example = json!({"subject": "r-42", "reason": "Art. 17 request 2026-10-19-07"})),
    responses(
        (status = 200, description = "With `?dryRun=All`: what the erasure would take, per class, and nothing erased or logged", body = Object),
        (status = 202, description = "The proposal, `proposed`, and its Location; nothing erased", body = Object),
        (status = 400, description = "No reason, a subject that cannot be searched for, or a model that names no subject", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 409, description = "The subject slot is ingested as keyedHash and the Portal holds no key for the space (spec.personalData.keyRef)", body = crate::error::ProblemDetails),
        (status = 503, description = "No database, space surface or key", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn erase_subject(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    Query(dry): Query<DryRunQuery>,
    body: Result<Json<SubjectRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let spec = owned(&state, &user, &project, &space)?;
    let dry = is_dry_run(&dry)?;
    let request = checked(body)?;
    let pool = erasure_log(&state)?;
    let hash = erasure::subject_hash(
        subject_key(&state.config)?,
        &project,
        &space,
        &request.subject,
    );
    let groups = groups_of(&state, (&project, &space), &spec, &request.subject).await?;
    if dry {
        return Ok(Json(erasure::preview(&groups)).into_response());
    }
    let proposed = erasure::begin(
        (&project, &space),
        hash.clone(),
        &request.reason,
        &actor_of(&user),
        groups,
    );
    erasure::insert(pool, &proposed).await.map_err(|err| {
        tracing::warn!(error = %err, "an erasure was not logged");
        ApiError::Unavailable("the erasure log could not be written; nothing was proposed".into())
    })?;
    audited(
        &state,
        &user,
        &project,
        &space,
        "proposed a data subject's erasure",
        json!({ "subject": hash, "reason": request.reason.trim(), "erasure": proposed.id, "entities": proposed.urns().len() }),
    )
    .await;
    let location = format!(
        "/api/v1/projects/{project}/spaces/{space}/erasures/{}",
        proposed.id
    );
    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location)],
        Json(proposed.view()),
    )
        .into_response())
}

/// The Entities naming the subject now, one group per class with the Attributes to erase.
async fn groups_of(
    state: &AppState,
    (project, space): (&str, &str),
    spec: &Value,
    subject: &str,
) -> Result<Vec<Group>, ApiError> {
    let classes = subject_classes(state, (project, space), spec).await?;
    let (_, found) = named(state, project, space, &classes, subject).await?;
    Ok(found
        .into_iter()
        .map(|(class, held)| Group {
            class: class.class,
            entities: held
                .iter()
                .filter_map(|e| e["id"].as_str().map(str::to_owned))
                .collect(),
            attrs: class.attrs,
        })
        .collect())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/erasures",
    summary = "List Erasures",
    description = "The space's newest 50 erasures, newest first, proposals waiting for a second owner included (DM-142). For the space's owners.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "`{ \"items\": [erasure] }`", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No database", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn list_erasures(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    owned(&state, &user, &project, &space)?;
    let items = erasure::list(erasure_log(&state)?, &project, &space)
        .await
        .map_err(log_unread)?;
    Ok(Json(
        json!({ "items": items.iter().map(erasure::Erasure::view).collect::<Vec<_>>() }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/erasures/{id}/approve",
    summary = "Approve an Erasure",
    description = "A second person with `approve` on the ContextSpace approves an owner's proposal by typing the subject again; its keyed hash must be the proposal's. The space is searched again and the erasure runs, as `open` until every store reports done (DM-126, DM-142). The proposer, a bearer token and an expired or decided proposal are refused.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = String, Path, description = "The erasure's id"),
    ),
    request_body(content = ApproveRequest, example = json!({"subject": "r-42"})),
    responses(
        (status = 200, description = "The approved erasure and its state per store", body = Object),
        (status = 400, description = "A subject that cannot be searched for", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller does not own the space, proposed the erasure (`self-approval`) or sent a bearer token", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space or erasure", body = crate::error::ProblemDetails),
        (status = 409, description = "The erasure is no longer a proposal: approved, declined or expired", body = crate::error::ProblemDetails),
        (status = 422, description = "The subject typed is not the one the proposal names (`subject`)", body = crate::error::ProblemDetails),
        (status = 503, description = "No database, space surface or key", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn approve_erasure(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, String)>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let spec = owned(&state, &user, &project, &space)?;
    refuse_bearer(&user)?;
    crate::auth::session::require_second_factor(&user.0.identity, &state.config)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    if let Some(why) = erasure::subject_refusal(&request.subject) {
        return Err(ApiError::BadRequest(why));
    }
    let pool = erasure_log(&state)?;
    let found = held(pool, (&project, &space, &id)).await?;
    let approver = actor_of(&user);
    if found.actor == approver {
        return Err(ApiError::SelfApproval(format!(
            "you proposed erasure {id}; another owner of space '{space}' approves it (DM-142)"
        )));
    }
    if found.state() != "proposed" {
        return Err(not_proposed(&found));
    }
    let hash = erasure::subject_hash(
        subject_key(&state.config)?,
        &project,
        &space,
        &request.subject,
    );
    if hash != found.subject {
        return Err(ApiError::Unprocessable {
            detail: "subject: this is not the data subject the proposal names; type the value the request gave".into(),
            errors: vec!["subject".into()],
        });
    }
    let groups = groups_of(&state, (&project, &space), &spec, &request.subject).await?;
    let approved = erasure::approve(pool, (&project, &space, &id), &approver, &groups)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "an erasure's approval was not logged");
            ApiError::Unavailable("the erasure log could not be written; nothing was erased".into())
        })?
        .ok_or_else(|| not_proposed(&found))?;
    audited(
        &state,
        &user,
        &project,
        &space,
        "approved a data subject's erasure",
        json!({ "subject": hash, "erasure": id, "proposedBy": approved.actor, "entities": approved.urns().len() }),
    )
    .await;
    erasure::attempt(&state, pool, &approved, Some(&request.subject)).await;
    let now = erasure::get(pool, &project, &space, &id)
        .await
        .ok()
        .flatten()
        .unwrap_or(approved);
    Ok(Json(now.view()))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/erasures/{id}/decline",
    summary = "Decline an Erasure",
    description = "An owner of the space, the proposer included, declines a proposal with a reason; nothing is erased (DM-142). Audited with the reason.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = String, Path, description = "The erasure's id"),
    ),
    request_body(content = DeclineRequest, example = json!({"reason": "the person withdrew the request"})),
    responses(
        (status = 200, description = "The declined erasure", body = Object),
        (status = 400, description = "No reason", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller does not own the space or sent a bearer token", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space or erasure", body = crate::error::ProblemDetails),
        (status = 409, description = "The erasure is no longer a proposal", body = crate::error::ProblemDetails),
        (status = 503, description = "No database", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn decline_erasure(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, String)>,
    body: Result<Json<DeclineRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    owned(&state, &user, &project, &space)?;
    refuse_bearer(&user)?;
    crate::auth::session::require_second_factor(&user.0.identity, &state.config)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    reason_checked(&request.reason)?;
    let pool = erasure_log(&state)?;
    let found = held(pool, (&project, &space, &id)).await?;
    if found.state() != "proposed" {
        return Err(not_proposed(&found));
    }
    let declined = erasure::decline(
        pool,
        (&project, &space, &id),
        &actor_of(&user),
        &request.reason,
    )
    .await
    .map_err(|err| {
        tracing::warn!(error = %err, "an erasure's decline was not logged");
        ApiError::Unavailable("the erasure log could not be written".into())
    })?
    .ok_or_else(|| not_proposed(&found))?;
    audited(
        &state,
        &user,
        &project,
        &space,
        "declined a data subject's erasure",
        json!({ "subject": declined.subject, "erasure": id, "reason": request.reason.trim() }),
    )
    .await;
    Ok(Json(declined.view()))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/erasures/{id}",
    summary = "Read an Erasure",
    description = "One erasure of the space: `open` until every store reports done, `done`, or `failed` once a store gave up after an hour, with the counts and the state of each store (DM-126, DM-127). For the space's owners.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = String, Path, description = "The erasure's id"),
    ),
    responses(
        (status = 200, description = "The erasure", body = Object),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space or erasure", body = crate::error::ProblemDetails),
        (status = 503, description = "No database", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn get_erasure(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    owned(&state, &user, &project, &space)?;
    let pool = erasure_log(&state)?;
    Ok(Json(held(pool, (&project, &space, &id)).await?.view()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/subjects/export",
            post(export_subject),
        )
        .route(
            "/projects/{project}/spaces/{space}/erasures",
            get(list_erasures).post(erase_subject),
        )
        .route(
            "/projects/{project}/spaces/{space}/erasures/{id}",
            get(get_erasure),
        )
        .route(
            "/projects/{project}/spaces/{space}/erasures/{id}/approve",
            post(approve_erasure),
        )
        .route(
            "/projects/{project}/spaces/{space}/erasures/{id}/decline",
            post(decline_erasure),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{Identity, Session};

    fn person(client: Option<&str>) -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                client: client.map(str::to_owned),
                subject: "sub-boris".into(),
                username: "boris".into(),
                email: Some("boris@hel.fi".into()),
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
        })
    }

    /// DM-124: the erasure key decides the hash, so a rotated artifact store key leaves a
    /// proposal's subject hash as it was; without one the artifact key still hashes.
    #[test]
    fn the_erasure_key_outlives_an_artifact_key_rotation() {
        let config = |erasure: Option<&'static str>, artifact: &'static str| {
            crate::config::Config::from_vars(move |key| {
                match key {
                    "JC_OIDC_ISSUER" => Some("https://idm.example.test/realms/dev"),
                    "JC_OIDC_CLIENT_ID" => Some("portal"),
                    "JC_OIDC_CLIENT_SECRET" => Some("secret"),
                    "JC_PORTAL_COOKIE_KEY" => {
                        Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
                    }
                    "JC_PORTAL_ERASURE_KEY" => erasure,
                    "JC_PORTAL_ARTIFACT_STORE_ENDPOINT" => Some("http://store:9000"),
                    "JC_PORTAL_ARTIFACT_STORE_BUCKET" => Some("artifacts"),
                    "JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY" => Some("portal"),
                    "JC_PORTAL_ARTIFACT_STORE_SECRET_KEY" => Some(artifact),
                    _ => None,
                }
                .map(str::to_owned)
            })
            .expect("config")
        };
        let hash = |config: &crate::config::Config| {
            let key = subject_key(config).expect("a key");
            erasure::subject_hash(key, "p", "s", "jana")
        };
        let before = hash(&config(Some("erasure-key-1"), "artifact-key-1"));
        assert_eq!(
            before,
            hash(&config(Some("erasure-key-1"), "artifact-key-2"))
        );
        assert_eq!(
            subject_key(&config(None, "artifact-key-1")).ok(),
            Some("artifact-key-1")
        );
        assert_ne!(before, hash(&config(None, "artifact-key-1")));
    }

    #[test]
    fn a_bearer_token_never_decides_an_erasure() {
        // A token an agent exchanged for an owner carries the owner and the agent's client (AG-94).
        let refused = refuse_bearer(&person(Some("helsinki-jc-agent-proxy")));
        assert!(
            matches!(&refused, Err(ApiError::Denied(m)) if m.contains("DM-142")),
            "{refused:?}"
        );
        assert!(refuse_bearer(&person(None)).is_ok());
        assert_eq!(actor_of(&person(None)), "user:boris");
    }
}
