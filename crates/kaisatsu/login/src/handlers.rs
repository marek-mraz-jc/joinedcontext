#![forbid(unsafe_code)]
//! Server-rendered Askama login handlers, CSRF check bound to `auth_session`, per-response CSP
//! nonce, `frame-ancestors 'none'`, timing-equal unknown-user handling, and SSO cookie issuance (W7, W9, W12).

use crate::brute_force::BruteForceProtector;
use crate::i18n::{Bundle, negotiate_for_realm, realm_locales};
use crate::machine::{LoginInput, LoginPolicyContext, LoginState, transition};
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Form, Router};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::page::{self, Page};
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_oidc::{ClientIp, OidcState};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub fn login_router(state: OidcState) -> Router {
    Router::new()
        .route(
            "/realms/{realm}/login-actions/authenticate",
            get(authenticate_page_get).post(authenticate_page_post),
        )
        .route(
            "/realms/{realm}/login-actions/restart",
            get(restart_login_get),
        )
        .route(
            "/realms/{realm}/login-actions/reset-credentials",
            get(reset_credentials_get).post(reset_credentials_post),
        )
        .route(
            "/realms/{realm}/login-actions/action-token",
            get(action_token_get).post(action_token_post),
        )
        .route("/realms/{realm}/login-resources/logo.svg", get(logo_get))
        .route(
            "/resources/kaisatsu/{version}/login.css",
            get(stylesheet_get),
        )
        .merge(kaisatsu_oidc::broker_router())
        .merge(crate::account::account_router())
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AuthPageQuery {
    #[serde(default)]
    session_code: String,
    #[serde(default)]
    client_id: String,
    /// The language picked in the page's picker (keycloak.v2's `kc_locale`).
    #[serde(default)]
    kc_locale: String,
}

impl AuthPageQuery {
    /// A `client_id` in the link, when present, must name the client the session was opened for.
    fn matches_client(&self, session_client_id: &str) -> bool {
        self.client_id.is_empty() || self.client_id == session_client_id
    }
}

/// `GET /resources/kaisatsu/{version}/login.css`: the stylesheet of every page, immutable
/// under its version; any other version is not found, so a stale link never gets new rules.
async fn stylesheet_get(Path(version): Path<String>) -> Response {
    if version != format!("{:016x}", page::LOGIN_CSS_VERSION) {
        return StatusCode::NOT_FOUND.into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        page::LOGIN_CSS,
    )
        .into_response()
}

/// `GET /realms/{realm}/login-resources/logo.svg`: the realm's logo from its branding block
/// (sanitised: no script, no foreign object, no handler), served as an image the browser never
/// runs: its own CSP forbids every script and fetch.
async fn logo_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
) -> Result<Response> {
    let Some(realm) = state.store.get_realm(&realm_name).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let branding = load_branding(&state, &realm, None).await?;
    if branding.logo_svg.is_empty() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        branding.logo_svg,
    )
        .into_response())
}

/// The realm's (or the session's organization's) branding block, sanitised; the platform's
/// defaults when none is stored.
pub(crate) async fn load_branding(
    state: &OidcState,
    realm: &RealmConfig,
    org_id: Option<&str>,
) -> Result<BrandingBlock> {
    let stored = match org_id {
        Some(org) => match state.store.get_branding(&realm.id, Some(org)).await? {
            Some(b) => Some(b),
            None => state.store.get_branding(&realm.id, None).await?,
        },
        None => state.store.get_branding(&realm.id, None).await?,
    };
    Ok(stored.unwrap_or_default().sanitised())
}

/// The app a session signs in to, as the page names it: the client's name translated
/// (`${jcClientPortal}` -> "the Portal"), else its id; empty without a client.
async fn app_name(
    state: &OidcState,
    realm: &RealmConfig,
    client_id: &str,
    bundle: &Bundle,
) -> Result<String> {
    if client_id.is_empty() {
        return Ok(String::new());
    }
    let client = state
        .store
        .get_client_by_client_id(&realm.id, client_id)
        .await?
        .unwrap_or_default();
    Ok(client
        .name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| bundle.translate_client_name(n))
        .unwrap_or(client.client_id))
}

/// The page's language: the session's (the app's `ui_locales`, then the picker's), the
/// browser's, the realm's default; only languages the realm offers.
fn page_locale(realm: &RealmConfig, ui_locales: Option<&str>, headers: &HeaderMap) -> &'static str {
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    negotiate_for_realm(realm, ui_locales, None, accept_lang)
}

/// A picked language the realm offers, else `None`.
fn picked_locale(realm: &RealmConfig, kc_locale: &str) -> Option<&'static str> {
    realm_locales(realm).into_iter().find(|l| *l == kc_locale)
}

fn new_nonce() -> String {
    random_handle_256()[..16].to_owned()
}

/// The `form-action` sources of a login page whose form ends the flow at `ends_at`: `'self'`, and
/// the origin of that redirect URI when it is another http(s) origin. Browsers apply
/// `form-action` to the redirect after the POST too, so without it the last step towards a
/// client on its own host (an Organization's Portal) is blocked. `ends_at` is the flow's
/// redirect URI, already validated against the client's registered ones.
fn form_action_sources(ends_at: Option<&str>) -> String {
    let origin = ends_at
        .and_then(|uri| url::Url::parse(uri).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .map(|url| url.origin().ascii_serialization());
    match origin {
        Some(origin) => format!("'self' {origin}"),
        None => "'self'".to_owned(),
    }
}

fn render_with_security_headers(
    tpl: &Page,
    extra_cookie: Option<String>,
    ends_at: Option<&str>,
) -> Response {
    let html = tpl.render().unwrap_or_default();
    let mut resp = (
        StatusCode::OK,
        page::security_headers(&tpl.nonce, &form_action_sources(ends_at)),
        html,
    )
        .into_response();
    if let Some(ck) = extra_cookie
        && let Ok(val) = header::HeaderValue::from_str(&ck)
    {
        resp.headers_mut().append(header::SET_COOKIE, val);
    }
    resp
}

/// The username field's label as keycloak.v2 chose it: "Username" when e-mail sign-in is off,
/// "Username or email" when the username is not the address, else "Email".
fn username_label(bundle: &Bundle, realm: &RealmConfig) -> String {
    if !realm.login_with_email_allowed {
        bundle.get("username").to_owned()
    } else if !realm.registration_email_as_username {
        bundle.get("usernameOrEmail").to_owned()
    } else {
        bundle.get("email").to_owned()
    }
}

fn text_field(name: &str, label: String, autocomplete: &str) -> page::Field {
    page::Field {
        name: name.into(),
        label,
        input_type: "text".into(),
        autocomplete: autocomplete.into(),
        ..page::Field::default()
    }
}

fn password_field(name: &str, label: String, autocomplete: &str) -> page::Field {
    page::Field {
        input_type: "password".into(),
        ..text_field(name, label, autocomplete)
    }
}

/// The hidden fields every login step's form carries: the session code, its CSRF token and the
/// step the form was rendered for (a form of an earlier step meets the "page expired" page).
fn step_hidden(auth_session: &AuthSession, page_id: &str) -> Vec<(String, String)> {
    vec![
        ("csrf_token".into(), auth_session.csrf_token.clone()),
        ("session_code".into(), auth_session.id.clone()),
        ("step".into(), page_id.to_owned()),
    ]
}

/// The page id (`data-page-id="login-{id}"`) of a login step.
fn page_id_of(login_state: &LoginState) -> &'static str {
    match login_state {
        LoginState::IdentityFirst | LoginState::Password { .. } => "login",
        LoginState::SecondFactorOtp { .. } => "login-otp",
        LoginState::EnrolTotp { .. } => "config-totp",
        LoginState::RecoveryCode { .. } => "recovery-code",
        LoginState::UpdatePassword { .. } => "update-password",
        LoginState::OrganizationChoice { .. } => "org-choice",
        LoginState::TermsAndConditions { .. } => "terms",
        LoginState::VerifyEmail { .. } => "verify-email",
        LoginState::RegisterWebAuthn { .. } => "webauthn-register",
        LoginState::Passkey { .. } | LoginState::SecondFactorWebAuthn { .. } => {
            "webauthn-authenticate"
        }
        LoginState::Consent { .. } => "consent",
        _ => "info",
    }
}

