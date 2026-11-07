//! What an agent's own ServiceAccount may do, and revoking it at once (API/01 §52, AG-130…AG-132).
//!
//! The Access tab reads one account's grants: each `spec.roles` entry with its scope, its window
//! and where it stands now, and the Portal rules its `Role` carries, `approve` never among them
//! (PF-58). Widening goes through the ordinary propose door as a red-lane access change a second
//! person approves (PF-58, T-4833). Revoking is the one shortcut: a person who may approve
//! ServiceAccounts removes one grant, and the Portal merges that Change at once because it only
//! takes access away, checked against the manifest the forge holds (AG-132).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::{ServiceAccountSpec, Verb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::mutate::ProposeOutcome;
use crate::auth::CurrentUser;
use crate::change::Operation;
use crate::error::ApiError;
use crate::permissions::AccountGrant;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

const KIND: &str = "ServiceAccount";

/// One account's access as its Access tab reads it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountAccess {
    pub account: String,
    pub project: String,
    /// Whether a person's token its client exchanged is decided as that person (AG-95): then the
    /// person's own rights apply on top of none of these.
    pub delegates: bool,
    pub grants: Vec<AccountGrant>,
    /// Whether the caller may propose a change of this account (a widening waits for a second
    /// person, PF-58).
    pub may_propose: bool,
    /// Whether the caller may revoke a grant at once (AG-132).
    pub may_revoke: bool,
    /// Why the account has no client secret the Portal can issue, `workload`, `key-pair` or
    /// `no-credential`, or `None` when it has one (PF-37, T-5626).
    pub no_secret: Option<String>,
    /// Whether the caller may issue a new secret: a Portal session with `approve` (T-5626).
    pub may_issue_secret: bool,
}

/// Which grant to revoke: its position in `spec.roles` and its role, so a list that moved since
/// the page read it removes nothing it did not show.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RevokeRequest {
    pub index: usize,
    pub role: String,
}

