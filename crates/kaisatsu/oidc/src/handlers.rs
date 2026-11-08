#![forbid(unsafe_code)]
//! OIDC and OAuth 2.1 protocol endpoints (W1, W8, W10, W12):
//! - `/.well-known/openid-configuration` and `/.well-known/oauth-authorization-server`
//! - `/protocol/openid-connect/{auth,token,certs,userinfo,logout,revoke,token/introspect,ext/par/request}`

use crate::brute_force::BruteForceProtector;
use crate::cimd::fetch_cimd_client;
use crate::client_ip::ClientIp;
use crate::dpop::verify_dpop_proof;
use crate::guard::OutboundGuard;
use crate::redirect::{validate_post_logout_redirect_uri, validate_redirect_uri};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Json, Router};
use base64::engine::{Engine, general_purpose::STANDARD as B64_STD};
use chrono::{Duration, Utc};
use jc_router::Mount;
use kaisatsu_core::claims::ActClaim;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Shared state for the OIDC protocol router.
#[derive(Clone)]
pub struct OidcState {
    pub store: Arc<dyn Store>,
    pub kek: Kek,
    pub guard: OutboundGuard,
    pub default_mount: Mount,
}

impl OidcState {
    pub fn mount_from(&self, ext: Option<&Mount>) -> Mount {
        ext.cloned().unwrap_or_else(|| self.default_mount.clone())
    }
}

pub fn oidc_router(state: OidcState) -> Router {
    Router::new()
        .route("/realms/{realm}", get(realm_info))
        .route(
            "/realms/{realm}/.well-known/openid-configuration",
            get(openid_configuration),
        )
        .route(
            "/realms/{realm}/.well-known/oauth-authorization-server",
            get(openid_configuration),
        )
        .route(
            "/.well-known/openid-configuration/realms/{realm}",
            get(openid_configuration),
        )
        .route(
            "/.well-known/oauth-authorization-server/realms/{realm}",
            get(openid_configuration),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/certs",
            get(jwks_certs),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/auth",
            get(authorize_get),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/ext/par/request",
            post(par_endpoint),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/token",
            post(token_endpoint),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/token/introspect",
            post(introspect_endpoint),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/revoke",
            post(revoke_endpoint),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/userinfo",
            get(userinfo_endpoint).post(userinfo_endpoint),
        )
        .route(
            "/realms/{realm}/protocol/openid-connect/logout",
            get(logout_endpoint).post(logout_endpoint),
        )
        .with_state(state)
}

async fn realm_info(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let issuer = mount.realm_issuer(&realm.realm);
    Ok(Json(json!({
        "realm": realm.realm,
        "public_key": "",
        "token-service": format!("{issuer}/protocol/openid-connect"),
        "account-service": format!("{issuer}/account"),
    })))
}

/// Discovery document (`/.well-known/openid-configuration` and `/.well-known/oauth-authorization-server`).
/// Per W1 and deployment `apisix-routes.yaml` (T-5611), `registration_endpoint` is never advertised
/// and `password` grant is not advertised in `grant_types_supported`.
async fn openid_configuration(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let issuer = mount.realm_issuer(&realm.realm);
    let oidc_base = format!("{issuer}/protocol/openid-connect");

    Ok(Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{oidc_base}/auth"),
        "token_endpoint": format!("{oidc_base}/token"),
        "introspection_endpoint": format!("{oidc_base}/token/introspect"),
        "userinfo_endpoint": format!("{oidc_base}/userinfo"),
        "end_session_endpoint": format!("{oidc_base}/logout"),
        "revocation_endpoint": format!("{oidc_base}/revoke"),
        "pushed_authorization_request_endpoint": format!("{oidc_base}/ext/par/request"),
        "jwks_uri": format!("{oidc_base}/certs"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "fragment", "form_post"],
        "grant_types_supported": [
            "authorization_code",
            "refresh_token",
            "client_credentials",
            "urn:ietf:params:oauth:grant-type:token-exchange",
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        ],
        "subject_types_supported": ["public", "pairwise"],
        "id_token_signing_alg_values_supported": ["ES256", "RS256", "EdDSA"],
        "token_endpoint_auth_methods_supported": [
            "client_secret_basic",
            "client_secret_post",
            "private_key_jwt"
        ],
        "code_challenge_methods_supported": ["S256"],
        "dpop_signing_alg_values_supported": ["ES256", "RS256", "EdDSA"],
        "backchannel_logout_supported": true,
        "backchannel_logout_session_supported": true,
        "client_id_metadata_document_supported": true,
        "acr_values_supported": ["pwd", "mfa"],
        "claims_supported": [
            "iss", "sub", "aud", "exp", "iat", "auth_time", "nonce", "acr", "amr", "azp",
            "preferred_username", "name", "given_name", "family_name", "email", "email_verified",
            "realm_access", "resource_access", "groups", "jc_org", "jc_groups"
        ],
        "scopes_supported": [
            "openid", "profile", "email", "roles", "web-origins", "offline_access",
            "acr", "basic", "address", "phone", "microprofile-jwt", "mcp:portal", "connector"
        ]
    })))
}

async fn jwks_certs(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let records = state.store.list_signing_keys(&realm.id).await?;
    let jwks = KeyManager::build_jwks(&records);
    let body = serde_json::to_string(&jwks).map_err(|e| KaisatsuError::Crypto(e.to_string()))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response())
}

/// Resolves or fetches a CIMD client when `client_id` is an `https://` URL (W1).
pub async fn resolve_or_fetch_client(
    state: &OidcState,
    realm: &RealmConfig,
    client_id: &str,
) -> Result<ClientRepresentation> {
    if let Some(existing) = state
        .store
        .get_client_by_client_id(&realm.id, client_id)
        .await?
    {
        if !existing.enabled {
            return Err(KaisatsuError::invalid_client("client is disabled"));
        }
        return Ok(existing);
    }
    if client_id.starts_with("https://") {
        let cimd_client = fetch_cimd_client(&state.guard, realm, client_id).await?;
        let saved = state.store.upsert_client(cimd_client).await?;
        return Ok(saved);
    }
    Err(KaisatsuError::invalid_client("unknown client_id"))
}

/// Validates requested scopes against the client's allowed scopes and parameterized scope regexps
/// (`connector:{id}` with regexp `[a-z2-7]{26}`, W1).
pub async fn resolve_scopes_for_client(
    store: &Arc<dyn Store>,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    requested_scope_str: Option<&str>,
) -> Result<(Vec<ClientScopeRepresentation>, String)> {
    let all_scopes = store.list_client_scopes(&realm.id).await?;
    let requested_tokens: Vec<&str> = requested_scope_str
        .unwrap_or("openid")
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();

    let mut active_scopes: Vec<ClientScopeRepresentation> = Vec::new();
    let mut granted_tokens: Vec<String> = Vec::new();

    if requested_tokens.contains(&"openid") {
        granted_tokens.push("openid".into());
    }

    // Include client's default scopes
    for def_name in &client.default_client_scopes {
        if let Some(sc) = all_scopes.iter().find(|s| &s.name == def_name) {
            if sc.include_in_token_scope() && !granted_tokens.contains(def_name) {
                granted_tokens.push(def_name.clone());
            }
            active_scopes.push(sc.clone());
        } else if !granted_tokens.contains(def_name)
            && matches!(
                def_name.as_str(),
                "profile" | "email" | "roles" | "acr" | "basic"
            )
        {
            granted_tokens.push(def_name.clone());
        }
    }

    for token in requested_tokens {
        if token == "openid" {
            continue;
        }
        if matches!(
            token,
            "profile" | "email" | "roles" | "acr" | "basic" | "address" | "phone"
        ) {
            if !granted_tokens.iter().any(|t| t == token) {
                granted_tokens.push(token.to_owned());
            }
            continue;
        }
        // Parameterized scope check (`family:param`, e.g. `connector:abcdefghij2345672345672345`)
        if let Some((family, param)) = token.split_once(':')
            && let Some(param_scope) = all_scopes
                .iter()
                .find(|s| s.name == family && s.is_parameterized())
        {
            let allowed_on_client = client.default_client_scopes.contains(&param_scope.name)
                || client.optional_client_scopes.contains(&param_scope.name);
            if !allowed_on_client {
                return Err(KaisatsuError::invalid_scope(format!(
                    "Invalid scopes: {token}"
                )));
            }
            if let Some(pattern) = param_scope.parameterized_regexp() {
                let anchored = format!("^(?:{pattern})$");
                let re = Regex::new(&anchored)
                    .map_err(|_| KaisatsuError::invalid_scope("invalid scope regexp"))?;
                if !re.is_match(param) {
                    return Err(KaisatsuError::invalid_scope(format!(
                        "parameterized scope '{token}' does not match required pattern"
                    )));
                }
            }
            if !active_scopes.iter().any(|s| s.id == param_scope.id) {
                active_scopes.push(param_scope.clone());
            }
            if param_scope.include_in_token_scope() && !granted_tokens.iter().any(|t| t == token) {
                granted_tokens.push(token.to_owned());
            }
            continue;
        }
        // Exact client scope check (e.g. `endpoint:{slug}`, `mcp:portal`, `gateway-hub`, `offline_access`)
        let allowed_on_client = client.default_client_scopes.iter().any(|s| s == token)
            || client.optional_client_scopes.iter().any(|s| s == token);
        if !allowed_on_client {
            return Err(KaisatsuError::invalid_scope(format!(
                "Invalid scopes: {token}"
            )));
        }
        if let Some(sc) = all_scopes.iter().find(|s| s.name == token) {
            if !active_scopes.iter().any(|s| s.id == sc.id) {
                active_scopes.push(sc.clone());
            }
            if sc.include_in_token_scope() && !granted_tokens.iter().any(|t| t == token) {
                granted_tokens.push(token.to_owned());
            }
        } else if token == "offline_access" && !granted_tokens.iter().any(|t| t == token) {
            granted_tokens.push(token.to_owned());
        }
    }

    Ok((active_scopes, granted_tokens.join(" ")))
}

/// Pushed Authorization Request (PAR, RFC 9126, W1).
async fn par_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Form(mut params): Form<BTreeMap<String, String>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let client = authenticate_client(&state, &realm, &mount, &headers, &params).await?;
    params.remove("client_secret");
    params.remove("client_assertion");
    params.remove("client_assertion_type");
    params.insert("client_id".into(), client.client_id.clone());

    validate_auth_request_params(&state, &realm, &client, &params).await?;

    let handle = random_handle_256();
    let request_uri = format!("urn:ietf:params:oauth:request_uri:{handle}");
    let rec = ParRequestRecord {
        request_uri_hash: sha256_hex(&request_uri),
        realm_id: realm.id.clone(),
        client_id: client.client_id,
        params,
        consumed: false,
        expires_at: Utc::now() + Duration::seconds(60),
    };
    state.store.put_par_request(rec).await?;

    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "request_uri": request_uri,
            "expires_in": 60
        })),
    )
        .into_response())
}