async fn build_policy_context(
    store: &Arc<dyn Store>,
    realm: &RealmConfig,
    auth_session: &AuthSession,
    user: &UserRepresentation,
) -> Result<LoginPolicyContext> {
    let creds = store.list_credentials(&realm.id, &user.id).await?;
    let has_totp = creds.iter().any(|c| c.credential_type == "otp");
    let has_webauthn = creds
        .iter()
        .any(|c| c.credential_type.starts_with("webauthn"));
    let user_groups = store.list_user_groups(&realm.id, &user.id).await?;
    let realm_requires = second_factor_required(store, realm, user).await?;
    let step_up = auth_session.acr_requested.as_deref() == Some("mfa");

    let memberships = store
        .list_memberships_for_person(&realm.id, &user.id)
        .await?;
    let mut org_choices: Vec<String> = memberships.into_iter().map(|m| m.org_id).collect();
    for g in &user_groups {
        if let Some(orgs) = g.attributes.get("jc_org") {
            for o in orgs {
                if !org_choices.contains(o) {
                    org_choices.push(o.clone());
                }
            }
        }
    }
    let pinned_org = auth_session
        .scope
        .split_whitespace()
        .find_map(|s| s.strip_prefix("organization:"))
        .map(str::to_owned)
        .or_else(|| auth_session.org_id.clone());

    // K2: Only honour `organization:<alias>` when the person is actually a member of that
    // organization (by id, alias, membership, group jc_org, or user.org_id). A non-member never
    // gets a foreign organization pinned onto their session or token.
    let mut member_org_tokens: Vec<String> = org_choices.clone();
    if let Some(ref u_org) = user.org_id
        && !member_org_tokens.contains(u_org)
    {
        member_org_tokens.push(u_org.clone());
    }
    if let Ok(all_orgs) = store.list_organizations(&realm.id, None).await {
        for o in all_orgs {
            if member_org_tokens.contains(&o.id) || member_org_tokens.contains(&o.alias) {
                if !member_org_tokens.contains(&o.id) {
                    member_org_tokens.push(o.id);
                }
                if !member_org_tokens.contains(&o.alias) {
                    member_org_tokens.push(o.alias);
                }
            }
        }
    }
    let pinned_org = pinned_org.filter(|pin| member_org_tokens.iter().any(|m| m == pin));

    let client = store
        .get_client_by_client_id(&realm.id, &auth_session.client_id)
        .await?
        .unwrap_or_default();
    let consents = store.list_consents(&realm.id, &user.id).await?;
    let has_consent = consents.iter().any(|c| c.client_id == client.client_id);

    let mut pending = user.required_actions.clone();
    if creds
        .iter()
        .any(|c| c.credential_type == "password" && c.temporary)
        && !pending.iter().any(|a| a == "UPDATE_PASSWORD")
    {
        pending.push("UPDATE_PASSWORD".into());
    }

    Ok(LoginPolicyContext {
        second_factor_required: realm_requires || step_up,
        has_totp_credential: has_totp,
        has_webauthn_credential: has_webauthn,
        pending_actions: pending,
        org_choices,
        pinned_org,
        consent_required: client.consent_required && !has_consent,
        app_action: auth_session
            .kc_action
            .clone()
            .filter(|_| !auth_session.kc_action_done),
    })
}

/// True when the realm requires a second factor of this person at every sign-in: the
/// `master` realm, or the effective realm role `mfa-required` (direct, composite or by group).
pub(crate) async fn second_factor_required(
    store: &Arc<dyn Store>,
    realm: &RealmConfig,
    user: &UserRepresentation,
) -> Result<bool> {
    if realm.realm == "master" {
        return Ok(true);
    }
    let user_groups = store.list_user_groups(&realm.id, &user.id).await?;
    let all_roles = store.list_roles(&realm.id, None).await?;
    let group_roles: Vec<String> = user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let eff_roles = kaisatsu_core::claims::resolve_effective_realm_roles(
        &user.realm_roles,
        &group_roles,
        &all_roles,
    );
    Ok(eff_roles.iter().any(|r| r == "mfa-required"))
}

async fn render_state_page(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    auth_session: &AuthSession,
    login_state: &LoginState,
    headers: &HeaderMap,
    alert_message: &str,
) -> Result<Response> {
    let alert = (!alert_message.is_empty()).then(|| page::Alert::danger(alert_message));
    render_state_page_with(
        state,
        mount,
        realm,
        auth_session,
        login_state,
        headers,
        alert,
    )
    .await
}

