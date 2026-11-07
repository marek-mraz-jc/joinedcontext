//! People: the whole lifecycle of a person in the organization's realm (PF-90…PF-94,
//! ADR-N-031, API/01 §24).
//!
//! A person lives in Keycloak and never in a manifest, so these routes call the realm's admin API
//! directly, each one held to a verb on the kind `Person` at organization scope (PF-91). What they
//! may never do: act on a person who holds a right the caller lacks, which is how a reset's
//! temporary password would open a stronger account; disable or delete the caller or the last
//! Organization Administrator (PF-93, PF-03). Membership stays as code: deleting a person is a
//! Change that takes them out of every `Group` and `RoleBinding`, and the Keycloak user goes only
//! once that Change is merged. Every action is one `person.changed` activity event naming the
//! actor, the action and the person's id, never a password, an e-mail body or a token (PF-90).

use std::collections::{BTreeMap, HashMap};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::StreamExt as _;
use jc_core::kinds::{Verb, PERSON};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::change::{Change, ChangePhase, ChangeStatus, Lane, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::git::Author;
use crate::people::{Admin, KcUser, NewPerson, People};
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

/// The languages the Portal speaks, and so the ones a person may be given (UI-09).
const LOCALES: [&str; 4] = ["en", "sk", "cs", "de"];
const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 100;
const MAX_NAME: usize = 255;
/// Session lookups a page of people sends the realm at once: Keycloak serves every admin call from
/// a database pool of 7 (T-4933), and a page of 50 sent together refused the home page (T-5000).
pub(super) const LAST_SEEN_AT_ONCE: usize = 4;

/// A person as the Portal shows one (API/01 §24).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub locale: Option<String>,
    pub enabled: bool,
    pub email_verified: bool,
    pub required_actions: Vec<String>,
    /// When the realm created the person (RFC 3339).
    pub created_at: Option<String>,
    /// The last access of the person's newest open session; `null` when none is open.
    pub last_seen: Option<String>,
    /// The Change a deletion waits for; `null` when none is pending.
    pub pending_deletion: Option<String>,
    /// When the link of the last invitation the Portal sent stops working, while a step is left
    /// (RFC 3339, PF-108); `null` once none is, or without a database to remember it in.
    pub invitation_expires: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PersonPage {
    pub items: Vec<Person>,
    /// `first` of the following page, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupRef {
    pub name: String,
}

/// A platform role the person holds, and the binding and the way it reaches them (PF-94).
#[derive(Debug, Serialize, ToSchema)]
pub struct PlatformRole {
    pub role: String,
    pub binding: String,
    #[schema(value_type = Object)]
    pub scope: Value,
    /// `{ "user": … }` or `{ "group": … }`.
    #[schema(value_type = Object)]
    pub via: Value,
}

/// An application role the person holds (PF-94).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppRole {
    pub project: String,
    pub app: String,
    pub role: String,
    #[schema(value_type = Object)]
    pub via: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonDetail {
    pub person: Person,
    pub groups: Vec<GroupRef>,
    pub platform_roles: Vec<PlatformRole>,
    pub app_roles: Vec<AppRole>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreatePerson {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    #[serde(default)]
    pub locale: Option<String>,
    /// The project the invitation leads into: its home page greets the person once their
    /// password is set (PF-108). It grants nothing.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EditPerson {
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
}

/// The one answer that may carry a temporary password, once (PF-92).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedPerson {
    pub person: Person,
    pub email_sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_password: Option<String>,
    /// The Organization whose `/{organization}/members` group the person joined, so they sign
    /// in as its member with no project (ADR-N-067 §2.5); absent when the realm has no such group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_of: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordReset {
    pub email_sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_password: Option<String>,
}

/// Whether a temporary password was set, never which (T-4321).
fn redacted(password: &Option<String>) -> Option<&'static str> {
    password.as_ref().map(|_| "[redacted]")
}

impl std::fmt::Debug for CreatedPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedPerson")
            .field("person", &self.person)
            .field("email_sent", &self.email_sent)
            .field("temporary_password", &redacted(&self.temporary_password))
            .field("member_of", &self.member_of)
            .finish()
    }
}

impl std::fmt::Debug for PasswordReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordReset")
            .field("email_sent", &self.email_sent)
            .field("temporary_password", &redacted(&self.temporary_password))
            .finish()
    }
}

pub(super) fn people(state: &AppState) -> Result<&People, ApiError> {
    state.people.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "no Keycloak admin client is configured, so people cannot be managed here".into(),
        )
    })
}

/// The caller holds `verb` on `Person` in a binding at organization scope (PF-91).
/// The verb on Person; administering anyone (every verb but read) is a decision door that
/// also wants a second factor in the session (T-5567).
pub(crate) fn allow(state: &AppState, identity: &Identity, verb: Verb) -> Result<(), ApiError> {
    crate::permissions::for_request(state, identity, ORG_NAMESPACE).check(PERSON, verb, None)?;
    if verb == Verb::Read {
        return Ok(());
    }
    crate::auth::session::require_second_factor(identity, &state.config)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(format!("invalid body: {err}")))
}

/// An e-mail address as the realm and the manifests carry it: trimmed, lower case, one `@`, a
/// dot in the domain, nothing that is not printable.
fn email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_ascii_lowercase();
    let valid = email.len() <= 254
        && email.split('@').count() == 2
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
        && email.chars().all(|c| c.is_ascii_graphic());
    if valid {
        Ok(email)
    } else {
        Err(ApiError::BadRequest(format!(
            "'{}' is not an e-mail address",
            raw.trim()
        )))
    }
}

fn name(field: &str, raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(format!(
            "{field} is 1 to {MAX_NAME} characters without control characters"
        )));
    }
    Ok(name.to_owned())
}

fn locale(raw: &str) -> Result<String, ApiError> {
    LOCALES
        .iter()
        .find(|locale| **locale == raw)
        .map(|locale| (*locale).to_owned())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "locale '{raw}' is not one of {}",
                LOCALES.join(", ")
            ))
        })
}