async fn validate_auth_request_params(
    state: &OidcState,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    params: &BTreeMap<String, String>,
) -> Result<()> {
    if !client.standard_flow_enabled {
        return Err(KaisatsuError::unauthorized_client(
            "standard flow is disabled on this client",
        ));
    }
    let response_type = params
        .get("response_type")
        .map(String::as_str)
        .unwrap_or_default();
    if response_type != "code" {
        return Err(KaisatsuError::unsupported_response_type(
            "only response_type=code is supported (implicit and hybrid are disabled by W1)",
        ));
    }
    let redirect_uri = params
        .get("redirect_uri")
        .ok_or_else(|| KaisatsuError::invalid_request("missing redirect_uri"))?;
    if !validate_redirect_uri(&client.redirect_uris, redirect_uri) {
        return Err(KaisatsuError::invalid_request("invalid redirect_uri"));
    }

    let pkce_required_method = client
        .attributes
        .get("pkce.code.challenge.method")
        .map_or("S256", String::as_str);
    let challenge = params
        .get("code_challenge")
        .map(String::as_str)
        .unwrap_or_default();
    let method = params
        .get("code_challenge_method")
        .map(String::as_str)
        .unwrap_or_default();
    if (!pkce_required_method.is_empty() || client.public_client || !challenge.is_empty())
        && (method != "S256" || challenge.len() < 43)
    {
        return Err(KaisatsuError::invalid_request(
            "PKCE S256 code_challenge is required; plain is refused (W1)",
        ));
    }

    let _ = resolve_scopes_for_client(
        &state.store,
        realm,
        client,
        params.get("scope").map(String::as_str),
    )
    .await?;
    Ok(())
}

/// Extracts an essential `acr` requirement from the `claims` JSON parameter or `acr_values` (W1).
fn extract_requested_acr(params: &BTreeMap<String, String>) -> Option<String> {
    if let Some(acr) = params.get("acr_values").filter(|s| !s.is_empty()) {
        return Some(acr.split_whitespace().next().unwrap_or(acr).to_owned());
    }
    if let Some(claims_json) = params.get("claims")
        && let Ok(val) = serde_json::from_str::<Value>(claims_json)
        && let Some(acr_obj) = val
            .pointer("/id_token/acr")
            .or_else(|| val.pointer("/userinfo/acr"))
    {
        if let Some(v) = acr_obj.get("value").and_then(Value::as_str) {
            return Some(v.to_owned());
        }
        if let Some(arr) = acr_obj.get("values").and_then(Value::as_array)
            && let Some(first) = arr.first().and_then(Value::as_str)
        {
            return Some(first.to_owned());
        }
    }
    None
}

/// The authorization response in the requested `response_mode` (`query`, `fragment`,
/// `form_post`), used by the SSO path here and by the interactive login.
pub fn redirect_with_params(
    redirect_uri: &str,
    response_mode: &str,
    pairs: &[(&str, &str)],
) -> Response {
    if response_mode == "form_post" {
        let mut inputs = String::new();
        for (k, v) in pairs {
            let ek = html_escape(k);
            let ev = html_escape(v);
            inputs.push_str(&format!(
                r#"<input type="hidden" name="{ek}" value="{ev}"/>"#
            ));
        }
        let action = html_escape(redirect_uri);
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Submit</title></head>\
             <body onload=\"document.forms[0].submit()\">\
             <form method=\"post\" action=\"{action}\">{inputs}<noscript><button type=\"submit\">Continue</button></noscript></form>\
             </body></html>"
        );
        return (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Html(html),
        )
            .into_response();
    }
    let Ok(mut url) = url::Url::parse(redirect_uri) else {
        return (StatusCode::BAD_REQUEST, "invalid redirect_uri").into_response();
    };
    if response_mode == "fragment" {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in pairs {
            ser.append_pair(k, v);
        }
        url.set_fragment(Some(&ser.finish()));
    } else {
        let mut qp = url.query_pairs_mut();
        for (k, v) in pairs {
            qp.append_pair(k, v);
        }
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Redirect::to(url.as_str()),
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn read_sso_cookie(headers: &HeaderMap, mount: &Mount) -> Option<String> {
    let target = mount.cookie_name("sso");
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

/// `GET /realms/{realm}/protocol/openid-connect/auth` (W1, W7).
async fn authorize_get(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(mut params): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));

    let client_id = params
        .get("client_id")
        .cloned()
        .ok_or_else(|| KaisatsuError::invalid_request("missing client_id"))?;
    let client = resolve_or_fetch_client(&state, &realm, &client_id).await?;

    // Resolve PAR `request_uri` if present
    if let Some(req_uri) = params.get("request_uri").cloned() {
        let par = state
            .store
            .consume_par_request(&realm.id, &sha256_hex(&req_uri))
            .await?
            .ok_or_else(|| KaisatsuError::invalid_request("invalid or expired request_uri"))?;
        if par.client_id != client.client_id {
            return Err(KaisatsuError::invalid_request(
                "request_uri does not belong to this client",
            ));
        }
        params = par.params;
    }

    validate_auth_request_params(&state, &realm, &client, &params).await?;

    let redirect_uri = params["redirect_uri"].clone();
    let response_mode = params
        .get("response_mode")
        .cloned()
        .unwrap_or_else(|| "query".into());
    let state_param = params.get("state").cloned();
    let prompt = params.get("prompt").cloned();
    let acr_requested = extract_requested_acr(&params);
    let max_age: Option<i64> = params.get("max_age").and_then(|s| s.parse().ok());

    // Check existing SSO session cookie unless `prompt=login` or `prompt=select_account`
    let existing_sso = if let Some(raw_handle) = read_sso_cookie(&headers, &mount) {
        state
            .store
            .get_sso_session_by_handle(&realm.id, &sha256_hex(&raw_handle))
            .await?
    } else {
        None
    };

    let now_unix = Utc::now().timestamp();
    let force_reauth = prompt.as_deref().is_some_and(|p| {
        p.split_whitespace()
            .any(|t| matches!(t, "login" | "select_account"))
    }) || max_age.is_some_and(|ma| {
        existing_sso
            .as_ref()
            .is_none_or(|sso| now_unix - sso.auth_time > ma)
    });

    let needs_step_up = acr_requested.as_deref() == Some("mfa")
        && existing_sso.as_ref().is_none_or(|sso| sso.acr != "mfa");

    if prompt.as_deref() == Some("none") {
        let Some(mut sso) = existing_sso.filter(|_| !force_reauth && !needs_step_up) else {
            let mut pairs = vec![("error", "login_required")];
            if let Some(st) = state_param.as_deref() {
                pairs.push(("state", st));
            }
            return Ok(redirect_with_params(&redirect_uri, &response_mode, &pairs));
        };
        if client.consent_required {
            let consents = state.store.list_consents(&realm.id, &sso.user_id).await?;
            if !consents.iter().any(|c| c.client_id == client.client_id) {
                let mut pairs = vec![("error", "consent_required")];
                if let Some(st) = state_param.as_deref() {
                    pairs.push(("state", st));
                }
                return Ok(redirect_with_params(&redirect_uri, &response_mode, &pairs));
            }
        }
        let (_, granted_scope) = resolve_scopes_for_client(
            &state.store,
            &realm,
            &client,
            params.get("scope").map(String::as_str),
        )
        .await?;
        let raw_code = random_handle_256();
        let code_rec = AuthorizationCodeRecord {
            code_hash: sha256_hex(&raw_code),
            realm_id: realm.id.clone(),
            org_id: sso.org_id.clone(),
            client_id: client.client_id.clone(),
            user_id: sso.user_id.clone(),
            sso_session_id: sso.id.clone(),
            redirect_uri: redirect_uri.clone(),
            scope: granted_scope,
            code_challenge: params.get("code_challenge").cloned().unwrap_or_default(),
            code_challenge_method: params
                .get("code_challenge_method")
                .cloned()
                .unwrap_or_else(|| "S256".into()),
            nonce: params.get("nonce").cloned(),
            acr: sso.acr.clone(),
            amr: sso.amr.clone(),
            dpop_jkt: params.get("dpop_jkt").cloned(),
            auth_time: sso.auth_time,
            consumed: false,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::seconds(realm.access_code_lifespan.max(60)),
        };
        sso.clients
            .insert(client.client_id.clone(), client.client_id.clone());
        sso.last_access = Utc::now().timestamp_millis();
        state.store.upsert_sso_session(sso).await?;
        state.store.put_auth_code(code_rec).await?;

        let mut pairs = vec![("code", raw_code.as_str())];
        if let Some(st) = state_param.as_deref() {
            pairs.push(("state", st));
        }
        return Ok(redirect_with_params(&redirect_uri, &response_mode, &pairs));
    }

    // If an active SSO session already satisfies the request and requires no consent, issue code immediately (W8 SSO)
    if let Some(mut sso) = existing_sso
        .clone()
        .filter(|_| !force_reauth && !needs_step_up)
    {
        let has_consent = if client.consent_required || prompt.as_deref() == Some("consent") {
            if prompt.as_deref() == Some("consent") {
                false
            } else {
                state
                    .store
                    .list_consents(&realm.id, &sso.user_id)
                    .await?
                    .iter()
                    .any(|c| c.client_id == client.client_id)
            }
        } else {
            true
        };
        if has_consent {
            let (_, granted_scope) = resolve_scopes_for_client(
                &state.store,
                &realm,
                &client,
                params.get("scope").map(String::as_str),
            )
            .await?;
            let raw_code = random_handle_256();
            let code_rec = AuthorizationCodeRecord {
                code_hash: sha256_hex(&raw_code),
                realm_id: realm.id.clone(),
                org_id: sso.org_id.clone(),
                client_id: client.client_id.clone(),
                user_id: sso.user_id.clone(),
                sso_session_id: sso.id.clone(),
                redirect_uri: redirect_uri.clone(),
                scope: granted_scope,
                code_challenge: params.get("code_challenge").cloned().unwrap_or_default(),
                code_challenge_method: params
                    .get("code_challenge_method")
                    .cloned()
                    .unwrap_or_else(|| "S256".into()),
                nonce: params.get("nonce").cloned(),
                acr: sso.acr.clone(),
                amr: sso.amr.clone(),
                dpop_jkt: params.get("dpop_jkt").cloned(),
                auth_time: sso.auth_time,
                consumed: false,
                created_at: Utc::now(),
                expires_at: Utc::now() + Duration::seconds(realm.access_code_lifespan.max(60)),
            };
            sso.clients
                .insert(client.client_id.clone(), client.client_id.clone());
            sso.last_access = Utc::now().timestamp_millis();
            state.store.upsert_sso_session(sso).await?;
            state.store.put_auth_code(code_rec).await?;

            let mut pairs = vec![("code", raw_code.as_str())];
            if let Some(st) = state_param.as_deref() {
                pairs.push(("state", st));
            }
            return Ok(redirect_with_params(&redirect_uri, &response_mode, &pairs));
        }
    }

    // Create 30-min `auth_session` and redirect to the login page (W2, W7)
    let (_, granted_scope) = resolve_scopes_for_client(
        &state.store,
        &realm,
        &client,
        params.get("scope").map(String::as_str),
    )
    .await?;
    let auth_session_id = uuid::Uuid::now_v7().to_string();
    let handle = random_handle_256();
    let csrf_token = random_handle_256();

    let (initial_step_json, pre_user_id, pre_amr, pre_acr) = match existing_sso
        .filter(|_| !force_reauth && needs_step_up)
    {
        Some(sso) => (
            serde_json::to_string(&json!({ "step": "SecondFactorOtp", "user_id": sso.user_id }))
                .unwrap_or_default(),
            Some(sso.user_id),
            sso.amr,
            sso.acr,
        ),
        None => (
            serde_json::to_string(&json!({ "step": "IdentityFirst" })).unwrap_or_default(),
            None,
            Vec::new(),
            String::new(),
        ),
    };

    let auth_session = AuthSession {
        id: auth_session_id.clone(),
        realm_id: realm.id.clone(),
        org_id: client.org_id.clone(),
        handle_hash: sha256_hex(&handle),
        client_id: client.client_id.clone(),
        redirect_uri,
        response_type: "code".into(),
        response_mode,
        scope: granted_scope,
        state: state_param,
        nonce: params.get("nonce").cloned(),
        code_challenge: params.get("code_challenge").cloned().unwrap_or_default(),
        code_challenge_method: params
            .get("code_challenge_method")
            .cloned()
            .unwrap_or_else(|| "S256".into()),
        acr_requested,
        prompt,
        login_hint: params.get("login_hint").cloned(),
        ui_locales: params
            .get("ui_locales")
            .or_else(|| params.get("kc_locale"))
            .cloned(),
        dpop_jkt: params.get("dpop_jkt").cloned(),
        csrf_token,
        step_state_json: initial_step_json,
        pending_totp_secret: None,
        user_id: pre_user_id,
        amr: pre_amr,
        acr_achieved: pre_acr,
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::minutes(30),
    };
    state.store.put_auth_session(auth_session).await?;

    let cookie_name = mount.cookie_name("auth");
    let cookie_path = mount.realm_cookie_path(&realm.realm);
    let set_cookie = if mount.path_prefix.is_empty() {
        format!("{cookie_name}={handle}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=1800")
    } else {
        format!(
            "{cookie_name}={handle}; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age=1800"
        )
    };
    let login_url = format!(
        "{}/realms/{}/login-actions/authenticate?session_code={auth_session_id}&client_id={}",
        mount.path_prefix, realm.realm, client.client_id
    );
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, set_cookie),
            (header::LOCATION, login_url),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        "",
    )
        .into_response())
}

