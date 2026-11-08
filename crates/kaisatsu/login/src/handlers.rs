#![forbid(unsafe_code)]
//! Server-rendered Askama login handlers, CSRF check bound to `auth_session`, per-response CSP
//! nonce, `frame-ancestors 'none'`, timing-equal unknown-user handling, and SSO cookie issuance (W7, W9, W12).

use crate::brute_force::BruteForceProtector;
use crate::i18n::{Bundle, negotiate_locale};
use crate::machine::{LoginInput, LoginPolicyContext, LoginState, transition};
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Form, Router};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_oidc::{ClientIp, OidcState};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub input_type: String,
    pub value: String,
    pub autocomplete: String,
    pub hint: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "login_page.html")]
struct LoginPageTemplate {
    lang: String,
    page_title: String,
    org_display: String,
    app_context: String,
    heading_text: String,
    page_id: String,
    csp_nonce: String,
    branding_css: String,
    alert_message: String,
    instruction_text: String,
    show_form: bool,
    form_action: String,
    csrf_token: String,
    session_code: String,
    fields: Vec<FormField>,
    submit_label: String,
    secondary_link_url: String,
    secondary_link_label: String,
    /// Inline SVG QR code of the `otpauth://` URI on the authenticator set-up page (our own
    /// markup, never user input), else empty.
    qr_svg: String,
    qr_hint: String,
}

