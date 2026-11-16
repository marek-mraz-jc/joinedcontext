#![forbid(unsafe_code)]
//! Upstream OIDC identity provider routes (`/realms/{realm}/broker/{alias}/login` and
//! `/realms/{realm}/broker/{alias}/endpoint`, W7, W11, K12).

use crate::broker::{UpstreamIdTokenClaims, UpstreamOidcBroker, idp_link_attribute};
use crate::client_ip::ClientIp;
use crate::handlers::OidcState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Form, Router};
use chrono::Utc;
use jc_router::Mount;
use kaisatsu_core::i18n::{Bundle, negotiate_for_realm};
use kaisatsu_core::page;
use kaisatsu_core::*;
use kaisatsu_crypto::{
    constant_time_str_eq, random_handle_256, sha256_hex, verify_and_maybe_rehash,
    verify_jwt_with_jwk,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingBrokerSession {
    pub idp_alias: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_claims: Option<UpstreamIdTokenClaims>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_existing_user_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerLoginQuery {
    pub session_code: String,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerEndpointQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerConfirmForm {
    pub csrf_token: String,
    pub session_code: String,
    #[serde(default)]
    pub password: Option<String>,
}

pub fn broker_router() -> Router<OidcState> {
    Router::new()
        .route(
            "/realms/{realm}/broker/{alias}/login",
            get(broker_login_get),
        )
        .route(
            "/realms/{realm}/broker/{alias}/endpoint",
            get(broker_endpoint_get).post(broker_endpoint_confirm_post),
        )
}

fn verify_auth_cookie(headers: &HeaderMap, auth_session: &AuthSession) -> Result<()> {
    if !crate::handlers::auth_cookie_holds(headers, &auth_session.handle_hash) {
        return Err(KaisatsuError::BadRequest(
            "mismatched auth session cookie".into(),
        ));
    }
    Ok(())
}

/// `GET /realms/{realm}/broker/{alias}/login?session_code=...` (K12):
/// Binds `state`, `nonce`, and PKCE S256 `pkce_verifier` to the caller's `AuthSession` and
/// redirects to the upstream OIDC authorization endpoint.
async fn broker_login_get(
    State(state): State<OidcState>,
    Path((realm_name, alias)): Path<(String, String)>,
    Query(query): Query<BrokerLoginQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let mut auth_session = state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
        .ok_or_else(|| KaisatsuError::BadRequest("login session expired".into()))?;
    verify_auth_cookie(&headers, &auth_session)?;

    let idp = state
        .store
        .get_idp(&realm.id, &alias)
        .await?
        .filter(|i| i.enabled)
        .ok_or_else(|| KaisatsuError::NotFound("identity provider not found".into()))?;

    let state_nonce = random_handle_256();
    let broker_state = format!("{}.{}", auth_session.id, state_nonce);
    let oidc_nonce = random_handle_256();
    let pkce_verifier = format!("{}{}", random_handle_256(), random_handle_256());
    let callback_url = format!(
        "{}/realms/{}/broker/{}/endpoint",
        mount.public_base(),
        realm.realm,
        idp.alias
    );

    let broker = UpstreamOidcBroker::new(state.guard.clone());
    let redirect = broker.start_login(
        &idp,
        &callback_url,
        &broker_state,
        &oidc_nonce,
        &pkce_verifier,
    )?;

    let pending = PendingBrokerSession {
        idp_alias: idp.alias.clone(),
        state: broker_state,
        nonce: oidc_nonce,
        pkce_verifier,
        pending_claims: None,
        pending_existing_user_id: None,
    };
    auth_session.broker_state_json = Some(serde_json::to_string(&pending).unwrap_or_default());
    state.store.put_auth_session(auth_session).await?;

    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, redirect.authorization_url),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        "",
    )
        .into_response())
}

