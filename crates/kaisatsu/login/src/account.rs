#![forbid(unsafe_code)]
//! The account console (Keycloak 26 `/realms/{realm}/account`): a server-sent page whose script
//! signs in through the `account-console` client (authorization code + PKCE) and calls the
//! account REST below with the bearer token it gets.
//!
//! - The REST takes only an access token with audience `account` from this realm, whose SSO
//!   session is still alive; it acts on the token's own person and nothing else.
//! - CORS answers only the origins of the realm's `account-console` client.
//! - Personal info changes `email`, `firstName`, `lastName` and the `locale` attribute; any other
//!   attribute, and any attribute a protocol mapper puts into a token, is refused. A new e-mail
//!   address is unverified.
//! - Credentials: list, remove an authenticator or passkey; the last authenticator is kept while
//!   the realm requires a second factor of the person. A password is changed by the
//!   application-initiated action `UPDATE_PASSWORD`, an authenticator added by `CONFIGURE_TOTP`.
//! - Sessions: list, sign out one, sign out all others (or all with `current=true`).

use crate::i18n::{Bundle, negotiate_locale};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use chrono::Utc;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_oidc::second_factor_required;
use kaisatsu_oidc::{ClientIp, OidcState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The client the console signs in through, provisioned by jc in each realm it serves.
pub const ACCOUNT_CONSOLE_CLIENT: &str = "account-console";
/// The audience every account REST token carries.
pub const ACCOUNT_AUDIENCE: &str = "account";
/// Attributes a person may set on themselves (Keycloak's default user profile).
const SELF_EDITABLE_ATTRIBUTES: &[&str] = &["locale"];
const MAX_NAME: usize = 255;
const MAX_EMAIL: usize = 254;

const CONSOLE_HTML: &str = include_str!("../templates/account.html");

pub fn account_router() -> Router<OidcState> {
    Router::new()
        .route(
            "/realms/{realm}/account",
            get(account_get).post(personal_info_post).options(preflight),
        )
        .route(
            "/realms/{realm}/account/",
            get(account_get).post(personal_info_post).options(preflight),
        )
        .route("/realms/{realm}/account/personal-info", get(console_page))
        .route(
            "/realms/{realm}/account/account-security/signing-in",
            get(console_page),
        )
        .route(
            "/realms/{realm}/account/account-security/device-activity",
            get(console_page),
        )
        .route(
            "/realms/{realm}/account/credentials",
            get(credentials_get).options(preflight),
        )
        .route(
            "/realms/{realm}/account/credentials/{id}",
            delete(credential_delete).options(preflight),
        )
        .route(
            "/realms/{realm}/account/sessions",
            get(sessions_get).delete(sessions_delete).options(preflight),
        )
        .route(
            "/realms/{realm}/account/sessions/{id}",
            delete(session_delete).options(preflight),
        )
}

/// The person an account REST call acts for, read from its bearer token.
struct Caller {
    realm: RealmConfig,
    user: UserRepresentation,
    session_id: String,
    mount: Mount,
}

async fn realm_of(state: &OidcState, name: &str) -> Result<RealmConfig> {
    state
        .store
        .get_realm(name)
        .await?
        .filter(|r| r.enabled)
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))
}