/// Authenticates a client at the token/PAR/introspection/revocation endpoints:
/// - `client_secret_basic` / `client_secret_post` (constant-time secret comparison, W10),
/// - `private_key_jwt` / `client-jwt` against `jwks.string` (T-4015, T-5626),
/// - `federated-jwt` against the realm's `kubernetes` identity provider (PF-47, T-2868),
/// - Public clients (`public_client == true`) by `client_id`.
pub async fn authenticate_client(
    state: &OidcState,
    realm: &RealmConfig,
    mount: &Mount,
    headers: &HeaderMap,
    form: &BTreeMap<String, String>,
) -> Result<ClientRepresentation> {
    // 1. Check `client_assertion` (federated-jwt or private_key_jwt)
    if let Some(assertion) = form.get("client_assertion").filter(|s| !s.is_empty()) {
        let assertion_type = form
            .get("client_assertion_type")
            .map(String::as_str)
            .unwrap_or_default();
        if assertion_type != "urn:ietf:params:oauth:client-assertion-type:jwt-bearer" {
            return Err(KaisatsuError::invalid_client(
                "unsupported client_assertion_type",
            ));
        }
        return verify_client_assertion(
            state,
            realm,
            mount,
            assertion,
            form.get("client_id").map(String::as_str),
        )
        .await;
    }

    // 2. Check HTTP Basic Authorization header
    let mut client_id_opt = form.get("client_id").cloned();
    let mut client_secret_opt = form.get("client_secret").cloned();

    if let Some(auth_hdr) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
    {
        let decoded = B64_STD
            .decode(auth_hdr.trim())
            .map_err(|_| KaisatsuError::invalid_client("malformed Basic auth"))?;
        let text = String::from_utf8(decoded)
            .map_err(|_| KaisatsuError::invalid_client("malformed Basic auth utf8"))?;
        let (cid, csec) = text
            .split_once(':')
            .ok_or_else(|| KaisatsuError::invalid_client("malformed Basic auth pair"))?;
        // RFC 6749 §2.3.1: each half is form-urlencoded as a whole; '&' and '=' are data.
        let cid_dec = form_urldecode(cid)
            .ok_or_else(|| KaisatsuError::invalid_client("malformed Basic auth encoding"))?;
        let csec_dec = form_urldecode(csec)
            .ok_or_else(|| KaisatsuError::invalid_client("malformed Basic auth encoding"))?;
        if let Some(form_cid) = &client_id_opt
            && form_cid != &cid_dec
        {
            return Err(KaisatsuError::invalid_client(
                "client_id in body does not match Basic header",
            ));
        }
        client_id_opt = Some(cid_dec);
        client_secret_opt = Some(csec_dec);
    }

    let client_id =
        client_id_opt.ok_or_else(|| KaisatsuError::invalid_client("missing client credentials"))?;
    let client = resolve_or_fetch_client(state, realm, &client_id).await?;

    if client.public_client {
        return Ok(client);
    }
    if client.client_authenticator_type == "federated-jwt"
        || client.client_authenticator_type == "client-jwt"
    {
        return Err(KaisatsuError::invalid_client(
            "client requires JWT assertion authentication",
        ));
    }

    let presented =
        client_secret_opt.ok_or_else(|| KaisatsuError::invalid_client("missing client_secret"))?;
    let expected = client
        .secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::invalid_client("invalid client_secret"))?;
    if presented.is_empty() || !constant_time_str_eq(&presented, expected) {
        return Err(KaisatsuError::invalid_client("invalid client_secret"));
    }
    Ok(client)
}

/// Decodes one `application/x-www-form-urlencoded` value (`+` is a space, `%XX` a byte) without
/// splitting on `&` or `=`; `None` on a broken escape or non-UTF-8 result.
fn form_urldecode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = raw.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// True when the JWT `aud` claim (a string or an array) names one of `allowed`.
fn audience_matches(claims: &Value, allowed: &[String]) -> bool {
    match claims.get("aud") {
        Some(Value::String(a)) => allowed.iter().any(|x| x == a),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .any(|a| allowed.iter().any(|x| x == a)),
        _ => false,
    }
}

async fn verify_client_assertion(
    state: &OidcState,
    realm: &RealmConfig,
    mount: &Mount,
    assertion: &str,
    form_client_id: Option<&str>,
) -> Result<ClientRepresentation> {
    let payload_b64 = assertion
        .split('.')
        .nth(1)
        .ok_or_else(|| KaisatsuError::invalid_client("malformed client_assertion"))?;
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| KaisatsuError::invalid_client("malformed client_assertion payload"))?;
    let unverified: Value = serde_json::from_slice(&payload_bytes)
        .map_err(|_| KaisatsuError::invalid_client("invalid client_assertion JSON"))?;
    let sub = unverified
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::invalid_client("client_assertion missing sub"))?;
    let jti = unverified
        .get("jti")
        .and_then(Value::as_str)
        .unwrap_or(assertion);

    let realm_issuer = mount.realm_issuer(&realm.realm);
    let all_clients = state
        .store
        .list_clients(&realm.id, None, None, false)
        .await?;

    // Case A: `federated-jwt` (Kubernetes ServiceAccount token, `jwt.credential.sub == sub`)
    if let Some(fed_client) = all_clients.iter().find(|c| {
        c.client_authenticator_type == "federated-jwt"
            && c.attributes.get("jwt.credential.sub").map(String::as_str) == Some(sub)
    }) {
        if !fed_client.enabled {
            return Err(KaisatsuError::invalid_client("client is disabled"));
        }
        // PF-47 / Portal client_auth.rs: Keycloak refuses `client_id` if it does not match
        if let Some(fcid) = form_client_id
            && fcid != fed_client.client_id
        {
            return Err(KaisatsuError::invalid_client(
                "client_id parameter does not match sub claim",
            ));
        }
        let idp_alias = fed_client
            .attributes
            .get("jwt.credential.issuer")
            .map_or("kubernetes", String::as_str);
        let idp = state
            .store
            .get_idp(&realm.id, idp_alias)
            .await?
            .ok_or_else(|| KaisatsuError::invalid_client("federated issuer IdP not found"))?;
        let expected_iss = idp
            .config
            .get("issuer")
            .map(String::as_str)
            .unwrap_or("https://kubernetes.default.svc.cluster.local");
        let jwks_url =
            idp.config.get("jwksUrl").cloned().unwrap_or_else(|| {
                format!("{}/openid/v1/jwks", expected_iss.trim_end_matches('/'))
            });

        let jwks: Value = state.guard.get_json(&jwks_url, None, None).await?;
        let keys = jwks
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| KaisatsuError::invalid_client("cluster JWKS has no keys"))?;
        // The projected token must be minted for this realm: `aud` is the realm issuer, so a
        // token the cluster minted for another audience (the API server, another realm) fails.
        let verified = keys.iter().any(|jwk| {
            verify_jwt_with_jwk::<Value>(assertion, jwk, Some(expected_iss), Some(&realm_issuer))
                .is_ok()
        });
        if !verified {
            return Err(KaisatsuError::invalid_client(
                "federated client_assertion signature verification failed",
            ));
        }
        return Ok(fed_client.clone());
    }

    // Case B: `client-jwt` / `private_key_jwt` (sub == client_id, verified against `jwks.string`)
    let target_cid = form_client_id.unwrap_or(sub);
    let client = state
        .store
        .get_client_by_client_id(&realm.id, target_cid)
        .await?
        .filter(|c| c.enabled)
        .ok_or_else(|| KaisatsuError::invalid_client("unknown client for assertion"))?;
    if client.client_id != sub {
        return Err(KaisatsuError::invalid_client(
            "client_assertion sub must equal client_id",
        ));
    }
    let jwks_str = client
        .attributes
        .get("jwks.string")
        .ok_or_else(|| KaisatsuError::invalid_client("client has no jwks.string configured"))?;
    let jwks: Value = serde_json::from_str(jwks_str)
        .map_err(|_| KaisatsuError::invalid_client("invalid client jwks.string"))?;
    let keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| KaisatsuError::invalid_client("client jwks has no keys"))?;
    // RFC 7523 §3: `iss` and `sub` are the client, `aud` names this server (the realm issuer or
    // one of its endpoints that take client authentication).
    let endpoints = format!("{realm_issuer}/protocol/openid-connect");
    let allowed_aud = [
        realm_issuer.clone(),
        format!("{endpoints}/token"),
        format!("{endpoints}/token/introspect"),
        format!("{endpoints}/revoke"),
        format!("{endpoints}/ext/par/request"),
    ];
    let verified = keys.iter().any(|jwk| {
        verify_jwt_with_jwk::<Value>(assertion, jwk, Some(sub), None)
            .is_ok_and(|(_, claims)| audience_matches(&claims, &allowed_aud))
    });
    if !verified {
        return Err(KaisatsuError::invalid_client(
            "client_assertion signature verification failed",
        ));
    }
    // Replay protection on `jti`
    state
        .store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&format!("client-assertion:{jti}")),
            realm_id: realm.id.clone(),
            expires_at: Utc::now() + Duration::minutes(10),
        })
        .await
        .map_err(|_| KaisatsuError::invalid_client("replayed client_assertion jti"))?;

    Ok(client)
}