/// `GET /realms/{realm}/broker/{alias}/endpoint?code=...&state=...` (K12):
/// Verifies `state` and auth session cookie, exchanges `code` with PKCE `code_verifier` through
/// `OutboundGuard`, verifies the upstream ID token against JWKS and `nonce`, and either signs in
/// a linked account, prompts for first-broker-login link confirmation, or creates a new person.
async fn broker_endpoint_get(
    State(state): State<OidcState>,
    Path((realm_name, alias)): Path<(String, String)>,
    Query(query): Query<BrokerEndpointQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));

    let presented_state = query
        .state
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::BadRequest("missing broker state".into()))?;
    let (session_id, _) = presented_state
        .split_once('.')
        .ok_or_else(|| KaisatsuError::BadRequest("malformed broker state".into()))?;

    let mut auth_session = state
        .store
        .get_auth_session_by_id(&realm.id, session_id)
        .await?
        .ok_or_else(|| KaisatsuError::BadRequest("login session expired".into()))?;
    verify_auth_cookie(&headers, &auth_session)?;

    let mut pending: PendingBrokerSession = serde_json::from_str(
        auth_session
            .broker_state_json
            .as_deref()
            .ok_or_else(|| KaisatsuError::BadRequest("no broker login in flight".into()))?,
    )
    .map_err(|_| KaisatsuError::BadRequest("invalid broker state".into()))?;

    if pending.idp_alias != alias || !constant_time_str_eq(presented_state, &pending.state) {
        return Err(KaisatsuError::BadRequest("broker state mismatch".into()));
    }

    let code = query
        .code
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            KaisatsuError::BadRequest("missing authorization code from upstream".into())
        })?;

    let idp = state
        .store
        .get_idp(&realm.id, &alias)
        .await?
        .filter(|i| i.enabled)
        .ok_or_else(|| KaisatsuError::NotFound("identity provider not found".into()))?;

    let token_url = idp
        .config
        .get("tokenUrl")
        .ok_or_else(|| KaisatsuError::BadRequest("IdP missing tokenUrl".into()))?;
    let client_id = idp
        .config
        .get("clientId")
        .ok_or_else(|| KaisatsuError::BadRequest("IdP missing clientId".into()))?;
    let client_secret = idp
        .config
        .get("clientSecret")
        .map(String::as_str)
        .unwrap_or_default();
    let callback_url = format!(
        "{}/realms/{}/broker/{}/endpoint",
        mount.public_base(),
        realm.realm,
        idp.alias
    );

    let (status, token_bytes) = state
        .guard
        .post_form(
            token_url,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &callback_url),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code_verifier", &pending.pkce_verifier),
            ],
        )
        .await?;
    if !(200..300).contains(&status) {
        return Err(KaisatsuError::Unauthorized(
            "upstream token exchange failed".into(),
        ));
    }
    let token_resp: Value = serde_json::from_slice(&token_bytes)
        .map_err(|_| KaisatsuError::Unauthorized("invalid upstream token JSON".into()))?;
    let id_token = token_resp
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::Unauthorized("upstream response missing id_token".into()))?;

    // Verify upstream ID token against JWKS (iss, aud, nonce, exp, signature)
    let jwks_url = idp
        .config
        .get("jwksUrl")
        .ok_or_else(|| KaisatsuError::BadRequest("IdP missing jwksUrl".into()))?;
    let issuer = idp.config.get("issuer").map(String::as_str);
    let jwks: Value = state.guard.get_json(jwks_url, None, None).await?;
    let keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| KaisatsuError::Unauthorized("upstream JWKS has no keys".into()))?;

    let mut verified_claims: Option<UpstreamIdTokenClaims> = None;
    for jwk in keys {
        if let Ok((_, claims)) = verify_jwt_with_jwk::<UpstreamIdTokenClaims>(
            id_token,
            jwk,
            issuer,
            Some(client_id.as_str()),
        ) {
            verified_claims = Some(claims);
            break;
        }
    }
    let claims = verified_claims.ok_or_else(|| {
        KaisatsuError::Unauthorized("upstream ID token did not verify against JWKS".into())
    })?;
    if claims
        .nonce
        .as_deref()
        .is_none_or(|n| !constant_time_str_eq(n, &pending.nonce))
    {
        return Err(KaisatsuError::Unauthorized(
            "upstream ID token nonce mismatch".into(),
        ));
    }

    // 1. Check if this upstream subject is already linked via `IdentityProviderLinkRecord` or user attribute.
    if let Some(link) = state
        .store
        .get_idp_link_by_external(&realm.id, &idp.alias, &claims.sub)
        .await?
        && let Some(user) = state
            .store
            .get_user_by_id(&realm.id, None, &link.user_id)
            .await?
            .filter(|u| u.enabled)
    {
        return hand_to_login(&state, &mount, &realm, auth_session, &user, &idp.alias).await;
    }

    // 2. First broker login: requires verified email and `idp.trust_email`.
    if !idp.trust_email || claims.email_verified != Some(true) {
        return Err(KaisatsuError::Forbidden(
            "upstream identity provider email is not verified or not trusted".into(),
        ));
    }
    let email = claims
        .email
        .as_deref()
        .filter(|e| !e.is_empty())
        .ok_or_else(|| KaisatsuError::Forbidden("upstream token carries no email".into()))?;

    if let Some(existing) = state
        .store
        .get_user_by_username_or_email(&realm.id, None, email)
        .await?
    {
        let attr_key = idp_link_attribute(&idp.alias);
        let already_linked = existing
            .attributes
            .get(&attr_key)
            .is_some_and(|v| v.iter().any(|s| s == &claims.sub));
        if already_linked && existing.enabled {
            return hand_to_login(&state, &mount, &realm, auth_session, &existing, &idp.alias)
                .await;
        }
        // Existing account is not yet linked: require explicit link confirmation (POST + CSRF),
        // and require local password verification when the existing account has a password.
        let creds = state
            .store
            .list_credentials(&realm.id, &existing.id)
            .await?;
        let needs_password = creds.iter().any(|c| c.credential_type == "password");
        pending.pending_claims = Some(claims);
        pending.pending_existing_user_id = Some(existing.id.clone());
        auth_session.broker_state_json = Some(serde_json::to_string(&pending).unwrap_or_default());
        state.store.put_auth_session(auth_session.clone()).await?;
        return render_broker_link_confirm_page(
            &state,
            &mount,
            &realm,
            &headers,
            &idp.alias,
            &auth_session,
            &existing.username,
            needs_password,
            false,
        )
        .await;
    }

    // 3. Create new person and link record when no existing account matches.
    let attr_key = idp_link_attribute(&idp.alias);
    let created = state
        .store
        .create_user(UserRepresentation {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            // Only the upstream IdP's own Organization: the client signed into grants no membership.
            org_id: idp.org_id.clone(),
            username: email.to_ascii_lowercase(),
            email: Some(email.to_ascii_lowercase()),
            first_name: claims.given_name.clone(),
            last_name: claims.family_name.clone(),
            enabled: true,
            email_verified: true,
            attributes: HashMap::from([(attr_key, vec![claims.sub.clone()])]),
            ..Default::default()
        })
        .await?;
    state
        .store
        .upsert_idp_link(IdentityProviderLinkRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: created.org_id.clone(),
            user_id: created.id.clone(),
            idp_alias: idp.alias.clone(),
            external_user_id: claims.sub,
            external_username: claims.preferred_username.or(claims.email),
            created_at: Utc::now(),
        })
        .await?;

    hand_to_login(&state, &mount, &realm, auth_session, &created, &idp.alias).await
}