/// Verifies the bearer token: this realm's issuer and keys, audience `account`, an access token,
/// DPoP when it is bound, not revoked, its SSO session alive and held by the token's person.
async fn caller(
    state: &OidcState,
    realm_name: &str,
    method: &Method,
    uri: &http::Uri,
    headers: &HeaderMap,
    mount_ext: Option<&Mount>,
) -> Result<Caller> {
    let realm = realm_of(state, realm_name).await?;
    let mount = state.mount_from(mount_ext);
    let (dpop_scheme, raw_token) = kaisatsu_oidc::parse_authz_header(headers)?;
    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    let (_, claims): (_, Value) =
        verify_jwt(raw_token, &keys, Some(&issuer), Some(ACCOUNT_AUDIENCE))?;
    if claims.get("typ").and_then(Value::as_str) != Some("Bearer") {
        return Err(KaisatsuError::Unauthorized("not an access token".into()));
    }
    let htu = kaisatsu_oidc::external_htu(&mount, uri.path());
    kaisatsu_oidc::verify_resource_dpop(
        state,
        &realm.id,
        &claims,
        headers,
        method.as_str(),
        &htu,
        raw_token,
        dpop_scheme,
    )
    .await?;
    let inactive = || KaisatsuError::Unauthorized("token is not active".into());
    if let Some(jti) = claims.get("jti").and_then(Value::as_str)
        && state
            .store
            .is_token_revoked(&realm.id, &sha256_hex(jti))
            .await?
    {
        return Err(inactive());
    }
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(inactive)?;
    let sid = claims
        .get("sid")
        .and_then(Value::as_str)
        .ok_or_else(inactive)?;
    let sso = state
        .store
        .get_sso_session(&realm.id, sid)
        .await?
        .ok_or_else(inactive)?;
    if !constant_time_str_eq(&sso.user_id, sub) {
        return Err(inactive());
    }
    let user = state
        .store
        .get_user_by_id(&realm.id, None, sub)
        .await?
        .filter(|u| u.enabled && u.service_account_client_id.is_none())
        .ok_or_else(inactive)?;
    Ok(Caller {
        realm,
        user,
        session_id: sso.id,
        mount,
    })
}

/// The origins the `account-console` client names (`+` stands for its redirect URIs' origins).
async fn console_origins(state: &OidcState, realm: &RealmConfig) -> Result<Vec<String>> {
    let Some(client) = state
        .store
        .get_client_by_client_id(&realm.id, ACCOUNT_CONSOLE_CLIENT)
        .await?
    else {
        return Ok(Vec::new());
    };
    let mut origins = Vec::new();
    for entry in &client.web_origins {
        if entry == "+" {
            origins.extend(client.redirect_uris.iter().filter_map(|u| origin_of(u)));
        } else if entry != "*" {
            origins.extend(origin_of(entry));
        }
    }
    Ok(origins)
}

fn origin_of(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim_end_matches('*')).ok()?;
    match url.origin() {
        url::Origin::Tuple(..) => Some(url.origin().ascii_serialization()),
        url::Origin::Opaque(_) => None,
    }
}

/// `Ok(None)` without an `Origin` header, the origin when the console client names it, else
/// a refusal: a page of another origin never reads or changes an account.
async fn allowed_origin(
    state: &OidcState,
    realm_name: &str,
    headers: &HeaderMap,
) -> Result<Option<HeaderValue>> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(None);
    };
    let realm = realm_of(state, realm_name).await?;
    let presented = origin.to_str().unwrap_or_default();
    if console_origins(state, &realm)
        .await?
        .iter()
        .any(|o| o == presented)
    {
        Ok(Some(origin.clone()))
    } else {
        Err(KaisatsuError::Forbidden(
            "this origin may not call the account REST".into(),
        ))
    }
}

fn with_cors(mut resp: Response, origin: Option<HeaderValue>) -> Response {
    let h = resp.headers_mut();
    if let Some(o) = origin {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
        h.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("WWW-Authenticate, DPoP-Nonce"),
        );
    }
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Runs an account REST handler: CORS first, then the caller, then `f`.
async fn rest<F, Fut>(state: OidcState, realm_name: String, parts: Parts, f: F) -> Response
where
    F: FnOnce(OidcState, Caller) -> Fut,
    Fut: std::future::Future<Output = Result<Response>>,
{
    let headers = &parts.headers;
    let method = &parts.method;
    let uri = parts
        .extensions
        .get::<OriginalUri>()
        .map_or_else(|| parts.uri.clone(), |o| o.0.clone());
    let mount_ext = parts.extensions.get::<Mount>();
    let origin = match allowed_origin(&state, &realm_name, headers).await {
        Ok(o) => o,
        Err(e) => return with_cors(e.into_response(), None),
    };
    let answer = match caller(&state, &realm_name, method, &uri, headers, mount_ext).await {
        Ok(c) => f(state, c).await,
        Err(e) => Err(e),
    };
    let resp = match answer {
        Ok(r) => r,
        Err(e @ KaisatsuError::Unauthorized(_)) => {
            let mut r = kaisatsu_oidc::into_dpop_aware_response(e);
            if !r.headers().contains_key(header::WWW_AUTHENTICATE) {
                r.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static(r#"Bearer error="invalid_token""#),
                );
            }
            r
        }
        Err(e) => e.into_response(),
    };
    with_cors(resp, origin)
}