fn rfc3339(millis: Option<i64>) -> Option<String> {
    millis
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn email_of(user: &KcUser) -> String {
    user.email
        .clone()
        .unwrap_or_else(|| user.username.clone())
        .to_ascii_lowercase()
}

fn person(user: &KcUser, last_seen: Option<i64>, pending: Option<String>) -> Person {
    Person {
        id: user.id.clone(),
        email: email_of(user),
        first_name: user.first_name.clone().unwrap_or_default(),
        last_name: user.last_name.clone().unwrap_or_default(),
        locale: user.locale().map(str::to_owned),
        enabled: user.enabled,
        email_verified: user.email_verified,
        required_actions: user.required_actions.clone(),
        created_at: rfc3339(user.created_timestamp),
        last_seen: rfc3339(last_seen),
        pending_deletion: pending,
        invitation_expires: None,
    }
}

async fn pending(state: &AppState, id: &str) -> Option<String> {
    let pool = state.db.as_ref()?;
    match crate::db::person_deletion(pool, id).await {
        Ok(row) => row.map(|row| row.change_name),
        Err(err) => {
            tracing::warn!(error = %err, "the pending removals could not be read");
            None
        }
    }
}

/// Fills in when each invitation still pending expires (PF-108). Without a database, or when it
/// does not answer, the expiry stays unknown: the list still answers.
async fn with_invitations(state: &AppState, people: &mut [Person]) {
    let Some(pool) = state.db.as_ref() else {
        return;
    };
    let waiting: Vec<String> = people
        .iter()
        .filter(|person| !person.required_actions.is_empty())
        .map(|person| person.id.clone())
        .collect();
    if waiting.is_empty() {
        return;
    }
    match crate::db::person_invitations(pool, &waiting).await {
        Ok(expiries) => {
            for person in people.iter_mut() {
                if person.required_actions.is_empty() {
                    continue;
                }
                person.invitation_expires = expiries.get(&person.id).and_then(|at| {
                    rfc3339(i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok())
                });
            }
        }
        Err(err) => tracing::warn!(error = %err, "the invitations' expiries could not be read"),
    }
}

/// Remembers when the invitation just sent expires, so the people page can say so, and where it
/// leads, so sending it again leads there too (PF-108).
async fn remember_invitation(state: &AppState, id: &str, project: Option<&str>) {
    let (Some(pool), Some(lifespan)) = (
        state.db.as_ref(),
        crate::api::organization_limits::invitation_lifespan(state),
    ) else {
        return;
    };
    let expires = time::OffsetDateTime::now_utc() + lifespan;
    if let Err(err) = crate::db::set_person_invitation(pool, id, expires, project).await {
        tracing::warn!(person = %id, error = %err, "the invitation's expiry is not remembered");
    }
}

async fn forget_invitation(state: &AppState, id: &str) {
    if let Some(pool) = state.db.as_ref() {
        if let Err(err) = crate::db::remove_person_invitation(pool, id).await {
            tracing::warn!(person = %id, error = %err, "the invitation of a deleted person is still remembered");
        }
    }
}

/// The project an invitation leads into, if the caller may read it (PF-108): `404` otherwise,
/// like every read of a project the caller is not given (PF-59).
fn invitation_project(
    state: &AppState,
    identity: &Identity,
    raw: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(project) = raw.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let known = project != ORG_NAMESPACE
        && state.mirror.namespaces().iter().any(|name| name == project)
        && crate::permissions::for_request(state, identity, project).may_read_project();
    if !known {
        return Err(ApiError::NotFound(format!("no project '{project}'")));
    }
    Ok(Some(project.to_owned()))
}

/// The client and the page an invitation's link returns to, when the Portal knows a client for
/// it (`JC_PORTAL_INVITATION_CLIENT_ID`).
fn welcome_page(state: &AppState, project: Option<&str>) -> Option<(String, String)> {
    let client = state.config.invitation_client_id.clone()?;
    let path = project.map_or_else(
        || "/".to_owned(),
        |p| format!("/projects/{p}/home?welcome=1"),
    );
    let page = state.config.public_base_url.join(&path).ok()?;
    Some((client, page.to_string()))
}

/// One `person.changed` event (PF-90): the actor, the action, the person's id.
pub(crate) async fn record(state: &AppState, identity: &Identity, action: &str, id: &str) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: ORG_NAMESPACE.to_owned(),
        space: None,
        kind: "person.changed".to_owned(),
        source: "portal".to_owned(),
        summary: format!("{} {action} person {id}", identity.username),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "action": action, "person": id, "actor": identity.username }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(person = %id, error = %err, "the person's change is not on the activity feed");
    }
}

/// The `Group` manifests naming `email` as a member.
/// How a manifest names one person: by their Keycloak user id (ADR-N-061), or by the e-mail a
/// manifest written before the id migration still holds, until that repository's migration
/// Change is merged.
struct Who {
    id: String,
    email: String,
}

impl Who {
    fn of(user: &KcUser) -> Self {
        Who {
            id: user.id.clone(),
            email: email_of(user),
        }
    }

    fn named_by(&self, user: &str) -> bool {
        user == self.id || (!self.email.is_empty() && user.eq_ignore_ascii_case(&self.email))
    }
}

fn groups_of(state: &AppState, who: &Who) -> Vec<ResourceEnvelope> {
    state.mirror.matching(|candidate| {
        candidate.kind == "Group"
            && candidate.metadata.namespace.as_deref() == Some(ORG_NAMESPACE)
            && members(&candidate.spec).any(|member| who.named_by(member))
    })
}

fn members(spec: &Value) -> impl Iterator<Item = &str> + '_ {
    spec.get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|member| member.get("user").and_then(Value::as_str))
}

/// How a subject list reaches `who`: the person themselves, or one of `groups`.
fn reaches(subjects: Option<&Value>, who: &Who, groups: &[String]) -> Option<Value> {
    subjects?.as_array()?.iter().find_map(|subject| {
        if let Some(user) = subject.get("user").and_then(Value::as_str) {
            if who.named_by(user) {
                return Some(json!({ "user": user }));
            }
        }
        subject
            .get("group")
            .and_then(Value::as_str)
            .filter(|group| groups.iter().any(|g| g == group))
            .map(|group| json!({ "group": group }))
    })
}

/// The bindings that give `who` a role, directly or through a group.
fn bindings_reaching(
    state: &AppState,
    who: &Who,
    groups: &[String],
) -> Vec<(ResourceEnvelope, Value)> {
    state
        .mirror
        .matching(|candidate| candidate.kind == "RoleBinding")
        .into_iter()
        .filter_map(|binding| {
            let via = reaches(binding.spec.get("subjects"), who, groups)?;
            Some((binding, via))
        })
        .collect()
}