/// [`render_state_page`] with any banner (the reset page's "e-mail on its way" is a success).
async fn render_state_page_with(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    auth_session: &AuthSession,
    login_state: &LoginState,
    headers: &HeaderMap,
    mut alert: Option<page::Alert>,
) -> Result<Response> {
    let bundle = Bundle::for_locale(page_locale(
        realm,
        auth_session.ui_locales.as_deref(),
        headers,
    ));
    let branding = load_branding(state, realm, auth_session.org_id.as_deref()).await?;
    let app = app_name(state, realm, &auth_session.client_id, &bundle).await?;
    let base = format!(
        "{}/realms/{}/login-actions/authenticate?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );
    let restart_url = format!(
        "{}/realms/{}/login-actions/restart?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );
    let reset_url = format!(
        "{}/realms/{}/login-actions/reset-credentials?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );
    let shell = page::Shell {
        bundle: &bundle,
        realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: &app,
        locale_base_url: &base,
    };
    let page_id = page_id_of(login_state);
    let nonce = new_nonce();
    // An app-initiated action (`kc_action`) is asked for, not required: no "you need to" banner.
    let app_action = auth_session.kc_action.is_some() && !auth_session.kc_action_done;
    let user_of = |user_id: &str| {
        let user_id = user_id.to_owned();
        let store = state.store.clone();
        let realm_id = realm.id.clone();
        async move { store.get_user_by_id(&realm_id, None, &user_id).await }
    };
    let mut form = page::Form {
        id: "kc-form-login".into(),
        action: base.clone(),
        hidden: step_hidden(auth_session, page_id),
        submit_name: "login".into(),
        submit_id: "kc-login".into(),
        submit_label: bundle.get("doLogIn").to_owned(),
        ..page::Form::default()
    };
    let mut tpl;
    let mut field_error: Option<&str> = None;
    match login_state {
        LoginState::IdentityFirst | LoginState::Password { .. } => {
            tpl = shell.page(page_id, bundle.get("loginAccountTitle"), &nonce);
            let mut username = text_field(
                "username",
                username_label(&bundle, realm),
                "username webauthn",
            );
            username.value = auth_session.login_hint.clone().unwrap_or_default();
            username.autofocus = true;
            let mut password = password_field(
                "password",
                bundle.get("password").to_owned(),
                "current-password",
            );
            if realm.reset_password_allowed {
                password.link_id = "kc-forgot-password".into();
                password.link_url = reset_url;
                password.link_label = bundle.get("doForgotPassword").to_owned();
            }
            form.fields = vec![username, password];
            if app_action {
                tpl.instructions
                    .push(bundle.get("reauthenticate").to_owned());
            }
        }
        LoginState::SecondFactorOtp { user_id, .. } => {
            tpl = shell.page(page_id, bundle.get("loginAccountTitle"), &nonce);
            if let Some(user) = user_of(user_id).await? {
                tpl.attempted = Some(page::Attempted {
                    label: username_label(&bundle, realm),
                    username: user.username,
                    restart_url: restart_url.clone(),
                    restart_label: bundle.get("restartLoginTooltip").to_owned(),
                });
            }
            form.id = "kc-otp-login-form".into();
            let mut otp = text_field(
                "otp",
                bundle.get("loginOtpOneTime").to_owned(),
                "one-time-code",
            );
            otp.autofocus = true;
            form.fields = vec![otp];
            field_error = Some("otp");
        }
        LoginState::EnrolTotp { user_id, .. } => {
            tpl = shell.page(page_id, bundle.get("loginTotpTitle"), &nonce);
            let secret = match &auth_session.pending_totp_secret {
                Some(existing) => existing.clone(),
                None => {
                    let fresh = generate_totp_secret();
                    let mut updated = auth_session.clone();
                    updated.pending_totp_secret = Some(fresh.clone());
                    state.store.put_auth_session(updated).await?;
                    fresh
                }
            };
            let account = user_of(user_id)
                .await?
                .map(|u| u.username)
                .unwrap_or_default();
            tpl.totp = Some(totp_setup(&bundle, realm, &shell.org(), &account, &secret));
            if alert.is_none() && !app_action {
                alert = Some(page::Alert::warning(bundle.get("configureTotpMessage")));
            }
            form.id = "kc-totp-settings-form".into();
            form.submit_id = "saveTOTPBtn".into();
            form.submit_label = bundle.get("doSubmit").to_owned();
            let mut otp = text_field(
                "otp",
                bundle.get("loginOtpOneTime").to_owned(),
                "one-time-code",
            );
            otp.required = true;
            form.fields = vec![
                otp,
                text_field(
                    "userLabel",
                    bundle.get("loginTotpDeviceName").to_owned(),
                    "off",
                ),
                logout_sessions_field(&bundle, false),
            ];
            (form.secondary_url, form.secondary_label) = cancel_link(auth_session, &bundle, true);
            form.secondary_id = "cancelTOTPBtn".into();
            field_error = Some("otp");
        }
        LoginState::RecoveryCode { .. } => {
            tpl = shell.page(page_id, bundle.get("recoveryCodeLabel"), &nonce);
            tpl.instructions
                .push(bundle.get("recoveryCodeHint").to_owned());
            form.fields = vec![text_field(
                "recovery_code",
                bundle.get("recoveryCodeLabel").to_owned(),
                "off",
            )];
        }
        LoginState::UpdatePassword { .. } => {
            tpl = shell.page(page_id, bundle.get("updatePasswordTitle"), &nonce);
            if alert.is_none() && !app_action {
                alert = Some(page::Alert::warning(bundle.get("updatePasswordMessage")));
            }
            form.id = "kc-passwd-update-form".into();
            form.submit_id = "kc-submit".into();
            form.submit_label = bundle.get("doSubmit").to_owned();
            let mut new_password = password_field(
                "new_password",
                bundle.get("passwordNew").to_owned(),
                "new-password",
            );
            new_password.autofocus = true;
            form.fields = vec![
                new_password,
                password_field(
                    "password_confirm",
                    bundle.get("passwordConfirm").to_owned(),
                    "new-password",
                ),
                logout_sessions_field(&bundle, false),
            ];
            (form.secondary_url, form.secondary_label) = cancel_link(auth_session, &bundle, true);
            form.secondary_id = "kc-cancel".into();
            let mismatch = alert
                .as_ref()
                .is_some_and(|a| a.text == bundle.get("notMatchPasswordMessage"));
            field_error = Some(if mismatch {
                "password_confirm"
            } else {
                "new_password"
            });
        }
        LoginState::OrganizationChoice { candidates, .. } => {
            tpl = shell.page(page_id, bundle.get("orgChoiceTitle"), &nonce);
            tpl.instructions
                .push(bundle.get("orgChoiceHint").to_owned());
            let mut org = text_field(
                "org_id",
                bundle.get("orgChoiceLabel").to_owned(),
                "organization",
            );
            org.value = candidates.first().cloned().unwrap_or_default();
            form.fields = vec![org];
            form.submit_label = bundle.get("proceedWithAction").to_owned();
        }
        LoginState::TermsAndConditions { .. } => {
            tpl = shell.page(page_id, bundle.get("termsTitle"), &nonce);
            tpl.instructions
                .push(bundle.get("termsInstruction").to_owned());
            form.submit_label = bundle.get("proceedWithAction").to_owned();
        }
        LoginState::VerifyEmail { user_id, .. } => {
            tpl = shell.page(page_id, bundle.get("emailVerifyTitle"), &nonce);
            let user = user_of(user_id).await?;
            // K7 / K9: the mail is queued once per session. It carries only the signed action
            // link (K9 stores no secret), so the step completes once that link set
            // `email_verified`; this random value only marks the mail as queued.
            if auth_session.pending_email_code_hash.is_none() {
                let code = random_handle_256();
                let mut updated = auth_session.clone();
                updated.pending_email_code_hash = Some(sha256_hex(&code));
                state.store.put_auth_session(updated).await?;
                if let Some(user) = &user
                    && let Some(ref email) = user.email
                {
                    let _ = state
                        .store
                        .enqueue_outbox(EmailOutboxRecord {
                            id: uuid::Uuid::now_v7().to_string(),
                            realm_id: realm.id.clone(),
                            org_id: user.org_id.clone(),
                            user_id: user.id.clone(),
                            to_address: email.clone(),
                            locale: bundle.locale.to_owned(),
                            template_kind: "VERIFY_EMAIL".into(),
                            action_jti: uuid::Uuid::now_v7().to_string(),
                            actions: vec!["VERIFY_EMAIL".into()],
                            client_id: Some(auth_session.client_id.clone()),
                            redirect_uri: Some(auth_session.redirect_uri.clone()),
                            lifespan_secs: 900,
                            issuer: mount.realm_issuer(&realm.realm),
                            status: "pending".into(),
                            attempts: 0,
                            last_error: None,
                            next_attempt_at: Utc::now(),
                            created_at: Utc::now(),
                            sent_at: None,
                        })
                        .await;
                }
            }
            if alert.is_none() {
                alert = Some(page::Alert::warning(bundle.get("verifyEmailMessage")));
            }
            let address = user.and_then(|u| u.email).unwrap_or_default();
            tpl.instructions
                .push(bundle.format("emailVerifyInstruction1", &[&address]));
            let resp = render_with_security_headers(
                &page::Page { alert, ..tpl },
                None,
                Some(&auth_session.redirect_uri),
            );
            return Ok(resp);
        }
        LoginState::Passkey { .. }
        | LoginState::SecondFactorWebAuthn { .. }
        | LoginState::RegisterWebAuthn { .. } => {
            tpl = shell.page(page_id, bundle.get("loginAccountTitle"), &nonce);
            let challenge = match &auth_session.webauthn_challenge {
                Some(c) => c.clone(),
                None => {
                    let fresh = random_handle_256();
                    let mut updated = auth_session.clone();
                    updated.webauthn_challenge = Some(fresh.clone());
                    state.store.put_auth_session(updated).await?;
                    fresh
                }
            };
            let field = if matches!(login_state, LoginState::RegisterWebAuthn { .. }) {
                "webauthn_register_response"
            } else {
                "webauthn_assertion_response"
            };
            form.hidden.push((field.into(), String::new()));
            form.hidden.push(("webauthn_challenge".into(), challenge));
            form.submit_label = bundle.get("proceedWithAction").to_owned();
        }
        LoginState::Consent { .. } => {
            tpl = shell.page(page_id, bundle.get("consentTitle"), &nonce);
            tpl.instructions
                .push(bundle.get("consentInstruction").to_owned());
            form.submit_label = bundle.get("proceedWithAction").to_owned();
        }
        _ => {
            tpl = shell.page(page_id, bundle.get("loginAccountTitle"), &nonce);
            form.submit_label = bundle.get("proceedWithAction").to_owned();
        }
    }

    // On the code and password pages the error belongs to the field it is about, as
    // keycloak.v2's templates show it; the sign-in page keeps the banner (either value can be
    // the wrong one, T-4203).
    if let (Some(name), Some(a)) = (field_error, alert.as_ref())
        && a.kind == "danger"
        && let Some(field) = form.fields.iter_mut().find(|f| f.name == name)
    {
        field.error = a.text.clone();
        alert = None;
    }
    tpl.alert = alert;
    tpl.form = Some(form);
    Ok(render_with_security_headers(
        &tpl,
        None,
        Some(&auth_session.redirect_uri),
    ))
}

/// keycloak.v2's authenticator set-up steps: the apps, the QR code (with the key and its
/// settings behind "Unable to scan?"), then the code and the device name.
fn totp_setup(
    bundle: &Bundle,
    realm: &RealmConfig,
    issuer: &str,
    account: &str,
    secret: &str,
) -> page::TotpSetup {
    let key = secret
        .as_bytes()
        .chunks(4)
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join(" ");
    let algorithm = realm
        .otp_policy_algorithm
        .trim_start_matches("Hmac")
        .to_ascii_uppercase();
    page::TotpSetup {
        step1: bundle.get("loginTotpStep1").to_owned(),
        apps: vec![
            bundle.get("totpAppFreeOTPName").to_owned(),
            bundle.get("totpAppGoogleName").to_owned(),
        ],
        step2: bundle.get("loginTotpStep2").to_owned(),
        qr_svg: totp_qr_svg(realm, issuer, account, secret, bundle.get("loginTotpStep2")),
        unable_to_scan: bundle.get("loginTotpUnableToScan").to_owned(),
        manual_step2: bundle.get("loginTotpManualStep2").to_owned(),
        key,
        manual_step3: bundle.get("loginTotpManualStep3").to_owned(),
        config: vec![
            format!(
                "{}: {}",
                bundle.get("loginTotpType"),
                bundle.get("loginTotp.totp")
            ),
            format!("{}: {algorithm}", bundle.get("loginTotpAlgorithm")),
            format!(
                "{}: {}",
                bundle.get("loginTotpDigits"),
                realm.otp_policy_digits
            ),
            format!(
                "{}: {}",
                bundle.get("loginTotpInterval"),
                realm.otp_policy_period
            ),
        ],
        step3: bundle.get("loginTotpStep3").to_owned(),
        step3_device: bundle.get("loginTotpStep3DeviceName").to_owned(),
    }
}

/// The `otpauth://` URI of a new authenticator as an inline SVG QR code (Keycloak shows one on
/// its set-up page). Every value is percent-encoded into the URI; the SVG holds only rects.
fn totp_qr_svg(
    realm: &RealmConfig,
    issuer: &str,
    account: &str,
    secret: &str,
    title: &str,
) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let algorithm = realm
        .otp_policy_algorithm
        .trim_start_matches("Hmac")
        .to_ascii_uppercase();
    let uri = format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm={algorithm}&digits={}&period={}",
        enc(issuer),
        enc(account),
        enc(secret),
        enc(issuer),
        realm.otp_policy_digits,
        realm.otp_policy_period,
    );
    let Ok(qr) = qrcodegen::QrCode::encode_text(&uri, qrcodegen::QrCodeEcc::Medium) else {
        return String::new();
    };
    let size = qr.size();
    let border = 4;
    let dim = size + 2 * border;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    let title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" width=\"200\" height=\"200\" role=\"img\" aria-labelledby=\"qr-title\" shape-rendering=\"crispEdges\"><title id=\"qr-title\">{title}</title><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    )
}

/// The User-Agent a new SSO session keeps: control characters dropped, at most 256 characters.
fn user_agent_of(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

fn device_label(raw: Option<&String>) -> String {
    let label: String = raw
        .map(|s| s.trim())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    if label.is_empty() {
        "Authenticator".into()
    } else {
        label
    }
}

/// The "sign out other devices" box of the password and authenticator pages (Keycloak's
/// `logout-sessions`); `checked` presets it, as Keycloak does on the password page.
fn logout_sessions_field(bundle: &Bundle, checked: bool) -> page::Field {
    page::Field {
        name: "logout-sessions".into(),
        label: bundle.get("logoutOtherSessions").to_owned(),
        input_type: "checkbox".into(),
        value: if checked { "checked" } else { "" }.into(),
        ..page::Field::default()
    }
}

/// The cancel link of an application-initiated action page: back to the client's registered
/// `redirect_uri` with `kc_action_status=cancelled`, nothing changed. Empty outside an action.
fn cancel_link(
    auth_session: &AuthSession,
    bundle: &Bundle,
    on_action_page: bool,
) -> (String, String) {
    if !on_action_page || auth_session.kc_action.is_none() || auth_session.kc_action_done {
        return (String::new(), String::new());
    }
    let Ok(mut url) = url::Url::parse(&auth_session.redirect_uri) else {
        return (String::new(), String::new());
    };
    url.query_pairs_mut()
        .append_pair("kc_action_status", "cancelled");
    if let Some(st) = &auth_session.state {
        url.query_pairs_mut().append_pair("state", st);
    }
    (url.to_string(), bundle.get("doCancel").to_owned())
}

/// Ends every SSO session of the person except the one this browser holds (the "sign out other
/// devices" box): their refresh tokens die with them and each client hears a back-channel logout.
async fn sign_out_other_sessions(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    user_id: &str,
    headers: &HeaderMap,
) -> Result<()> {
    let current = read_cookie(headers, &mount.cookie_name("sso")).map(|raw| sha256_hex(&raw));
    for sso in state
        .store
        .list_user_sso_sessions(&realm.id, user_id)
        .await?
    {
        if current.as_deref() == Some(sso.handle_hash.as_str()) {
            continue;
        }
        if let Some(ended) = state.store.delete_sso_session(&realm.id, &sso.id).await? {
            let _ = kaisatsu_oidc::dispatch_backchannel_logout(state, mount, realm, &ended).await;
        }
    }
    Ok(())
}

pub(crate) fn read_cookie(headers: &HeaderMap, target: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';').map(str::trim))
        .filter_map(|part| part.split_once('='))
        .find(|(name, v)| *name == target && !v.is_empty())
        .map(|(_, v)| v.to_owned())
}