async fn preflight(
    State(state): State<OidcState>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let realm_name = params.get("realm").cloned().unwrap_or_default();
    match allowed_origin(&state, &realm_name, &headers).await {
        Ok(Some(origin)) => (
            StatusCode::NO_CONTENT,
            [
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
                (
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET, POST, DELETE"),
                ),
                (
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static("Authorization, Content-Type, DPoP"),
                ),
                (
                    header::ACCESS_CONTROL_MAX_AGE,
                    HeaderValue::from_static("600"),
                ),
                (header::VARY, HeaderValue::from_static("Origin")),
            ],
        )
            .into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// A refused account change: which field, and the message key the console shows for it.
fn field_error(status: StatusCode, field: &str, key: &str) -> Response {
    (status, Json(json!({ "field": field, "errorMessage": key }))).into_response()
}

async fn audit(state: &OidcState, c: &Caller, kind: &str, ip: &ClientIp, details: &[(&str, &str)]) {
    let _ = state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: c.realm.id.clone(),
            org_id: c.user.org_id.clone(),
            time: Utc::now().timestamp_millis(),
            event_type: kind.into(),
            client_id: Some(ACCOUNT_CONSOLE_CLIENT.into()),
            user_id: Some(c.user.id.clone()),
            session_id: Some(c.session_id.clone()),
            ip_address: ip.for_audit(),
            error: None,
            details: details
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        })
        .await;
}

// ---------------------------------------------------------------- personal info

/// `GET /realms/{realm}/account`: the console page, or with a bearer token the person's
/// personal info (as Keycloak answers JSON there).
async fn account_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(page): Query<PageQuery>,
    parts: Parts,
) -> Response {
    if !parts.headers.contains_key(header::AUTHORIZATION) {
        let mount_ext = parts.extensions.get::<Mount>().cloned().map(Extension);
        return console_page(
            State(state),
            Path(realm_name),
            Query(page),
            parts.headers,
            mount_ext,
        )
        .await;
    }
    rest(state, realm_name, parts, |_, c| async move {
        Ok(Json(personal_info(&c.user)).into_response())
    })
    .await
}

fn personal_info(user: &UserRepresentation) -> Value {
    let attributes: BTreeMap<&str, &Vec<String>> = SELF_EDITABLE_ATTRIBUTES
        .iter()
        .filter_map(|k| user.attributes.get(*k).map(|v| (*k, v)))
        .collect();
    json!({
        "id": user.id,
        "username": user.username,
        "email": user.email.clone().unwrap_or_default(),
        "firstName": user.first_name.clone().unwrap_or_default(),
        "lastName": user.last_name.clone().unwrap_or_default(),
        "emailVerified": user.email_verified,
        "attributes": attributes,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersonalInfoUpdate {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    attributes: Option<BTreeMap<String, Vec<String>>>,
}

/// Every user attribute a protocol mapper of the realm (client or client scope) puts into a
/// token: a person who could set one would grant themselves a claim.
async fn claim_bearing_attributes(store: &Arc<dyn Store>, realm_id: &str) -> Result<Vec<String>> {
    let mut mappers: Vec<ProtocolMapperRepresentation> = Vec::new();
    for c in store.list_clients(realm_id, None, None, false).await? {
        mappers.extend(c.protocol_mappers);
    }
    for s in store.list_client_scopes(realm_id).await? {
        mappers.extend(s.protocol_mappers);
    }
    Ok(mappers
        .iter()
        .filter_map(|m| m.config.get("user.attribute").cloned())
        .collect())
}

fn clean_text(raw: &str, max: usize) -> std::result::Result<Option<String>, &'static str> {
    let v = raw.trim();
    if v.chars().count() > max {
        return Err("accountFieldTooLong");
    }
    if v.chars().any(char::is_control) {
        return Err("accountFieldInvalid");
    }
    Ok((!v.is_empty()).then(|| v.to_owned()))
}

fn plausible_email(v: &str) -> bool {
    let Some((local, domain)) = v.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !v.contains(char::is_whitespace)
        && !domain.contains('@')
}

/// `POST /realms/{realm}/account`: updates the token's own person.
async fn personal_info_post(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    parts: Parts,
    ip: ClientIp,
    body: axum::body::Bytes,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        update_personal_info(&state, c, &ip, &body).await
    })
    .await
}