pub fn login_router(state: OidcState) -> Router {
    Router::new()
        .route(
            "/realms/{realm}/login-actions/authenticate",
            get(authenticate_page_get).post(authenticate_page_post),
        )
        .route(
            "/realms/{realm}/login-actions/reset-credentials",
            get(reset_credentials_get).post(reset_credentials_post),
        )
        .route(
            "/realms/{realm}/login-actions/action-token",
            get(action_token_get).post(action_token_post),
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
}

impl AuthPageQuery {
    /// A `client_id` in the link, when present, must name the client the session was opened for.
    fn matches_client(&self, session_client_id: &str) -> bool {
        self.client_id.is_empty() || self.client_id == session_client_id
    }
}

fn render_with_security_headers(tpl: &LoginPageTemplate, extra_cookie: Option<String>) -> Response {
    let html = tpl.render().unwrap_or_default();
    let csp = format!(
        "default-src 'self'; script-src 'nonce-{}'; style-src 'self' 'nonce-{}'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'",
        tpl.csp_nonce, tpl.csp_nonce
    );
    let mut resp = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::X_FRAME_OPTIONS, "DENY".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate".to_owned(),
            ),
        ],
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
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(
        auth_session.ui_locales.as_deref(),
        None,
        accept_lang,
        &realm.default_locale,
    );
    let bundle = Bundle::for_locale(locale);
    let branding = state
        .store
        .get_branding(&realm.id, auth_session.org_id.as_deref())
        .await?
        .unwrap_or_else(|| BrandingBlock {
            organisation: realm.display_name.clone(),
            instance_name: realm.display_name.clone(),
            ..Default::default()
        })
        .sanitised();

    let client = state
        .store
        .get_client_by_client_id(&realm.id, &auth_session.client_id)
        .await?
        .unwrap_or_default();
    let app_name = client
        .name
        .as_deref()
        .map(|n| bundle.translate_client_name(n))
        .unwrap_or_else(|| client.client_id.clone());
    let org_display = if realm.display_name.is_empty() {
        bundle.get("jcOrganizationFallback").to_owned()
    } else {
        realm.display_name.clone()
    };

    let nonce = random_handle_256()[..16].to_owned();
    let form_action = format!(
        "{}/realms/{}/login-actions/authenticate?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );
    let reset_url = format!(
        "{}/realms/{}/login-actions/reset-credentials?session_code={}&client_id={}",
        mount.path_prefix, realm.realm, auth_session.id, auth_session.client_id
    );

    let mut enrol_secret: Option<String> = None;
    let (page_id, heading_text, instruction_text, fields, submit_label, sec_url, sec_label) =
        match login_state {
            LoginState::IdentityFirst | LoginState::Password { .. } => {
                let h = bundle.format("jcSignInToApp", &[&app_name]);
                let f = vec![
                    FormField {
                        name: "username".into(),
                        label: bundle.get("usernameOrEmailLabel").to_owned(),
                        input_type: "text".into(),
                        value: auth_session.login_hint.clone().unwrap_or_default(),
                        autocomplete: "username webauthn".into(),
                        hint: bundle.get("usernameOrEmailHint").to_owned(),
                        error: String::new(),
                    },
                    FormField {
                        name: "password".into(),
                        label: bundle.get("passwordLabel").to_owned(),
                        input_type: "password".into(),
                        value: String::new(),
                        autocomplete: "current-password".into(),
                        hint: bundle.get("passwordHint").to_owned(),
                        error: String::new(),
                    },
                ];
                let reauth = auth_session.kc_action.is_some() && !auth_session.kc_action_done;
                (
                    "login".to_owned(),
                    h,
                    if reauth {
                        bundle.get("reauthenticate").to_owned()
                    } else {
                        String::new()
                    },
                    f,
                    bundle.get("doLogIn").to_owned(),
                    if realm.reset_password_allowed {
                        reset_url
                    } else {
                        String::new()
                    },
                    bundle.get("doForgotPassword").to_owned(),
                )
            }
            LoginState::SecondFactorOtp { .. } | LoginState::EnrolTotp { .. } => {
                let is_enrol = matches!(login_state, LoginState::EnrolTotp { .. });
                let title = if is_enrol {
                    bundle.get("otpEnrolTitle").to_owned()
                } else {
                    bundle.get("loginAccountTitle").to_owned()
                };
                let instruction = if is_enrol {
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
                    enrol_secret = Some(secret.clone());
                    let grouped = secret
                        .as_bytes()
                        .chunks(4)
                        .map(|c| String::from_utf8_lossy(c))
                        .collect::<Vec<_>>()
                        .join(" ");
                    bundle.format(
                        "otpEnrolInstruction",
                        &[
                            &grouped,
                            &realm.otp_policy_algorithm,
                            &realm.otp_policy_digits.to_string(),
                            &realm.otp_policy_period.to_string(),
                        ],
                    )
                } else {
                    String::new()
                };
                let mut f = vec![FormField {
                    name: "otp".into(),
                    label: bundle.get("otpLabel").to_owned(),
                    input_type: "text".into(),
                    value: String::new(),
                    autocomplete: "one-time-code".into(),
                    hint: bundle.get("otpHint").to_owned(),
                    error: String::new(),
                }];
                if is_enrol {
                    f.push(FormField {
                        name: "userLabel".into(),
                        label: bundle.get("otpDeviceLabel").to_owned(),
                        input_type: "text".into(),
                        value: String::new(),
                        autocomplete: "off".into(),
                        hint: bundle.get("otpDeviceHint").to_owned(),
                        error: String::new(),
                    });
                    f.push(logout_sessions_field(&bundle, false));
                }
                let (cancel_url, cancel_label) = cancel_link(auth_session, &bundle, is_enrol);
                (
                    if is_enrol { "config-totp" } else { "login-otp" }.to_owned(),
                    title,
                    instruction,
                    f,
                    if is_enrol {
                        bundle.get("doSubmit").to_owned()
                    } else {
                        bundle.get("doLogIn").to_owned()
                    },
                    cancel_url,
                    cancel_label,
                )
            }
            LoginState::RecoveryCode { .. } => {
                let f = vec![FormField {
                    name: "recovery_code".into(),
                    label: bundle.get("recoveryCodeLabel").to_owned(),
                    input_type: "text".into(),
                    value: String::new(),
                    autocomplete: "off".into(),
                    hint: bundle.get("recoveryCodeHint").to_owned(),
                    error: String::new(),
                }];
                (
                    "recovery-code".to_owned(),
                    bundle.get("recoveryCodeLabel").to_owned(),
                    String::new(),
                    f,
                    bundle.get("doLogIn").to_owned(),
                    String::new(),
                    String::new(),
                )
            }
            LoginState::UpdatePassword { .. } => {
                let f = vec![
                    FormField {
                        name: "new_password".into(),
                        label: bundle.get("passwordNewLabel").to_owned(),
                        input_type: "password".into(),
                        value: String::new(),
                        autocomplete: "new-password".into(),
                        hint: bundle.get("passwordHint").to_owned(),
                        error: String::new(),
                    },
                    FormField {
                        name: "password_confirm".into(),
                        label: bundle.get("passwordConfirmLabel").to_owned(),
                        input_type: "password".into(),
                        value: String::new(),
                        autocomplete: "new-password".into(),
                        hint: bundle.get("passwordConfirmHint").to_owned(),
                        error: String::new(),
                    },
                    logout_sessions_field(&bundle, true),
                ];
                let (cancel_url, cancel_label) = cancel_link(auth_session, &bundle, true);
                (
                    "update-password".to_owned(),
                    bundle.get("updatePasswordTitle").to_owned(),
                    String::new(),
                    f,
                    bundle.get("doSubmit").to_owned(),
                    cancel_url,
                    cancel_label,
                )
            }
            LoginState::OrganizationChoice { candidates, .. } => {
                let f = vec![FormField {
                    name: "org_id".into(),
                    label: bundle.get("orgChoiceLabel").to_owned(),
                    input_type: "text".into(),
                    value: candidates.first().cloned().unwrap_or_default(),
                    autocomplete: "organization".into(),
                    hint: bundle.get("orgChoiceHint").to_owned(),
                    error: String::new(),
                }];
                (
                    "org-choice".to_owned(),
                    bundle.get("orgChoiceTitle").to_owned(),
                    String::new(),
                    f,
                    bundle.get("proceedWithAction").to_owned(),
                    String::new(),
                    String::new(),
                )
            }
            LoginState::TermsAndConditions { .. } => (
                "terms".to_owned(),
                bundle.get("termsTitle").to_owned(),
                bundle.get("termsInstruction").to_owned(),
                Vec::new(),
                bundle.get("proceedWithAction").to_owned(),
                String::new(),
                String::new(),
            ),
            LoginState::VerifyEmail { user_id, .. } => {
                // K7 / K9: Ensure a verification code and outbox email are queued once for this session.
                if auth_session.pending_email_code_hash.is_none() {
                    // The mail carries only the signed action link (K9 stores no secret), so the
                    // step completes once that link set `email_verified`; this random value only
                    // marks the mail as queued and can never be typed (it was a fixed "654321").
                    let code = random_handle_256();
                    let mut updated = auth_session.clone();
                    updated.pending_email_code_hash = Some(sha256_hex(&code));
                    state.store.put_auth_session(updated).await?;
                    if let Some(user) = state.store.get_user_by_id(&realm.id, None, user_id).await?
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
                let f = vec![FormField {
                    name: "email_code".into(),
                    label: bundle.get("emailVerifyTitle").to_owned(),
                    input_type: "text".into(),
                    value: String::new(),
                    autocomplete: "one-time-code".into(),
                    hint: bundle.get("emailSentMessage").to_owned(),
                    error: String::new(),
                }];
                (
                    "verify-email".to_owned(),
                    bundle.get("emailVerifyTitle").to_owned(),
                    bundle.get("emailSentMessage").to_owned(),
                    f,
                    bundle.get("proceedWithAction").to_owned(),
                    String::new(),
                    String::new(),
                )
            }
            LoginState::Passkey { .. }
            | LoginState::SecondFactorWebAuthn { .. }
            | LoginState::RegisterWebAuthn { .. } => {
                let is_register = matches!(login_state, LoginState::RegisterWebAuthn { .. });
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
                let field_name = if is_register {
                    "webauthn_register_response"
                } else {
                    "webauthn_assertion_response"
                };
                let page = if is_register {
                    "webauthn-register"
                } else {
                    "webauthn-authenticate"
                };
                let f = vec![FormField {
                    name: field_name.into(),
                    label: bundle.get("loginAccountTitle").to_owned(),
                    input_type: "hidden".into(),
                    value: String::new(),
                    autocomplete: "off".into(),
                    hint: challenge,
                    error: String::new(),
                }];
                (
                    page.to_owned(),
                    bundle.get("loginAccountTitle").to_owned(),
                    String::new(),
                    f,
                    bundle.get("proceedWithAction").to_owned(),
                    String::new(),
                    String::new(),
                )
            }
            LoginState::Consent { .. } => (
                "consent".to_owned(),
                bundle.get("consentTitle").to_owned(),
                bundle.get("consentInstruction").to_owned(),
                Vec::new(),
                bundle.get("proceedWithAction").to_owned(),
                String::new(),
                String::new(),
            ),
            _ => (
                "info".to_owned(),
                bundle.get("loginAccountTitle").to_owned(),
                String::new(),
                Vec::new(),
                bundle.get("proceedWithAction").to_owned(),
                String::new(),
                String::new(),
            ),
        };

    // On the authenticator and password pages the error belongs to the field it is about, as
    // Keycloak's templates show it; other pages keep the banner.
    let mut fields = fields;
    let mut alert_message = alert_message;
    let error_field = match page_id.as_str() {
        "config-totp" => Some("otp"),
        "update-password" if alert_message == bundle.get("notMatchPasswordMessage") => {
            Some("password_confirm")
        }
        "update-password" => Some("new_password"),
        _ => None,
    };
    if let Some(field) = fields
        .iter_mut()
        .find(|f| !alert_message.is_empty() && Some(f.name.as_str()) == error_field)
    {
        field.error = alert_message.to_owned();
        alert_message = "";
    }
    let qr_svg = match (&enrol_secret, login_state) {
        (Some(secret), LoginState::EnrolTotp { user_id, .. }) => {
            let account = state
                .store
                .get_user_by_id(&realm.id, None, user_id)
                .await?
                .map(|u| u.username)
                .unwrap_or_default();
            totp_qr_svg(
                realm,
                &org_display,
                &account,
                secret,
                bundle.get("otpScanHint"),
            )
        }
        _ => String::new(),
    };
    let tpl = LoginPageTemplate {
        lang: bundle.locale.to_owned(),
        page_title: bundle.format("jcSignInTo", &[&app_name, &org_display]),
        org_display,
        app_context: app_name,
        heading_text,
        page_id,
        csp_nonce: nonce,
        branding_css: branding.css_variables(),
        alert_message: alert_message.to_owned(),
        instruction_text,
        show_form: true,
        form_action,
        csrf_token: auth_session.csrf_token.clone(),
        session_code: auth_session.id.clone(),
        fields,
        submit_label,
        secondary_link_url: sec_url,
        secondary_link_label: sec_label,
        qr_svg,
        qr_hint: format!(
            "{} {}",
            bundle.get("otpScanHint"),
            bundle.get("otpAppsHint")
        ),
    };

    Ok(render_with_security_headers(&tpl, None))
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
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" width=\"180\" height=\"180\" role=\"img\" aria-labelledby=\"qr-title\" shape-rendering=\"crispEdges\"><title id=\"qr-title\">{title}</title><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    )
}

/// The label a person gives a new authenticator: trimmed, at most 64 characters, control
/// characters dropped; "Authenticator" when left empty.
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
fn logout_sessions_field(bundle: &Bundle, checked: bool) -> FormField {
    FormField {
        name: "logout-sessions".into(),
        label: bundle.get("logoutOtherSessions").to_owned(),
        input_type: "checkbox".into(),
        value: if checked { "checked" } else { "" }.into(),
        autocomplete: "off".into(),
        hint: bundle.get("logoutOtherSessionsHint").to_owned(),
        error: String::new(),
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

pub(crate) fn render_expired_session_page(realm: &RealmConfig, status: StatusCode) -> Response {
    let bundle = Bundle::for_locale(&realm.default_locale);
    let tpl = LoginPageTemplate {
        lang: bundle.locale.to_owned(),
        page_title: bundle.get("pageExpiredTitle").to_owned(),
        org_display: realm.display_name.clone(),
        app_context: String::new(),
        heading_text: bundle.get("pageExpiredTitle").to_owned(),
        page_id: "expired".into(),
        csp_nonce: random_handle_256()[..16].to_owned(),
        branding_css: BrandingBlock::default().css_variables(),
        alert_message: String::new(),
        instruction_text: bundle.get("jcPageExpiredExplained").to_owned(),
        show_form: false,
        form_action: String::new(),
        csrf_token: String::new(),
        session_code: String::new(),
        fields: Vec::new(),
        submit_label: String::new(),
        secondary_link_url: String::new(),
        secondary_link_label: String::new(),
        qr_svg: String::new(),
        qr_hint: String::new(),
    };
    let mut resp = render_with_security_headers(&tpl, None);
    *resp.status_mut() = status;
    resp
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
    let Some(auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
        .filter(|s| query.matches_client(&s.client_id))
    else {
        return Ok(render_expired_session_page(&realm, StatusCode::OK));
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
    };

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
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
    }

    // Verify CSRF token bound to `auth_session` (W7)
    let presented_csrf = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if !constant_time_str_eq(presented_csrf, &auth_session.csrf_token) {
        return Err(KaisatsuError::Forbidden("invalid login CSRF token".into()));
    }

    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(
        auth_session.ui_locales.as_deref(),
        None,
        accept_lang,
        &realm.default_locale,
    );
    let bundle = Bundle::for_locale(locale);
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
                bundle.get("invalidUserMessage"),
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
                bundle.get("invalidUserMessage"),
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
        if let Err(e) = check_password_policy(
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
                &e.public_detail(),
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

async fn reset_credentials_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(None, None, accept_lang, &realm.default_locale);
    let bundle = Bundle::for_locale(locale);

    let tpl = LoginPageTemplate {
        lang: bundle.locale.to_owned(),
        page_title: bundle.get("emailForgotTitle").to_owned(),
        org_display: realm.display_name,
        app_context: String::new(),
        heading_text: bundle.get("emailForgotTitle").to_owned(),
        page_id: "reset-credentials".into(),
        csp_nonce: random_handle_256()[..16].to_owned(),
        branding_css: BrandingBlock::default().css_variables(),
        alert_message: String::new(),
        instruction_text: bundle.get("emailInstruction").to_owned(),
        show_form: true,
        form_action: format!(
            "{}/realms/{}/login-actions/reset-credentials",
            mount.path_prefix, realm.realm
        ),
        csrf_token: String::new(),
        session_code: String::new(),
        fields: vec![FormField {
            name: "username".into(),
            label: bundle.get("usernameOrEmailLabel").to_owned(),
            input_type: "text".into(),
            value: String::new(),
            autocomplete: "username".into(),
            hint: bundle.get("usernameOrEmailHint").to_owned(),
            error: String::new(),
        }],
        submit_label: bundle.get("proceedWithAction").to_owned(),
        secondary_link_url: String::new(),
        secondary_link_label: String::new(),
        qr_svg: String::new(),
        qr_hint: String::new(),
    };
    Ok(render_with_security_headers(&tpl, None))
}

/// W3, K10: reset-password queues a signed single-use reset mail in the outbox when the user
/// exists and has an e-mail address, and performs dummy work otherwise so both paths return
/// the exact same page and timing band.
async fn reset_credentials_post(
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
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(None, None, accept_lang, &realm.default_locale);
    let bundle = Bundle::for_locale(locale);

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
                client_id: form.get("client_id").cloned(),
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

    let tpl = LoginPageTemplate {
        lang: bundle.locale.to_owned(),
        page_title: bundle.get("emailForgotTitle").to_owned(),
        org_display: realm.display_name,
        app_context: String::new(),
        heading_text: bundle.get("emailForgotTitle").to_owned(),
        page_id: "reset-credentials-sent".into(),
        csp_nonce: random_handle_256()[..16].to_owned(),
        branding_css: BrandingBlock::default().css_variables(),
        alert_message: String::new(),
        instruction_text: bundle.get("emailSentMessage").to_owned(),
        show_form: false,
        form_action: String::new(),
        csrf_token: String::new(),
        session_code: String::new(),
        fields: Vec::new(),
        submit_label: String::new(),
        secondary_link_url: String::new(),
        secondary_link_label: String::new(),
        qr_svg: String::new(),
        qr_hint: String::new(),
    };
    Ok(render_with_security_headers(&tpl, None))
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
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = negotiate_locale(None, None, accept_lang, &realm.default_locale);
    let bundle = Bundle::for_locale(locale);

    let Ok(claims) = crate::email::EmailSender::consume_action_token(
        &state.store,
        &realm.id,
        &issuer,
        &query.key,
    )
    .await
    else {
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
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
        state.store.put_auth_session(auth_session).await?;

        let cookie_name = mount.cookie_name("auth");
        let cookie_path = mount.realm_cookie_path(&realm.realm);
        let set_cookie = if mount.path_prefix.is_empty() {
            format!("{cookie_name}={handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=900")
        } else {
            format!(
                "{cookie_name}={handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age=900"
            )
        };

        let mut fields = Vec::new();
        if needs_password_form {
            fields.push(FormField {
                name: "new_password".into(),
                label: bundle.get("passwordLabel").to_owned(),
                input_type: "password".into(),
                value: String::new(),
                autocomplete: "new-password".into(),
                hint: bundle.get("passwordHint").to_owned(),
                error: String::new(),
            });
        }
        if let Some(ref secret) = pending_totp {
            fields.push(FormField {
                name: "otp".into(),
                label: bundle.get("otpLabel").to_owned(),
                input_type: "text".into(),
                value: String::new(),
                autocomplete: "one-time-code".into(),
                hint: bundle.format(
                    "otpEnrolInstruction",
                    &[
                        secret,
                        &realm.otp_policy_algorithm,
                        &realm.otp_policy_digits.to_string(),
                        &realm.otp_policy_period.to_string(),
                    ],
                ),
                error: String::new(),
            });
        }
        let tpl = LoginPageTemplate {
            lang: bundle.locale.to_owned(),
            page_title: bundle.get("updatePasswordTitle").to_owned(),
            org_display: realm.display_name,
            app_context: String::new(),
            heading_text: bundle.get("updatePasswordTitle").to_owned(),
            page_id: "action-token-form".into(),
            csp_nonce: random_handle_256()[..16].to_owned(),
            branding_css: BrandingBlock::default().css_variables(),
            alert_message: String::new(),
            instruction_text: String::new(),
            show_form: true,
            form_action: format!(
                "{}/realms/{}/login-actions/action-token",
                mount.path_prefix, realm.realm
            ),
            csrf_token: csrf,
            session_code: session_id,
            fields,
            submit_label: bundle.get("proceedWithAction").to_owned(),
            secondary_link_url: String::new(),
            secondary_link_label: String::new(),
            qr_svg: String::new(),
            qr_hint: String::new(),
        };
        return Ok(render_with_security_headers(&tpl, Some(set_cookie)));
    }

    finish_action_token_response(&realm, &bundle, validated_redirect.as_deref())
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
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
    };
    if !auth_cookie_matches(&headers, &mount, &auth_session) {
        return Ok(render_expired_session_page(&realm, StatusCode::BAD_REQUEST));
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

    let bundle = Bundle::for_locale(
        auth_session
            .ui_locales
            .as_deref()
            .unwrap_or(&realm.default_locale),
    );

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
            return Err(KaisatsuError::BadRequest("invalid TOTP code".into()));
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
        check_password_policy(
            &realm.password_policy,
            new_password,
            &user.username,
            user.email.as_deref(),
            &history,
        )?;
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
    finish_action_token_response(&realm, &bundle, redirect)
}

fn finish_action_token_response(
    realm: &RealmConfig,
    bundle: &Bundle,
    redirect_uri: Option<&str>,
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
    let tpl = LoginPageTemplate {
        lang: bundle.locale.to_owned(),
        page_title: bundle.get("accountUpdatedMessage").to_owned(),
        org_display: realm.display_name.clone(),
        app_context: String::new(),
        heading_text: bundle.get("accountUpdatedMessage").to_owned(),
        page_id: "account-updated".into(),
        csp_nonce: random_handle_256()[..16].to_owned(),
        branding_css: BrandingBlock::default().css_variables(),
        alert_message: String::new(),
        instruction_text: bundle.get("accountUpdatedMessage").to_owned(),
        show_form: false,
        form_action: String::new(),
        csrf_token: String::new(),
        session_code: String::new(),
        fields: Vec::new(),
        submit_label: String::new(),
        secondary_link_url: String::new(),
        secondary_link_label: String::new(),
        qr_svg: String::new(),
        qr_hint: String::new(),
    };
    Ok(render_with_security_headers(&tpl, None))
}