fn account(
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(ResourceEnvelope, ServiceAccountSpec), ApiError> {
    let env = state.mirror.get(project, KIND, name).ok_or_else(|| {
        ApiError::NotFound(format!("no ServiceAccount {name} in project {project}"))
    })?;
    let spec = serde_json::from_value(env.spec.clone()).map_err(|e| {
        ApiError::Conflict(format!(
            "ServiceAccount {name} in the repository does not parse ({e}); fix its manifest first"
        ))
    })?;
    Ok((env, spec))
}

fn as_value(env: &ResourceEnvelope) -> Result<Value, ApiError> {
    serde_json::to_value(env).map_err(|e| ApiError::Internal(e.to_string()))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/access",
    summary = "Read An Agent Account's Access",
    description = "Every grant of one ServiceAccount: role, scope, validity window and its state now (active, pending, expired), with the Portal rules its Role carries, never `approve` (API/01 §52, AG-130, AG-131). Needs `read` on ServiceAccount.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
    ),
    responses(
        (status = 200, description = "The account's grants", body = AccountAccess),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "No `read` on ServiceAccount here", body = crate::error::ProblemDetails),
        (status = 404, description = "No such ServiceAccount", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn account_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<AccountAccess>, ApiError> {
    let identity = &user.0.identity;
    let effective = crate::permissions::for_request(&state, identity, &project);
    // `propose` implies `read` (jc-core `Rule::grants`), which `check` alone does not apply.
    if !effective.may_read(KIND) {
        effective.check(KIND, Verb::Read, None)?;
    }
    let (env, spec) = account(&state, &project, &name)?;
    let target = as_value(&env)?;
    let may_propose = effective.check(KIND, Verb::Propose, Some(&target)).is_ok();
    let may_approve = effective.check(KIND, Verb::Approve, Some(&target)).is_ok();
    Ok(Json(AccountAccess {
        grants: crate::permissions::account_access(&state.mirror, &spec, chrono::Utc::now()),
        no_secret: crate::api::account_secret::no_secret(&spec).map(|(code, _)| code.to_owned()),
        may_issue_secret: identity.client.is_none() && may_approve,
        delegates: spec.delegation.is_some(),
        may_revoke: identity.client.is_none() && may_propose && may_approve,
        may_propose,
        account: name,
        project,
    }))
}

/// Whether `head` is `base` with entries of `spec.roles` removed and nothing else changed (AG-132).
pub fn only_removes_grants(base: &Value, head: &Value) -> bool {
    let roles = |v: &Value| v.pointer("/spec/roles").and_then(Value::as_array).cloned();
    let (Some(before), Some(after)) = (roles(base), roles(head)) else {
        return false;
    };
    let mut rest = before.clone();
    for grant in &after {
        let Some(at) = rest.iter().position(|kept| kept == grant) else {
            return false;
        };
        rest.remove(at);
    }
    let without_roles = |v: &Value| {
        let mut v = v.clone();
        if let Some(spec) = v.get_mut("spec").and_then(Value::as_object_mut) {
            spec.remove("roles");
        }
        v.as_object_mut().map(|o| o.remove("status"));
        v
    };
    after.len() < before.len() && without_roles(base) == without_roles(head)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/revoke",
    summary = "Revoke An Agent Account's Grant Now",
    description = "Removes one grant of a ServiceAccount at once (API/01 §52, AG-132): a person who may propose and approve ServiceAccounts sends it from their Portal session, the Portal writes the Change and merges it because it only removes access, checked against the manifest on the main branch; recorded as an `access.revoked` activity event. A token of a service account or an agent is refused. The account's next token is decided without the grant once the Portal has synced.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
    ),
    request_body(content = RevokeRequest, content_type = "application/json", example = json!({ "index": 1, "role": "space-writer" })),
    responses(
        (status = 202, description = "The Change, deploying when merged, pending when the forge held something else", body = crate::change::Change),
        (status = 400, description = "Malformed body", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, or no `propose` and `approve` on ServiceAccount here", body = crate::error::ProblemDetails),
        (status = 404, description = "No such ServiceAccount", body = crate::error::ProblemDetails),
        (status = 409, description = "The grant moved, or it is the account's last one", body = crate::error::ProblemDetails),
    )
)]
pub async fn revoke_grant(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Result<Json<RevokeRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a person revokes access in the Portal; a service account's or an agent's token never changes access (PF-58, AG-11)".into(),
        ));
    }
    let effective = crate::permissions::for_request(&state, identity, &project);
    // The kind first, so a caller who may not revoke learns nothing of the account (PF-59).
    for verb in [Verb::Propose, Verb::Approve] {
        if !effective.may(KIND, verb) {
            effective.check(KIND, verb, None)?;
        }
    }
    crate::auth::session::require_second_factor(identity, &state.config)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let (env, spec) = account(&state, &project, &name)?;
    let target = as_value(&env)?;
    effective.check(KIND, Verb::Propose, Some(&target))?;
    effective.check(KIND, Verb::Approve, Some(&target))?;
    if spec.roles.get(request.index).map(|g| g.role.as_str()) != Some(request.role.as_str()) {
        return Err(ApiError::Conflict(format!(
            "ServiceAccount {name} has no grant {} at position {}; reload its Access tab",
            request.role, request.index
        )));
    }
    if spec.roles.len() == 1 {
        return Err(ApiError::Conflict(format!(
            "{} is the last grant of ServiceAccount {name}; an account holds at least one, so delete the account to take all its access away",
            request.role
        )));
    }
    let who = identity
        .email
        .as_deref()
        .unwrap_or(&identity.username)
        .to_owned();
    let change = remove_grants(
        &state,
        identity,
        &project,
        &name,
        target,
        &[request.index],
        &format!(
            "revoke {} from ServiceAccount {name}\n\nRevoked in the Portal by {who}",
            request.role
        ),
    )
    .await?;
    record(&state, identity, &project, &name, &request.role, &change).await;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// Proposes `manifest` without its grants at `indices` and merges the Change at once when the