/// `POST /realms/{realm}/broker/{alias}/endpoint` (K12):
/// Confirms linking an upstream identity to an existing local account after verifying the CSRF
/// token, auth session cookie, and (when the existing account has a password) the local password.
async fn broker_endpoint_confirm_post(
    State(state): State<OidcState>,
    Path((realm_name, alias)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    client_ip: ClientIp,
    Form(form): Form<BrokerConfirmForm>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let auth_session = state
        .store
        .get_auth_session_by_id(&realm.id, &form.session_code)
        .await?
        .ok_or_else(|| KaisatsuError::BadRequest("login session expired".into()))?;
    verify_auth_cookie(&headers, &auth_session)?;

    if !constant_time_str_eq(&form.csrf_token, &auth_session.csrf_token) {
        return Err(KaisatsuError::Forbidden("invalid CSRF token".into()));
    }

    let pending: PendingBrokerSession = serde_json::from_str(
        auth_session
            .broker_state_json
            .as_deref()
            .ok_or_else(|| KaisatsuError::BadRequest("no pending broker link".into()))?,
    )
    .map_err(|_| KaisatsuError::BadRequest("invalid pending broker state".into()))?;
    if pending.idp_alias != alias {
        return Err(KaisatsuError::BadRequest("broker alias mismatch".into()));
    }
    let claims = pending
        .pending_claims
        .ok_or_else(|| KaisatsuError::BadRequest("no pending broker claims".into()))?;
    let existing_id = pending
        .pending_existing_user_id
        .ok_or_else(|| KaisatsuError::BadRequest("no pending account to link".into()))?;

    let mut existing = state
        .store
        .get_user_by_id(&realm.id, None, &existing_id)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::Unauthorized("account not found".into()))?;

    let creds = state
        .store
        .list_credentials(&realm.id, &existing.id)
        .await?;
    if let Some(pw_cred) = creds.iter().find(|c| c.credential_type == "password") {
        let submitted = form.password.as_deref().unwrap_or_default();
        // The login page's brute-force protector: a locked account gets the wrong-password
        // answer after the same hashing work, and each wrong password counts.
        let bf = crate::brute_force::BruteForceProtector::new(state.store.clone());
        let ip = client_ip.as_deref();
        let locked = bf.is_locked(&realm, Some(&existing.id), ip).await?;
        let outcome = {
            let (submitted, pw_cred) = (submitted.to_owned(), pw_cred.clone());
            crate::handlers::password_work(move || verify_and_maybe_rehash(&submitted, &pw_cred))
                .await??
        };
        let valid = outcome.valid && !locked;
        if !valid {
            if !locked {
                bf.record_failure(&realm, Some(&existing.id), ip).await?;
            }
            return render_broker_link_confirm_page(
                &state,
                &mount,
                &realm,
                &headers,
                &alias,
                &auth_session,
                &existing.username,
                true,
                true,
            )
            .await;
        }
        bf.record_success(&realm, &existing.id).await?;
    }

    let attr_key = idp_link_attribute(&alias);
    existing
        .attributes
        .entry(attr_key)
        .or_default()
        .push(claims.sub.clone());
    let existing = state.store.update_user(existing).await?;
    state
        .store
        .upsert_idp_link(IdentityProviderLinkRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: existing.org_id.clone(),
            user_id: existing.id.clone(),
            idp_alias: alias.clone(),
            external_user_id: claims.sub,
            external_username: claims.preferred_username.or(claims.email),
            created_at: Utc::now(),
        })
        .await?;

    hand_to_login(&state, &mount, &realm, auth_session, &existing, &alias).await
}