fn select_signing_key<'a>(
    keys: &'a [SigningKeyRecord],
    realm: &RealmConfig,
    client: &ClientRepresentation,
    for_id_token: bool,
) -> Result<&'a SigningKeyRecord> {
    let attr_key = if for_id_token {
        "id.token.signed.response.alg"
    } else {
        "access.token.signed.response.alg"
    };
    let wanted_alg = client
        .attributes
        .get(attr_key)
        .map(String::as_str)
        .unwrap_or(&realm.default_signature_algorithm);
    keys.iter()
        .find(|k| k.status == "ACTIVE" && k.algorithm == wanted_alg)
        .or_else(|| keys.iter().find(|k| k.status == "ACTIVE"))
        .ok_or_else(|| KaisatsuError::Crypto("no active signing key in realm".into()))
}

#[allow(clippy::too_many_arguments)]
async fn mint_tokens_for_principal(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    user: &UserRepresentation,
    active_scopes: &[ClientScopeRepresentation],
    granted_scope_str: &str,
    acr: &str,
    amr: &[String],
    sso_session_id: Option<&str>,
    nonce: Option<&str>,
    auth_time: Option<i64>,
    dpop_jkt: Option<&str>,
    act: Option<ActClaim>,
    include_refresh: bool,
    existing_family: Option<RefreshTokenFamily>,
) -> Result<Value> {
    let issuer = mount.realm_issuer(&realm.realm);
    let keys = state.store.list_signing_keys(&realm.id).await?;
    let user_groups = state.store.list_user_groups(&realm.id, &user.id).await?;
    let all_realm_roles = state.store.list_roles(&realm.id, None).await?;

    let now = Utc::now().timestamp();
    let access_ttl = client
        .attributes
        .get("access.token.lifespan")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(realm.access_token_lifespan.max(60));
    let access_exp = now + access_ttl;
    let access_jti = uuid::Uuid::now_v7().to_string();

    let mapper_ctx = MapperContext {
        realm,
        issuer: &issuer,
        client,
        user,
        user_groups: &user_groups,
        all_realm_roles: &all_realm_roles,
        active_scopes,
        granted_scope_string: granted_scope_str,
        acr,
        amr,
        session_id: sso_session_id,
        nonce,
        jti: &access_jti,
        iat: now,
        exp: access_exp,
        auth_time,
        dpop_jkt,
        act,
        active_org: user.org_id.as_deref(),
    };

    let access_claims = evaluate_mappers(&mapper_ctx, TokenKind::AccessToken)?;
    let access_key = select_signing_key(&keys, realm, client, false)?;
    let access_token = sign_jwt(access_key, &state.kek, Some("JWT"), &access_claims)?;

    let token_type = if dpop_jkt.is_some() { "DPoP" } else { "Bearer" };
    let mut response = json!({
        "access_token": access_token,
        "expires_in": access_ttl,
        "token_type": token_type,
        "not-before-policy": realm.not_before,
        "scope": granted_scope_str,
    });

    if let Some(sid) = sso_session_id {
        response["session_state"] = Value::String(sid.to_owned());
    }

    let wants_id_token = granted_scope_str.split_whitespace().any(|s| s == "openid");
    if wants_id_token {
        let id_jti = uuid::Uuid::now_v7().to_string();
        let mut id_ctx = mapper_ctx.clone();
        id_ctx.jti = &id_jti;
        let id_claims = evaluate_mappers(&id_ctx, TokenKind::IdToken)?;
        let id_key = select_signing_key(&keys, realm, client, true)?;
        let id_token = sign_jwt(id_key, &state.kek, Some("JWT"), &id_claims)?;
        response["id_token"] = Value::String(id_token);
    }

    if include_refresh {
        let is_offline = granted_scope_str
            .split_whitespace()
            .any(|s| s == "offline_access");
        if is_offline {
            // W1, KNOWLEDGE T-6301: offline_access requires the user to hold the `offline_access`
            // realm role (directly or via `default-roles-{realm}` composite).
            let group_roles: Vec<String> = user_groups
                .iter()
                .flat_map(|g| g.realm_roles.iter().cloned())
                .collect();
            let eff_roles = kaisatsu_core::claims::resolve_effective_realm_roles(
                &user.realm_roles,
                &group_roles,
                &all_realm_roles,
            );
            let default_composite = format!("default-roles-{}", realm.realm);
            let holds_offline = eff_roles.iter().any(|r| r == "offline_access")
                || user.realm_roles.iter().any(|r| r == &default_composite);
            if !holds_offline {
                return Err(KaisatsuError::invalid_scope(
                    "user does not hold offline_access role",
                ));
            }
        }

        let refresh_ttl = if is_offline {
            realm.offline_session_idle_timeout.max(3600)
        } else {
            client
                .attributes
                .get("client.session.idle.timeout")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(realm.sso_session_idle_timeout.max(3600))
        };
        // W2: the refresh token is an opaque handle `{family id}.{256-bit random}`; only its
        // SHA-256 is stored. The family id finds the family; the hash proves the handle.
        let family_id = existing_family
            .as_ref()
            .map(|f| f.id.clone())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let refresh_token = format!("{family_id}.{}", random_handle_256());
        let refresh_hash = sha256_hex(&refresh_token);
        match existing_family {
            Some(mut fam) => {
                let expected = std::mem::replace(&mut fam.current_jti_hash, refresh_hash);
                fam.previous_hashes.push(expected.clone());
                let excess = fam
                    .previous_hashes
                    .len()
                    .saturating_sub(MAX_PREVIOUS_REFRESH_HASHES);
                fam.previous_hashes.drain(..excess);
                fam.rotation_counter += 1;
                fam.expires_at = Utc::now() + Duration::seconds(refresh_ttl);
                if let Some(jkt) = dpop_jkt {
                    fam.dpop_jkt = Some(jkt.to_owned());
                }
                // Two requests with the same handle: only the first rotates.
                if !state.store.rotate_refresh_family(fam, &expected).await? {
                    return Err(KaisatsuError::invalid_grant(
                        "Refresh token was already used",
                    ));
                }
            }
            None => {
                state
                    .store
                    .upsert_refresh_family(RefreshTokenFamily {
                        id: family_id.clone(),
                        realm_id: realm.id.clone(),
                        org_id: user.org_id.clone(),
                        sso_session_id: sso_session_id.unwrap_or("offline").to_owned(),
                        user_id: user.id.clone(),
                        client_id: client.client_id.clone(),
                        scope: granted_scope_str.to_owned(),
                        acr: acr.to_owned(),
                        amr: amr.to_vec(),
                        auth_time: auth_time.unwrap_or(now),
                        rotation_counter: 1,
                        current_jti_hash: refresh_hash,
                        previous_hashes: Vec::new(),
                        dpop_jkt: dpop_jkt.map(str::to_owned),
                        offline: is_offline,
                        revoked: false,
                        expires_at: Utc::now() + Duration::seconds(refresh_ttl),
                    })
                    .await?;
            }
        }

        if is_offline {
            state
                .store
                .upsert_offline_session(OfflineSession {
                    id: family_id.clone(),
                    realm_id: realm.id.clone(),
                    org_id: user.org_id.clone(),
                    user_id: user.id.clone(),
                    client_id: client.client_id.clone(),
                    scopes: granted_scope_str
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect(),
                    created_at: Utc::now(),
                    last_access: Utc::now(),
                    idle_expires_at: Utc::now() + Duration::seconds(refresh_ttl),
                    max_expires_at: Utc::now()
                        + Duration::seconds(realm.offline_session_max_lifespan.max(refresh_ttl)),
                })
                .await?;
        }

        response["refresh_token"] = Value::String(refresh_token);
        response["refresh_expires_in"] = if is_offline {
            json!(0)
        } else {
            json!(refresh_ttl)
        };
    }

    Ok(response)
}