/// forge confirms it only removes them (AG-132); otherwise it stays pending for a second person.
async fn remove_grants(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    name: &str,
    mut manifest: Value,
    indices: &[usize],
    what: &str,
) -> Result<crate::change::Change, ApiError> {
    if let Some(roles) = manifest
        .pointer_mut("/spec/roles")
        .and_then(Value::as_array_mut)
    {
        let mut at = 0;
        roles.retain(|_| {
            at += 1;
            !indices.contains(&(at - 1))
        });
    }
    if let Some(object) = manifest.as_object_mut() {
        object.remove("status");
    }
    let outcome = crate::api::mutate::propose_with_identity(
        identity,
        state,
        project,
        "serviceaccounts",
        Some(name),
        Operation::Update,
        false,
        manifest,
    )
    .await?;
    let ProposeOutcome::Change(change) = outcome else {
        return Err(ApiError::Internal(
            "removing a grant proposed no Change".into(),
        ));
    };
    let message = format!(
        "Merge change proposal {}: {what}; it only removes access (AG-132)",
        change.metadata.name
    );
    Ok(crate::api::changes::merge_if_narrowing(
        state,
        project,
        change,
        &message,
        |base, head| match (serde_json::to_value(base), serde_json::to_value(head)) {
            (Ok(base), Ok(head)) => only_removes_grants(&base, &head),
            _ => false,
        },
    )
    .await)
}

/// The grants of `spec` the reaper removes at `now`: every expired one, but never the last grant
/// standing, which an account must hold and which grants nothing once expired (AG-131).
pub fn reapable(spec: &ServiceAccountSpec, now: chrono::DateTime<chrono::Utc>) -> Vec<usize> {
    let mut expired: Vec<usize> = spec
        .roles
        .iter()
        .enumerate()
        .filter(|(_, grant)| {
            grant
                .validity
                .as_ref()
                .and_then(|v| v.not_after)
                .is_some_and(|to| to < now)
        })
        .map(|(at, _)| at)
        .collect();
    if expired.len() == spec.roles.len() {
        expired.pop();
    }
    expired
}

/// Removes every expired grant of every ServiceAccount once a minute, on the replica that holds
/// the reconciler lock (AG-131). The grant already granted nothing after its `notAfter`; this
/// takes it out of the manifest so the Access tab and the repository agree. The Portal writes it
/// under its own name, as a member of the bootstrap group, and the Change merges only when the
/// forge confirms it removes access and adds none.
pub fn spawn_grant_reaper(state: AppState) {
    tokio::spawn(async move {
        let mut every_minute = tokio::time::interval(std::time::Duration::from_secs(60));
        every_minute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut tried = std::collections::HashMap::new();
        loop {
            every_minute.tick().await;
            if !state.leads() {
                continue;
            }
            reap_expired_grants(&state, chrono::Utc::now(), &mut tried).await;
        }
    });
}

/// One pass of [`spawn_grant_reaper`]. `tried` holds when each account was last written: a
/// Change still pending (the forge refused the merge) is not proposed again for an hour.
pub async fn reap_expired_grants(
    state: &AppState,
    now: chrono::DateTime<chrono::Utc>,
    tried: &mut std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
) {
    let portal = crate::auth::Identity {
        subject: "joinedcontext-portal".to_owned(),
        username: "joinedcontext-portal".to_owned(),
        email: None,
        name: Some("joinedcontext Portal".to_owned()),
        roles: Vec::new(),
        groups: vec![state.config.bootstrap_admins.clone()],
        client: None,
        second_factor: false,
    };
    for project in state.mirror.namespaces() {
        let accounts = state
            .mirror
            .list(&project, KIND, &crate::store::ListOptions::default())
            .items;
        for env in accounts {
            let Ok(spec) = serde_json::from_value::<ServiceAccountSpec>(env.spec.clone()) else {
                continue;
            };
            let expired = reapable(&spec, now);
            if expired.is_empty() {
                continue;
            }
            let name = env.metadata.name.clone();
            let key = format!("{project}/{name}");
            if tried
                .get(&key)
                .is_some_and(|at| now - *at < chrono::Duration::hours(1))
            {
                continue;
            }
            tried.insert(key, now);
            let Ok(manifest) = as_value(&env) else {
                continue;
            };
            match remove_grants(
                state,
                &portal,
                &project,
                &name,
                manifest,
                &expired,
                &format!(
                    "remove {} expired grant(s) from ServiceAccount {name}",
                    expired.len()
                ),
            )
            .await
            {
                Ok(change) => {
                    tracing::info!(%project, account = %name, change = %change.metadata.name, "expired grants removed")
                }
                Err(err) => {
                    tracing::warn!(%project, account = %name, error = %err, "expired grants not removed; they grant nothing meanwhile")
                }
            }
        }
    }
}

