//! Comments on a space's entities with `@mentions`, and the caller's notifications (API/01 §35,
//! ADR-N-042 §3.1, T-3106). Commenting needs a read of the space and of the entity, as the
//! gateway decides it for the caller (T-3284), and changes no data; a mention notifies only a
//! person a binding in force lets read the space.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::api::entity_trash::readable;
use crate::auth::CurrentUser;
use crate::entity_comments::{Author, Comment, NewComment, Notification, MAX_TEXT};
use crate::error::ApiError;
use crate::state::AppState;

/// One comment as the caller reads it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    /// The caller wrote it, so they may remove it.
    pub mine: bool,
}

/// A new comment, with the mentions that notified nobody.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Commented {
    #[serde(flatten)]
    pub comment: CommentView,
    /// Mentions of people who may not read the space: kept in the text, notified to nobody.
    pub unknown_mentions: Vec<String>,
}

/// The caller's notifications.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notifications {
    pub items: Vec<Notification>,
    pub unread: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct EntityQuery {
    /// The entity's NGSI-LD URN.
    pub urn: String,
}

/// A comment to write.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRequest {
    /// The entity's NGSI-LD URN.
    pub urn: String,
    /// 1 to 4,000 characters; `@identifier` mentions a person.
    pub text: String,
}

fn stored(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "entity comments not reachable");
    ApiError::Unavailable("the comments could not be read or written".into())
}

/// An NGSI-LD URN (PF-43): `urn:ngsi-ld:{Type}:` and an id, no whitespace, at most 1 KiB.
fn urn_of(urn: &str) -> Result<&str, ApiError> {
    let rest = urn.strip_prefix("urn:ngsi-ld:").unwrap_or_default();
    let ok = urn.len() <= 1024
        && rest
            .split_once(':')
            .is_some_and(|(kind, id)| !kind.is_empty() && !id.is_empty())
        && !urn.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(urn)
    } else {
        Err(ApiError::BadRequest(
            "urn must be an NGSI-LD URN, urn:ngsi-ld:{Type}:{id} (PF-43)".into(),
        ))
    }
}

/// The identifiers a text mentions, lower case, each once, in order: `@` at the start or after a
/// character that is not part of a word, then the identifier's characters, without a trailing
/// full stop or comma. An e-mail address written inside a sentence (`a@b.org`) is no mention.
pub fn mentions_in(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut found: Vec<String> = Vec::new();
    let part =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@');
    let mut at = 0;
    while at < chars.len() {
        let starts = chars[at] == '@'
            && (at == 0 || !(chars[at - 1].is_alphanumeric() || chars[at - 1] == '_'));
        if !starts {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < chars.len() && part(chars[end]) {
            end += 1;
        }
        let token: String = chars[at + 1..end].iter().collect();
        let token = token.trim_end_matches(['.', '-', '@']).to_ascii_lowercase();
        // `@@x` would store a mention under the audience `@x` (T-5979): no person is named so.
        if !token.is_empty()
            && !token.starts_with('@')
            && token.len() <= 256
            && !found.contains(&token)
        {
            found.push(token);
        }
        at = end;
    }
    found
}

/// Whether the caller may read the entity `urn` of `space`: one read of it on the space surface
/// with the caller's own token, so their Policy decides and the Portal decides nothing itself
/// (API/01 §35, T-3284). Hidden and missing are the same `404`, so no answer tells an entity's
/// existence to someone who may not read it.
async fn entity_readable(
    state: &AppState,
    headers: &HeaderMap,
    space: &str,
    urn: &str,
) -> Result<(), ApiError> {
    read_as_person(state, headers, space, urn).await.map(drop)
}

/// The entity `urn` of `space` as the caller reads it: through the gateway with the caller's own
/// token, so their Policy hides what it hides (attributes, rows) and the Portal's own account
/// reads nothing for them (T-3284, T-5698). A session without the person's token, a hidden and a
/// missing entity all answer the same `404`.
pub(crate) async fn read_as_person(
    state: &AppState,
    headers: &HeaderMap,
    space: &str,
    urn: &str,
) -> Result<Value, ApiError> {
    let base = state.config.gateway_url.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "this Portal has no gateway address (JC_PORTAL_GATEWAY_URL), so it cannot read the \
             entity as you"
                .into(),
        )
    })?;
    let token = crate::agents::identity::persons_token(headers, state.config.trust_edge_token)
        .ok_or_else(|| {
            ApiError::NotFound(
                "an entity is read here with your own access token, which this session does \
                 not carry; sign in through the platform's address"
                    .into(),
            )
        })?;
    let mut url = reqwest::Url::parse(base)
        .map_err(|_| ApiError::Unavailable("the gateway address is not a URL".into()))?;
    url.path_segments_mut()
        .map_err(|_| ApiError::Unavailable("the gateway address is not a URL".into()))?
        .pop_if_empty()
        .extend(["cs", space, "ngsi-ld", "v1", "entities", urn]);
    let answer = crate::api::pipelines::http()
        .get(url)
        .bearer_auth(token)
        .header(header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|err| {
            tracing::warn!(error = %err.without_url(), "the gateway did not answer an entity read");
            ApiError::Unavailable("the gateway did not answer; try again shortly".into())
        })?;
    match answer.status().as_u16() {
        200..=299 => answer.json().await.map_err(|_| {
            ApiError::Unavailable("the gateway answered an entity that is not JSON".into())
        }),
        401 | 403 | 404 => Err(ApiError::NotFound(format!(
            "no entity {urn} you may read in space '{space}'"
        ))),
        status => Err(ApiError::Unavailable(format!(
            "the gateway answered {status} reading the entity; try again shortly"
        ))),
    }
}