/// `POST /realms/{realm}/protocol/openid-connect/token` (W1, W12).
async fn token_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    OriginalUri(orig_uri): OriginalUri,
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
    let client = authenticate_client(&state, &realm, &mount, &headers, &form).await?;

    // Verify optional DPoP header (RFC 9449)
    let dpop_jkt = if let Some(dpop_hdr) = headers.get("dpop").and_then(|v| v.to_str().ok()) {
        let full_htu = format!(
            "{}://{}{}{}",
            mount.scheme,
            mount.host,
            mount.path_prefix,
            orig_uri.path()
        );
        let proof = verify_dpop_proof(dpop_hdr.trim(), "POST", &full_htu, None, None)?;
        state
            .store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: sha256_hex(&format!("dpop:{}", proof.jti)),
                realm_id: realm.id.clone(),
                expires_at: Utc::now() + Duration::minutes(10),
            })
            .await
            .map_err(|_| KaisatsuError::invalid_dpop_proof("replayed DPoP proof jti"))?;
        Some(proof.jkt)
    } else {
        None
    };

    let grant_type = form
        .get("grant_type")
        .map(String::as_str)
        .unwrap_or_default();

    let body = match grant_type {
        "authorization_code" => {
            handle_authorization_code_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        "refresh_token" => {
            handle_refresh_token_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        "client_credentials" => {
            handle_client_credentials_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        "urn:ietf:params:oauth:grant-type:token-exchange" => {
            handle_token_exchange_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        "urn:ietf:params:oauth:grant-type:jwt-bearer" => {
            handle_jwt_bearer_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        "password" => {
            handle_password_grant(
                &state,
                &mount,
                &realm,
                &client,
                &form,
                dpop_jkt.as_deref(),
                &client_ip,
            )
            .await?
        }
        _ => {
            return Err(KaisatsuError::unsupported_grant_type(format!(
                "unsupported grant_type: {grant_type}"
            )));
        }
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        serde_json::to_string(&body).unwrap_or_default(),
    )
        .into_response())
}

async fn handle_authorization_code_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    client_ip: &ClientIp,
) -> Result<Value> {
    let raw_code = form
        .get("code")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::invalid_request("missing code"))?;
    let redirect_uri = form
        .get("redirect_uri")
        .ok_or_else(|| KaisatsuError::invalid_request("missing redirect_uri"))?;

    let code_hash = sha256_hex(raw_code);
    let (code_opt, already_consumed) = state.store.consume_auth_code(&realm.id, &code_hash).await?;
    let code_rec = code_opt.ok_or_else(|| {
        KaisatsuError::invalid_grant("Code not valid or belongs to another realm")
    })?;

    // W12: Second use of an authorization code revokes the tokens/session issued from it!
    if already_consumed {
        state
            .store
            .revoke_refresh_families_for_session(
                &realm.id,
                &code_rec.sso_session_id,
                Some(&code_rec.client_id),
            )
            .await?;
        let _ = state
            .store
            .delete_sso_session(&realm.id, &code_rec.sso_session_id)
            .await;
        return Err(KaisatsuError::invalid_grant("Code already used"));
    }

    if code_rec.expires_at <= Utc::now() {
        return Err(KaisatsuError::invalid_grant("Code is expired"));
    }
    // Regression checks for FerrisKey GHSA-v2hq-9v44-43ff (cross-client / cross-realm) and
    // Rauthy GHSA-55g7-fq48-7mr4 (redirect_uri binding)
    if code_rec.realm_id != realm.id || code_rec.client_id != client.client_id {
        return Err(KaisatsuError::invalid_grant(
            "Code was not issued to this client or realm",
        ));
    }
    if code_rec.redirect_uri != *redirect_uri {
        return Err(KaisatsuError::invalid_grant("redirect_uri does not match"));
    }

    // PKCE S256 verification (`plain` refused by W1)
    if !code_rec.code_challenge.is_empty()
        || client.public_client
        || !client
            .attributes
            .get("pkce.code.challenge.method")
            .map_or("S256", String::as_str)
            .is_empty()
    {
        if code_rec.code_challenge_method != "S256" {
            return Err(KaisatsuError::invalid_grant("PKCE S256 required"));
        }
        let verifier = form
            .get("code_verifier")
            .ok_or_else(|| KaisatsuError::invalid_grant("PKCE code_verifier missing"))?;
        if verifier.len() < 43 || verifier.len() > 128 {
            return Err(KaisatsuError::invalid_grant(
                "PKCE code_verifier invalid length",
            ));
        }
        let computed = sha256_base64url(verifier);
        if !constant_time_str_eq(&computed, &code_rec.code_challenge) {
            return Err(KaisatsuError::invalid_grant("PKCE verification failed"));
        }
    }

    if let Some(bound_jkt) = &code_rec.dpop_jkt
        && dpop_jkt != Some(bound_jkt.as_str())
    {
        return Err(KaisatsuError::invalid_dpop_proof(
            "authorization code is bound to a DPoP key",
        ));
    }

    let user = state
        .store
        .get_user_by_id(&realm.id, None, &code_rec.user_id)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::invalid_grant("User disabled or not found"))?;

    let (active_scopes, granted_scope_str) =
        resolve_scopes_for_client(&state.store, realm, client, Some(&code_rec.scope)).await?;

    state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: user.org_id.clone(),
            time: Utc::now().timestamp_millis(),
            event_type: "CODE_TO_TOKEN".into(),
            client_id: Some(client.client_id.clone()),
            user_id: Some(user.id.clone()),
            session_id: Some(code_rec.sso_session_id.clone()),
            ip_address: client_ip.for_audit(),
            error: None,
            details: BTreeMap::new(),
        })
        .await?;

    mint_tokens_for_principal(
        state,
        mount,
        realm,
        client,
        &user,
        &active_scopes,
        &granted_scope_str,
        &code_rec.acr,
        &code_rec.amr,
        Some(&code_rec.sso_session_id),
        code_rec.nonce.as_deref(),
        Some(code_rec.auth_time),
        dpop_jkt,
        None,
        true,
        None,
    )
    .await
}

/// Rotated-away refresh handle hashes kept per family for reuse detection.
const MAX_PREVIOUS_REFRESH_HASHES: usize = 64;

/// Finds the family of an opaque refresh handle `{family id}.{secret}`. `Some((family, true))`
/// when the handle is the family's current one, `Some((family, false))` when it is a handle the
/// family rotated away; `None` for anything else (unknown family or a forged secret, which must
/// not let a guesser revoke someone's session).
async fn find_refresh_family(
    state: &OidcState,
    realm_id: &str,
    raw: &str,
) -> Result<Option<(RefreshTokenFamily, bool)>> {
    let Some((fid, secret)) = raw.split_once('.') else {
        return Ok(None);
    };
    if fid.is_empty() || secret.is_empty() {
        return Ok(None);
    }
    let Some(family) = state.store.get_refresh_family(realm_id, fid).await? else {
        return Ok(None);
    };
    let presented = sha256_hex(raw);
    if constant_time_str_eq(&presented, &family.current_jti_hash) {
        return Ok(Some((family, true)));
    }
    let rotated = family.previous_hashes.iter().fold(false, |found, h| {
        found | constant_time_str_eq(&presented, h)
    });
    Ok(rotated.then_some((family, false)))
}

async fn handle_refresh_token_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    _client_ip: &ClientIp,
) -> Result<Value> {
    let raw_refresh = form
        .get("refresh_token")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::invalid_request("missing refresh_token"))?;
    let (family, is_current) = find_refresh_family(state, &realm.id, raw_refresh)
        .await?
        .ok_or_else(|| KaisatsuError::invalid_grant("Invalid refresh token"))?;
    // Regression check for Rauthy GHSA-7qh2-3hc5-2vqp: the family belongs to the authenticated client
    if family.client_id != client.client_id {
        return Err(KaisatsuError::invalid_grant(
            "Refresh token was issued to another client",
        ));
    }
    if family.revoked || family.expires_at <= Utc::now() {
        return Err(KaisatsuError::invalid_grant("Session not active"));
    }

    // W1, W12: Refresh token rotation and reuse detection. A rotated-away handle presented
    // again revokes the entire refresh token family and the SSO session.
    if !is_current {
        let mut revoked_fam = family.clone();
        revoked_fam.revoked = true;
        state.store.upsert_refresh_family(revoked_fam).await?;
        let _ = state
            .store
            .delete_sso_session(&realm.id, &family.sso_session_id)
            .await;
        return Err(KaisatsuError::invalid_grant(
            "Refresh token reuse detected; session revoked",
        ));
    }

    // W1: Public clients that bound their refresh token with DPoP must present the matching DPoP proof
    if let Some(bound_jkt) = &family.dpop_jkt
        && dpop_jkt != Some(bound_jkt.as_str())
    {
        return Err(KaisatsuError::invalid_dpop_proof(
            "Refresh token is bound to a DPoP key",
        ));
    }

    if !family.offline {
        let sso = state
            .store
            .get_sso_session(&realm.id, &family.sso_session_id)
            .await?
            .ok_or_else(|| KaisatsuError::invalid_grant("Session not active"))?;
        let mut updated_sso = sso;
        updated_sso.last_access = Utc::now().timestamp_millis();
        updated_sso.idle_expires_at =
            Utc::now() + Duration::seconds(realm.sso_session_idle_timeout.max(3600));
        state.store.upsert_sso_session(updated_sso).await?;
    }

    let user = state
        .store
        .get_user_by_id(&realm.id, None, &family.user_id)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::invalid_grant("User disabled or not found"))?;

    let (active_scopes, granted_scope_str) =
        resolve_scopes_for_client(&state.store, realm, client, Some(&family.scope)).await?;

    let sso_id = family.sso_session_id.clone();
    let acr = family.acr.clone();
    let amr = family.amr.clone();
    let auth_time = family.auth_time;
    let bound_jkt = dpop_jkt
        .map(str::to_owned)
        .or_else(|| family.dpop_jkt.clone());

    mint_tokens_for_principal(
        state,
        mount,
        realm,
        client,
        &user,
        &active_scopes,
        &granted_scope_str,
        &acr,
        &amr,
        Some(&sso_id),
        None,
        Some(auth_time),
        bound_jkt.as_deref(),
        None,
        true,
        Some(family),
    )
    .await
}