pub(crate) fn read_auth_cookie(headers: &HeaderMap, mount: &Mount) -> Option<String> {
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

/// K1: Verifies that the browser's auth session cookie matches `auth_session.handle_hash` in
/// constant time (`subtle`).
pub(crate) fn auth_cookie_matches(
    headers: &HeaderMap,
    mount: &Mount,
    auth_session: &AuthSession,
) -> bool {
    let Some(raw_cookie) = read_auth_cookie(headers, mount) else {
        return false;
    };
    let presented_hash = sha256_hex(&raw_cookie);
    constant_time_str_eq(&presented_hash, &auth_session.handle_hash)
}

/// keycloak.v2's "page has expired" (theme's login-page-expired.ftl): a form of an earlier
/// step, a stale link, or a session this browser does not hold. With a session this browser
/// holds it offers both ways on: back to the current step, or sign in again from the start.
pub(crate) async fn render_expired_session_page(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    headers: &HeaderMap,
    auth_session: Option<&AuthSession>,
    status: StatusCode,
) -> Result<Response> {
    let bundle = Bundle::for_locale(page_locale(
        realm,
        auth_session.and_then(|s| s.ui_locales.as_deref()),
        headers,
    ));
    let branding =
        load_branding(state, realm, auth_session.and_then(|s| s.org_id.as_deref())).await?;
    let app = match auth_session {
        Some(s) => app_name(state, realm, &s.client_id, &bundle).await?,
        None => String::new(),
    };
    let base = auth_session.map(|s| {
        format!(
            "{}/realms/{}/login-actions/{{}}?session_code={}&client_id={}",
            mount.path_prefix, realm.realm, s.id, s.client_id
        )
    });
    let shell = page::Shell {
        bundle: &bundle,
        realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: &app,
        locale_base_url: "",
    };
    let mut tpl = shell.page("expired", bundle.get("pageExpiredTitle"), &new_nonce());
    tpl.instructions
        .push(bundle.get("jcPageExpiredExplained").to_owned());
    match base {
        Some(base) => {
            tpl.links = vec![
                page::Link {
                    id: "loginContinueLink".into(),
                    class: "pf-m-primary",
                    url: base.replace("{}", "authenticate"),
                    label: bundle.get("jcPageExpiredContinue").to_owned(),
                },
                page::Link {
                    id: "loginRestartLink".into(),
                    class: "pf-m-link",
                    url: base.replace("{}", "restart"),
                    label: bundle.get("jcPageExpiredRestart").to_owned(),
                },
            ];
        }
        None => tpl
            .instructions
            .push(bundle.get("jcErrorNextStep").to_owned()),
    }
    let mut resp = render_with_security_headers(&tpl, None, None);
    *resp.status_mut() = status;
    Ok(resp)
}

/// `GET /realms/{realm}/login-actions/restart`: "Restart login" and "Start signing in again"
/// send the session this browser holds back to its first step (the person signs in from the
/// start; nothing already passed is kept), then to the sign-in page.
async fn restart_login_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<AuthPageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let Some(mut auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
        .filter(|s| query.matches_client(&s.client_id))
        .filter(|s| auth_cookie_matches(&headers, &mount, s))
    else {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    auth_session.step_state_json =
        serde_json::to_string(&LoginState::IdentityFirst).unwrap_or_default();
    auth_session.user_id = None;
    auth_session.amr.clear();
    auth_session.acr_achieved.clear();
    auth_session.pending_totp_secret = None;
    auth_session.webauthn_challenge = None;
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
    )
        .into_response())
}