/// Who revoked what (AG-132). A record that cannot be written is logged, not a failure.
async fn record(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    account: &str,
    role: &str,
    change: &crate::change::Change,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "access.revoked".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} revoked {role} from ServiceAccount {account}",
            identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "actor": identity.username,
            "account": account,
            "role": role,
            "change": change.metadata.name,
            "phase": change.status.phase,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the revoke was not recorded");
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/serviceaccounts/{name}/access",
            get(account_access),
        )
        .route(
            "/projects/{project}/serviceaccounts/{name}/revoke",
            post(revoke_grant),
        )
}

#[cfg(test)]
mod tests {
    use super::only_removes_grants;
    use serde_json::json;

    fn sa(roles: serde_json::Value, purpose: &str) -> serde_json::Value {
        json!({ "kind": "ServiceAccount", "metadata": { "name": "bot" },
                "spec": { "purpose": purpose, "roles": roles } })
    }

    #[test]
    fn removing_one_grant_narrows() {
        let a = json!({ "role": "reader", "scope": { "project": "p" } });
        let b = json!({ "role": "writer", "scope": { "project": "p" } });
        assert!(only_removes_grants(
            &sa(json!([a, b]), "x"),
            &sa(json!([a]), "x")
        ));
    }

    #[test]
    fn adding_changing_or_keeping_every_grant_does_not() {
        let a = json!({ "role": "reader", "scope": { "project": "p" } });
        let b = json!({ "role": "writer", "scope": { "project": "p" } });
        let wide = json!({ "role": "writer", "scope": { "organization": "org" } });
        let base = sa(json!([a, b]), "x");
        assert!(
            !only_removes_grants(&base, &sa(json!([a, b]), "x")),
            "nothing removed"
        );
        assert!(
            !only_removes_grants(&base, &sa(json!([a, wide]), "x")),
            "a scope widened"
        );
        assert!(
            !only_removes_grants(&base, &sa(json!([wide]), "x")),
            "swapped for a wider one"
        );
        assert!(
            !only_removes_grants(&base, &sa(json!([a]), "y")),
            "another field changed too"
        );
        assert!(
            !only_removes_grants(&base, &json!({ "spec": {} })),
            "no roles at all"
        );
    }

    fn spec(roles: serde_json::Value) -> jc_core::kinds::ServiceAccountSpec {
        serde_json::from_value(json!({
            "owner": { "user": "jana" }, "purpose": "test", "roles": roles,
            "credentials": [{ "kind": "oauth-client", "name": "main" }]
        }))
        .unwrap()
    }

    #[test]
    fn the_reaper_takes_expired_grants_and_never_the_last_one() {
        use super::reapable;
        let now: chrono::DateTime<chrono::Utc> = "2026-10-21T12:00:00Z".parse().unwrap();
        let ended = json!({ "role": "writer", "scope": { "project": "p" }, "validity": { "notAfter": "2026-10-21T11:59:59Z" } });
        let ends_now = json!({ "role": "writer", "scope": { "project": "q" }, "validity": { "notAfter": "2026-10-21T12:00:00Z" } });
        let open = json!({ "role": "reader", "scope": { "project": "p" } });
        let later = json!({ "role": "reader", "scope": { "project": "q" }, "validity": { "notBefore": "2026-10-22T00:00:00Z" } });
        assert_eq!(
            reapable(&spec(json!([open, ended, ends_now, later])), now),
            vec![1]
        );
        assert_eq!(
            reapable(&spec(json!([ended, ended])), now),
            vec![0],
            "one stays"
        );
        assert!(reapable(&spec(json!([open])), now).is_empty());
    }

    #[tokio::test]
    async fn a_bearer_token_never_revokes() {
        use crate::auth::session::{Identity, Session};
        let config = crate::config::Config::for_tests();
        let groups = vec![config.bootstrap_admins.clone()];
        let state = crate::state::AppState::new(config, None);
        let agent = crate::auth::CurrentUser(Session {
            identity: Identity {
                client: Some("ovzdusie-vendor-ci".into()),
                subject: "sa".into(),
                username: "service-account-ovzdusie-vendor-ci".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups,
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        });
        let refused = super::revoke_grant(
            agent,
            axum::extract::State(state),
            axum::extract::Path(("ovzdusie".to_owned(), "vendor-ci".to_owned())),
            Ok(axum::Json(super::RevokeRequest {
                index: 0,
                role: "x".into(),
            })),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(refused, crate::error::ApiError::Denied(_)),
            "{refused:?}"
        );
    }
}
