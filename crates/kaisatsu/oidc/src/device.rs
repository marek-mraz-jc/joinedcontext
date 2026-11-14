#![forbid(unsafe_code)]
//! OAuth 2.0 Device Authorization Grant (RFC 8628, D1), as Keycloak serves it for a client whose
//! `oauth2.device.authorization.grant.enabled` attribute is `true` (prod's `jcctl`):
//! - `POST /protocol/openid-connect/auth/device`: the device gets a device code and a user code;
//!   any other client is `unauthorized_client`.
//! - `GET`/`POST /realms/{realm}/device`: the verification page. Only a signed-in person enters a
//!   user code (no SSO session: the login runs first and comes back here); a person gets
//!   [`MAX_WRONG_CODES`] wrong codes per [`WRONG_CODE_WINDOW_SECS`], then the page refuses.
//!   The person allows or denies, once.
//! - The token endpoint's `urn:ietf:params:oauth:grant-type:device_code` arm
//!   (`handlers::handle_device_code_grant`): `authorization_pending`, `slow_down` for a poll inside
//!   the interval, `access_denied`, `expired_token`; the tokens go once.
//!
//! Codes live in the store (`kaisatsu.device_code` on PostgreSQL) as SHA-256 hashes only.

use crate::handlers::{
    OidcState, authenticate_client, mapper_scope_names, read_sso_cookie, resolve_scopes_for_client,
};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::i18n::{Bundle, negotiate_for_realm};
use kaisatsu_core::page;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use serde_json::json;
use std::collections::BTreeMap;

pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const ENABLED: &str = "oauth2.device.authorization.grant.enabled";
/// Wrong user codes one person may enter per window before the page refuses them.
pub const MAX_WRONG_CODES: u32 = 5;
pub const WRONG_CODE_WINDOW_SECS: i64 = 900;
/// The client the verification page's login runs as: the account console's, as for any page of
/// the realm's own that needs a signed-in person.
const LOGIN_CLIENT: &str = "account-console";

/// Refuses a client the realm does not let start a device authorization (Keycloak's text).
pub fn ensure_device_enabled(client: &ClientRepresentation) -> Result<()> {
    if client.attributes.get(ENABLED).map(String::as_str) == Some("true") {
        return Ok(());
    }
    Err(KaisatsuError::unauthorized_client(
        "Client is not allowed to initiate OAuth 2.0 Device Authorization Grant. The flow is disabled for the client.",
    ))
}

/// Seconds from the client's attribute, else the realm's attribute, else `default`, kept in
/// `min..=max`.
fn seconds(
    realm: &RealmConfig,
    client: &ClientRepresentation,
    client_attr: &str,
    realm_attr: &str,
    default: i64,
    (min, max): (i64, i64),
) -> i64 {
    client
        .attributes
        .get(client_attr)
        .or_else(|| realm.attributes.get(realm_attr))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
        .clamp(min, max)
}