async fn authenticate_page_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<AuthPageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let Some(mut auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
        .filter(|s| query.matches_client(&s.client_id))
    else {
        return render_expired_session_page(&state, &mount, &realm, &headers, None, StatusCode::OK)
            .await;
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    // The language picker: the session keeps the language picked for its later pages.
    if let Some(picked) = picked_locale(&realm, &query.kc_locale) {
        auth_session.ui_locales = Some(picked.to_owned());
        state.store.put_auth_session(auth_session.clone()).await?;
    }

    let current_state: LoginState =
        serde_json::from_str(&auth_session.step_state_json).unwrap_or(LoginState::IdentityFirst);

    render_state_page(
        &state,
        &mount,
        &realm,
        &auth_session,
        &current_state,
        &headers,
        "",
    )
    .await
}

async fn authenticate_page_post(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<AuthPageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    client_ip: ClientIp,
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let session_code = form
        .get("session_code")
        .or(Some(&query.session_code))
        .cloned()
        .unwrap_or_default();

    let Some(auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, &session_code)
        .await?
        .filter(|s| query.matches_client(&s.client_id))
    else {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }

    // Verify CSRF token bound to `auth_session` (W7)
    let presented_csrf = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if !constant_time_str_eq(presented_csrf, &auth_session.csrf_token) {
        return Err(KaisatsuError::Forbidden("invalid login CSRF token".into()));
    }

    // A form of an earlier step (sent again with Back) meets keycloak.v2's "page has expired",
    // never the current step's checks with another step's fields.
    let rendered_for: LoginState =
        serde_json::from_str(&auth_session.step_state_json).unwrap_or(LoginState::IdentityFirst);
    if form
        .get("step")
        .is_some_and(|step| step != page_id_of(&rendered_for))
    {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            Some(&auth_session),
            StatusCode::OK,
        )
        .await;
    }

    let bundle = Bundle::for_locale(page_locale(
        &realm,
        auth_session.ui_locales.as_deref(),
        &headers,
    ));
    let bf = BruteForceProtector::new(state.store.clone());

    let mut current_state: LoginState =
        serde_json::from_str(&auth_session.step_state_json).unwrap_or(LoginState::IdentityFirst);

    // Allow switching from IdentityFirst/Password to Passkey, or from SecondFactorOtp to RecoveryCode/WebAuthn2fa
    if form.get("switch_to").map(String::as_str) == Some("passkey")
        && matches!(
            current_state,
            LoginState::IdentityFirst | LoginState::Password { .. }
        )
    {
        let next = LoginState::Passkey {
            user_id: match &current_state {
                LoginState::Password { user_id } => Some(user_id.clone()),
                _ => None,
            },
        };
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }
    if form.get("switch_to").map(String::as_str) == Some("recovery_code")
        && let LoginState::SecondFactorOtp {
            ref user_id,
            first_factor_passed: true,
        }
        | LoginState::SecondFactorWebAuthn {
            ref user_id,
            first_factor_passed: true,
        } = current_state
    {
        let next = LoginState::RecoveryCode {
            user_id: user_id.clone(),
            first_factor_passed: true,
        };
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // K11: Direct conditional UI passkey assertion submitted from IdentityFirst or Passkey
    if matches!(
        current_state,
        LoginState::IdentityFirst | LoginState::Passkey { .. }
    ) && let Some(assertion_str) = form
        .get("webauthn_assertion_response")
        .filter(|s| !s.trim().is_empty())
    {
        let Some(challenge) = auth_session.webauthn_challenge.clone() else {
            return Err(KaisatsuError::BadRequest(
                "no WebAuthn challenge in session".into(),
            ));
        };
        let assertion_json: serde_json::Value = serde_json::from_str(assertion_str)
            .map_err(|_| KaisatsuError::BadRequest("invalid WebAuthn assertion JSON".into()))?;
        let expected_uid = match &current_state {
            LoginState::Passkey { user_id } => user_id.as_deref(),
            _ => None,
        };
        let mut auth_session = auth_session;
        // Challenge is single-use: clear it before verification.
        auth_session.webauthn_challenge = None;
        state.store.put_auth_session(auth_session.clone()).await?;
        let (user, user_verified) = crate::passkey::complete_passkey_authentication(
            &state.store,
            &mount,
            &realm,
            expected_uid,
            &challenge,
            &assertion_json,
            true,
        )
        .await?;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(
            LoginState::Passkey {
                user_id: Some(user.id.clone()),
            },
            LoginInput::VerifyPasskeyOk {
                user_id: user.id,
                user_verified,
            },
            &ctx,
        )
        .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // Track the state at the start of this POST so a failed combined password+OTP submit
    // stays on the password step (K8).
    let state_before_post = current_state.clone();
    let is_combined_pw_otp = matches!(
        current_state,
        LoginState::IdentityFirst | LoginState::Password { .. }
    ) && form.contains_key("otp");

    // Handle combined username + password form submission from `IdentityFirst` or `Password`
    if matches!(
        current_state,
        LoginState::IdentityFirst | LoginState::Password { .. }
    ) {
        let username = form
            .get("username")
            .map(String::as_str)
            .unwrap_or_default()
            .trim();
        let password = form.get("password").map(String::as_str).unwrap_or_default();

        if username.is_empty() {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("missingUsernameMessage"),
            )
            .await;
        }

        // Timing-equal lookup: if user does not exist, run dummy Argon2id hash and return the
        // exact same invalidUserMessage page (W3, W7, W12).
        let Some(user) = state
            .store
            .get_user_by_username_or_email(&realm.id, None, username)
            .await?
            .filter(|u| u.enabled)
        else {
            kaisatsu_crypto::password::dummy_verify_password(password);
            let _ = bf.record_failure(&realm, None, client_ip.as_deref()).await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        };

        if bf
            .is_locked(&realm, Some(&user.id), client_ip.as_deref())
            .await?
        {
            kaisatsu_crypto::password::dummy_verify_password(password);
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }

        let creds = state.store.list_credentials(&realm.id, &user.id).await?;
        let Some(pw_cred) = creds.iter().find(|c| c.credential_type == "password") else {
            kaisatsu_crypto::password::dummy_verify_password(password);
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        };

        let outcome = verify_and_maybe_rehash(password, pw_cred)?;
        if !outcome.valid {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }
        // The person's counter is cleared only when the whole login completes (in
        // `advance_or_render` at `Code`), so a known password cannot reset OTP guessing.
        if let Some(new_phc) = outcome.rehashed_phc {
            let mut updated = pw_cred.clone();
            updated.secret_data = new_phc;
            updated.credential_data.clear();
            state.store.upsert_credential(updated).await?;
        }

        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let s1 = transition(
            LoginState::IdentityFirst,
            LoginInput::SubmitIdentifier {
                user_id: user.id.clone(),
                routed_idp: None,
            },
            &ctx,
        )
        .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        current_state = transition(s1, LoginInput::VerifyPasswordOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;

        // If `otp` was also submitted in the same POST (or transition reached `Code`), handle next
        if !form.contains_key("otp") || !matches!(current_state, LoginState::SecondFactorOtp { .. })
        {
            return advance_or_render(
                &state,
                &mount,
                &realm,
                auth_session,
                current_state,
                &headers,
                &client_ip,
            )
            .await;
        }
    }

    if let LoginState::SecondFactorOtp { ref user_id, .. }
    | LoginState::EnrolTotp { ref user_id, .. } = current_state
    {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let otp_code = form
            .get("otp")
            .map(String::as_str)
            .unwrap_or_default()
            .trim();

        // A locked person gets the same answer as a wrong code, before any code is checked.
        if bf
            .is_locked(&realm, Some(&user.id), client_ip.as_deref())
            .await?
        {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }

        if let LoginState::EnrolTotp { .. } = current_state {
            // The secret is the one the server generated and showed on the enrolment page; a
            // missing one (page never rendered) re-renders the page, which generates it.
            let Some(new_secret) = auth_session.pending_totp_secret.clone() else {
                return render_state_page(
                    &state,
                    &mount,
                    &realm,
                    &auth_session,
                    &current_state,
                    &headers,
                    "",
                )
                .await;
            };
            let cred = CredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                user_id: user.id.clone(),
                credential_type: "otp".into(),
                user_label: Some(device_label(form.get("userLabel"))),
                created_date: Utc::now().timestamp_millis(),
                secret_data: serde_json::json!({ "value": new_secret }).to_string(),
                credential_data: serde_json::json!({
                    "subType": "totp",
                    "digits": realm.otp_policy_digits,
                    "period": realm.otp_policy_period,
                    "algorithm": realm.otp_policy_algorithm,
                    "secretEncoding": "BASE32",
                })
                .to_string(),
                temporary: false,
            };
            let now_secs = Utc::now().timestamp() as u64;
            if verify_totp_code(
                &cred,
                otp_code,
                realm.otp_policy_look_ahead_window,
                now_secs,
            )?
            .is_none()
            {
                let _ = bf
                    .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                    .await;
                return render_state_page(
                    &state,
                    &mount,
                    &realm,
                    &auth_session,
                    &current_state,
                    &headers,
                    bundle.get("invalidTotpMessage"),
                )
                .await;
            }
            state.store.upsert_credential(cred).await?;
            if form.contains_key("logout-sessions") {
                sign_out_other_sessions(&state, &mount, &realm, &user.id, &headers).await?;
            }
            let mut auth_session = auth_session;
            auth_session.pending_totp_secret = None;
            if auth_session.kc_action.as_deref() == Some("CONFIGURE_TOTP") {
                auth_session.kc_action_done = true;
            }
            let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
            let next = transition(current_state, LoginInput::CompleteEnrolTotpOk, &ctx)
                .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
            return advance_or_render(
                &state,
                &mount,
                &realm,
                auth_session,
                next,
                &headers,
                &client_ip,
            )
            .await;
        }

        let creds = state.store.list_credentials(&realm.id, &user.id).await?;
        let otp_cred = creds
            .iter()
            .find(|c| c.credential_type == "otp")
            .ok_or_else(|| KaisatsuError::BadRequest("no OTP credential".into()))?;
        let now_secs = Utc::now().timestamp() as u64;
        let Some(matched_step) = verify_totp_code(
            otp_cred,
            otp_code,
            realm.otp_policy_look_ahead_window,
            now_secs,
        )?
        else {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            // K8: On a failed combined password+OTP POST, render the original password step
            // (`state_before_post`) without echoing the password or advancing `auth_session`.
            let render_state = if is_combined_pw_otp {
                &state_before_post
            } else {
                &current_state
            };
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                render_state,
                &headers,
                if is_combined_pw_otp {
                    bundle.get("invalidUserMessage")
                } else {
                    bundle.get("invalidTotpMessage")
                },
            )
            .await;
        };

        let step_key = sha256_hex(&format!("totp:{}:{}:{matched_step}", user.id, otp_cred.id));
        if state
            .store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: step_key,
                realm_id: realm.id.clone(),
                expires_at: Utc::now() + Duration::seconds(90),
            })
            .await
            .is_err()
        {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            let render_state = if is_combined_pw_otp {
                &state_before_post
            } else {
                &current_state
            };
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                render_state,
                &headers,
                if is_combined_pw_otp {
                    bundle.get("invalidUserMessage")
                } else {
                    bundle.get("invalidTotpMessage")
                },
            )
            .await;
        }

        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::VerifyOtpOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // UPDATE_PASSWORD (a temporary password, or the required action): the new password must
    // pass the realm policy and history; it replaces the old one, which becomes history.
    if let LoginState::UpdatePassword { ref user_id, .. } = current_state {
        let mut user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let new_password = form
            .get("new_password")
            .map(String::as_str)
            .unwrap_or_default();
        let history: Vec<CredentialRecord> = state
            .store
            .list_credentials(&realm.id, &user.id)
            .await?
            .into_iter()
            .filter(|c| c.credential_type.starts_with("password"))
            .collect();
        if form
            .get("password_confirm")
            .is_some_and(|confirm| confirm != new_password)
        {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("notMatchPasswordMessage"),
            )
            .await;
        }
        if let Some(violation) = first_policy_violation(
            &realm.password_policy,
            new_password,
            &user.username,
            user.email.as_deref(),
            &history,
        ) {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                &policy_message(&bundle, &violation),
            )
            .await;
        }
        state
            .store
            .upsert_credential(CredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                user_id: user.id.clone(),
                credential_type: "password".into(),
                user_label: Some("Password".into()),
                created_date: Utc::now().timestamp_millis(),
                secret_data: hash_password(new_password)?,
                credential_data: String::new(),
                temporary: false,
            })
            .await?;
        if user.required_actions.iter().any(|a| a == "UPDATE_PASSWORD") {
            user.required_actions.retain(|a| a != "UPDATE_PASSWORD");
            user = state.store.update_user(user).await?;
        }
        if form.contains_key("logout-sessions") {
            sign_out_other_sessions(&state, &mount, &realm, &user.id, &headers).await?;
        }
        let mut auth_session = auth_session;
        if auth_session.kc_action.as_deref() == Some("UPDATE_PASSWORD") {
            auth_session.kc_action_done = true;
        }
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::CompleteUpdatePasswordOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    if let LoginState::TermsAndConditions { ref user_id, .. } = current_state {
        let mut user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        user.required_actions
            .retain(|a| a != "TERMS_AND_CONDITIONS");
        let user = state.store.update_user(user).await?;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::AcceptTermsOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    if let LoginState::Consent { ref user_id, .. } = current_state {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let now_ms = Utc::now().timestamp_millis();
        state
            .store
            .upsert_consent(ConsentRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                org_id: user.org_id.clone(),
                user_id: user.id.clone(),
                client_id: auth_session.client_id.clone(),
                granted_client_scopes: auth_session
                    .scope
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                created_date: now_ms,
                last_updated_date: now_ms,
            })
            .await?;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::GrantConsent, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // K7: Recovery code verification (single-use, constant-time, brute-force counted like OTP)
    if let LoginState::RecoveryCode { ref user_id, .. } = current_state {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        if bf
            .is_locked(&realm, Some(&user.id), client_ip.as_deref())
            .await?
        {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }
        let submitted = form
            .get("recovery_code")
            .map(String::as_str)
            .unwrap_or_default();
        let creds = state.store.list_credentials(&realm.id, &user.id).await?;
        let Some(rec_cred) = creds
            .iter()
            .find(|c| c.credential_type == "recovery-authn-codes")
        else {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        };
        let mut stored_hashes: Vec<String> =
            serde_json::from_str::<serde_json::Value>(&rec_cred.secret_data)
                .ok()
                .and_then(|v| v.get("hashes")?.as_array().cloned())
                .map(|arr| {
                    arr.into_iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
        let Some(idx) = verify_recovery_code(submitted, &stored_hashes) else {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        };
        // Single use even under concurrent submits: the first request to record the code's hash
        // wins; the stored list update below is only bookkeeping (K7).
        let used_hash = stored_hashes.remove(idx);
        if state
            .store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: sha256_hex(&format!("recovery:{}:{used_hash}", user.id)),
                realm_id: realm.id.clone(),
                expires_at: Utc::now() + Duration::days(3650),
            })
            .await
            .is_err()
        {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }
        let mut updated_cred = rec_cred.clone();
        updated_cred.secret_data = serde_json::json!({ "hashes": stored_hashes }).to_string();
        updated_cred.credential_data =
            serde_json::json!({ "remaining": stored_hashes.len() }).to_string();
        state.store.upsert_credential(updated_cred).await?;

        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::VerifyRecoveryCodeOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // K7: VerifyEmail step completion (either after action-link consumption or by verification code)
    if let LoginState::VerifyEmail { ref user_id, .. } = current_state {
        let mut user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let submitted_code = form
            .get("email_code")
            .map(String::as_str)
            .unwrap_or_default()
            .trim();
        let code_matches = auth_session
            .pending_email_code_hash
            .as_deref()
            .is_some_and(|expected| constant_time_str_eq(&sha256_hex(submitted_code), expected));
        if !user.email_verified && !code_matches {
            let _ = bf
                .record_failure(&realm, Some(&user.id), client_ip.as_deref())
                .await;
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                bundle.get("invalidUserMessage"),
            )
            .await;
        }
        user.email_verified = true;
        user.required_actions.retain(|a| a != "VERIFY_EMAIL");
        let user = state.store.update_user(user).await?;
        let mut auth_session = auth_session;
        auth_session.pending_email_code_hash = None;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::CompleteVerifyEmailOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // K11: RegisterWebAuthn (`webauthn-register` / `CONFIGURE_PASSKEY`) required action step
    if let LoginState::RegisterWebAuthn { ref user_id, .. } = current_state {
        let mut user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let Some(challenge) = auth_session.webauthn_challenge.clone() else {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                "",
            )
            .await;
        };
        let raw_resp = form
            .get("webauthn_register_response")
            .map(String::as_str)
            .unwrap_or_default();
        let resp_json: serde_json::Value = serde_json::from_str(raw_resp)
            .map_err(|_| KaisatsuError::BadRequest("invalid WebAuthn registration JSON".into()))?;
        let mut auth_session = auth_session;
        auth_session.webauthn_challenge = None;
        state.store.put_auth_session(auth_session.clone()).await?;
        crate::passkey::complete_passkey_registration(
            &state.store,
            &mount,
            &realm,
            &mut user,
            &challenge,
            &resp_json,
            form.get("label").cloned(),
            false,
        )
        .await?;
        user.required_actions
            .retain(|a| a != "webauthn-register" && a != "CONFIGURE_PASSKEY");
        let user = state.store.update_user(user).await?;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::CompleteRegisterWebAuthnOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    // K11: SecondFactorWebAuthn step
    if let LoginState::SecondFactorWebAuthn { ref user_id, .. } = current_state {
        let Some(challenge) = auth_session.webauthn_challenge.clone() else {
            return render_state_page(
                &state,
                &mount,
                &realm,
                &auth_session,
                &current_state,
                &headers,
                "",
            )
            .await;
        };
        let raw_resp = form
            .get("webauthn_assertion_response")
            .map(String::as_str)
            .unwrap_or_default();
        let resp_json: serde_json::Value = serde_json::from_str(raw_resp)
            .map_err(|_| KaisatsuError::BadRequest("invalid WebAuthn assertion JSON".into()))?;
        let uid = user_id.clone();
        let mut auth_session = auth_session;
        auth_session.webauthn_challenge = None;
        state.store.put_auth_session(auth_session.clone()).await?;
        let (user, _) = crate::passkey::complete_passkey_authentication(
            &state.store,
            &mount,
            &realm,
            Some(&uid),
            &challenge,
            &resp_json,
            false,
        )
        .await?;
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::VerifyWebAuthn2faOk, &ctx)
            .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    if let LoginState::OrganizationChoice { ref user_id, .. } = current_state {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        let chosen = form.get("org_id").cloned().unwrap_or_default();
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(
            current_state,
            LoginInput::SelectOrganization { org_id: chosen },
            &ctx,
        )
        .map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
        return advance_or_render(
            &state,
            &mount,
            &realm,
            auth_session,
            next,
            &headers,
            &client_ip,
        )
        .await;
    }

    Err(KaisatsuError::BadRequest("unsupported login action".into()))
}

async fn advance_or_render(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    mut auth_session: AuthSession,
    next_state: LoginState,
    headers: &HeaderMap,
    client_ip: &ClientIp,
) -> Result<Response> {
    if let LoginState::Code {
        user_id,
        acr,
        amr,
        chosen_org,
    } = next_state
    {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, &user_id)
            .await?
            .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
        // Every required factor passed: the person's failure counter starts over (RI).
        BruteForceProtector::new(state.store.clone())
            .record_success(realm, &user.id)
            .await?;
        let now = Utc::now();
        let sso_id = uuid::Uuid::now_v7().to_string();
        let sso_handle = random_handle_256();
        let sso = SsoSession {
            id: sso_id.clone(),
            realm_id: realm.id.clone(),
            org_id: chosen_org.clone().or_else(|| user.org_id.clone()),
            handle_hash: sha256_hex(&sso_handle),
            user_id: user.id.clone(),
            username: user.username.clone(),
            ip_address: client_ip.for_audit(),
            user_agent: user_agent_of(headers),
            start: now.timestamp_millis(),
            last_access: now.timestamp_millis(),
            auth_time: now.timestamp(),
            acr: acr.clone(),
            amr: amr.clone(),
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
            org_id: chosen_org.or(user.org_id),
            client_id: auth_session.client_id.clone(),
            user_id: user.id.clone(),
            sso_session_id: sso_id.clone(),
            redirect_uri: auth_session.redirect_uri.clone(),
            scope: auth_session.scope.clone(),
            mapper_scopes: auth_session.mapper_scopes.clone(),
            code_challenge: auth_session.code_challenge.clone(),
            code_challenge_method: auth_session.code_challenge_method.clone(),
            nonce: auth_session.nonce.clone(),
            acr,
            amr,
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
                org_id: None,
                time: now.timestamp_millis(),
                event_type: "LOGIN".into(),
                client_id: Some(auth_session.client_id),
                user_id: Some(user.id),
                session_id: Some(sso_id),
                ip_address: client_ip.for_audit(),
                error: None,
                details: BTreeMap::new(),
            })
            .await?;

        let cookie_name = mount.cookie_name("sso");
        let cookie_path = mount.realm_cookie_path(&realm.realm);
        let ttl = realm.sso_session_max_lifespan.max(3600);
        let set_cookie = if mount.path_prefix.is_empty() {
            format!(
                "{cookie_name}={sso_handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}"
            )
        } else {
            format!(
                "{cookie_name}={sso_handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}"
            )
        };

        // The code goes back in the response mode the client asked for (query, fragment,
        // form_post), as on the SSO path.
        let mut pairs = vec![("code", raw_code.as_str())];
        if let Some(st) = &auth_session.state {
            pairs.push(("state", st.as_str()));
        }
        if auth_session.kc_action.is_some() && auth_session.kc_action_done {
            pairs.push(("kc_action_status", "success"));
        }
        let mut resp = kaisatsu_oidc::redirect_with_params(
            &auth_session.redirect_uri,
            &auth_session.response_mode,
            &pairs,
        );
        let cookie = header::HeaderValue::from_str(&set_cookie)
            .map_err(|_| KaisatsuError::Storage("invalid session cookie".into()))?;
        resp.headers_mut().append(header::SET_COOKIE, cookie);
        return Ok(resp);
    }

    auth_session.step_state_json = serde_json::to_string(&next_state).unwrap_or_default();
    state.store.put_auth_session(auth_session.clone()).await?;
    render_state_page(state, mount, realm, &auth_session, &next_state, headers, "").await
}