async fn handle_client_credentials_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    client_ip: &ClientIp,
) -> Result<Value> {
    if client.public_client || !client.service_accounts_enabled {
        return Err(KaisatsuError::unauthorized_client(
            "client is not enabled for client_credentials",
        ));
    }
    let sa_username = format!("service-account-{}", client.client_id);
    let user = match state
        .store
        .get_user_by_username_or_email(&realm.id, None, &sa_username)
        .await?
    {
        // Only the client's own service account: a person who happens to hold the name (or an
        // e-mail equal to it) is never the principal of a client_credentials token.
        Some(u) if u.service_account_client_id.as_deref() == Some(client.client_id.as_str()) => {
            if !u.enabled {
                return Err(KaisatsuError::unauthorized_client(
                    "service account is disabled",
                ));
            }
            u
        }
        Some(_) => {
            return Err(KaisatsuError::unauthorized_client(
                "service account name is held by another user",
            ));
        }
        None => {
            state
                .store
                .create_user(UserRepresentation {
                    id: uuid::Uuid::now_v7().to_string(),
                    realm_id: realm.id.clone(),
                    org_id: client.org_id.clone(),
                    username: sa_username,
                    enabled: true,
                    email_verified: false,
                    service_account_client_id: Some(client.client_id.clone()),
                    ..Default::default()
                })
                .await?
        }
    };

    let (active_scopes, granted_scope_str) = resolve_scopes_for_client(
        &state.store,
        realm,
        client,
        form.get("scope").map(String::as_str),
    )
    .await?;

    state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: client.org_id.clone(),
            time: Utc::now().timestamp_millis(),
            event_type: "CLIENT_LOGIN".into(),
            client_id: Some(client.client_id.clone()),
            user_id: Some(user.id.clone()),
            session_id: None,
            ip_address: client_ip.for_audit(),
            error: None,
            details: BTreeMap::new(),
        })
        .await?;

    mint_tokens_for_principal(
        state,
        mount,
        realm,
        client,
        &user,
        &active_scopes,
        &granted_scope_str,
        "0",
        &[],
        None,
        None,
        None,
        dpop_jkt,
        None,
        false,
        None,
    )
    .await
}

/// Standard Token Exchange V2 (`urn:ietf:params:oauth:grant-type:token-exchange`, RFC 8693, W1):
/// 1. Requester client must have `standard.token.exchange.enabled == "true"`.
/// 2. `subject_token` must be a valid access token of this realm whose `aud` contains the requester
///    (`client.client_id`), and whose subject/session is still active.
/// 3. Every requested `scope` must be linked on the requester client (default or optional).
/// 4. Every requested `audience` must be mapped by one of the requester's active scopes or mappers
///    (else `400 invalid_request` `"Requested audience not available: {aud}"`, matching Portal
///    `src/auth/oidc.rs:647`).
/// 5. Emits `act` claim (`{"sub": requester_or_actor}`) for delegation (W1).
async fn handle_token_exchange_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    requester: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    client_ip: &ClientIp,
) -> Result<Value> {
    if requester.public_client
        || requester
            .attributes
            .get("standard.token.exchange.enabled")
            .map(String::as_str)
            != Some("true")
    {
        return Err(KaisatsuError::invalid_request(
            "Standard token exchange is not enabled for the requested client",
        ));
    }

    let subject_token = form
        .get("subject_token")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::invalid_request("missing subject_token"))?;

    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    // Verify subject_token signature, issuer, expiry, and that `aud` contains `requester.client_id`
    let (_, subj_claims): (_, Value) = verify_jwt(
        subject_token,
        &keys,
        Some(&issuer),
        Some(&requester.client_id),
    )
    .map_err(|_| KaisatsuError::invalid_request("Invalid token"))?;

    if subj_claims.get("typ").and_then(Value::as_str) != Some("Bearer") {
        return Err(KaisatsuError::invalid_request("Invalid token"));
    }
    if let Some(jti) = subj_claims.get("jti").and_then(Value::as_str)
        && state
            .store
            .is_token_revoked(&realm.id, &sha256_hex(jti))
            .await?
    {
        return Err(KaisatsuError::invalid_request("Invalid token"));
    }
    if let Some(sid) = subj_claims.get("sid").and_then(Value::as_str)
        && state.store.get_sso_session(&realm.id, sid).await?.is_none()
    {
        return Err(KaisatsuError::invalid_request("Invalid token"));
    }

    let subject_id = subj_claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::invalid_request("Invalid token"))?;
    let user = state
        .store
        .get_user_by_id(&realm.id, None, subject_id)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::invalid_request("Invalid token"))?;

    let (active_scopes, granted_scope_str) = resolve_scopes_for_client(
        &state.store,
        realm,
        requester,
        form.get("scope").map(String::as_str),
    )
    .await?;

    // Check that any requested `audience` is mapped by one of the requester's active scopes or mappers
    if let Some(req_aud) = form.get("audience").filter(|s| !s.is_empty()) {
        let mapped_audiences: Vec<&str> = active_scopes
            .iter()
            .flat_map(|s| s.protocol_mappers.iter())
            .chain(requester.protocol_mappers.iter())
            .filter(|m| m.protocol_mapper == "oidc-audience-mapper")
            .filter_map(|m| {
                m.config
                    .get("included.custom.audience")
                    .or_else(|| m.config.get("included.client.audience"))
                    .map(String::as_str)
            })
            .collect();
        if !mapped_audiences.contains(&req_aud.as_str()) {
            return Err(KaisatsuError::invalid_request(format!(
                "Requested audience not available: {req_aud}"
            )));
        }
    }

    // Build `act` delegation claim (W1)
    let actor_sub = if let Some(actor_token) = form.get("actor_token").filter(|s| !s.is_empty()) {
        let (_, act_claims): (_, Value) = verify_jwt(actor_token, &keys, Some(&issuer), None)
            .map_err(|_| KaisatsuError::invalid_request("Invalid actor_token"))?;
        act_claims
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or(&requester.client_id)
            .to_owned()
    } else {
        requester.client_id.clone()
    };
    let existing_act = subj_claims
        .get("act")
        .and_then(|v| serde_json::from_value::<ActClaim>(v.clone()).ok())
        .map(Box::new);
    let act = Some(ActClaim {
        sub: actor_sub,
        act: existing_act,
    });

    let acr = subj_claims
        .get("acr")
        .and_then(Value::as_str)
        .unwrap_or("pwd");
    let amr: Vec<String> = subj_claims
        .get("amr")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let sid = subj_claims.get("sid").and_then(Value::as_str);
    let auth_time = subj_claims.get("auth_time").and_then(Value::as_i64);

    let wants_refresh = requester
        .attributes
        .get("standard.token.exchange.enableRefreshRequestedTokenType")
        .is_some_and(|v| v == "SAME_SESSION")
        && sid.is_some();

    state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: user.org_id.clone(),
            time: Utc::now().timestamp_millis(),
            event_type: "TOKEN_EXCHANGE".into(),
            client_id: Some(requester.client_id.clone()),
            user_id: Some(user.id.clone()),
            session_id: sid.map(str::to_owned),
            ip_address: client_ip.for_audit(),
            error: None,
            details: BTreeMap::new(),
        })
        .await?;

    let mut out = mint_tokens_for_principal(
        state,
        mount,
        realm,
        requester,
        &user,
        &active_scopes,
        &granted_scope_str,
        acr,
        &amr,
        sid,
        None,
        auth_time,
        dpop_jkt,
        act,
        wants_refresh,
        None,
    )
    .await?;
    out["issued_token_type"] =
        Value::String("urn:ietf:params:oauth:token-type:access_token".into());
    Ok(out)
}

/// RFC 7523 JWT Authorization Grant (`urn:ietf:params:oauth:grant-type:jwt-bearer`, W1).
///
/// Trust anchor is the requesting client's own `jwks.string`, so the assertion can only speak for
/// that client: `iss` is the `client_id`, `sub` is the client's own service account (its user id
/// or the `client_id`), `aud` is the realm issuer, `jti` is single-use. A confidential client
/// can never name a person or another client's account as `sub` (that would be impersonation).
async fn handle_jwt_bearer_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    client_ip: &ClientIp,
) -> Result<Value> {
    if client.public_client || !client.service_accounts_enabled {
        return Err(KaisatsuError::unauthorized_client(
            "client is not enabled for jwt-bearer",
        ));
    }
    let assertion = form
        .get("assertion")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::invalid_request("missing assertion"))?;

    let jwks_str = client
        .attributes
        .get("jwks.string")
        .ok_or_else(|| KaisatsuError::invalid_grant("client has no jwks.string for jwt-bearer"))?;
    let jwks: Value = serde_json::from_str(jwks_str)
        .map_err(|_| KaisatsuError::invalid_grant("invalid client jwks.string"))?;
    let keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| KaisatsuError::invalid_grant("client jwks has no keys"))?;

    let issuer = mount.realm_issuer(&realm.realm);
    let claims = keys
        .iter()
        .find_map(|jwk| {
            verify_jwt_with_jwk::<Value>(assertion, jwk, Some(&client.client_id), Some(&issuer))
                .ok()
                .map(|(_, c)| c)
        })
        .ok_or_else(|| KaisatsuError::invalid_grant("jwt-bearer assertion did not verify"))?;
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::invalid_grant("assertion missing sub"))?;
    let jti = claims
        .get("jti")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::invalid_grant("assertion missing jti"))?;

    let sa_username = format!("service-account-{}", client.client_id);
    let user = state
        .store
        .get_user_by_username_or_email(&realm.id, None, &sa_username)
        .await?
        .filter(|u| {
            u.enabled && u.service_account_client_id.as_deref() == Some(client.client_id.as_str())
        })
        .ok_or_else(|| KaisatsuError::invalid_grant("client has no service account"))?;
    if sub != user.id && sub != client.client_id {
        return Err(KaisatsuError::invalid_grant(
            "assertion sub must be the client's own service account",
        ));
    }
    state
        .store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&format!("jwt-bearer:{}:{jti}", client.client_id)),
            realm_id: realm.id.clone(),
            expires_at: Utc::now() + Duration::hours(1),
        })
        .await
        .map_err(|_| KaisatsuError::invalid_grant("replayed jwt-bearer assertion"))?;

    let (active_scopes, granted_scope_str) = resolve_scopes_for_client(
        &state.store,
        realm,
        client,
        form.get("scope").map(String::as_str),
    )
    .await?;

    state
        .store
        .append_event(AuditEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.id.clone(),
            org_id: client.org_id.clone(),
            time: Utc::now().timestamp_millis(),
            event_type: "CLIENT_LOGIN".into(),
            client_id: Some(client.client_id.clone()),
            user_id: Some(user.id.clone()),
            session_id: None,
            ip_address: client_ip.for_audit(),
            error: None,
            details: BTreeMap::new(),
        })
        .await?;

    mint_tokens_for_principal(
        state,
        mount,
        realm,
        client,
        &user,
        &active_scopes,
        &granted_scope_str,
        "0",
        &[],
        None,
        None,
        None,
        dpop_jkt,
        None,
        false,
        None,
    )
    .await
}

