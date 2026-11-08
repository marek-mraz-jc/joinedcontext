#![forbid(unsafe_code)]
//! Upstream OIDC identity provider routes (`/realms/{realm}/broker/{alias}/login` and
//! `/realms/{realm}/broker/{alias}/endpoint`, W7, W11, K12).

use crate::broker::{UpstreamIdTokenClaims, UpstreamOidcBroker, idp_link_attribute};
use crate::client_ip::ClientIp;
use crate::handlers::{OidcState, redirect_with_params};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Form, Router};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::{
    constant_time_str_eq, random_handle_256, sha256_hex, verify_and_maybe_rehash,
    verify_jwt_with_jwk,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

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

fn read_auth_cookie(headers: &HeaderMap, mount: &Mount) -> Option<String> {
    let target = mount.cookie_name("auth");
    for val in headers.get_all(header::COOKIE) {
        if let Ok(s) = val.to_str() {
            for part in s.split(';').map(str::trim) {
                if let Some((name, v)) = part.split_once('=')
                    && name == target
                    && !v.is_empty()
                {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

fn verify_auth_cookie(
    headers: &HeaderMap,
    mount: &Mount,
    auth_session: &AuthSession,
) -> Result<()> {
    let Some(raw) = read_auth_cookie(headers, mount) else {
        return Err(KaisatsuError::BadRequest(
            "missing auth session cookie".into(),
        ));
    };
    if !constant_time_str_eq(&sha256_hex(&raw), &auth_session.handle_hash) {
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
    verify_auth_cookie(&headers, &mount, &auth_session)?;

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
    client_ip: ClientIp,
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
    verify_auth_cookie(&headers, &mount, &auth_session)?;

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
        return issue_broker_session_and_code(
            &state,
            &mount,
            &realm,
            auth_session,
            &user,
            &client_ip,
        )
        .await;
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
            return issue_broker_session_and_code(
                &state,
                &mount,
                &realm,
                auth_session,
                &existing,
                &client_ip,
            )
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
        return Ok(render_broker_link_confirm_page(
            &mount,
            &realm,
            &idp.alias,
            &auth_session,
            &existing.username,
            needs_password,
            "",
        ));
    }

    // 3. Create new person and link record when no existing account matches.
    let attr_key = idp_link_attribute(&idp.alias);
    let created = state
        .store
        .create_user(UserRepresentation {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: idp.org_id.clone().or_else(|| auth_session.org_id.clone()),
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
            idp_alias: idp.alias,
            external_user_id: claims.sub,
            external_username: claims.preferred_username.or(claims.email),
            created_at: Utc::now(),
        })
        .await?;

    issue_broker_session_and_code(&state, &mount, &realm, auth_session, &created, &client_ip).await
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
    verify_auth_cookie(&headers, &mount, &auth_session)?;

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
        let outcome = verify_and_maybe_rehash(submitted, pw_cred)?;
        if !outcome.valid {
            return Ok(render_broker_link_confirm_page(
                &mount,
                &realm,
                &alias,
                &auth_session,
                &existing.username,
                true,
                "Invalid password for account linking.",
            ));
        }
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
            idp_alias: alias,
            external_user_id: claims.sub,
            external_username: claims.preferred_username.or(claims.email),
            created_at: Utc::now(),
        })
        .await?;

    issue_broker_session_and_code(&state, &mount, &realm, auth_session, &existing, &client_ip).await
}

fn render_broker_link_confirm_page(
    mount: &Mount,
    realm: &RealmConfig,
    alias: &str,
    auth_session: &AuthSession,
    account_name: &str,
    needs_password: bool,
    alert: &str,
) -> Response {
    let nonce = random_handle_256()[..16].to_owned();
    let action = format!(
        "{}/realms/{}/broker/{}/endpoint",
        mount.path_prefix, realm.realm, alias
    );
    let pw_input = if needs_password {
        r#"<div class="kc-form-group"><label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password"/></div>"#
    } else {
        ""
    };
    let alert_html = if alert.is_empty() {
        String::new()
    } else {
        format!(r#"<div class="kc-alert" role="alert">{alert}</div>"#)
    };
    let html = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Confirm account link</title>\
         <style nonce=\"{nonce}\">body {{ font-family: system-ui, sans-serif; padding: 2rem; }}</style></head>\
         <body data-page-id=\"login-broker-link-confirm\">\
         <h1>Confirm account link</h1>\
         {alert_html}\
         <p id=\"instruction1\">Link your {alias} identity to account <strong>{account}</strong>?</p>\
         <form id=\"kc-broker-link-form\" action=\"{action}\" method=\"post\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"/>\
         <input type=\"hidden\" name=\"session_code\" value=\"{session}\"/>\
         {pw_input}\
         <button id=\"kc-login\" type=\"submit\">Confirm link</button>\
         </form></body></html>",
        nonce = nonce,
        alert_html = alert_html,
        alias = crate::handlers::html_escape(alias),
        account = crate::handlers::html_escape(account_name),
        action = crate::handlers::html_escape(&action),
        csrf = auth_session.csrf_token,
        session = auth_session.id,
        pw_input = pw_input,
    );
    let csp = format!(
        "default-src 'self'; script-src 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'"
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::X_FRAME_OPTIONS, "DENY".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        html,
    )
        .into_response()
}

async fn issue_broker_session_and_code(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    auth_session: AuthSession,
    user: &UserRepresentation,
    client_ip: &ClientIp,
) -> Result<Response> {
    let now = Utc::now();
    let sso_id = uuid::Uuid::now_v7().to_string();
    let sso_handle = random_handle_256();
    let sso = SsoSession {
        id: sso_id.clone(),
        realm_id: realm.id.clone(),
        org_id: user.org_id.clone().or_else(|| auth_session.org_id.clone()),
        handle_hash: sha256_hex(&sso_handle),
        user_id: user.id.clone(),
        username: user.username.clone(),
        ip_address: client_ip.for_audit(),
        user_agent: String::new(),
        start: now.timestamp_millis(),
        last_access: now.timestamp_millis(),
        auth_time: now.timestamp(),
        acr: "pwd".into(),
        amr: vec!["fed".into()],
        clients: HashMap::from([(
            auth_session.client_id.clone(),
            auth_session.client_id.clone(),
        )]),
        idle_expires_at: now + Duration::seconds(realm.sso_session_idle_timeout.max(3600)),
        max_expires_at: now + Duration::seconds(realm.sso_session_max_lifespan.max(36000)),
    };
    state.store.upsert_sso_session(sso).await?;

    let raw_code = random_handle_256();
    let code_rec = AuthorizationCodeRecord {
        code_hash: sha256_hex(&raw_code),
        realm_id: realm.id.clone(),
        org_id: user.org_id.clone().or(auth_session.org_id),
        client_id: auth_session.client_id.clone(),
        user_id: user.id.clone(),
        sso_session_id: sso_id.clone(),
        redirect_uri: auth_session.redirect_uri.clone(),
        scope: auth_session.scope.clone(),
        code_challenge: auth_session.code_challenge.clone(),
        code_challenge_method: auth_session.code_challenge_method.clone(),
        nonce: auth_session.nonce.clone(),
        acr: "pwd".into(),
        amr: vec!["fed".into()],
        dpop_jkt: auth_session.dpop_jkt.clone(),
        auth_time: now.timestamp(),
        consumed: false,
        created_at: now,
        expires_at: now + Duration::seconds(realm.access_code_lifespan.max(60)),
    };
    state.store.put_auth_code(code_rec).await?;
    state
        .store
        .delete_auth_session(&realm.id, &auth_session.id)
        .await?;

    state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: user.org_id.clone(),
            time: now.timestamp_millis(),
            event_type: "LOGIN".into(),
            client_id: Some(auth_session.client_id),
            user_id: Some(user.id.clone()),
            session_id: Some(sso_id),
            ip_address: client_ip.for_audit(),
            error: None,
            details: BTreeMap::from([("identity_provider".into(), "oidc".into())]),
        })
        .await?;

    let cookie_name = mount.cookie_name("sso");
    let cookie_path = mount.realm_cookie_path(&realm.realm);
    let ttl = realm.sso_session_max_lifespan.max(3600);
    let set_cookie = if mount.path_prefix.is_empty() {
        format!("{cookie_name}={sso_handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}")
    } else {
        format!(
            "{cookie_name}={sso_handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}"
        )
    };

    let mut pairs = vec![("code", raw_code.as_str())];
    if let Some(st) = &auth_session.state {
        pairs.push(("state", st.as_str()));
    }
    let mut resp = redirect_with_params(
        &auth_session.redirect_uri,
        &auth_session.response_mode,
        &pairs,
    );
    let cookie = header::HeaderValue::from_str(&set_cookie)
        .map_err(|_| KaisatsuError::Storage("invalid session cookie".into()))?;
    resp.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(resp)
}
