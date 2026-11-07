#![forbid(unsafe_code)]
//! Server-rendered Askama login handlers, CSRF check bound to `auth_session`, per-response CSP
//! nonce, `frame-ancestors 'none'`, timing-equal unknown-user handling, and SSO cookie issuance (W7, W9, W12).

use crate::brute_force::BruteForceProtector;
use crate::i18n::{negotiate_locale, Bundle};
use crate::machine::{transition, LoginInput, LoginPolicyContext, LoginState};
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use chrono::{Duration, Utc};
use jc_router::Mount;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use wardkeep_core::*;
use wardkeep_crypto::*;
use wardkeep_oidc::OidcState;

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
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AuthPageQuery {
    #[serde(default)]
    session_code: String,
    #[serde(default)]
    client_id: String,
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
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate".to_owned()),
        ],
        html,
    )
        .into_response();
    if let Some(ck) = extra_cookie {
        if let Ok(val) = header::HeaderValue::from_str(&ck) {
            resp.headers_mut().append(header::SET_COOKIE, val);
        }
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
    let all_roles = store.list_roles(&realm.id, None).await?;
    let group_roles: Vec<String> = user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let eff_roles = wardkeep_core::claims::resolve_effective_realm_roles(
        &user.realm_roles,
        &group_roles,
        &all_roles,
    );
    let mfa_role = eff_roles.iter().any(|r| r == "mfa-required");
    let step_up = auth_session.acr_requested.as_deref() == Some("mfa");
    let is_master = realm.realm == "master";

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

    let client = store
        .get_client_by_client_id(&realm.id, &auth_session.client_id)
        .await?
        .unwrap_or_default();
    let consents = store.list_consents(&realm.id, &user.id).await?;
    let has_consent = consents.iter().any(|c| c.client_id == client.client_id);

    let mut pending = user.required_actions.clone();
    if creds.iter().any(|c| c.credential_type == "password" && c.temporary)
        && !pending.iter().any(|a| a == "UPDATE_PASSWORD")
    {
        pending.push("UPDATE_PASSWORD".into());
    }

    Ok(LoginPolicyContext {
        second_factor_required: mfa_role || step_up || is_master,
        has_totp_credential: has_totp,
        has_webauthn_credential: has_webauthn,
        pending_actions: pending,
        org_choices,
        pinned_org,
        consent_required: client.consent_required && !has_consent,
    })
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
                (
                    "login".to_owned(),
                    h,
                    String::new(),
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
                let f = vec![FormField {
                    name: "otp".into(),
                    label: bundle.get("otpLabel").to_owned(),
                    input_type: "text".into(),
                    value: String::new(),
                    autocomplete: "one-time-code".into(),
                    hint: bundle.get("otpHint").to_owned(),
                    error: String::new(),
                }];
                (
                    "login-otp".to_owned(),
                    title,
                    String::new(),
                    f,
                    bundle.get("doLogIn").to_owned(),
                    String::new(),
                    String::new(),
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
                let f = vec![FormField {
                    name: "new_password".into(),
                    label: bundle.get("passwordLabel").to_owned(),
                    input_type: "password".into(),
                    value: String::new(),
                    autocomplete: "new-password".into(),
                    hint: bundle.get("passwordHint").to_owned(),
                    error: String::new(),
                }];
                (
                    "update-password".to_owned(),
                    bundle.get("updatePasswordTitle").to_owned(),
                    String::new(),
                    f,
                    bundle.get("proceedWithAction").to_owned(),
                    String::new(),
                    String::new(),
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
    };

    Ok(render_with_security_headers(&tpl, None))
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
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let Some(auth_session) = state
        .store
        .get_auth_session_by_id(&realm.id, &query.session_code)
        .await?
    else {
        let bundle = Bundle::for_locale(&realm.default_locale);
        let tpl = LoginPageTemplate {
            lang: bundle.locale.to_owned(),
            page_title: bundle.get("pageExpiredTitle").to_owned(),
            org_display: realm.display_name,
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
        };
        return Ok(render_with_security_headers(&tpl, None));
    };

    let current_state: LoginState = serde_json::from_str(&auth_session.step_state_json)
        .unwrap_or(LoginState::IdentityFirst);

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
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let session_code = form
        .get("session_code")
        .or(Some(&query.session_code))
        .cloned()
        .unwrap_or_default();

    let auth_session = state
        .store
        .get_auth_session_by_id(&realm.id, &session_code)
        .await?
        .ok_or_else(|| WardkeepError::BadRequest("login session expired".into()))?;

    // Verify CSRF token bound to `auth_session` (W7)
    let presented_csrf = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if !constant_time_str_eq(presented_csrf, &auth_session.csrf_token) {
        return Err(WardkeepError::Forbidden("invalid login CSRF token".into()));
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

    let mut current_state: LoginState = serde_json::from_str(&auth_session.step_state_json)
        .unwrap_or(LoginState::IdentityFirst);

    // Handle combined username + password form submission from `IdentityFirst` or `Password`
    if matches!(current_state, LoginState::IdentityFirst | LoginState::Password { .. }) {
        let username = form
            .get("username")
            .map(String::as_str)
            .unwrap_or_default()
            .trim();
        let password = form
            .get("password")
            .map(String::as_str)
            .unwrap_or_default();

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
            wardkeep_crypto::password::dummy_verify_password(password);
            let _ = bf.record_failure(&realm, None, "127.0.0.1").await;
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

        if bf.is_locked(&realm, Some(&user.id), "127.0.0.1").await? {
            wardkeep_crypto::password::dummy_verify_password(password);
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
            wardkeep_crypto::password::dummy_verify_password(password);
            let _ = bf.record_failure(&realm, Some(&user.id), "127.0.0.1").await;
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
            let _ = bf.record_failure(&realm, Some(&user.id), "127.0.0.1").await;
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
        bf.record_success(&realm, &user.id).await?;
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
        .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;
        current_state = transition(s1, LoginInput::VerifyPasswordOk, &ctx)
            .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;

        // If `otp` was also submitted in the same POST (or transition reached `Code`), handle next
        if form.get("otp").is_none() || !matches!(current_state, LoginState::SecondFactorOtp { .. }) {
            return advance_or_render(&state, &mount, &realm, auth_session, current_state, &headers)
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
            .ok_or_else(|| WardkeepError::Unauthorized("user not found".into()))?;
        let otp_code = form.get("otp").map(String::as_str).unwrap_or_default().trim();

        if let LoginState::EnrolTotp { .. } = current_state {
            let new_secret = form
                .get("totp_secret")
                .cloned()
                .unwrap_or_else(|| "enrol-seed-secret-256".into());
            let cred = CredentialRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                user_id: user.id.clone(),
                credential_type: "otp".into(),
                user_label: Some("Authenticator".into()),
                created_date: Utc::now().timestamp_millis(),
                secret_data: serde_json::json!({ "value": new_secret }).to_string(),
                credential_data: serde_json::json!({
                    "subType": "totp",
                    "digits": realm.otp_policy_digits,
                    "period": realm.otp_policy_period,
                    "algorithm": realm.otp_policy_algorithm,
                })
                .to_string(),
                temporary: false,
            };
            let now_secs = Utc::now().timestamp() as u64;
            if verify_totp_code(&cred, otp_code, realm.otp_policy_look_ahead_window, now_secs)?
                .is_none()
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
            state.store.upsert_credential(cred).await?;
            let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
            let next = transition(current_state, LoginInput::CompleteEnrolTotpOk, &ctx)
                .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;
            return advance_or_render(&state, &mount, &realm, auth_session, next, &headers).await;
        }

        let creds = state.store.list_credentials(&realm.id, &user.id).await?;
        let otp_cred = creds
            .iter()
            .find(|c| c.credential_type == "otp")
            .ok_or_else(|| WardkeepError::BadRequest("no OTP credential".into()))?;
        let now_secs = Utc::now().timestamp() as u64;
        let Some(matched_step) = verify_totp_code(
            otp_cred,
            otp_code,
            realm.otp_policy_look_ahead_window,
            now_secs,
        )?
        else {
            let _ = bf.record_failure(&realm, Some(&user.id), "127.0.0.1").await;
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

        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(current_state, LoginInput::VerifyOtpOk, &ctx)
            .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;
        return advance_or_render(&state, &mount, &realm, auth_session, next, &headers).await;
    }

    if let LoginState::Consent { ref user_id, .. } = current_state {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| WardkeepError::Unauthorized("user not found".into()))?;
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
            .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;
        return advance_or_render(&state, &mount, &realm, auth_session, next, &headers).await;
    }

    if let LoginState::OrganizationChoice { ref user_id, .. } = current_state {
        let user = state
            .store
            .get_user_by_id(&realm.id, None, user_id)
            .await?
            .ok_or_else(|| WardkeepError::Unauthorized("user not found".into()))?;
        let chosen = form
            .get("org_id")
            .cloned()
            .unwrap_or_default();
        let ctx = build_policy_context(&state.store, &realm, &auth_session, &user).await?;
        let next = transition(
            current_state,
            LoginInput::SelectOrganization { org_id: chosen },
            &ctx,
        )
        .map_err(|e| WardkeepError::BadRequest(e.to_string()))?;
        return advance_or_render(&state, &mount, &realm, auth_session, next, &headers).await;
    }

    Err(WardkeepError::BadRequest("unsupported login action".into()))
}

async fn advance_or_render(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    mut auth_session: AuthSession,
    next_state: LoginState,
    headers: &HeaderMap,
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
            .ok_or_else(|| WardkeepError::Unauthorized("user not found".into()))?;
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
            ip_address: "127.0.0.1".into(),
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
                ip_address: "127.0.0.1".into(),
                error: None,
                details: BTreeMap::new(),
            })
            .await?;

        let cookie_name = mount.cookie_name("sso");
        let cookie_path = mount.realm_cookie_path(&realm.realm);
        let ttl = realm.sso_session_max_lifespan.max(3600);
        let set_cookie = if mount.path_prefix.is_empty() {
            format!("{cookie_name}={sso_handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}")
        } else {
            format!("{cookie_name}={sso_handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age={ttl}")
        };

        let mut redirect_url = url::Url::parse(&auth_session.redirect_uri)
            .map_err(|_| WardkeepError::BadRequest("invalid redirect_uri".into()))?;
        redirect_url
            .query_pairs_mut()
            .append_pair("code", &raw_code);
        if let Some(st) = &auth_session.state {
            redirect_url.query_pairs_mut().append_pair("state", st);
        }

        return Ok((
            StatusCode::SEE_OTHER,
            [
                (header::SET_COOKIE, set_cookie),
                (header::LOCATION, redirect_url.to_string()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            "",
        )
            .into_response());
    }

    auth_session.step_state_json = serde_json::to_string(&next_state).unwrap_or_default();
    state.store.put_auth_session(auth_session.clone()).await?;
    render_state_page(
        state,
        mount,
        realm,
        &auth_session,
        &next_state,
        headers,
        "",
    )
    .await
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
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
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
    };
    Ok(render_with_security_headers(&tpl, None))
}

/// W3: reset-password always answers "If an account matches, an e-mail with a link is on its way"
/// so it never reveals whether the address exists.
async fn reset_credentials_post(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    Form(_form): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
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
    };
    Ok(render_with_security_headers(&tpl, None))
}