/// Resource Owner Password Credentials grant (`password`, W1):
/// - Allowed ONLY when `client.direct_access_grants_enabled` is true.
/// - Refused for CIMD clients, `app-*` clients, and `mcp-*` clients (W1).
/// - Refused for any person holding `mfa-required` (directly or via composite) or an enrolled OTP
///   unless `totp` is provided and verifies (RI `custom direct grant`).
async fn handle_password_grant(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    client: &ClientRepresentation,
    form: &BTreeMap<String, String>,
    dpop_jkt: Option<&str>,
    client_ip: &ClientIp,
) -> Result<Value> {
    if !client.direct_access_grants_enabled
        || client.is_cimd
        || client.client_id.starts_with("app-")
        || client.client_id.starts_with("mcp-")
    {
        return Err(KaisatsuError::unauthorized_client(
            "Direct access grants are not enabled for this client",
        ));
    }

    let username = form.get("username").map(String::as_str).unwrap_or_default();
    let password = form.get("password").map(String::as_str).unwrap_or_default();

    let bf = BruteForceProtector::new(state.store.clone());
    let ip = client_ip.as_deref();
    let Some(user) = state
        .store
        .get_user_by_username_or_email(&realm.id, None, username)
        .await?
        .filter(|u| u.enabled && u.service_account_client_id.is_none())
    else {
        kaisatsu_crypto::password::dummy_verify_password(password);
        let _ = bf.record_failure(realm, None, ip).await;
        return Err(KaisatsuError::invalid_grant("Invalid user credentials"));
    };
    // Same brute-force protector as the login pages (RI `bruteForceProtected`): a locked person
    // gets the same answer as a wrong password, after the same hashing work.
    if bf.is_locked(realm, Some(&user.id), ip).await? {
        kaisatsu_crypto::password::dummy_verify_password(password);
        return Err(KaisatsuError::invalid_grant("Invalid user credentials"));
    }

    let creds = state.store.list_credentials(&realm.id, &user.id).await?;
    let Some(pw_cred) = creds.iter().find(|c| c.credential_type == "password") else {
        kaisatsu_crypto::password::dummy_verify_password(password);
        let _ = bf.record_failure(realm, Some(&user.id), ip).await;
        return Err(KaisatsuError::invalid_grant("Invalid user credentials"));
    };

    let outcome = verify_and_maybe_rehash(password, pw_cred)?;
    if !outcome.valid {
        let _ = bf.record_failure(realm, Some(&user.id), ip).await;
        return Err(KaisatsuError::invalid_grant("Invalid user credentials"));
    }
    if let Some(new_phc) = outcome.rehashed_phc {
        let mut updated = pw_cred.clone();
        updated.secret_data = new_phc;
        updated.credential_data.clear();
        state.store.upsert_credential(updated).await?;
    }

    // Check if second factor is required or configured (RI `custom direct grant`)
    let user_groups = state.store.list_user_groups(&realm.id, &user.id).await?;
    let all_roles = state.store.list_roles(&realm.id, None).await?;
    let group_roles: Vec<String> = user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let eff_roles = kaisatsu_core::claims::resolve_effective_realm_roles(
        &user.realm_roles,
        &group_roles,
        &all_roles,
    );
    let mfa_required = eff_roles.iter().any(|r| r == "mfa-required");
    let otp_cred = creds.iter().find(|c| c.credential_type == "otp");

    let mut amr = vec!["pwd".to_owned()];
    let mut acr = "pwd".to_owned();

    if mfa_required || otp_cred.is_some() {
        let Some(otp_cred) = otp_cred else {
            return Err(KaisatsuError::oauth(
                400,
                "invalid_grant",
                "OTP required by role mfa-required but not enrolled",
            ));
        };
        let submitted_totp = form
            .get("totp")
            .or_else(|| form.get("otp"))
            .map(String::as_str)
            .ok_or_else(|| KaisatsuError::invalid_grant("Missing totp code"))?;
        let now_secs = Utc::now().timestamp() as u64;
        let Some(matched_step) = verify_totp_code(
            otp_cred,
            submitted_totp,
            realm.otp_policy_look_ahead_window,
            now_secs,
        )?
        else {
            let _ = bf.record_failure(realm, Some(&user.id), ip).await;
            return Err(KaisatsuError::invalid_grant("Invalid totp code"));
        };

        // Refuse replay of a used TOTP code in the same step window (W7)
        let step_key = sha256_hex(&format!("totp:{}:{}:{matched_step}", user.id, otp_cred.id));
        state
            .store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: step_key,
                realm_id: realm.id.clone(),
                expires_at: Utc::now() + Duration::seconds(90),
            })
            .await
            .map_err(|_| KaisatsuError::invalid_grant("TOTP code already used in this window"))?;

        amr.push("otp".into());
        acr = "mfa".into();
    }
    bf.record_success(realm, &user.id).await?;

    let now = Utc::now();
    let sso_id = uuid::Uuid::now_v7().to_string();
    let sso = SsoSession {
        id: sso_id.clone(),
        realm_id: realm.id.clone(),
        org_id: user.org_id.clone(),
        handle_hash: sha256_hex(&random_handle_256()),
        user_id: user.id.clone(),
        username: user.username.clone(),
        ip_address: client_ip.for_audit(),
        start: now.timestamp_millis(),
        last_access: now.timestamp_millis(),
        auth_time: now.timestamp(),
        acr: acr.clone(),
        amr: amr.clone(),
        clients: HashMap::from([(client.client_id.clone(), client.client_id.clone())]),
        idle_expires_at: now + Duration::seconds(realm.sso_session_idle_timeout.max(3600)),
        max_expires_at: now + Duration::seconds(realm.sso_session_max_lifespan.max(36000)),
    };
    state.store.upsert_sso_session(sso).await?;

    let (active_scopes, granted_scope_str) = resolve_scopes_for_client(
        &state.store,
        realm,
        client,
        form.get("scope").map(String::as_str),
    )
    .await?;

    mint_tokens_for_principal(
        state,
        mount,
        realm,
        client,
        &user,
        &active_scopes,
        &granted_scope_str,
        &acr,
        &amr,
        Some(&sso_id),
        None,
        Some(now.timestamp()),
        dpop_jkt,
        None,
        true,
        None,
    )
    .await
}

/// RFC 7662 Token Introspection (`POST /realms/{realm}/protocol/openid-connect/token/introspect`, W1):
/// - Only confidential clients may introspect.
/// - Only tokens of the same realm; anything not found, expired, or revoked returns `{"active": false}`.
async fn introspect_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<Json<Value>> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let client = authenticate_client(&state, &realm, &mount, &headers, &form).await?;
    if client.public_client {
        return Err(KaisatsuError::unauthorized_client(
            "public clients may not call token introspection (W1)",
        ));
    }

    let Some(token) = form.get("token").filter(|s| !s.is_empty()) else {
        return Ok(Json(json!({ "active": false })));
    };
    if let Some((family, current)) = find_refresh_family(&state, &realm.id, token).await? {
        let session_alive = family.offline
            || state
                .store
                .get_sso_session(&realm.id, &family.sso_session_id)
                .await?
                .is_some();
        if !current || family.revoked || family.expires_at <= Utc::now() || !session_alive {
            return Ok(Json(json!({ "active": false })));
        }
        let mut out = json!({
            "active": true,
            "typ": if family.offline { "Offline" } else { "Refresh" },
            "iss": mount.realm_issuer(&realm.realm),
            "sub": family.user_id,
            "azp": family.client_id,
            "client_id": family.client_id,
            "scope": family.scope,
            "exp": family.expires_at.timestamp(),
        });
        if !family.offline {
            out["sid"] = Value::String(family.sso_session_id.clone());
        }
        return Ok(Json(out));
    }
    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    let Ok((_, mut claims)): Result<(_, Map<String, Value>)> =
        verify_jwt(token, &keys, Some(&issuer), None)
    else {
        return Ok(Json(json!({ "active": false })));
    };

    if let Some(jti) = claims.get("jti").and_then(Value::as_str)
        && state
            .store
            .is_token_revoked(&realm.id, &sha256_hex(jti))
            .await?
    {
        return Ok(Json(json!({ "active": false })));
    }
    if let Some(sid) = claims.get("sid").and_then(Value::as_str)
        && state.store.get_sso_session(&realm.id, sid).await?.is_none()
    {
        return Ok(Json(json!({ "active": false })));
    }

    claims.insert("active".into(), Value::Bool(true));
    if let Some(azp) = claims.get("azp").cloned() {
        claims.insert("client_id".into(), azp);
    }
    Ok(Json(Value::Object(claims)))
}

/// RFC 7009 Token Revocation (`POST /realms/{realm}/protocol/openid-connect/revoke`, W1):
/// A refresh token revocation ends that client's grant in the SSO session (KNOWLEDGE T-6483).
async fn revoke_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Result<StatusCode> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let client = authenticate_client(&state, &realm, &mount, &headers, &form).await?;

    let Some(token) = form.get("token").filter(|s| !s.is_empty()) else {
        return Ok(StatusCode::OK);
    };
    if let Some((family, _)) = find_refresh_family(&state, &realm.id, token).await? {
        // Only the client the refresh token was issued to may revoke it (RFC 7009 §2.1).
        if family.client_id == client.client_id {
            let mut revoked = family.clone();
            revoked.revoked = true;
            state.store.upsert_refresh_family(revoked).await?;
            if family.offline {
                state
                    .store
                    .delete_user_client_offline_sessions(
                        &realm.id,
                        &family.user_id,
                        &client.client_id,
                    )
                    .await?;
            } else {
                state
                    .store
                    .revoke_refresh_families_for_session(
                        &realm.id,
                        &family.sso_session_id,
                        Some(&client.client_id),
                    )
                    .await?;
            }
        }
        return Ok(StatusCode::OK);
    }
    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    if let Ok((_, claims)) = verify_jwt::<Value>(token, &keys, Some(&issuer), None)
        && claims.get("azp").and_then(Value::as_str) == Some(client.client_id.as_str())
    {
        if let Some(jti) = claims.get("jti").and_then(Value::as_str) {
            let _ = state
                .store
                .mark_token_revoked(RevokedTokenRecord {
                    jti_hash: sha256_hex(jti),
                    realm_id: realm.id.clone(),
                    expires_at: Utc::now() + Duration::hours(24),
                })
                .await;
        }
        if let Some(sid) = claims.get("sid").and_then(Value::as_str) {
            state
                .store
                .revoke_refresh_families_for_session(&realm.id, sid, Some(&client.client_id))
                .await?;
        }
    }
    Ok(StatusCode::OK)
}

