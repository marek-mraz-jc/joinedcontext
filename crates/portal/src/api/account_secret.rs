//! Issuing a ServiceAccount's client secret, shown once (PF-37, API/01 §52, T-5626).
//!
//! An account without a workload and without device keys signs in from outside the cluster with
//! the confidential client `{project}-{name}` the reconciler writes (`workload_clients`). Keycloak
//! generates its secret and nobody has seen it: a person who may approve ServiceAccounts asks this
//! door for a new one from their Portal session, the Portal has Keycloak regenerate it and hands
//! it back in this one answer. The Portal keeps no copy, logs none and has no route that reads it
//! again. The previous secret is refused from that moment, the tokens the account holds are
//! refused at the gateway's next revocation poll, and an `access.revoked` activity event names who
//! did it. Issuing and rotating are the same act.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::service_account::keycloak_client_id;
use jc_core::kinds::{ServiceAccountSpec, Verb};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::reconciler::app_clients::{managed, Admin};
use crate::reconciler::workload_clients::{ClientCredential, ACCOUNT_ATTRIBUTE};
use crate::state::AppState;

const KIND: &str = "ServiceAccount";

/// A new client secret, in this answer and nowhere else.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedSecret {
    /// The Keycloak client the external system signs in as, `{project}-{name}`.
    pub client_id: String,
    /// The secret, shown once: the Portal keeps no copy.
    pub client_secret: String,
    /// Where the external system posts `grant_type=client_credentials`.
    pub token_url: String,
}

/// The secret stays out of every `{:?}`.
impl std::fmt::Debug for IssuedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedSecret")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .finish()
    }
}

/// Why an account has no secret to issue, as a code (`key-pair`, `workload`, `no-credential`) and
/// a sentence, or `None` when it has one (PF-37, PF-47).
pub fn no_secret(spec: &ServiceAccountSpec) -> Option<(&'static str, &'static str)> {
    match ClientCredential::of(spec) {
        Some(ClientCredential::Secret) => None,
        Some(ClientCredential::Keys(_)) => Some((
            "key-pair",
            "signs in with its own key pair, so it has no secret; rotate a key by changing its credential's jwks",
        )),
        None if spec.workload.is_some() => Some((
            "workload",
            "is bound to a cluster workload, whose pod token opens its client; it has no secret",
        )),
        None => Some((
            "no-credential",
            "has no oauth-client credential; add one to give it a client",
        )),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/serviceaccounts/{name}/secret",
    summary = "Issue A New Client Secret",
    description = "Has Keycloak generate a new secret for the ServiceAccount's client `{project}-{name}` and answers it once (PF-37, API/01 §52): the Portal keeps no copy and no route reads it again. The previous secret is refused at once, the tokens the account holds are refused at the gateway's next revocation poll, and an `access.revoked` activity event records it. Only a person in a Portal session with `approve` on ServiceAccount; a token of a service account or an agent is refused. The account needs an `oauth-client` credential without `jwks` and no `workload`, and its client must be the one the Portal manages.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "ServiceAccount name"),
    ),
    responses(
        (status = 200, description = "The new secret, shown once", body = IssuedSecret),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, or no `approve` on ServiceAccount here", body = crate::error::ProblemDetails),
        (status = 404, description = "No such ServiceAccount", body = crate::error::ProblemDetails),
        (status = 409, description = "The account has no secret (a workload, a key pair, no credential), or its client is not the Portal's (not created yet, or the platform's own)", body = crate::error::ProblemDetails),
        (status = 503, description = "Keycloak cannot be reached, or the Portal manages no client", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn issue_secret(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    authorize(&state, identity, &project, &name)?;
    let oidc = state.config.oidc.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no sign-in realm, so it manages no client".into())
    })?;
    let (client_id, client_secret) = regenerate(&state, oidc, &project, &name).await?;
    record(&state, identity, &project, &name, &client_id).await;
    let token_url = format!(
        "{}/protocol/openid-connect/token",
        oidc.issuer.as_str().trim_end_matches('/')
    );
    let mut response = Json(IssuedSecret {
        client_id,
        client_secret,
        token_url,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// A person's session with `approve` on the account, and an account that signs in with a secret.
fn authorize(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a person issues a client secret in the Portal; a service account's or an agent's token never does (PF-58)".into(),
        ));
    }
    let effective = crate::permissions::for_request(state, identity, project);
    // The kind first, so a caller who may not issue learns nothing of the account (PF-59).
    if !effective.may(KIND, Verb::Approve) {
        effective.check(KIND, Verb::Approve, None)?;
    }
    crate::auth::session::require_second_factor(identity, &state.config)?;
    let env = state.mirror.get(project, KIND, name).ok_or_else(|| {
        ApiError::NotFound(format!("no ServiceAccount {name} in project {project}"))
    })?;
    let target = serde_json::to_value(&env).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check(KIND, Verb::Approve, Some(&target))?;
    let spec: ServiceAccountSpec = serde_json::from_value(env.spec).map_err(|e| {
        ApiError::Conflict(format!(
            "ServiceAccount {name} in the repository does not parse ({e}); fix its manifest first"
        ))
    })?;
    match no_secret(&spec) {
        Some((_, why)) => Err(ApiError::Conflict(format!("ServiceAccount {name} {why}"))),
        None => Ok(()),
    }
}

/// Has Keycloak generate a new secret for the account's own managed client and ends the tokens
/// the old one got; answers the client id and the secret. A client this account's Portal did not
/// write (missing, or the platform's) is refused before anything changes.
async fn regenerate(
    state: &AppState,
    oidc: &crate::config::OidcConfig,
    project: &str,
    name: &str,
) -> Result<(String, String), ApiError> {
    let admin = Admin::new(
        &oidc.backchannel(),
        oidc.client_id.clone(),
        oidc.auth.clone(),
    )
    .ok_or_else(|| {
        ApiError::Unavailable("the issuer is not a realm URL, so no client is managed".into())
    })?;
    let unreachable = |err: String| {
        tracing::warn!(%project, account = %name, error = %err, "client secret not issued");
        ApiError::Unavailable("Keycloak did not answer; try again in a minute".into())
    };
    let token = admin.token().await.map_err(unreachable)?;
    let client_id = keycloak_client_id(project, name);
    let client = admin
        .find(&token, &client_id)
        .await
        .map_err(unreachable)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "client {client_id} does not exist yet; the Portal creates it within a minute of the account's merge, then try again"
            ))
        })?;
    let Some(uuid) = owned_by(&client, &format!("{project}/{name}")) else {
        return Err(ApiError::Conflict(format!(
            "client {client_id} belongs to the platform's deployment, not to this account in the Portal; rotate it as runbook 4 says"
        )));
    };
    let issued = admin
        .post_for(&token, &format!("/clients/{uuid}/client-secret"))
        .await
        .map_err(unreachable)?;
    let Some(secret) = issued.get("value").and_then(Value::as_str) else {
        return Err(unreachable("the realm answered no secret".into()));
    };
    end_tokens(state, &admin, &token, uuid).await;
    Ok((client_id, secret.to_owned()))
}