/// keycloak.v2's login-idp-link-confirm with the local password when the account has one: the
/// realm's language and branding, the session's CSRF token and code.
#[allow(clippy::too_many_arguments)]
async fn render_broker_link_confirm_page(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    headers: &HeaderMap,
    alias: &str,
    auth_session: &AuthSession,
    account_name: &str,
    needs_password: bool,
    wrong_password: bool,
) -> Result<Response> {
    let accept = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let bundle = Bundle::for_locale(negotiate_for_realm(
        realm,
        auth_session.ui_locales.as_deref(),
        None,
        accept,
    ));
    let branding = state
        .store
        .get_branding(&realm.id, auth_session.org_id.as_deref())
        .await?
        .unwrap_or_default()
        .sanitised();
    let shell = page::Shell {
        bundle: &bundle,
        realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: "",
        locale_base_url: "",
    };
    let nonce = random_handle_256()[..16].to_owned();
    let mut tpl = shell.page(
        "broker-link-confirm",
        bundle.get("confirmLinkIdpTitle"),
        &nonce,
    );
    tpl.instructions.push(bundle.format(
        "federatedIdentityConfirmLinkMessage",
        &[bundle.get("username"), account_name],
    ));
    let mut fields = Vec::new();
    if needs_password {
        fields.push(page::Field {
            name: "password".into(),
            label: bundle.get("password").to_owned(),
            input_type: "password".into(),
            autocomplete: "current-password".into(),
            autofocus: true,
            required: true,
            error: if wrong_password {
                bundle.get("invalidPasswordMessage").to_owned()
            } else {
                String::new()
            },
            ..page::Field::default()
        });
    }
    tpl.form = Some(page::Form {
        id: "kc-broker-link-form".into(),
        action: format!(
            "{}/realms/{}/broker/{}/endpoint",
            mount.path_prefix, realm.realm, alias
        ),
        hidden: vec![
            ("csrf_token".into(), auth_session.csrf_token.clone()),
            ("session_code".into(), auth_session.id.clone()),
        ],
        fields,
        submit_id: "kc-login".into(),
        submit_label: bundle.get("confirmLinkIdpContinue").to_owned(),
        ..page::Form::default()
    });
    if wrong_password {
        tpl.alert = Some(page::Alert::danger(bundle.get("invalidPasswordMessage")));
    }
    Ok((
        StatusCode::OK,
        page::security_headers(&tpl.nonce, "'self'"),
        tpl.html(),
    )
        .into_response())
}

/// Hands a verified upstream sign-in to the login pages (S-4-01): the upstream assertion is the
/// first factor, and the login machine runs everything after it as for a password sign-in (the
/// second factor or its enrolment, required actions, the organization choice, consent) before
/// any code is issued.
async fn hand_to_login(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    mut auth_session: AuthSession,
    user: &UserRepresentation,
    alias: &str,
) -> Result<Response> {
    auth_session.step_state_json =
        serde_json::json!({ "step": "UpstreamIdp", "idp_alias": alias, "assertion_verified": true })
            .to_string();
    auth_session.user_id = Some(user.id.clone());
    auth_session.amr = vec!["fed".into()];
    auth_session.acr_achieved = "pwd".into();
    auth_session.broker_state_json = None;
    let location = format!(
        "{}/realms/{}/login-actions/authenticate?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );
    state.store.put_auth_session(auth_session).await?;
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        "",
    )
        .into_response())
}