/// `POST /realms/{realm}/protocol/openid-connect/auth/device` (RFC 8628 §3.1, §3.2).
pub async fn device_authorization(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let client = authenticate_client(&state, &realm, &mount, &headers, &form).await?;
    ensure_device_enabled(&client)?;
    let (active, granted) = resolve_scopes_for_client(
        &state.store,
        &realm,
        &client,
        form.get("scope").map(String::as_str),
    )
    .await?;
    let lifespan = seconds(
        &realm,
        &client,
        "oauth2.device.code.lifespan",
        "oauth2DeviceCodeLifespan",
        600,
        (60, 3600),
    );
    let interval = seconds(
        &realm,
        &client,
        "oauth2.device.polling.interval",
        "oauth2DevicePollingInterval",
        5,
        (1, 60),
    );
    let now = Utc::now();
    // A user code another pending authorization holds is drawn again (the store refuses it).
    let mut issued = None;
    for _ in 0..3 {
        let device_code = random_handle_256();
        let user_code = random_user_code();
        let rec = DeviceCodeRecord {
            device_code_hash: sha256_hex(&device_code),
            user_code_hash: sha256_hex(&normalize_user_code(&user_code)),
            realm_id: realm.id.clone(),
            client_id: client.client_id.clone(),
            scope: granted.clone(),
            mapper_scopes: mapper_scope_names(&client, &active),
            status: DeviceStatus::Pending,
            user_id: None,
            sso_session_id: None,
            acr: String::new(),
            amr: Vec::new(),
            auth_time: 0,
            interval_secs: interval,
            last_poll_at: None,
            consumed: false,
            created_at: now,
            expires_at: now + Duration::seconds(lifespan),
        };
        match state.store.put_device_code(rec).await {
            Ok(()) => {
                issued = Some((device_code, user_code));
                break;
            }
            Err(KaisatsuError::Conflict(_)) => {}
            Err(e) => return Err(e),
        }
    }
    let (device_code, user_code) =
        issued.ok_or_else(|| KaisatsuError::Storage("no free user code".into()))?;
    let verification = format!("{}/device", mount.realm_issuer(&realm.realm));
    let body = json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": verification,
        "verification_uri_complete": format!("{verification}?user_code={user_code}"),
        "expires_in": lifespan,
        "interval": interval,
    });
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        body.to_string(),
    )
        .into_response())
}

#[derive(serde::Deserialize, Default)]
pub struct DevicePageQuery {
    #[serde(default)]
    pub user_code: Option<String>,
}

/// The person's SSO session behind the request's cookie, if it lives.
async fn signed_in(
    state: &OidcState,
    realm: &RealmConfig,
    mount: &Mount,
    headers: &HeaderMap,
) -> Result<Option<SsoSession>> {
    let Some(handle) = read_sso_cookie(headers, mount) else {
        return Ok(None);
    };
    let sso = state
        .store
        .get_sso_session_by_handle(&realm.id, &sha256_hex(&handle))
        .await?;
    let Some(sso) = sso.filter(|s| s.max_expires_at > Utc::now() && s.idle_expires_at > Utc::now())
    else {
        return Ok(None);
    };
    // A session without the second factor the person now needs approves no device: sign in again.
    Ok(crate::sso_session_suffices(state, realm, &sso)
        .await?
        .then_some(sso))
}

/// The page's CSRF token: a MAC of the SSO session id, so only a page this session loaded posts.
fn csrf_of(state: &OidcState, sso: &SsoSession) -> String {
    URL_SAFE_NO_PAD.encode(state.kek.mac("kaisatsu:device-csrf:v1", sso.id.as_bytes()))
}

fn page_path(mount: &Mount, realm: &RealmConfig) -> String {
    format!("{}/realms/{}/device", mount.path_prefix, realm.realm)
}

/// A device page (keycloak.v2's login-oauth2-device-verify-user-code, login-oauth-grant and
/// info) in the realm's language and branding; `fill` adds the page's content.
#[allow(clippy::too_many_arguments)]
async fn render(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    headers: &HeaderMap,
    status: StatusCode,
    page_id: &str,
    heading_key: &str,
    fill: impl FnOnce(&Bundle, &mut page::Page),
) -> Result<Response> {
    let accept = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let bundle = Bundle::for_locale(negotiate_for_realm(realm, None, None, accept));
    let branding = state
        .store
        .get_branding(&realm.id, None)
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
    let mut tpl = shell.page(page_id, bundle.get(heading_key), &nonce);
    fill(&bundle, &mut tpl);
    Ok((
        status,
        page::security_headers(&tpl.nonce, "'self'"),
        tpl.html(),
    )
        .into_response())
}