/// The sign-in session a reset page was opened from (its link carries the session code), when
/// this browser holds it.
async fn reset_session(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    query: &AuthPageQuery,
    headers: &HeaderMap,
) -> Result<Option<AuthSession>> {
    if query.session_code.is_empty() {
        return Ok(None);
    }
    Ok(state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
        .filter(|s| query.matches_client(&s.client_id))
        .filter(|s| auth_cookie_matches(headers, mount, s)))
}

async fn reset_credentials_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<AuthPageQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let mut session = reset_session(&state, &mount, &realm, &query, &headers).await?;
    if let (Some(s), Some(picked)) = (session.as_mut(), picked_locale(&realm, &query.kc_locale)) {
        s.ui_locales = Some(picked.to_owned());
        state.store.put_auth_session(s.clone()).await?;
    }
    let ui = session
        .as_ref()
        .and_then(|s| s.ui_locales.clone())
        .or_else(|| picked_locale(&realm, &query.kc_locale).map(str::to_owned));
    let bundle = Bundle::for_locale(page_locale(&realm, ui.as_deref(), &headers));
    let branding = load_branding(
        &state,
        &realm,
        session.as_ref().and_then(|s| s.org_id.as_deref()),
    )
    .await?;
    let app = match &session {
        Some(s) => app_name(&state, &realm, &s.client_id, &bundle).await?,
        None => String::new(),
    };
    let link_query = session
        .as_ref()
        .map(|s| format!("?session_code={}&client_id={}", s.id, s.client_id))
        .unwrap_or_default();
    let action = format!(
        "{}/realms/{}/login-actions/reset-credentials{link_query}",
        mount.path_prefix, realm.realm
    );
    let shell = page::Shell {
        bundle: &bundle,
        realm: &realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: &app,
        locale_base_url: &action,
    };
    let mut tpl = shell.page(
        "reset-credentials",
        bundle.get("emailForgotTitle"),
        &new_nonce(),
    );
    let mut username = text_field("username", username_label(&bundle, &realm), "off");
    username.autofocus = true;
    let (back_url, back_label) = match &session {
        Some(s) => (
            format!(
                "{}/realms/{}/login-actions/authenticate?session_code={}&client_id={}",
                mount.path_prefix, realm.realm, s.id, s.client_id
            ),
            bundle.get("backToLogin").to_owned(),
        ),
        None => (String::new(), String::new()),
    };
    tpl.form = Some(page::Form {
        id: "kc-reset-password-form".into(),
        action,
        hidden: session
            .as_ref()
            .map(|s| vec![("csrf_token".into(), s.csrf_token.clone())])
            .unwrap_or_default(),
        fields: vec![username],
        submit_id: "kc-form-buttons".into(),
        submit_label: bundle.get("doSubmit").to_owned(),
        secondary_id: "kc-back-to-login".into(),
        secondary_url: back_url,
        secondary_label: back_label,
        ..page::Form::default()
    });
    tpl.info.push(if realm.login_with_email_allowed {
        bundle.get("emailInstruction").to_owned()
    } else {
        bundle.get("emailInstructionUsername").to_owned()
    });
    Ok(render_with_security_headers(&tpl, None, None))
}