/// The identifiers a notification for the caller may be addressed to: their username and their
/// e-mail, lower case.
/// The identifiers the caller's notifications are stored under: username and e-mail, lower case.
/// An identifier starting with `@` names an audience (`@connector-admin`, `@approve:{project}`),
/// never a person, so a username or e-mail spelled that way is no recipient: it would read every
/// audience notice without the right to act on it.
fn recipients(user: &CurrentUser) -> Vec<String> {
    let identity = &user.0.identity;
    let mut ids: Vec<String> = Vec::new();
    for id in std::iter::once(identity.username.as_str()).chain(identity.email.as_deref()) {
        let id = id.to_ascii_lowercase();
        if !id.starts_with('@') && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/comments",
    summary = "List The Comments On An Entity",
    description = "The comments on one entity of the space, oldest first (API/01 §35).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        EntityQuery,
    ),
    responses(
        (status = 200, description = "The comments", body = [CommentView]),
        (status = 400, description = "Not an NGSI-LD URN", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space, or no entity of it the caller may read (API/01 §35, T-3284)", body = crate::error::ProblemDetails),
        (status = 503, description = "The comments are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_comments(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<EntityQuery>,
) -> Result<Json<Vec<CommentView>>, ApiError> {
    readable(&state, &user, &project, &space)?;
    let urn = urn_of(&query.urn)?;
    entity_readable(&state, &headers, &space, urn).await?;
    let me = user.0.identity.subject.as_str();
    let comments = state
        .comments
        .list(&project, &space, urn)
        .await
        .map_err(stored)?;
    Ok(Json(
        comments
            .into_iter()
            .map(|comment| CommentView {
                mine: comment.author_subject.as_deref() == Some(me),
                comment,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/comments",
    summary = "Comment On An Entity",
    description = "Comments on one entity of the space; each `@identifier` of a person who may read the space is notified once (API/01 §35). Changes no data.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(
        content = CommentRequest,
        description = "The entity and the text.",
        content_type = "application/json",
        example = json!({
            "urn": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:7",
            "text": "@demo.editor@hel.fi the count looks stale since Monday"
        })
    ),
    responses(
        (status = 201, description = "The comment", body = Commented),
        (status = 400, description = "Not an NGSI-LD URN, or no text, or more than 4,000 characters", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space, or no entity of it the caller may read (API/01 §35, T-3284)", body = crate::error::ProblemDetails),
        (status = 409, description = "The entity holds the most comments it keeps", body = crate::error::ProblemDetails),
        (status = 503, description = "The comments are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn add_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<CommentRequest>,
) -> Result<(StatusCode, Json<Commented>), ApiError> {
    readable(&state, &user, &project, &space)?;
    let urn = urn_of(&request.urn)?;
    entity_readable(&state, &headers, &space, urn).await?;
    let text = request.text.trim();
    let length = text.chars().count();
    if length == 0 || length > MAX_TEXT {
        return Err(ApiError::BadRequest(format!(
            "text must be 1 to {MAX_TEXT} characters; it is {length}"
        )));
    }
    let identity = &user.0.identity;
    let now = chrono::Utc::now();
    let me = recipients(&user);
    let (notified, unknown): (Vec<String>, Vec<String>) = mentions_in(text)
        .into_iter()
        .filter(|person| !me.contains(person))
        .partition(|person| {
            crate::permissions::may_read_space(&state.mirror, &project, &space, person, now)
        });
    let name = identity
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&identity.username);
    let comment = state
        .comments
        .add(
            Author {
                id: &identity.username,
                name,
                subject: &identity.subject,
            },
            NewComment {
                project: &project,
                space: &space,
                urn,
                text,
                mentions: &notified,
            },
        )
        .await
        .map_err(stored)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "this entity holds the most comments it keeps ({})",
                crate::entity_comments::MAX_PER_ENTITY
            ))
        })?;
    Ok((
        StatusCode::CREATED,
        Json(Commented {
            comment: CommentView {
                comment,
                mine: true,
            },
            unknown_mentions: unknown,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/spaces/{space}/comments/{id}",
    summary = "Remove My Comment",
    description = "Removes one of the caller's own comments and the notifications it sent (API/01 §35). Another caller's comment is 404.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = i64, Path, description = "The comment's id"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such comment of the caller's, or no such space", body = crate::error::ProblemDetails),
        (status = 503, description = "The comments are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn remove_comment(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, i64)>,
) -> Result<StatusCode, ApiError> {
    readable(&state, &user, &project, &space)?;
    if state
        .comments
        .remove(&project, &space, &user.0.identity.subject, id)
        .await
        .map_err(stored)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no comment {id} of yours here")))
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    summary = "List My Notifications",
    description = "The caller's notifications, newest first, at most 100, with how many are unread (API/01 §35): mentions, and the connector events the caller may act on (a change opened through a connector, a connector request above the ceiling, a connector of theirs ending within 7 days).",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's notifications", body = Notifications),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 503, description = "The notifications are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_notifications(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Notifications>, ApiError> {
    let ids = recipients(&user);
    let Some(me) = ids.first() else {
        return Ok(Json(Notifications {
            items: Vec::new(),
            unread: 0,
        }));
    };
    let identity = &user.0.identity;
    // The connector events (T-5979): the caller's own expiring connectors, then the audience
    // notices they may act on today, each copied to them once.
    crate::api::connectors::notify_expiring(&state, identity, me).await?;
    let audiences = crate::api::connectors::audiences(&state, identity);
    state
        .comments
        .deliver(me, &ids, &audiences)
        .await
        .map_err(stored)?;
    let (items, unread) = state.comments.notifications(&ids).await.map_err(stored)?;
    Ok(Json(Notifications { items, unread }))
}

#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/read",
    summary = "Mark A Notification Read",
    description = "Marks one of the caller's notifications read (API/01 §35). Another caller's is 404.",
    tag = "auth",
    params(("id" = i64, Path, description = "The notification's id")),
    responses(
        (status = 204, description = "Read"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such notification of the caller's", body = crate::error::ProblemDetails),
        (status = 503, description = "The notifications are not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn read_notification(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if state
        .comments
        .mark_read(&recipients(&user), id)
        .await
        .map_err(stored)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no notification {id} of yours")))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/comments",
            get(list_comments).post(add_comment),
        )
        .route(
            "/projects/{project}/spaces/{space}/comments/{id}",
            delete(remove_comment),
        )
        .route("/notifications", get(list_notifications))
        .route("/notifications/{id}/read", post(read_notification))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_is_an_at_sign_after_no_word_and_an_address_inside_a_word_is_none() {
        assert_eq!(
            mentions_in(
                "@Demo.Editor@hel.fi, see this. Also (@ana) and @ana again; mail x@y.org or @."
            ),
            vec!["demo.editor@hel.fi", "ana"]
        );
        assert!(mentions_in("no one here").is_empty());
        assert_eq!(mentions_in("end with @bob."), vec!["bob"]);
        assert!(mentions_in("@@connector-admin and @@approve:p").is_empty());
    }

    fn named(username: &str, email: &str) -> CurrentUser {
        CurrentUser(crate::auth::session::Session {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-forger".into(),
                username: username.into(),
                email: Some(email.into()),
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

    #[tokio::test]
    async fn a_person_named_like_an_audience_reads_none_of_its_notices() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let notice = |key| crate::entity_comments::Notice {
            kind: "connector.pending",
            project: "",
            about: "c-1",
            author: "sami",
            author_name: "Sami",
            connector: "doprava-in-claude",
            link: "/ai-connectors",
            key,
        };
        let approvers = crate::entity_comments::approvers_of("p");
        for (recipient, key) in [
            (crate::entity_comments::CONNECTOR_ADMINS, "k-admin"),
            (approvers.as_str(), "k-approve"),
        ] {
            assert!(state
                .comments
                .notify(recipient, notice(key))
                .await
                .expect("stored"));
        }
        let forger = named("@connector-admin", "@approve:p");
        assert_eq!(recipients(&forger), Vec::<String>::new());
        let Json(read) = list_notifications(forger, State(state.clone()))
            .await
            .expect("answers");
        assert_eq!((read.items.len(), read.unread), (0, 0));
        assert_eq!(
            recipients(&named("Ana", "ana@hel.fi")),
            vec!["ana", "ana@hel.fi"]
        );
    }

    #[test]
    fn only_an_ngsi_ld_urn_names_an_entity() {
        assert!(urn_of("urn:ngsi-ld:Station:hel.fi:bikes:7").is_ok());
        for bad in [
            "urn:ngsi-ld:Station",
            "urn:ngsi-ld::7",
            "station 7",
            "urn:ngsi-ld:Station:a b",
        ] {
            assert!(urn_of(bad).is_err(), "{bad}");
        }
    }
}