async fn update_personal_info(
    state: &OidcState,
    c: Caller,
    ip: &ClientIp,
    body: &[u8],
) -> Result<Response> {
    let Ok(req) = serde_json::from_slice::<PersonalInfoUpdate>(body) else {
        return Ok(field_error(
            StatusCode::BAD_REQUEST,
            "",
            "accountInvalidRequest",
        ));
    };
    let mut user = c.user.clone();
    if req.id.as_ref().is_some_and(|id| *id != user.id) {
        return Ok(field_error(
            StatusCode::BAD_REQUEST,
            "id",
            "accountFieldReadOnly",
        ));
    }
    if req
        .username
        .as_ref()
        .is_some_and(|u| u.trim() != user.username)
    {
        return Ok(field_error(
            StatusCode::BAD_REQUEST,
            "username",
            "accountFieldReadOnly",
        ));
    }
    if req.email_verified.is_some_and(|v| v != user.email_verified) {
        return Ok(field_error(
            StatusCode::BAD_REQUEST,
            "emailVerified",
            "accountFieldReadOnly",
        ));
    }
    for (field, raw, slot) in [
        ("firstName", &req.first_name, &mut user.first_name),
        ("lastName", &req.last_name, &mut user.last_name),
    ] {
        if let Some(raw) = raw {
            match clean_text(raw, MAX_NAME) {
                Ok(v) => *slot = v,
                Err(key) => return Ok(field_error(StatusCode::BAD_REQUEST, field, key)),
            }
        }
    }
    let mut email_changed = false;
    if let Some(raw) = &req.email {
        let new_email = match clean_text(raw, MAX_EMAIL) {
            Ok(Some(v)) if plausible_email(&v) => v,
            Ok(None) => {
                return Ok(field_error(
                    StatusCode::BAD_REQUEST,
                    "email",
                    "accountEmailRequired",
                ));
            }
            Ok(Some(_)) => {
                return Ok(field_error(
                    StatusCode::BAD_REQUEST,
                    "email",
                    "accountEmailInvalid",
                ));
            }
            Err(key) => return Ok(field_error(StatusCode::BAD_REQUEST, "email", key)),
        };
        if !user
            .email
            .as_deref()
            .is_some_and(|old| old.eq_ignore_ascii_case(&new_email))
        {
            if !c.realm.duplicate_emails_allowed
                && state
                    .store
                    .get_user_by_username_or_email(&c.realm.id, None, &new_email)
                    .await?
                    .is_some_and(|other| other.id != user.id)
            {
                return Ok(field_error(
                    StatusCode::CONFLICT,
                    "email",
                    "accountEmailInUse",
                ));
            }
            user.email = Some(new_email);
            // A new address is not verified until its owner confirms it.
            user.email_verified = false;
            if c.realm.verify_email && !user.required_actions.iter().any(|a| a == "VERIFY_EMAIL") {
                user.required_actions.push("VERIFY_EMAIL".into());
            }
            email_changed = true;
        }
    }
    if let Some(attrs) = &req.attributes {
        let claim_bearing = claim_bearing_attributes(&state.store, &c.realm.id).await?;
        for (key, values) in attrs {
            if user.attributes.get(key) == Some(values) {
                continue;
            }
            let editable = SELF_EDITABLE_ATTRIBUTES.contains(&key.as_str())
                && !claim_bearing.iter().any(|a| a == key);
            if !editable {
                return Ok(field_error(
                    StatusCode::BAD_REQUEST,
                    key,
                    "accountAttributeReadOnly",
                ));
            }
            if key == "locale"
                && !(values.is_empty()
                    || (values.len() == 1
                        && crate::i18n::SUPPORTED_LOCALES.contains(&values[0].as_str())))
            {
                return Ok(field_error(
                    StatusCode::BAD_REQUEST,
                    key,
                    "accountFieldInvalid",
                ));
            }
            if values.is_empty() {
                user.attributes.remove(key);
            } else {
                user.attributes.insert(key.clone(), values.clone());
            }
        }
    }
    let user = state.store.update_user(user).await?;
    audit(state, &c, "UPDATE_PROFILE", ip, &[]).await;
    if email_changed {
        audit(state, &c, "UPDATE_EMAIL", ip, &[]).await;
    }
    Ok(Json(personal_info(&user)).into_response())
}