/// W3, K10: reset-password queues a signed single-use reset mail in the outbox when the user
/// exists and has an e-mail address, and performs dummy work otherwise so both paths return
/// the exact same page and timing band. Opened from a sign-in, it goes back to that sign-in
/// page with the "e-mail on its way" banner, as keycloak.v2 does.
async fn reset_credentials_post(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<AuthPageQuery>,
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
    let session = reset_session(&state, &mount, &realm, &query, &headers).await?;
    if let Some(s) = &session {
        let presented = form
            .get("csrf_token")
            .map(String::as_str)
            .unwrap_or_default();
        if !constant_time_str_eq(presented, &s.csrf_token) {
            return Err(KaisatsuError::Forbidden("invalid CSRF token".into()));
        }
    }
    let bundle = Bundle::for_locale(page_locale(
        &realm,
        session.as_ref().and_then(|s| s.ui_locales.as_deref()),
        &headers,
    ));

    let identifier = form
        .get("username")
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    if !identifier.is_empty() && realm.reset_password_allowed {
        if let Some(user) = state
            .store
            .get_user_by_username_or_email(&realm.id, None, identifier)
            .await?
            .filter(|u| u.enabled)
            && let Some(ref email) = user.email
        {
            let outbox = EmailOutboxRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                org_id: user.org_id.clone(),
                user_id: user.id.clone(),
                to_address: email.clone(),
                locale: bundle.locale.to_owned(),
                template_kind: "RESET_CREDENTIALS".into(),
                action_jti: uuid::Uuid::now_v7().to_string(),
                actions: vec!["UPDATE_PASSWORD".into()],
                client_id: session
                    .as_ref()
                    .map(|s| s.client_id.clone())
                    .or_else(|| form.get("client_id").cloned()),
                redirect_uri: None,
                lifespan_secs: crate::email::ACTION_TOKEN_TTL_SECS,
                issuer: mount.realm_issuer(&realm.realm),
                status: "pending".into(),
                attempts: 0,
                last_error: None,
                next_attempt_at: Utc::now(),
                created_at: Utc::now(),
                sent_at: None,
            };
            let _ = state.store.enqueue_outbox(outbox).await;
        } else {
            // Keep timing in the same band when the user does not exist.
            let _ = sha256_hex(identifier);
        }
    }

    if let Some(s) = session {
        let mut on_login = s.clone();
        on_login.step_state_json =
            serde_json::to_string(&LoginState::IdentityFirst).unwrap_or_default();
        on_login.login_hint = Some(identifier.to_owned()).filter(|i| !i.is_empty());
        state.store.put_auth_session(on_login.clone()).await?;
        return render_state_page_with(
            &state,
            &mount,
            &realm,
            &on_login,
            &LoginState::IdentityFirst,
            &headers,
            Some(page::Alert::success(bundle.get("emailSentMessage"))),
        )
        .await;
    }

    let branding = load_branding(&state, &realm, None).await?;
    let shell = page::Shell {
        bundle: &bundle,
        realm: &realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: "",
        locale_base_url: "",
    };
    let mut tpl = shell.page(
        "reset-credentials-sent",
        bundle.get("emailForgotTitle"),
        &new_nonce(),
    );
    tpl.alert = Some(page::Alert::success(bundle.get("emailSentMessage")));
    Ok(render_with_security_headers(&tpl, None, None))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionTokenQuery {
    #[serde(default)]
    key: String,
}

/// `GET /realms/{realm}/login-actions/action-token?key=...` (W7, K10):
/// Consumes a signed single-use action link (`EmailSender::consume_action_token`).
/// - If the actions only contain non-form actions (`VERIFY_EMAIL`, `TERMS_AND_CONDITIONS`),
///   applies them immediately and redirects to the validated `redirect_uri` or shows "account updated".
/// - If the actions include `UPDATE_PASSWORD` or `CONFIGURE_TOTP`, creates an `AuthSession` bound
///   to an auth cookie and renders the corresponding interactive step.
async fn action_token_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<ActionTokenQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let issuer = mount.realm_issuer(&realm.realm);
    let locale = page_locale(&realm, None, &headers);

    let Ok(claims) = crate::email::EmailSender::consume_action_token(
        &state.store,
        &realm.id,
        &issuer,
        &query.key,
    )
    .await
    else {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    };

    let mut user = state
        .store
        .get_user_by_id(&realm.id, None, &claims.sub)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::BadRequest("user not found".into()))?;

    // Validate redirect_uri against the client if both are present on the token.
    let mut validated_redirect: Option<String> = None;
    if let (Some(cid), Some(r_uri)) = (&claims.client_id, &claims.redirect_uri)
        && let Some(client) = state.store.get_client_by_client_id(&realm.id, cid).await?
        && kaisatsu_oidc::validate_redirect_uri(&client.redirect_uris, r_uri)
    {
        validated_redirect = Some(r_uri.clone());
    }

    let mut needs_password_form = false;
    let mut needs_totp_form = false;
    for act in &claims.actions {
        match act.as_str() {
            "VERIFY_EMAIL" => {
                user.email_verified = true;
                user.required_actions.retain(|a| a != "VERIFY_EMAIL");
            }
            "TERMS_AND_CONDITIONS" => {
                user.required_actions
                    .retain(|a| a != "TERMS_AND_CONDITIONS");
            }
            "UPDATE_PASSWORD" => {
                needs_password_form = true;
            }
            "CONFIGURE_TOTP" => {
                needs_totp_form = true;
            }
            _ => {}
        }
    }
    user = state.store.update_user(user).await?;

    if needs_password_form || needs_totp_form {
        let session_id = uuid::Uuid::now_v7().to_string();
        let handle = random_handle_256();
        let csrf = random_handle_256();
        let pending_totp = if needs_totp_form {
            Some(generate_totp_secret())
        } else {
            None
        };
        let auth_session = AuthSession {
            id: session_id.clone(),
            realm_id: realm.id.clone(),
            org_id: user.org_id.clone(),
            handle_hash: sha256_hex(&handle),
            client_id: claims.client_id.clone().unwrap_or_else(|| "account".into()),
            redirect_uri: validated_redirect.unwrap_or_default(),
            response_type: "action".into(),
            response_mode: "query".into(),
            scope: claims.actions.join(" "),
            mapper_scopes: Vec::new(),
            state: None,
            nonce: None,
            code_challenge: String::new(),
            code_challenge_method: String::new(),
            acr_requested: None,
            prompt: None,
            login_hint: None,
            ui_locales: Some(locale.to_owned()),
            dpop_jkt: None,
            csrf_token: csrf.clone(),
            step_state_json: serde_json::to_string(&LoginState::ResetPasswordSet {
                user_id: user.id.clone(),
            })
            .unwrap_or_default(),
            pending_totp_secret: pending_totp.clone(),
            webauthn_challenge: None,
            pending_email_code_hash: None,
            broker_state_json: None,
            kc_action: None,
            kc_action_done: false,
            user_id: Some(user.id.clone()),
            amr: Vec::new(),
            acr_achieved: String::new(),
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(15),
        };
        state.store.put_auth_session(auth_session.clone()).await?;

        let cookie_name = mount.cookie_name("auth");
        let cookie_path = mount.realm_cookie_path(&realm.realm);
        let set_cookie = if mount.path_prefix.is_empty() {
            format!("{cookie_name}={handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=900")
        } else {
            format!(
                "{cookie_name}={handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age=900"
            )
        };

        return render_action_form(
            &state,
            &mount,
            &realm,
            &auth_session,
            &headers,
            None,
            Some(set_cookie),
        )
        .await;
    }

    finish_action_token_response(
        &state,
        &mount,
        &realm,
        &headers,
        validated_redirect.as_deref(),
        claims.client_id.as_deref(),
    )
    .await
}