/// The "enter the code your device shows" form; `error` lands on the field.
fn code_form(
    bundle: &Bundle,
    action: &str,
    csrf: &str,
    user_code: &str,
    error: &str,
) -> page::Form {
    page::Form {
        id: "kc-user-verify-device-user-code-form".into(),
        action: action.to_owned(),
        hidden: vec![("csrf".into(), csrf.to_owned())],
        fields: vec![page::Field {
            name: "user_code".into(),
            label: bundle.get("verifyOAuth2DeviceUserCode").to_owned(),
            input_type: "text".into(),
            value: user_code.to_owned(),
            autocomplete: "off".into(),
            error: error.to_owned(),
            required: true,
            autofocus: true,
            ..page::Field::default()
        }],
        submit_id: "kc-login".into(),
        submit_label: bundle.get("doSubmit").to_owned(),
        ..page::Form::default()
    }
}

/// Starts the realm's login for the verification page; it comes back to `return_to`.
async fn start_login(
    state: &OidcState,
    realm: &RealmConfig,
    mount: &Mount,
    return_to: String,
) -> Result<Response> {
    let id = uuid::Uuid::now_v7().to_string();
    let handle = random_handle_256();
    let now = Utc::now();
    state
        .store
        .put_auth_session(AuthSession {
            id: id.clone(),
            realm_id: realm.id.clone(),
            org_id: None,
            handle_hash: sha256_hex(&handle),
            client_id: LOGIN_CLIENT.into(),
            redirect_uri: return_to,
            response_type: "code".into(),
            response_mode: "query".into(),
            scope: "openid".into(),
            mapper_scopes: Vec::new(),
            state: None,
            nonce: None,
            // No verifier exists for this challenge: the code the login hands back redeems nothing.
            code_challenge: random_handle_256(),
            code_challenge_method: "S256".into(),
            acr_requested: None,
            prompt: None,
            login_hint: None,
            ui_locales: None,
            dpop_jkt: None,
            csrf_token: random_handle_256(),
            step_state_json: json!({ "step": "IdentityFirst" }).to_string(),
            pending_totp_secret: None,
            webauthn_challenge: None,
            pending_email_code_hash: None,
            broker_state_json: None,
            kc_action: None,
            kc_action_done: false,
            user_id: None,
            amr: Vec::new(),
            acr_achieved: String::new(),
            created_at: now,
            expires_at: now + Duration::minutes(30),
        })
        .await?;
    let cookie_name = mount.cookie_name("auth");
    let path = if mount.path_prefix.is_empty() {
        "/".to_owned()
    } else {
        mount.realm_cookie_path(&realm.realm)
    };
    let set_cookie = format!(
        "{cookie_name}={handle}; Path={path}; HttpOnly; Secure; SameSite=Lax; Max-Age=1800"
    );
    let login = format!(
        "{}/realms/{}/login-actions/authenticate?session_code={id}&client_id={LOGIN_CLIENT}",
        mount.path_prefix, realm.realm
    );
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, set_cookie),
            (header::LOCATION, login),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        "",
    )
        .into_response())
}

/// `GET /realms/{realm}/device[?user_code=]`: the code form for a signed-in person, else the
/// login first.
pub async fn device_page(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(q): Query<DevicePageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let user_code = q
        .user_code
        .as_deref()
        .map(normalize_user_code)
        .filter(|c| c.len() == 8)
        .map(|c| format!("{}-{}", &c[..4], &c[4..]))
        .unwrap_or_default();
    let Some(sso) = signed_in(&state, &realm, &mount, &headers).await? else {
        let mut back = format!("{}/device", mount.realm_issuer(&realm.realm));
        if !user_code.is_empty() {
            back = format!("{back}?user_code={user_code}");
        }
        return start_login(&state, &realm, &mount, back).await;
    };
    let action = page_path(&mount, &realm);
    let csrf = csrf_of(&state, &sso);
    render(
        &state,
        &mount,
        &realm,
        &headers,
        StatusCode::OK,
        "login-oauth2-device-verify-user-code",
        "oauth2DeviceVerificationTitle",
        |b, p| {
            p.form = Some(code_form(b, &action, &csrf, &user_code, ""));
        },
    )
    .await
}