/// RFC 9449 §7: a token bound to a key (`cnf.jkt`) is only honoured under the `DPoP` scheme with
/// a fresh, single-use proof for this request, made by that key and carrying the token hash
/// (`ath`). A stolen bound token presented as a Bearer token gets nothing.
async fn enforce_dpop_binding(
    state: &OidcState,
    realm_id: &str,
    claims: &Value,
    headers: &HeaderMap,
    (method, htu): (&str, &str),
    raw_token: &str,
    dpop_scheme: bool,
) -> Result<()> {
    let Some(bound) = claims.pointer("/cnf/jkt").and_then(Value::as_str) else {
        // RFC 9449 section 7.1: the DPoP scheme carries a DPoP-bound token only.
        if dpop_scheme {
            return Err(KaisatsuError::Unauthorized(
                "the DPoP scheme needs a DPoP-bound token".into(),
            ));
        }
        return Ok(());
    };
    if !dpop_scheme {
        return Err(KaisatsuError::Unauthorized(
            "a DPoP-bound token needs the DPoP scheme".into(),
        ));
    }
    let proof_hdr = headers
        .get("dpop")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| KaisatsuError::Unauthorized("DPoP proof missing".into()))?;
    let proof = verify_dpop_proof(proof_hdr.trim(), method, htu, Some(raw_token), None)
        .map_err(|_| KaisatsuError::Unauthorized("invalid DPoP proof".into()))?;
    if proof.jkt != bound {
        return Err(KaisatsuError::Unauthorized(
            "DPoP proof key does not match the token".into(),
        ));
    }
    state
        .store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&format!("dpop:{}", proof.jti)),
            realm_id: realm_id.to_owned(),
            expires_at: Utc::now() + Duration::minutes(10),
        })
        .await
        .map_err(|_| KaisatsuError::Unauthorized("replayed DPoP proof".into()))
}

/// UserInfo endpoint (`GET/POST /realms/{realm}/protocol/openid-connect/userinfo`, W1).
async fn userinfo_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    method: axum::http::Method,
    OriginalUri(orig_uri): OriginalUri,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Map<String, Value>>> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let authz = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let (dpop_scheme, raw_token) = match authz.strip_prefix("DPoP ") {
        Some(t) => (true, t),
        None => (false, authz.strip_prefix("Bearer ").unwrap_or_default()),
    };
    let raw_token = Some(raw_token.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::Unauthorized("missing bearer token".into()))?;

    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    let (_, claims): (_, Value) = verify_jwt(raw_token, &keys, Some(&issuer), None)?;
    if claims.get("typ").and_then(Value::as_str) != Some("Bearer") {
        return Err(KaisatsuError::Unauthorized("not an access token".into()));
    }
    let htu = format!(
        "{}://{}{}{}",
        mount.scheme,
        mount.host,
        mount.path_prefix,
        orig_uri.path()
    );
    enforce_dpop_binding(
        &state,
        &realm.id,
        &claims,
        &headers,
        (method.as_str(), &htu),
        raw_token,
        dpop_scheme,
    )
    .await?;
    // A revoked token, or one whose SSO session ended (logout, code replay, refresh reuse),
    // gets no claims, as at introspection.
    if let Some(jti) = claims.get("jti").and_then(Value::as_str)
        && state
            .store
            .is_token_revoked(&realm.id, &sha256_hex(jti))
            .await?
    {
        return Err(KaisatsuError::Unauthorized("token is not active".into()));
    }
    if let Some(sid) = claims.get("sid").and_then(Value::as_str)
        && state.store.get_sso_session(&realm.id, sid).await?.is_none()
    {
        return Err(KaisatsuError::Unauthorized("token is not active".into()));
    }
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::Unauthorized("missing sub".into()))?;
    let azp = claims
        .get("azp")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let scope_str = claims
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("openid");

    let user = state
        .store
        .get_user_by_id(&realm.id, None, sub)
        .await?
        .filter(|u| u.enabled)
        .ok_or_else(|| KaisatsuError::Unauthorized("user not found".into()))?;
    let client = state
        .store
        .get_client_by_client_id(&realm.id, azp)
        .await?
        .unwrap_or_default();
    let (active_scopes, _) =
        resolve_scopes_for_client(&state.store, &realm, &client, Some(scope_str)).await?;
    let user_groups = state.store.list_user_groups(&realm.id, &user.id).await?;
    let all_roles = state.store.list_roles(&realm.id, None).await?;

    let ctx = MapperContext {
        realm: &realm,
        issuer: &issuer,
        client: &client,
        user: &user,
        user_groups: &user_groups,
        all_realm_roles: &all_roles,
        active_scopes: &active_scopes,
        granted_scope_string: scope_str,
        acr: claims.get("acr").and_then(Value::as_str).unwrap_or("pwd"),
        amr: &[],
        session_id: claims.get("sid").and_then(Value::as_str),
        nonce: None,
        jti: "",
        iat: 0,
        exp: 0,
        auth_time: None,
        dpop_jkt: None,
        act: None,
        active_org: user.org_id.as_deref(),
    };
    let info = evaluate_mappers(&ctx, TokenKind::UserInfo)?;
    Ok(Json(info))
}

/// Sends back-channel logout tokens (`typ == "Logout"`, `events: {"http://schemas.openid.net/event/backchannel-logout": {}}`,
/// no `nonce`) to every client in `sso_session.clients` that configures `backchannel.logout.url` (W1, W8).
pub async fn dispatch_backchannel_logout(
    state: &OidcState,
    mount: &Mount,
    realm: &RealmConfig,
    session: &SsoSession,
) -> Result<()> {
    let keys = state.store.list_signing_keys(&realm.id).await?;
    let issuer = mount.realm_issuer(&realm.realm);
    let now = Utc::now().timestamp();

    for client_id in session.clients.keys() {
        let Some(client) = state
            .store
            .get_client_by_client_id(&realm.id, client_id)
            .await?
        else {
            continue;
        };
        let Some(logout_url) = client
            .attributes
            .get("backchannel.logout.url")
            .filter(|u| !u.is_empty())
        else {
            continue;
        };
        let key = select_signing_key(&keys, realm, &client, true)?;
        let logout_claims = json!({
            "iss": issuer,
            "sub": session.user_id,
            "aud": client.client_id,
            "iat": now,
            "exp": now + 120,
            "jti": uuid::Uuid::now_v7().to_string(),
            "sid": session.id,
            "typ": "Logout",
            "events": {
                "http://schemas.openid.net/event/backchannel-logout": {}
            }
        });
        let logout_token = sign_jwt(key, &state.kek, Some("Logout"), &logout_claims)?;
        let _ = state
            .guard
            .post_form(logout_url, &[("logout_token", &logout_token)])
            .await;
    }
    Ok(())
}

/// RP-Initiated Logout (`GET/POST /realms/{realm}/protocol/openid-connect/logout`, W1):
/// Validates `id_token_hint` and `post_logout_redirect_uri`, ends the SSO session, dispatches
/// back-channel logout, and clears the SSO cookie.
async fn logout_endpoint(
    State(state): State<OidcState>,
    Path(realm_name): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let realm = state
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    let mount = state.mount_from(mount_ext.as_ref().map(|e| &e.0));

    let id_token_hint = query.get("id_token_hint");
    let post_logout = query.get("post_logout_redirect_uri");
    let client_id_param = query.get("client_id");

    let mut session_to_end: Option<SsoSession> = None;
    let mut hint_client_id: Option<String> = None;

    if let Some(hint) = id_token_hint {
        let keys = state.store.list_signing_keys(&realm.id).await?;
        let issuer = mount.realm_issuer(&realm.realm);
        let (_, claims): (_, Value) = verify_jwt(hint, &keys, Some(&issuer), None)
            .map_err(|_| KaisatsuError::BadRequest("invalid id_token_hint".into()))?;
        hint_client_id = claims.get("azp").and_then(Value::as_str).map(str::to_owned);
        if let Some(sid) = claims.get("sid").and_then(Value::as_str) {
            session_to_end = state.store.get_sso_session(&realm.id, sid).await?;
        } else if let Some(sub) = claims.get("sub").and_then(Value::as_str) {
            session_to_end = state
                .store
                .list_user_sso_sessions(&realm.id, sub)
                .await?
                .into_iter()
                .next();
        }
    }

    if session_to_end.is_none()
        && let Some(handle) = read_sso_cookie(&headers, &mount)
    {
        session_to_end = state
            .store
            .get_sso_session_by_handle(&realm.id, &sha256_hex(&handle))
            .await?;
    }

    // Validate `post_logout_redirect_uri` against the client's registered URIs (W1, W12)
    if let Some(target_uri) = post_logout {
        let cid = client_id_param.or(hint_client_id.as_ref()).ok_or_else(|| {
            KaisatsuError::BadRequest(
                "post_logout_redirect_uri requires id_token_hint or client_id".into(),
            )
        })?;
        let client = state
            .store
            .get_client_by_client_id(&realm.id, cid)
            .await?
            .ok_or_else(|| KaisatsuError::BadRequest("unknown client_id".into()))?;
        if !validate_post_logout_redirect_uri(&client, target_uri) {
            return Err(KaisatsuError::BadRequest(
                "Invalid post_logout_redirect_uri".into(),
            ));
        }
    }

    if let Some(sso) = session_to_end {
        let _ = state.store.delete_sso_session(&realm.id, &sso.id).await?;
        let _ = dispatch_backchannel_logout(&state, &mount, &realm, &sso).await;
    }

    let cookie_name = mount.cookie_name("sso");
    let cookie_path = mount.realm_cookie_path(&realm.realm);
    let clear_cookie = if mount.path_prefix.is_empty() {
        format!("{cookie_name}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
    } else {
        format!("{cookie_name}=; Path={cookie_path}; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
    };

    if let Some(target_uri) = post_logout {
        let mut url = url::Url::parse(target_uri)
            .map_err(|_| KaisatsuError::BadRequest("invalid post_logout_redirect_uri".into()))?;
        if let Some(st) = query.get("state") {
            url.query_pairs_mut().append_pair("state", st);
        }
        return Ok((
            StatusCode::SEE_OTHER,
            [
                (header::SET_COOKIE, clear_cookie),
                (header::LOCATION, url.to_string()),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            "",
        )
            .into_response());
    }

    Ok((
        StatusCode::OK,
        [
            (header::SET_COOKIE, clear_cookie),
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
        ],
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Signed out</title></head><body><h1>Signed out</h1></body></html>",
    )
        .into_response())
}