/// The client's id when the Portal manages it for the account `label` (`{project}/{name}`).
fn owned_by<'a>(client: &'a Value, label: &str) -> Option<&'a str> {
    let account = client
        .get("attributes")
        .and_then(|a| a.get(ACCOUNT_ATTRIBUTE))
        .and_then(Value::as_str);
    (managed(client) && account == Some(label))
        .then(|| client.get("id").and_then(Value::as_str))
        .flatten()
}

/// Refuses every token the account's service-account user holds until now (T-5568). A lookup
/// that fails leaves them valid until they expire, which the log says; the new secret stands.
async fn end_tokens(state: &AppState, admin: &Admin, token: &str, uuid: &str) {
    match admin
        .get::<Value>(token, &format!("/clients/{uuid}/service-account-user"))
        .await
    {
        Ok(user) => {
            if let Some(subject) = user.get("id").and_then(Value::as_str) {
                state
                    .revoke_subject(subject, crate::auth::session::now_unix())
                    .await;
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "the account's earlier tokens stay valid until they expire")
        }
    }
}

/// Who issued the secret (PF-37). A record that cannot be written is logged, not a failure.
async fn record(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    account: &str,
    client_id: &str,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "access.revoked".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} issued a new client secret for ServiceAccount {account}; the previous secret and its tokens are refused",
            identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "actor": identity.username,
            "account": account,
            "client": client_id,
            "credential": "client-secret",
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the new secret was not recorded");
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/serviceaccounts/{name}/secret",
        post(issue_secret),
    )
}

#[cfg(test)]
mod tests {
    use super::{owned_by, IssuedSecret};
    use serde_json::json;

    #[test]
    fn only_the_accounts_own_managed_client_is_issued_a_secret() {
        let ours = json!({ "id": "u1", "attributes": {
            "managed-by": "joinedcontext", "joinedcontext.serviceaccount": "p/vendor" } });
        assert_eq!(owned_by(&ours, "p/vendor"), Some("u1"));
        assert_eq!(owned_by(&ours, "p/other"), None);
        let platform =
            json!({ "id": "u2", "attributes": { "joinedcontext.serviceaccount": "p/vendor" } });
        assert_eq!(owned_by(&platform, "p/vendor"), None);
    }

    #[test]
    fn a_debug_line_never_carries_the_secret() {
        let issued = IssuedSecret {
            client_id: "p-vendor".into(),
            client_secret: "s3cret-value".into(),
            token_url: "https://idm/realms/r/protocol/openid-connect/token".into(),
        };
        let line = format!("{issued:?}");
        assert!(line.contains("p-vendor"), "{line}");
        assert!(!line.contains("s3cret-value"), "{line}");
    }
}