// ---------------------------------------------------------------- credentials

async fn credentials_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    parts: Parts,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        Ok(Json(credential_groups(&state, &c).await?).into_response())
    })
    .await
}

/// Keycloak's credential groups: metadata only, never secret data.
async fn credential_groups(state: &OidcState, c: &Caller) -> Result<Value> {
    let creds = state
        .store
        .list_credentials(&c.realm.id, &c.user.id)
        .await?;
    let meta = |t: &str| -> Vec<Value> {
        creds
            .iter()
            .filter(|cr| cr.credential_type == t)
            .map(|cr| {
                json!({ "credential": {
                    "id": cr.id,
                    "type": cr.credential_type,
                    "userLabel": cr.user_label,
                    "createdDate": cr.created_date,
                }})
            })
            .collect()
    };
    let passkeys: Vec<Value> = state
        .store
        .list_webauthn_credentials(&c.realm.id, &c.user.id)
        .await?
        .into_iter()
        .map(|p| {
            json!({ "credential": {
                "id": p.id,
                "type": "webauthn-passwordless",
                "userLabel": p.user_label,
                "createdDate": p.created_at.timestamp_millis(),
            }})
        })
        .collect();
    let otp_required = second_factor_required(state, &c.realm, &c.user).await?;
    Ok(json!([
        {
            "type": "password",
            "category": "basic-authentication",
            "removeable": false,
            "updateAction": "UPDATE_PASSWORD",
            "userCredentialMetadatas": meta("password"),
        },
        {
            "type": "otp",
            "category": "two-factor",
            "removeable": true,
            "required": otp_required,
            "createAction": "CONFIGURE_TOTP",
            "userCredentialMetadatas": meta("otp"),
        },
        {
            "type": "webauthn-passwordless",
            "category": "passwordless",
            "removeable": true,
            "userCredentialMetadatas": passkeys,
        },
    ]))
}

async fn credential_delete(
    State(state): State<OidcState>,
    Path((realm_name, id)): Path<(String, String)>,
    parts: Parts,
    ip: ClientIp,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        remove_credential(&state, c, &id, &ip).await
    })
    .await
}