/// Whether the person's wrong codes reached the cap in the current window.
async fn capped(state: &OidcState, realm: &RealmConfig, user_id: &str) -> Result<bool> {
    let now = Utc::now();
    Ok(state
        .store
        .get_login_failure(&realm.id, &format!("device:{user_id}"))
        .await?
        .is_some_and(|r| {
            r.window_started_at + Duration::seconds(WRONG_CODE_WINDOW_SECS) > now
                && r.num_failures >= MAX_WRONG_CODES
        }))
}

async fn count_wrong_code(
    state: &OidcState,
    realm: &RealmConfig,
    user_id: &str,
    client_ip: &str,
) -> Result<()> {
    let key = format!("device:{user_id}");
    let now = Utc::now();
    let mut rec = state
        .store
        .get_login_failure(&realm.id, &key)
        .await?
        .filter(|r| r.window_started_at + Duration::seconds(WRONG_CODE_WINDOW_SECS) > now)
        .unwrap_or_else(|| LoginFailureRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            target_key: key,
            num_failures: 0,
            last_failure_ms: 0,
            last_ip_failure: String::new(),
            locked_until: None,
            window_started_at: now,
        });
    rec.num_failures += 1;
    rec.last_failure_ms = now.timestamp_millis();
    client_ip.clone_into(&mut rec.last_ip_failure);
    state.store.upsert_login_failure(rec).await
}