/// PF-93: the caller may act on this person only when they hold every right the person holds, so
/// a reset's temporary password never opens a stronger account. The bootstrap administrators'
/// realm group or role counts as every right.
async fn holds_what_they_hold(
    state: &AppState,
    identity: &Identity,
    admin: &Admin<'_>,
    user: &KcUser,
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, ORG_NAMESPACE);
    if effective.bootstrap {
        return Ok(());
    }
    let email = email_of(user);
    let held = admin.groups_and_roles(&user.id).await?;
    let bootstrap = &state.config.bootstrap_admins;
    if held.iter().any(|name| name == bootstrap) {
        return Err(ApiError::Denied(format!(
            "{email} is in {bootstrap}, which holds every right; only another member of it may \
             change this person (PF-93)"
        )));
    }
    let who = Who::of(user);
    let groups: Vec<String> = groups_of(state, &who)
        .into_iter()
        .map(|group| group.metadata.name)
        .collect();
    for (binding, _) in bindings_reaching(state, &who, &groups) {
        let manifest =
            serde_json::to_value(&binding).map_err(|e| ApiError::Internal(e.to_string()))?;
        crate::permissions::within_own_rights(
            state,
            identity,
            &manifest,
            None,
            "people administrator",
        )
        .map_err(|err| match err {
            ApiError::Denied(reason) => ApiError::Denied(format!(
                "{email} holds a right you do not, through the binding '{}': {reason} (PF-93)",
                binding.metadata.name
            )),
            other => other,
        })?;
    }
    Ok(())
}

/// PF-93, PF-03: nobody disables or deletes themselves, or the last Organization Administrator.
fn not_self_nor_last(state: &AppState, identity: &Identity, user: &KcUser) -> Result<(), ApiError> {
    let email = email_of(user);
    let own = identity
        .email
        .as_deref()
        .is_some_and(|own| own.eq_ignore_ascii_case(&email));
    if user.id == identity.subject || own {
        return Err(ApiError::Conflict(
            "you cannot disable or delete yourself; another administrator can (PF-93)".into(),
        ));
    }
    let administrators = crate::permissions::administrator_people(&state.mirror);
    let who = Who::of(user);
    if !administrators.is_empty() && administrators.iter().all(|admin| who.named_by(admin)) {
        return Err(ApiError::Conflict(format!(
            "{email} is the last Organization Administrator; bind another person to an \
             administrator role first (PF-03)"
        )));
    }
    Ok(())
}

/// A person's account, their consents and their second factor are decided by a person in a
/// Portal session; a service account's or an agent's token never removes them, whatever its
/// grants (AG-133, PF-58).
pub(super) fn refuse_bearer(identity: &Identity) -> Result<(), ApiError> {
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a person is deleted, gets a new password or invitation, or loses a consent or their \
             second factor, only by a person in the Portal, never with a service account's or an \
             agent's token (AG-133, PF-58)"
                .into(),
        ));
    }
    Ok(())
}

/// The person `id`, after the verb and the takeover guard.
pub(crate) async fn target<'a>(
    state: &'a AppState,
    identity: &Identity,
    verb: Verb,
    id: &str,
) -> Result<(Admin<'a>, KcUser), ApiError> {
    allow(state, identity, verb)?;
    let admin = people(state)?.admin().await?;
    let user = admin.get(id).await?;
    if verb != Verb::Read {
        holds_what_they_hold(state, identity, &admin, &user).await?;
    }
    Ok((admin, user))
}