async fn action_token_post(
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
    let session_code = form
        .get("session_code")
        .map(String::as_str)
        .unwrap_or_default();
    let Some(auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, session_code)
        .await?
    else {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return render_expired_session_page(
            &state,
            &mount,
            &realm,
            &headers,
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let presented_csrf = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if !constant_time_str_eq(presented_csrf, &auth_session.csrf_token) {
        return Err(KaisatsuError::Forbidden("invalid CSRF token".into()));
    }

    let user_id = auth_session
        .user_id
        .as_deref()
        .ok_or_else(|| KaisatsuError::BadRequest("missing user in action session".into()))?;
    let mut user = state
        .store
        .get_user_by_id(&realm.id, None, user_id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;

    let bundle = Bundle::for_locale(page_locale(
        &realm,
        auth_session.ui_locales.as_deref(),
        &headers,
    ));

    // Every check runs before the first write: a wrong code must not leave the new password
    // stored, or the retry is refused as a reused password and the link can never finish.
    let totp_credential = if auth_session
        .scope
        .split_whitespace()
        .any(|a| a == "CONFIGURE_TOTP")
    {
        let secret = auth_session
            .pending_totp_secret
            .as_deref()
            .ok_or_else(|| KaisatsuError::BadRequest("missing pending TOTP secret".into()))?;
        let otp_code = form.get("otp").map(String::as_str).unwrap_or_default();
        let cred = CredentialRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            user_id: user.id.clone(),
            credential_type: "otp".into(),
            user_label: Some("Authenticator".into()),
            created_date: Utc::now().timestamp_millis(),
            secret_data: serde_json::json!({ "value": secret }).to_string(),
            credential_data: serde_json::json!({
                "subType": "totp",
                "digits": realm.otp_policy_digits,
                "period": realm.otp_policy_period,
                "algorithm": realm.otp_policy_algorithm,
                "secretEncoding": "BASE32",
            })
            .to_string(),
            temporary: false,
        };
        let now_secs = u64::try_from(Utc::now().timestamp()).unwrap_or_default();
        if verify_totp_code(
            &cred,
            otp_code,
            realm.otp_policy_look_ahead_window,
            now_secs,
        )?
        .is_none()
        {
            let error = Some(("otp", bundle.get("invalidTotpMessage").to_owned()));
            return render_action_form(
                &state,
                &mount,
                &realm,
                &auth_session,
                &headers,
                error,
                None,
            )
            .await;
        }
        Some(cred)
    } else {
        None
    };

    if auth_session
        .scope
        .split_whitespace()
        .any(|a| a == "UPDATE_PASSWORD")
    {
        let new_password = form
            .get("new_password")
            .map(String::as_str)
            .unwrap_or_default();
        let history: Vec<CredentialRecord> = state
            .store
            .list_credentials(&realm.id, &user.id)
            .await?
            .into_iter()
            .filter(|c| c.credential_type.starts_with("password"))
            .collect();
        if form
            .get("password_confirm")
            .is_some_and(|confirm| confirm != new_password)
        {
            let error = Some((
                "password_confirm",
                bundle.get("notMatchPasswordMessage").to_owned(),
            ));
            return render_action_form(
                &state,
                &mount,
                &realm,
                &auth_session,
                &headers,
                error,
                None,
            )
            .await;
        }
        if let Some(violation) = first_policy_violation(
            &realm.password_policy,
            new_password,
            &user.username,
            user.email.as_deref(),
            &history,
        ) {
            let error = Some(("new_password", policy_message(&bundle, &violation)));
            return render_action_form(
                &state,
                &mount,
                &realm,
                &auth_session,
                &headers,
                error,
                None,
            )
            .await;
        }
        state
            .store
            .upsert_credential(CredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                user_id: user.id.clone(),
                credential_type: "password".into(),
                user_label: Some("Password".into()),
                created_date: Utc::now().timestamp_millis(),
                secret_data: hash_password(new_password)?,
                credential_data: String::new(),
                temporary: false,
            })
            .await?;
        user.required_actions.retain(|a| a != "UPDATE_PASSWORD");
    }

    if let Some(cred) = totp_credential {
        state.store.upsert_credential(cred).await?;
        user.required_actions.retain(|a| a != "CONFIGURE_TOTP");
    }

    state.store.update_user(user).await?;
    state
        .store
        .delete_auth_session(&realm.id, &auth_session.id)
        .await?;

    let redirect =
        (!auth_session.redirect_uri.is_empty()).then_some(auth_session.redirect_uri.as_str());
    let client = (!auth_session.client_id.is_empty()).then_some(auth_session.client_id.as_str());
    finish_action_token_response(&state, &mount, &realm, &headers, redirect, client).await
}

/// After an action link: back to the app's redirect URI, or keycloak.v2's info page "Your
/// account is updated." with the way back to the app when its client is known.
async fn finish_action_token_response(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    headers: &HeaderMap,
    redirect_uri: Option<&str>,
    client_id: Option<&str>,
) -> Result<Response> {
    if let Some(uri) = redirect_uri {
        return Ok((
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, uri.to_owned()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            "",
        )
            .into_response());
    }
    let bundle = Bundle::for_locale(page_locale(realm, None, headers));
    let branding = load_branding(state, realm, None).await?;
    let client = match client_id {
        Some(id) => state.store.get_client_by_client_id(&realm.id, id).await?,
        None => None,
    };
    let app = match client_id {
        Some(id) => app_name(state, realm, id, &bundle).await?,
        None => String::new(),
    };
    let shell = page::Shell {
        bundle: &bundle,
        realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: &app,
        locale_base_url: "",
    };
    let mut tpl = shell.page(
        "account-updated",
        bundle.get("accountUpdatedMessage"),
        &new_nonce(),
    );
    tpl.instructions
        .push(bundle.get("accountUpdatedMessage").to_owned());
    if let Some(base) = client
        .and_then(|c| c.base_url)
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
    {
        tpl.links.push(page::Link {
            id: "backToApplication".into(),
            class: "pf-m-primary",
            url: base,
            label: if app.is_empty() {
                bundle.get("backToApplication").to_owned()
            } else {
                bundle.format("jcBackTo", &[&app])
            },
        });
    }
    Ok(render_with_security_headers(&tpl, None, None))
}
/// The form an e-mailed action link opens: "Choose a new password" (keycloak.v2's
/// login-update-password) and/or the authenticator set-up, as one step. `error` lands on the
/// field it is about, and the page answers 400.
#[allow(clippy::too_many_arguments)]
async fn render_action_form(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    auth_session: &AuthSession,
    headers: &HeaderMap,
    error: Option<(&str, String)>,
    set_cookie: Option<String>,
) -> Result<Response> {
    let bundle = Bundle::for_locale(page_locale(
        realm,
        auth_session.ui_locales.as_deref(),
        headers,
    ));
    let branding = load_branding(state, realm, auth_session.org_id.as_deref()).await?;
    let app = app_name(state, realm, &auth_session.client_id, &bundle).await?;
    let shell = page::Shell {
        bundle: &bundle,
        realm,
        branding: &branding,
        path_prefix: &mount.path_prefix,
        app: &app,
        locale_base_url: "",
    };
    let wants = |a: &str| auth_session.scope.split_whitespace().any(|x| x == a);
    let heading = if wants("UPDATE_PASSWORD") {
        bundle.get("updatePasswordTitle")
    } else {
        bundle.get("loginTotpTitle")
    };
    let mut tpl = shell.page("action-token-form", heading, &new_nonce());
    let mut fields = Vec::new();
    if wants("UPDATE_PASSWORD") {
        let mut new_password = password_field(
            "new_password",
            bundle.get("passwordNew").to_owned(),
            "new-password",
        );
        new_password.autofocus = true;
        fields.push(new_password);
        fields.push(password_field(
            "password_confirm",
            bundle.get("passwordConfirm").to_owned(),
            "new-password",
        ));
    }
    if let (true, Some(secret)) = (
        wants("CONFIGURE_TOTP"),
        auth_session.pending_totp_secret.as_deref(),
    ) {
        let account = match &auth_session.user_id {
            Some(id) => state
                .store
                .get_user_by_id(&realm.id, None, id)
                .await?
                .map(|u| u.username)
                .unwrap_or_default(),
            None => String::new(),
        };
        tpl.totp = Some(totp_setup(&bundle, realm, &shell.org(), &account, secret));
        let mut otp = text_field(
            "otp",
            bundle.get("loginOtpOneTime").to_owned(),
            "one-time-code",
        );
        otp.required = true;
        fields.push(otp);
    }
    let refused = error.is_some();
    if let Some((name, text)) = error
        && let Some(field) = fields.iter_mut().find(|f| f.name == name)
    {
        field.error = text;
    }
    tpl.form = Some(page::Form {
        id: "kc-passwd-update-form".into(),
        action: format!(
            "{}/realms/{}/login-actions/action-token",
            mount.path_prefix, realm.realm
        ),
        hidden: vec![
            ("csrf_token".into(), auth_session.csrf_token.clone()),
            ("session_code".into(), auth_session.id.clone()),
        ],
        fields,
        submit_id: "kc-submit".into(),
        submit_label: bundle.get("doSubmit").to_owned(),
        ..page::Form::default()
    });
    let mut resp = render_with_security_headers(&tpl, set_cookie, Some(&auth_session.redirect_uri));
    if refused {
        *resp.status_mut() = StatusCode::BAD_REQUEST;
    }
    Ok(resp)
}

/// The message key and argument of the first password-policy rule a password breaks, in the
/// words keycloak.v2 used (`invalidPasswordMinLengthMessage`, ...).
fn policy_message(bundle: &Bundle, violation: &PolicyViolation) -> String {
    match violation {
        PolicyViolation::MinLength(n) => {
            bundle.format("invalidPasswordMinLengthMessage", &[&n.to_string()])
        }
        PolicyViolation::UpperCase(n) => {
            bundle.format("invalidPasswordMinUpperCaseCharsMessage", &[&n.to_string()])
        }
        PolicyViolation::LowerCase(n) => {
            bundle.format("invalidPasswordMinLowerCaseCharsMessage", &[&n.to_string()])
        }
        PolicyViolation::Digits(n) => {
            bundle.format("invalidPasswordMinDigitsMessage", &[&n.to_string()])
        }
        PolicyViolation::SpecialChars(n) => {
            bundle.format("invalidPasswordMinSpecialCharsMessage", &[&n.to_string()])
        }
        PolicyViolation::NotUsername => bundle.get("invalidPasswordNotUsernameMessage").to_owned(),
        PolicyViolation::NotEmail => bundle.get("invalidPasswordNotEmailMessage").to_owned(),
        PolicyViolation::History(n) => {
            bundle.format("invalidPasswordHistoryMessage", &[&n.to_string()])
        }
    }
}