/// `POST /realms/{realm}/device`: a signed-in person enters a user code, then allows or denies.
pub async fn device_submit(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    client_ip: crate::client_ip::ClientIp,
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let action = page_path(&mount, &realm);
    let Some(sso) = signed_in(&state, &realm, &mount, &headers).await? else {
        return Ok((
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, action),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
        )
            .into_response());
    };
    let csrf = csrf_of(&state, &sso);
    let presented = form.get("csrf").map(String::as_str).unwrap_or_default();
    if !constant_time_str_eq(presented, &csrf) {
        let again = action.clone();
        return render(
            &state,
            &mount,
            &realm,
            &headers,
            StatusCode::FORBIDDEN,
            "login-page-expired",
            "pageExpiredTitle",
            |b, p| {
                p.instructions
                    .push(b.get("jcPageExpiredExplained").to_owned());
                p.links.push(page::Link {
                    id: "loginRestartLink".into(),
                    class: "pf-m-primary",
                    url: again,
                    label: b.get("oauth2DeviceVerificationTitle").to_owned(),
                });
            },
        )
        .await;
    }
    if capped(&state, &realm, &sso.user_id).await? {
        return render(
            &state,
            &mount,
            &realm,
            &headers,
            StatusCode::TOO_MANY_REQUESTS,
            "info",
            "oauth2DeviceVerificationTitle",
            |b, p| {
                p.alert = Some(page::Alert::danger(b.get("jcDeviceTooManyCodes")));
            },
        )
        .await;
    }
    let entered = form
        .get("user_code")
        .map(String::as_str)
        .unwrap_or_default();
    let code = normalize_user_code(entered);
    let now = Utc::now();
    let found = if code.len() == 8 {
        state
            .store
            .get_device_code_by_user_code(&realm.id, &sha256_hex(&code))
            .await?
    } else {
        None
    }
    .filter(|r| r.status == DeviceStatus::Pending && !r.consumed && r.expires_at > now);
    let client = match &found {
        Some(rec) => state
            .store
            .get_client_by_client_id(&realm.id, &rec.client_id)
            .await?
            .filter(|c| c.enabled && ensure_device_enabled(c).is_ok()),
        None => None,
    };
    let (Some(mut rec), Some(client)) = (found, client) else {
        count_wrong_code(&state, &realm, &sso.user_id, &client_ip.for_audit()).await?;
        return render(
            &state,
            &mount,
            &realm,
            &headers,
            StatusCode::BAD_REQUEST,
            "login-oauth2-device-verify-user-code",
            "oauth2DeviceVerificationTitle",
            |b, p| {
                p.form = Some(code_form(
                    b,
                    &action,
                    &csrf,
                    "",
                    b.get("oauth2DeviceInvalidUserCodeMessage"),
                ));
            },
        )
        .await;
    };
    state
        .store
        .clear_login_failure(&realm.id, &format!("device:{}", sso.user_id))
        .await?;
    let shown = format!("{}-{}", &code[..4], &code[4..]);
    let app = client
        .name
        .clone()
        .unwrap_or_else(|| client.client_id.clone());
    match form.get("decision").map(String::as_str) {
        None => {
            render(
                &state,
                &mount,
                &realm,
                &headers,
                StatusCode::OK,
                "login-oauth-grant",
                "oauth2DeviceVerificationTitle",
                |b, p| {
                    let app = b.translate_client_name(&app);
                    p.set_heading(b.format("oauthGrantTitle", &[app.as_str()]));
                    p.instructions.push(b.get("oauthGrantRequest").to_owned());
                    p.instructions
                        .extend(rec.scope.split_whitespace().map(str::to_owned));
                    p.info.push(shown.clone());
                    p.form = Some(page::Form {
                        id: "kc-oauth".into(),
                        action: action.clone(),
                        hidden: vec![
                            ("csrf".into(), csrf.clone()),
                            ("user_code".into(), shown.clone()),
                        ],
                        submit_name: "decision".into(),
                        submit_value: "allow".into(),
                        submit_id: "kc-login".into(),
                        submit_label: b.get("doYes").to_owned(),
                        secondary_value: "deny".into(),
                        secondary_id: "kc-cancel".into(),
                        secondary_label: b.get("doNo").to_owned(),
                        ..page::Form::default()
                    });
                },
            )
            .await
        }
        Some(decision @ ("allow" | "deny")) => {
            let allow = decision == "allow";
            if allow {
                rec.status = DeviceStatus::Approved;
                rec.user_id = Some(sso.user_id.clone());
                rec.sso_session_id = Some(sso.id.clone());
                rec.acr.clone_from(&sso.acr);
                rec.amr.clone_from(&sso.amr);
                rec.auth_time = sso.auth_time;
            } else {
                rec.status = DeviceStatus::Denied;
            }
            if !state.store.decide_device_code(rec).await? {
                return render(
                    &state,
                    &mount,
                    &realm,
                    &headers,
                    StatusCode::CONFLICT,
                    "info",
                    "oauth2DeviceVerificationFailedHeader",
                    |b, p| {
                        p.instructions
                            .push(b.get("oauth2DeviceExpiredUserCodeMessage").to_owned());
                    },
                )
                .await;
            }
            state
                .store
                .append_event(AuditEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm.id.clone(),
                    org_id: None,
                    time: now.timestamp_millis(),
                    event_type: if allow {
                        "OAUTH2_DEVICE_VERIFY_USER_CODE".into()
                    } else {
                        "OAUTH2_DEVICE_VERIFY_USER_CODE_ERROR".into()
                    },
                    client_id: Some(client.client_id.clone()),
                    user_id: Some(sso.user_id.clone()),
                    session_id: Some(sso.id.clone()),
                    ip_address: client_ip.for_audit(),
                    error: (!allow).then(|| "consent_denied".into()),
                    details: BTreeMap::new(),
                })
                .await?;
            let (heading, text) = if allow {
                (
                    "oauth2DeviceVerificationCompleteHeader",
                    "oauth2DeviceVerificationCompleteMessage",
                )
            } else {
                (
                    "oauth2DeviceVerificationFailedHeader",
                    "oauth2DeviceConsentDeniedMessage",
                )
            };
            render(
                &state,
                &mount,
                &realm,
                &headers,
                StatusCode::OK,
                "info",
                heading,
                |b, p| {
                    p.instructions.push(b.get(text).to_owned());
                },
            )
            .await
        }
        Some(_) => Err(KaisatsuError::invalid_request("decision is allow or deny")),
    }
}