/// Each person's last access, in the order of `users`, at most `LAST_SEEN_AT_ONCE` lookups in
/// flight; a lookup the realm refuses reads as never seen.
async fn last_seen_of(admin: &Admin<'_>, users: &[KcUser]) -> Vec<Option<i64>> {
    let lookups: Vec<_> = users.iter().map(|user| admin.last_seen(&user.id)).collect();
    futures_util::stream::iter(lookups)
        .buffered(LAST_SEEN_AT_ONCE)
        .map(|seen| seen.unwrap_or(None))
        .collect()
        .await
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/people",
    summary = "List People",
    description = "Searches the organization's people, a page at a time.",
    tag = "people",
    params(
        ("search" = Option<String>, Query, description = "A substring of the name or e-mail"),
        ("first" = Option<u32>, Query, description = "Offset, default 0"),
        ("max" = Option<u32>, Query, description = "Page size, default 50, at most 100"),
    ),
    responses(
        (status = 200, description = "One page of people", body = PersonPage),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 403, description = "The caller lacks read on Person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn list_people(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<PersonPage>, ApiError> {
    let identity = &user.0.identity;
    allow(&state, identity, Verb::Read)?;
    if let Some(unknown) = query
        .keys()
        .find(|key| !matches!(key.as_str(), "search" | "first" | "max"))
    {
        return Err(ApiError::BadRequest(format!(
            "unknown query parameter '{unknown}'"
        )));
    }
    let number = |key: &str, default: u32| -> Result<u32, ApiError> {
        query.get(key).map_or(Ok(default), |raw| {
            raw.parse()
                .map_err(|_| ApiError::BadRequest(format!("{key} is a whole number, not '{raw}'")))
        })
    };
    let first = number("first", 0)?;
    let max = number("max", DEFAULT_PAGE)?;
    if max == 0 || max > MAX_PAGE {
        return Err(ApiError::BadRequest(format!("max is 1 to {MAX_PAGE}")));
    }
    let search = query.get("search").map(|s| s.trim()).unwrap_or_default();
    if search.chars().count() > MAX_NAME {
        return Err(ApiError::BadRequest(
            "search is at most 255 characters".into(),
        ));
    }
    let admin = people(&state)?.admin().await?;
    let mut users = admin.list(search, first, max + 1).await?;
    let next = (users.len() > max as usize).then(|| first + max);
    users.truncate(max as usize);
    let seen = last_seen_of(&admin, &users).await;
    let mut items = Vec::with_capacity(users.len());
    for (user, seen) in users.iter().zip(seen) {
        items.push(person(user, seen, pending(&state, &user.id).await));
    }
    with_invitations(&state, &mut items).await;
    Ok(Json(PersonPage { items, next }))
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people",
    summary = "Create Person",
    description = "Creates a person and sends the sign-up e-mail.",
    tag = "people",
    request_body(content = CreatePerson, example = json!({ "email": "jana.kovacova@example.org", "firstName": "Jana", "lastName": "Kováčová", "locale": "sk", "project": "helsinki" })),
    responses(
        (status = 201, description = "Created; the only answer that may carry a temporary password", body = CreatedPerson),
        (status = 400, description = "Invalid input", body = ProblemDetails),
        (status = 403, description = "The caller lacks create on Person", body = ProblemDetails),
        (status = 404, description = "No project of that name the caller may read", body = ProblemDetails),
        (status = 409, description = "The e-mail is taken", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn create_person(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let created = create(&state, &user.0.identity, &body).await?;
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// The route's whole body, which `jc_person_create` calls too (T-2732): the caller's `create` on
/// Person first, then the input, then the realm.
pub(crate) async fn create(
    state: &AppState,
    identity: &Identity,
    body: &[u8],
) -> Result<CreatedPerson, ApiError> {
    allow(state, identity, Verb::Create)?;
    let body: CreatePerson = parse(body)?;
    let address = email(&body.email)?;
    let first_name = name("firstName", &body.first_name)?;
    let last_name = name("lastName", &body.last_name)?;
    let language = body.locale.as_deref().map(locale).transpose()?;
    let project = invitation_project(state, identity, body.project.as_deref())?;
    let admin = people(state)?.admin().await?;
    let id = admin
        .create(&NewPerson {
            email: &address,
            first_name: &first_name,
            last_name: &last_name,
            locale: language.as_deref(),
        })
        .await
        .map_err(|err| match err {
            crate::people::PeopleError::Conflict => ApiError::Conflict(format!(
                "{address} already belongs to a person of the realm"
            )),
            other => other.into(),
        })?;
    let member_of = join_members(state, &admin, &id, project.as_deref()).await?;
    let back = welcome_page(state, project.as_deref());
    let (email_sent, temporary_password) = invite(
        state,
        &admin,
        &id,
        &["VERIFY_EMAIL", "UPDATE_PASSWORD"],
        back.as_ref().map(|(c, p)| (c.as_str(), p.as_str())),
    )
    .await?;
    if email_sent {
        remember_invitation(state, &id, project.as_deref()).await;
    }
    record(state, identity, "created", &id).await;
    let created = admin.get(&id).await?;
    let mut answer = person(&created, None, None);
    with_invitations(state, std::slice::from_mut(&mut answer)).await;
    Ok(CreatedPerson {
        person: answer,
        email_sent,
        temporary_password,
        member_of,
    })
}

/// Puts a new person in `/{organization}/members` of the invitation's project's Organization,
/// or the installation's: a member reads the catalogue and what the Organization's Policies grant
/// them, with no project (ADR-N-067 §2.2, §2.5). A realm without that group (an installation
/// before ADR-N-067) leaves them outside, with a warning, as before.
async fn join_members(
    state: &AppState,
    admin: &Admin<'_>,
    id: &str,
    project: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let organization = match project {
        Some(project) => crate::permissions::organization_of(&state.mirror, project),
        None => crate::permissions::installation_organization(&state.mirror),
    };
    let Some(organization) = organization else {
        return Ok(None);
    };
    if admin.join_members(id, &organization).await? {
        Ok(Some(organization))
    } else {
        tracing::warn!(%organization, "the realm has no members group for the organization; the invited person is no member yet");
        Ok(None)
    }
}

/// The realm's e-mail, or when it cannot send one, a temporary password answered once (PF-92).
async fn invite(
    state: &AppState,
    admin: &Admin<'_>,
    id: &str,
    actions: &[&str],
    back: Option<(&str, &str)>,
) -> Result<(bool, Option<String>), ApiError> {
    let lifespan = crate::api::organization_limits::invitation_lifespan(state);
    if admin.send_actions(id, actions, lifespan, back).await? {
        return Ok((true, None));
    }
    let password = crate::people::temporary_password();
    admin.set_temporary_password(id, &password).await?;
    Ok((false, Some(password)))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/people/{id}",
    summary = "Get Person",
    description = "One person with their groups and roles.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 200, description = "The person and where they are granted something", body = PersonDetail),
        (status = 403, description = "The caller lacks read on Person", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn get_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PersonDetail>, ApiError> {
    let (admin, found) = target(&state, &user.0.identity, Verb::Read, &id).await?;
    let who = Who::of(&found);
    let groups: Vec<String> = groups_of(&state, &who)
        .into_iter()
        .map(|group| group.metadata.name)
        .collect();
    let mut platform_roles: Vec<PlatformRole> = bindings_reaching(&state, &who, &groups)
        .into_iter()
        .map(|(binding, via)| PlatformRole {
            role: binding
                .spec
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            binding: binding.metadata.name,
            scope: binding.spec.get("scope").cloned().unwrap_or(Value::Null),
            via,
        })
        .collect();
    platform_roles.sort_by(|a, b| (&a.role, &a.binding).cmp(&(&b.role, &b.binding)));
    let mut app_roles = Vec::new();
    for app in state.mirror.matching(|candidate| candidate.kind == "App") {
        for entry in app
            .spec
            .get("access")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(via) = reaches(entry.get("subjects"), &who, &groups) {
                app_roles.push(AppRole {
                    project: app.metadata.namespace.clone().unwrap_or_default(),
                    app: app.metadata.name.clone(),
                    role: entry
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    via,
                });
            }
        }
    }
    app_roles.sort_by(|a, b| (&a.project, &a.app, &a.role).cmp(&(&b.project, &b.app, &b.role)));
    let seen = admin.last_seen(&found.id).await.unwrap_or(None);
    let mut shown = person(&found, seen, pending(&state, &found.id).await);
    with_invitations(&state, std::slice::from_mut(&mut shown)).await;
    Ok(Json(PersonDetail {
        person: shown,
        groups: groups.into_iter().map(|name| GroupRef { name }).collect(),
        platform_roles,
        app_roles,
    }))
}

#[utoipa::path(
    patch,
    path = "/api/v1/organization/people/{id}",
    summary = "Edit Person",
    description = "Edits a person's name, e-mail or language.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    request_body(content = EditPerson, example = json!({ "lastName": "Nováková" })),
    responses(
        (status = 200, description = "The person as edited", body = Person),
        (status = 400, description = "Invalid input", body = ProblemDetails),
        (status = 403, description = "The caller lacks update on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 409, description = "The e-mail is taken", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn edit_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Person>, ApiError> {
    let identity = &user.0.identity;
    allow(&state, identity, Verb::Update)?;
    let edit: EditPerson = parse(&body)?;
    let first_name = edit
        .first_name
        .as_deref()
        .map(|v| name("firstName", v))
        .transpose()?;
    let last_name = edit
        .last_name
        .as_deref()
        .map(|v| name("lastName", v))
        .transpose()?;
    let address = edit.email.as_deref().map(email).transpose()?;
    let language = edit.locale.as_deref().map(locale).transpose()?;
    if first_name.is_none() && last_name.is_none() && address.is_none() && language.is_none() {
        return Err(ApiError::BadRequest(
            "name at least one of firstName, lastName, email, locale".into(),
        ));
    }
    let (admin, found) = target(&state, identity, Verb::Update, &id).await?;
    let changed_email = address.filter(|address| *address != email_of(&found));
    admin
        .update(&found.id, |rep| {
            if let Some(first) = &first_name {
                rep["firstName"] = json!(first);
            }
            if let Some(last) = &last_name {
                rep["lastName"] = json!(last);
            }
            if let Some(language) = &language {
                if !rep["attributes"].is_object() {
                    rep["attributes"] = json!({});
                }
                rep["attributes"]["locale"] = json!([language]);
            }
            if let Some(address) = &changed_email {
                rep["email"] = json!(address);
                rep["emailVerified"] = json!(false);
                let mut actions: Vec<Value> = rep["requiredActions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !actions.iter().any(|a| a == "VERIFY_EMAIL") {
                    actions.push(json!("VERIFY_EMAIL"));
                }
                rep["requiredActions"] = Value::Array(actions);
            }
        })
        .await
        .map_err(|err| match err {
            crate::people::PeopleError::Conflict => ApiError::Conflict(format!(
                "{} already belongs to another person of the realm",
                changed_email.as_deref().unwrap_or_default()
            )),
            other => other.into(),
        })?;
    if changed_email.is_some() {
        // The realm asks for the new address to be verified; without mail it waits for the login.
        let lifespan = crate::api::organization_limits::invitation_lifespan(&state);
        admin
            .send_actions(&found.id, &["VERIFY_EMAIL"], lifespan, None)
            .await?;
    }
    record(&state, identity, "edited", &found.id).await;
    let edited = admin.get(&found.id).await?;
    Ok(Json(person(
        &edited,
        None,
        pending(&state, &found.id).await,
    )))
}

/// Refuses every token of the person issued until now, here and, through `/internal/revocations`,
/// on the gateway's doors (T-5568, AR-6): Keycloak's logout ends sessions, never a token already
/// issued. A person's Keycloak user id is their tokens' `sub`.
async fn revoke(state: &AppState, id: &str) {
    state
        .revoke_subject(id, crate::auth::session::now_unix())
        .await;
}

/// [`revoke`], then Keycloak's logout, which ends the refresh tokens and sessions. The mark comes
/// first, so a failed logout still leaves the person refused.
async fn end_access(state: &AppState, admin: &Admin<'_>, id: &str) -> Result<(), ApiError> {
    revoke(state, id).await;
    admin.sign_out(id).await.map_err(ApiError::from)
}

/// Offboarding (T-5975): a disabled or removed person's connectors are revoked, not only refused
/// while the person stays disabled, so enabling them again revives none. The activity row names
/// how many.
async fn offboard(state: &AppState, identity: &Identity, id: &str) -> Result<(), ApiError> {
    let revoked = state.connectors.revoke_all(id).await?;
    if revoked > 0 {
        let noun = if revoked == 1 {
            "connector"
        } else {
            "connectors"
        };
        let action = format!("revoked {revoked} {noun} of");
        record(state, identity, &action, id).await;
    }
    Ok(())
}

/// Disable or enable, the person as it now is.
async fn set_enabled(
    state: &AppState,
    identity: &Identity,
    id: &str,
    enabled: bool,
) -> Result<Json<Person>, ApiError> {
    let (admin, found) = target(state, identity, Verb::Disable, id).await?;
    if !enabled {
        not_self_nor_last(state, identity, &found)?;
    }
    admin
        .update(&found.id, |rep| rep["enabled"] = json!(enabled))
        .await?;
    if !enabled {
        end_access(state, &admin, &found.id).await?;
        offboard(state, identity, &found.id).await?;
    }
    record(
        state,
        identity,
        if enabled { "enabled" } else { "disabled" },
        &found.id,
    )
    .await;
    let now = admin.get(&found.id).await?;
    Ok(Json(person(&now, None, pending(state, &found.id).await)))
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/disable",
    summary = "Disable Person",
    description = "Disables a person and ends their sessions; never the caller or the last Organization Administrator.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 200, description = "The person, disabled", body = Person),
        (status = 403, description = "The caller lacks disable on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 409, description = "The caller themselves, or the last Organization Administrator", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn disable_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Person>, ApiError> {
    set_enabled(&state, &user.0.identity, &id, false).await
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/enable",
    summary = "Enable Person",
    description = "Enables a disabled person.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 200, description = "The person, enabled", body = Person),
        (status = 403, description = "The caller lacks disable on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn enable_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Person>, ApiError> {
    set_enabled(&state, &user.0.identity, &id, true).await
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/reset-password",
    summary = "Reset Password",
    description = "Sends the realm's password reset; without SMTP answers a temporary password once. Needs `update` on Person and every right the person holds.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 202, description = "The reset e-mail went", body = PasswordReset),
        (status = 200, description = "The realm cannot send mail: a temporary password, once", body = PasswordReset),
        (status = 403, description = "The caller lacks update on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn reset_password(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    // The answer can carry a temporary password: a credential for a person, never for a token.
    refuse_bearer(identity)?;
    let (admin, found) = target(&state, identity, Verb::Update, &id).await?;
    let (email_sent, temporary_password) =
        invite(&state, &admin, &found.id, &["UPDATE_PASSWORD"], None).await?;
    record(&state, identity, "reset the password of", &found.id).await;
    let status = if email_sent {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(PasswordReset {
            email_sent,
            temporary_password,
        }),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/resend-invitation",
    summary = "Resend Invitation",
    description = "Sends the realm's e-mail again for the steps the person has not taken, with a fresh link that leads into the project the first one did. Needs `create` on Person and every right the person holds.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 202, description = "The e-mail went", body = PasswordReset),
        (status = 200, description = "The realm cannot send mail: a temporary password, once", body = PasswordReset),
        (status = 403, description = "The caller lacks create on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 409, description = "The person has no step left: the invitation was accepted", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn resend_invitation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    refuse_bearer(identity)?;
    let (admin, found) = target(&state, identity, Verb::Create, &id).await?;
    if found.required_actions.is_empty() {
        return Err(ApiError::Conflict(
            "the person has taken every step: the invitation was accepted".into(),
        ));
    }
    let project = match state.db.as_ref() {
        Some(pool) => crate::db::person_invitation_project(pool, &found.id)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(person = %found.id, error = %err, "the invitation's project could not be read");
                None
            }),
        None => None,
    };
    let back = welcome_page(&state, project.as_deref());
    let steps: Vec<&str> = found.required_actions.iter().map(String::as_str).collect();
    let (email_sent, temporary_password) = invite(
        &state,
        &admin,
        &found.id,
        &steps,
        back.as_ref().map(|(c, p)| (c.as_str(), p.as_str())),
    )
    .await?;
    if email_sent {
        remember_invitation(&state, &found.id, None).await;
    }
    record(&state, identity, "sent the invitation again to", &found.id).await;
    let status = if email_sent {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(PasswordReset {
            email_sent,
            temporary_password,
        }),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/remove-second-factor",
    summary = "Remove Second Factor",
    description = "Removes every OTP and WebAuthn credential of the person. Needs `disable` on Person and every right the person holds.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "The caller lacks disable on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn remove_second_factor(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    refuse_bearer(identity)?;
    let (admin, found) = target(&state, identity, Verb::Disable, &id).await?;
    admin.remove_second_factor(&found.id).await?;
    record(&state, identity, "removed the second factor of", &found.id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/{id}/sign-out",
    summary = "Sign Person Out",
    description = "Ends every session of a person.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 204, description = "Signed out everywhere"),
        (status = 403, description = "The caller lacks disable on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn sign_out_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    let (admin, found) = target(&state, identity, Verb::Disable, &id).await?;
    end_access(&state, &admin, &found.id).await?;
    record(&state, identity, "signed out", &found.id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/people/{id}/consents/{client}",
    summary = "Revoke A Consent",
    description = "Revokes a person's consent to one client, the hub, an Endpoint's MCP client or an App, which also ends that client's offline sessions of theirs. No grant changes (API/01 §41, T-3792).",
    tag = "people",
    params(
        ("id" = String, Path, description = "The Keycloak user id"),
        ("client" = String, Path, description = "The client id the person consented to"),
    ),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "The caller lacks disable on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person or consent", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client", body = ProblemDetails),
    )
)]
pub async fn revoke_consent(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((id, client)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    refuse_bearer(identity)?;
    let (admin, found) = target(&state, identity, Verb::Disable, &id).await?;
    admin.revoke_consent(&found.id, &client).await?;
    record(
        &state,
        identity,
        &format!("revoked the {client} consent of"),
        &found.id,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// What a person's removal writes: every Group and RoleBinding naming them, without them.
#[derive(Default)]
struct Removal {
    edited: Vec<ResourceEnvelope>,
    removed: Vec<ResourceEnvelope>,
}

fn removal(state: &AppState, people: &[Who]) -> Result<Removal, ApiError> {
    let mut removal = Removal::default();
    let mut elsewhere = Vec::new();
    for mut envelope in state
        .mirror
        .matching(|candidate| matches!(candidate.kind.as_str(), "Group" | "RoleBinding"))
    {
        let namespace = envelope.metadata.namespace.clone().unwrap_or_default();
        let field = if envelope.kind == "Group" {
            "members"
        } else {
            "subjects"
        };
        let Some(list) = envelope.spec.get_mut(field).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = list.len();
        list.retain(|entry| {
            !entry
                .get("user")
                .and_then(Value::as_str)
                .is_some_and(|user| people.iter().any(|who| who.named_by(user)))
        });
        if list.len() == before {
            continue;
        }
        let emptied = list.is_empty() && envelope.kind == "RoleBinding";
        if namespace != ORG_NAMESPACE && state.mirror.repository_of(&namespace).is_some() {
            elsewhere.push(format!(
                "{} {} in project {namespace}",
                envelope.kind, envelope.metadata.name
            ));
            continue;
        }
        envelope.strip_status();
        if emptied {
            removal.removed.push(envelope);
        } else {
            removal.edited.push(envelope);
        }
    }
    if !elsewhere.is_empty() {
        elsewhere.sort();
        return Err(ApiError::Conflict(format!(
            "the people to remove are still named by {}, which live in their project's own \
             repository; take them out there first (PF-93)",
            elsewhere.join(", ")
        )));
    }
    Ok(removal)
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/people/{id}",
    summary = "Delete Person",
    description = "Proposes one Change taking the person out of every Group and RoleBinding and disables them; the Keycloak user is deleted once it is merged. A person nothing names is deleted at once. Needs `delete` on Person and every right the person holds.",
    tag = "people",
    params(("id" = String, Path, description = "The Keycloak user id")),
    responses(
        (status = 202, description = "The Change the deletion waits for", body = Change),
        (status = 204, description = "Deleted: no manifest named the person"),
        (status = 403, description = "The caller lacks delete on Person, or a right the person holds", body = ProblemDetails),
        (status = 404, description = "No such person", body = ProblemDetails),
        (status = 409, description = "The caller, the last Organization Administrator, a removal already pending, or a reference in a project's own repository", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client, forge or database", body = ProblemDetails),
    )
)]
pub async fn delete_person(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    refuse_bearer(identity)?;
    let (admin, found) = target(&state, identity, Verb::Delete, &id).await?;
    not_self_nor_last(&state, identity, &found)?;
    remove(&state, identity, &admin, &[found]).await
}

/// The most people one removal takes: each one is a few realm calls in a row (T-5606).
const REMOVE_AT_ONCE: usize = 200;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RemovePeople {
    /// The Keycloak user ids of the people to remove, 1 to 200.
    pub ids: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/people/remove",
    summary = "Remove People",
    description = "Removes up to 200 people at once, a department leaving: proposes ONE Change taking all of them out of every Group and RoleBinding and disables each of them; their Keycloak users are deleted once it is merged. When nothing names any of them they are deleted at once. The same rules as Delete Person hold for each one: `delete` on Person and every right the person holds, never the caller nor the last Organization Administrator (T-5606).",
    tag = "people",
    request_body(content = RemovePeople, example = json!({ "ids": ["7d1f0c9e-4b8a-4f63-9a51-2c0d8e3b6f14", "0b6f0e4c-1d2e-4f00-9a7b-3c4d5e6f7a8b"] })),
    responses(
        (status = 202, description = "The one Change the removals wait for", body = Change),
        (status = 204, description = "Deleted: no manifest named any of them"),
        (status = 400, description = "The body is not a list of ids", body = ProblemDetails),
        (status = 403, description = "The caller lacks delete on Person, or a right one of the people holds (named)", body = ProblemDetails),
        (status = 404, description = "No such person (named)", body = ProblemDetails),
        (status = 409, description = "The caller, the last Organization Administrator, a removal already pending, or a reference in a project's own repository", body = ProblemDetails),
        (status = 422, description = "No id, or more than 200", body = ProblemDetails),
        (status = 503, description = "No Keycloak admin client, forge or database", body = ProblemDetails),
    )
)]
pub async fn remove_people(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    refuse_bearer(identity)?;
    allow(&state, identity, Verb::Delete)?;
    let ids = removal_ids(&body)?;
    let admin = people(&state)?.admin().await?;
    let found = removable(&state, identity, &admin, &ids).await?;
    remove(&state, identity, &admin, &found).await
}

/// The ids a bulk removal names, sorted and deduplicated: 1 to [`REMOVE_AT_ONCE`] of them.
fn removal_ids(body: &[u8]) -> Result<Vec<String>, ApiError> {
    let RemovePeople { mut ids } = parse(body)?;
    ids.sort();
    ids.dedup();
    if ids.is_empty() || ids.len() > REMOVE_AT_ONCE {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "a removal takes 1 to {REMOVE_AT_ONCE} people, and this one names {}",
                ids.len()
            ),
            errors: vec![format!("/ids: 1 to {REMOVE_AT_ONCE} ids, e.g. [\"6f1c…\"]")],
        });
    }
    Ok(ids)
}

/// Every named person, each held to the single route's rules: an unknown id 404s, a right
/// the caller lacks 403s, the caller or the last administrator 409s.
async fn removable(
    state: &AppState,
    identity: &Identity,
    admin: &Admin<'_>,
    ids: &[String],
) -> Result<Vec<KcUser>, ApiError> {
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        let user = admin.get(id).await.map_err(|err| match err {
            crate::people::PeopleError::NotFound => {
                ApiError::NotFound(format!("the realm has no person {id}"))
            }
            other => other.into(),
        })?;
        holds_what_they_hold(state, identity, admin, &user).await?;
        not_self_nor_last(state, identity, &user)?;
        found.push(user);
    }
    not_every_administrator(state, &found)?;
    Ok(found)
}

/// A removal of several people never takes every Organization Administrator at once, though
/// none of them is the last one alone (PF-03).
fn not_every_administrator(state: &AppState, people: &[KcUser]) -> Result<(), ApiError> {
    let administrators = crate::permissions::administrator_people(&state.mirror);
    let removed: Vec<Who> = people.iter().map(Who::of).collect();
    if !administrators.is_empty()
        && administrators
            .iter()
            .all(|admin| removed.iter().any(|who| who.named_by(admin)))
    {
        return Err(ApiError::Conflict(
            "this removal takes every Organization Administrator; keep one or bind another \
             person to an administrator role first (PF-03)"
                .into(),
        ));
    }
    Ok(())
}

/// The branch a removal's Change is proposed on: one per person, or one per set of people.
fn removal_branch(people: &[KcUser]) -> String {
    use sha2::Digest as _;
    let short: String = match people {
        [one] => one
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect(),
        many => {
            let mut ids: Vec<&str> = many.iter().map(|user| user.id.as_str()).collect();
            ids.sort_unstable();
            let digest = sha2::Sha256::digest(ids.join("\n"));
            let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
            return format!("portal/remove-{}-people-{hex}", many.len());
        }
    };
    format!("portal/remove-person-{short}")
}

/// Removes `people`, each one already checked (PF-93): deleted at once when no manifest names any
/// of them, else ONE Change takes all of them out of every Group and RoleBinding, and each one is
/// disabled, signed out and held as a pending removal until it is merged (T-5606).
async fn remove(
    state: &AppState,
    identity: &Identity,
    admin: &Admin<'_>,
    people: &[KcUser],
) -> Result<Response, ApiError> {
    let named: Vec<Who> = people.iter().map(Who::of).collect();
    let removal = removal(state, &named)?;
    if removal.edited.is_empty() && removal.removed.is_empty() {
        for found in people {
            admin.delete(&found.id).await?;
            revoke(state, &found.id).await;
            offboard(state, identity, &found.id).await?;
            forget_invitation(state, &found.id).await;
            record(state, identity, "deleted", &found.id).await;
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let pool = state.db.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "the person is named in the repository, and without a database the Portal cannot \
             hold their removal until its Change is merged"
                .into(),
        )
    })?;
    for found in people {
        if let Some(open) = crate::db::person_deletion(pool, &found.id)
            .await
            .map_err(db_error)?
        {
            return Err(ApiError::Conflict(format!(
                "a removal of person {} is already open: {}; approve or reject it first",
                found.id, open.change_name
            )));
        }
    }
    let manifests: Vec<Value> = removal
        .edited
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let changes: Vec<_> = manifests
        .iter()
        .map(crate::permissions::AccessChange::Write)
        .chain(removal.removed.iter().map(|envelope| {
            crate::permissions::AccessChange::Remove {
                kind: &envelope.kind,
                namespace: envelope
                    .metadata
                    .namespace
                    .as_deref()
                    .unwrap_or(ORG_NAMESPACE),
                name: &envelope.metadata.name,
            }
        }))
        .collect();
    crate::permissions::keeps_an_administrator_after(&state.mirror, &changes)?;

    let gitea = state
        .forge_for(ORG_NAMESPACE)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let held = crate::api::mutate::turn(&gitea, &removal_branch(people)).await;
    let branch = crate::api::mutate::create_or_reuse_branch(&gitea, &held, &default_branch).await?;
    let path_of = |envelope: &ResourceEnvelope| -> Result<String, ApiError> {
        let info = crate::resource::by_kind(&envelope.kind).ok_or_else(|| {
            ApiError::Internal(format!("no catalogue entry for {}", envelope.kind))
        })?;
        let namespace = envelope
            .metadata
            .namespace
            .as_deref()
            .unwrap_or(ORG_NAMESPACE);
        crate::api::mutate::resolve_repo_path(envelope, info, namespace)
    };
    let mut uploads = Vec::new();
    for envelope in &removal.edited {
        let yaml = serde_norway::to_string(envelope)
            .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;
        uploads.push((path_of(envelope)?, yaml));
    }
    let mut deletes = Vec::new();
    for envelope in &removal.removed {
        let path = path_of(envelope)?;
        if let Some(file) = gitea.get_file(&path, &default_branch).await? {
            deletes.push((path, file.sha));
        }
    }
    let (author_name, author_email) =
        crate::api::mutate::author_credentials(identity, ORG_NAMESPACE);
    // Ids only, never the e-mails: the title and the body are read by everyone with the repository.
    let ids: Vec<&str> = people.iter().map(|found| found.id.as_str()).collect();
    let (title, whom) = match ids.as_slice() {
        [one] => (
            format!("remove person {one} from every group and binding"),
            format!("person {one}"),
        ),
        many => (
            format!("remove {} people from every group and binding", many.len()),
            format!("{} people ({})", many.len(), many.join(", ")),
        ),
    };
    gitea
        .change_files(
            &branch,
            &title,
            Author {
                name: &author_name,
                email: &author_email,
            },
            &uploads,
            &deletes,
        )
        .await?;
    let body = format!(
        "Proposed via joinedcontext Portal. Takes {whom} out of {} manifest(s) and removes {} \
         binding(s) left naming nobody; each Keycloak user is deleted once this is merged \
         (PF-93).",
        removal.edited.len(),
        removal.removed.len()
    );
    let pr = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;
    let meta = crate::api::changes::change_meta(state, &gitea, pr.number, ORG_NAMESPACE);
    let change = Change::new(
        meta,
        ChangeStatus::new(
            Lane::Red,
            ChangePhase::PendingApproval,
            PlanSummary::new(0, removal.edited.len(), removal.removed.len()),
        )
        .in_repository(&pr.repository)
        .proposed_through(&pr.body)
        .with_merge_request(pr.url.clone()),
    );
    for found in people {
        crate::db::insert_person_deletion(
            pool,
            &found.id,
            pr.number,
            &change.metadata.name,
            &identity.username,
        )
        .await
        .map_err(db_error)?;
    }
    // Disabled at once: a person on the way out keeps no session while the Change waits.
    for found in people {
        admin
            .update(&found.id, |rep| rep["enabled"] = json!(false))
            .await?;
        end_access(state, admin, &found.id).await?;
        offboard(state, identity, &found.id).await?;
        record(state, identity, "proposed the removal of", &found.id).await;
    }
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

fn db_error(err: sqlx::Error) -> ApiError {
    tracing::error!(error = %err, "the pending removals could not be read or written");
    ApiError::from_db(&err, "people")
}

/// The reconciler's step for pending removals (PF-93): a merged Change deletes the Keycloak user,
/// a Change closed without merging drops the pending removal and leaves the person disabled for
/// an administrator to enable; an open one waits.
pub async fn finish_deletions(
    people: &People,
    pool: &sqlx::PgPool,
    gitea: &crate::git::GiteaClient,
) {
    let rows = match crate::db::person_deletions(pool).await {
        Ok(rows) if rows.is_empty() => return,
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, "the pending removals could not be read");
            return;
        }
    };
    let admin = match people.admin().await {
        Ok(admin) => admin,
        Err(err) => {
            tracing::warn!(error = ?err, "the realm refused the admin client: pending removals wait");
            return;
        }
    };
    let mut decided: BTreeMap<String, &str> = BTreeMap::new();
    // A department's removal is one Change for many rows: each pull request is read once.
    let mut read: HashMap<u64, (bool, bool)> = HashMap::new();
    for row in rows {
        let Ok(number) = u64::try_from(row.pull_request) else {
            continue;
        };
        let (merged, closed) = match read.get(&number) {
            Some(known) => *known,
            None => match gitea.pull_request(number).await {
                Ok(pr) => *read
                    .entry(number)
                    .or_insert((pr.merged, pr.state == "closed")),
                Err(err) => {
                    tracing::warn!(change = %row.change_name, error = %err, "the removal's Change could not be read");
                    continue;
                }
            },
        };
        if merged {
            match admin.delete(&row.person_id).await {
                Ok(()) | Err(crate::people::PeopleError::NotFound) => {
                    decided.insert(row.person_id, "deleted");
                }
                Err(err) => {
                    tracing::warn!(person = %row.person_id, error = ?err, "the merged removal did not delete the person yet");
                }
            }
        } else if closed {
            decided.insert(row.person_id, "kept");
        }
    }
    for (person, outcome) in decided {
        if outcome == "deleted" {
            if let Err(err) = crate::db::remove_person_invitation(pool, &person).await {
                tracing::warn!(person = %person, error = %err, "the invitation of a deleted person is still remembered");
            }
        }
        if let Err(err) = crate::db::remove_person_deletion(pool, &person).await {
            tracing::warn!(person = %person, error = %err, "the finished removal stays recorded");
        } else {
            tracing::info!(person = %person, outcome, "a pending removal was finished");
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/people", get(list_people).post(create_person))
        .route(
            "/organization/people/{id}",
            get(get_person).patch(edit_person).delete(delete_person),
        )
        .route("/organization/people/remove", post(remove_people))
        .route("/organization/people/{id}/disable", post(disable_person))
        .route("/organization/people/{id}/enable", post(enable_person))
        .route(
            "/organization/people/{id}/reset-password",
            post(reset_password),
        )
        .route(
            "/organization/people/{id}/resend-invitation",
            post(resend_invitation),
        )
        .route(
            "/organization/people/{id}/remove-second-factor",
            post(remove_second_factor),
        )
        .route("/organization/people/{id}/sign-out", post(sign_out_person))
        .route(
            "/organization/people/{id}/consents/{client}",
            delete(revoke_consent),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bulk_removal_names_one_to_two_hundred_distinct_ids() {
        let ids = removal_ids(br#"{"ids":["b","a","b"]}"#).expect("two distinct ids");
        assert_eq!(ids, vec!["a".to_owned(), "b".to_owned()]);
        assert!(matches!(
            removal_ids(br#"{"ids":[]}"#),
            Err(ApiError::Unprocessable { .. })
        ));
        let many: Vec<String> = (0..=REMOVE_AT_ONCE).map(|n| format!("p{n}")).collect();
        let body = serde_json::to_vec(&serde_json::json!({ "ids": many })).expect("json");
        assert!(matches!(
            removal_ids(&body),
            Err(ApiError::Unprocessable { .. })
        ));
        assert!(matches!(removal_ids(b"{}"), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn a_created_person_and_a_reset_never_show_the_temporary_password() {
        let user: KcUser = serde_json::from_value(serde_json::json!({"id": "p1"})).unwrap();
        let created = CreatedPerson {
            person: person(&user, None, None),
            email_sent: false,
            temporary_password: Some("s3cr3t-test".into()),
            member_of: None,
        };
        let reset = PasswordReset {
            email_sent: false,
            temporary_password: Some("s3cr3t-test".into()),
        };
        for text in [format!("{created:?}"), format!("{reset:?}")] {
            assert!(
                !text.contains("s3cr3t-test") && text.contains("[redacted]"),
                "{text}"
            );
        }
    }

    #[test]
    fn an_email_is_trimmed_lowered_and_shaped() {
        assert_eq!(
            email(" Jana.K@Example.ORG ").ok().as_deref(),
            Some("jana.k@example.org")
        );
        for bad in [
            "",
            "jana",
            "@example.org",
            "jana@example",
            "a@b@c.d",
            "jana @x.org",
            "jana@.org",
        ] {
            assert!(email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_name_and_a_locale_are_checked() {
        assert!(name("firstName", "  ").is_err());
        assert!(name("firstName", &"x".repeat(256)).is_err());
        assert!(name("firstName", "a\u{0}b").is_err());
        assert_eq!(name("firstName", " Jana ").ok().as_deref(), Some("Jana"));
        assert!(locale("sk").is_ok());
        assert!(locale("fr").is_err());
    }
}