async fn remove_credential(
    state: &OidcState,
    c: Caller,
    id: &str,
    ip: &ClientIp,
) -> Result<Response> {
    let creds = state
        .store
        .list_credentials(&c.realm.id, &c.user.id)
        .await?;
    if let Some(cred) = creds.iter().find(|cr| cr.id == id) {
        match cred.credential_type.as_str() {
            "otp" => {
                let others = creds
                    .iter()
                    .filter(|cr| cr.credential_type == "otp" && cr.id != id)
                    .count();
                if others == 0 && second_factor_required(state, &c.realm, &c.user).await? {
                    return Ok(field_error(
                        StatusCode::CONFLICT,
                        "otp",
                        "accountOtpRequired",
                    ));
                }
            }
            _ => {
                return Ok(field_error(
                    StatusCode::BAD_REQUEST,
                    &cred.credential_type,
                    "accountCredentialNotRemovable",
                ));
            }
        }
        state
            .store
            .delete_credential(&c.realm.id, &c.user.id, id)
            .await?;
        audit(state, &c, "REMOVE_TOTP", ip, &[("credential_id", id)]).await;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // A passkey: the store deletes by id alone, so only one of the caller's own goes.
    let own_passkey = state
        .store
        .list_webauthn_credentials(&c.realm.id, &c.user.id)
        .await?
        .into_iter()
        .any(|p| p.id == id && p.user_id == c.user.id);
    if own_passkey {
        state
            .store
            .delete_webauthn_credential(&c.realm.id, None, id)
            .await?;
        audit(state, &c, "REMOVE_CREDENTIAL", ip, &[("credential_id", id)]).await;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    Err(KaisatsuError::NotFound("credential not found".into()))
}

// ---------------------------------------------------------------- sessions

async fn sessions_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    parts: Parts,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        let mut out = Vec::new();
        for s in state
            .store
            .list_user_sso_sessions(&c.realm.id, &c.user.id)
            .await?
        {
            let mut clients = Vec::new();
            for client_id in s.clients.keys() {
                let name = state
                    .store
                    .get_client_by_client_id(&c.realm.id, client_id)
                    .await?
                    .and_then(|cl| cl.name)
                    .unwrap_or_else(|| client_id.clone());
                clients.push(json!({ "clientId": client_id, "clientName": name }));
            }
            clients.sort_by(|a, b| a["clientId"].as_str().cmp(&b["clientId"].as_str()));
            let (browser, os, mobile) = device_of(&s.user_agent);
            out.push(json!({
                "id": s.id,
                "ipAddress": s.ip_address,
                "browser": browser,
                "os": os,
                "mobile": mobile,
                "started": s.start / 1000,
                "lastAccess": s.last_access / 1000,
                "expires": s.max_expires_at.timestamp(),
                "clients": clients,
                "current": s.id == c.session_id,
            }));
        }
        Ok(Json(out).into_response())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct AllSessionsQuery {
    #[serde(default)]
    current: bool,
}

async fn end_session(state: &OidcState, c: &Caller, sso: &SsoSession) -> Result<()> {
    if let Some(ended) = state.store.delete_sso_session(&c.realm.id, &sso.id).await? {
        let _ = kaisatsu_oidc::dispatch_backchannel_logout(state, &c.mount, &c.realm, &ended).await;
    }
    Ok(())
}

/// `DELETE /account/sessions`: signs out every other session, and this one too with
/// `current=true`.
async fn sessions_delete(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(q): Query<AllSessionsQuery>,
    parts: Parts,
    ip: ClientIp,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        for s in state
            .store
            .list_user_sso_sessions(&c.realm.id, &c.user.id)
            .await?
        {
            if q.current || s.id != c.session_id {
                end_session(&state, &c, &s).await?;
            }
        }
        audit(
            &state,
            &c,
            "LOGOUT",
            &ip,
            &[("scope", if q.current { "all" } else { "others" })],
        )
        .await;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

/// `DELETE /account/sessions/{id}`: one of the caller's own sessions; any other id is 404.
async fn session_delete(
    State(state): State<OidcState>,
    Path((realm_name, id)): Path<(String, String)>,
    parts: Parts,
    ip: ClientIp,
) -> Response {
    rest(state, realm_name, parts, |state, c| async move {
        let Some(sso) = state
            .store
            .get_sso_session(&c.realm.id, &id)
            .await?
            .filter(|s| s.user_id == c.user.id)
        else {
            return Err(KaisatsuError::NotFound("session not found".into()));
        };
        end_session(&state, &c, &sso).await?;
        audit(&state, &c, "LOGOUT", &ip, &[("session_id", &id)]).await;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

// ---------------------------------------------------------------- the console page

#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default)]
    ui_locales: Option<String>,
}

/// The console page: one document for every console path; its script reads the path.
async fn console_page(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(page): Query<PageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Response {
    match render_console(
        &state,
        &realm_name,
        page.ui_locales.as_deref(),
        &headers,
        mount_ext.as_ref().map(|e| &e.0),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// JSON for an inline `<script type="application/json">`: `<` escaped so no text closes it.
fn script_json(v: &Value) -> String {
    v.to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn render_console(
    state: &OidcState,
    realm_name: &str,
    ui_locales: Option<&str>,
    headers: &HeaderMap,
    mount_ext: Option<&Mount>,
) -> Result<Response> {
    let realm = realm_of(state, realm_name).await?;
    let mount = state.mount_from(mount_ext);
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(ui_locales, None, accept_lang, &realm.default_locale);
    let bundle = Bundle::for_locale(locale);
    let strings: BTreeMap<&str, &str> = bundle
        .keys()
        .filter(|k| k.starts_with("account") || matches!(*k, "doCancel" | "passwordLabel"))
        .map(|k| (k, bundle.get(k)))
        .collect();
    let branding = state
        .store
        .get_branding(&realm.id, None)
        .await?
        .unwrap_or_else(|| BrandingBlock {
            organisation: realm.display_name.clone(),
            instance_name: realm.display_name.clone(),
            ..Default::default()
        })
        .sanitised();
    let realm_path = format!("{}/realms/{}", mount.path_prefix, realm.realm);
    let display = if realm.display_name.is_empty() {
        realm.realm.clone()
    } else {
        realm.display_name.clone()
    };
    let cfg = json!({
        "clientId": ACCOUNT_CONSOLE_CLIENT,
        "base": format!("{realm_path}/account"),
        "oidc": format!("{realm_path}/protocol/openid-connect"),
        "locale": locale,
        "realm": display,
    });
    let nonce = random_handle_256()[..22].to_owned();
    let html = CONSOLE_HTML
        .replace("__LANG__", locale)
        .replace(
            "__TITLE__",
            &html_escape(&format!("{} · {display}", bundle.get("accountTitle"))),
        )
        .replace("__REALM__", &html_escape(&display))
        .replace("__BRANDING_CSS__", &branding.css_variables())
        .replace("__CONFIG__", &script_json(&cfg))
        .replace("__STRINGS__", &script_json(&json!(strings)))
        .replace("__NONCE__", &nonce);
    let csp = format!(
        "default-src 'self'; script-src 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}'; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'"
    );
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::X_FRAME_OPTIONS, "DENY".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::REFERRER_POLICY, "no-referrer".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        html,
    )
        .into_response())
}

/// Browser, operating system and a mobile flag read from a User-Agent, as Keycloak's
/// DeviceRepresentation names them; an unknown agent answers "Other".
fn device_of(ua: &str) -> (&'static str, &'static str, bool) {
    let browser = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .iter()
    .find(|(needle, _)| ua.contains(needle))
    .map_or("Other", |(_, name)| name);
    let os = [
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iOS"),
        ("Windows", "Windows"),
        ("Mac OS X", "macOS"),
        ("CrOS", "ChromeOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(needle, _)| ua.contains(needle))
    .map_or("Other", |(_, name)| name);
    (browser, os, ua.contains("Mobile"))
}

#[cfg(test)]
mod tests {
    use super::device_of;

    #[test]
    fn a_user_agent_names_browser_and_system() {
        assert_eq!(
            device_of(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/140.0 Safari/537.36"
            ),
            ("Chrome", "Linux", false)
        );
        assert_eq!(
            device_of(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"
            ),
            ("Safari", "iOS", true)
        );
        assert_eq!(device_of(""), ("Other", "Other", false));
    }
}
