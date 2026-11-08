#![forbid(unsafe_code)]
//! Keycloak Admin REST compatibility layer (`/admin/realms/{realm}/...`) and registry-driven
//! native REST endpoints (`/api/v1/realms/{realm}/...`), W1, W5, W6.
//!
//! Every write decision goes through [`kaisatsu_core::Authorizer::check`] and leaves an
//! [`kaisatsu_core::AdminEvent`] row without secrets (W6). Cross-organization calls receive 404,
//! never 403.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use chrono::Utc;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::{random_handle_256, sha256_hex, verify_jwt};
use kaisatsu_oidc::OidcState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminState {
    pub oidc: OidcState,
    pub authorizer: Arc<dyn Authorizer>,
    pub registry: Arc<OperationRegistry>,
    pub dns_resolver: Arc<dyn crate::domain::DnsResolver>,
}

impl AdminState {
    #[must_use]
    pub fn new(oidc: OidcState, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            oidc,
            authorizer,
            registry: Arc::new(OperationRegistry::new()),
            dns_resolver: Arc::new(crate::domain::HickoryDnsResolver::new()),
        }
    }

    #[must_use]
    pub fn with_dns_resolver(mut self, dns_resolver: Arc<dyn crate::domain::DnsResolver>) -> Self {
        self.dns_resolver = dns_resolver;
        self
    }
}

pub fn admin_router(state: AdminState) -> Router {
    Router::new()
        // Realms & Partial Import
        .route("/admin/realms", post(compat_create_realm))
        .route(
            "/admin/realms/{realm}",
            get(compat_get_realm).put(compat_put_realm),
        )
        .route(
            "/admin/realms/{realm}/partialImport",
            post(compat_partial_import),
        )
        // Users
        .route(
            "/admin/realms/{realm}/users",
            get(compat_list_users).post(compat_create_user),
        )
        .route(
            "/admin/realms/{realm}/users/{id}",
            get(compat_get_user)
                .put(compat_update_user)
                .delete(compat_delete_user),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/execute-actions-email",
            put(compat_execute_actions_email),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/reset-password",
            put(compat_reset_password),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/logout",
            post(compat_logout_user),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/credentials",
            get(compat_list_user_credentials),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/credentials/{cred_id}",
            delete(compat_delete_user_credential),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/sessions",
            get(compat_list_user_sessions),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/consents",
            get(compat_list_user_consents),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/consents/{client_id}",
            delete(compat_revoke_user_consent),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/groups",
            get(compat_list_user_groups),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/groups/{group_id}",
            put(compat_add_user_to_group).delete(compat_remove_user_from_group),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/role-mappings/realm/composite",
            get(compat_get_user_composite_realm_roles),
        )
        .route(
            "/admin/realms/{realm}/users/{id}/role-mappings/clients/{client_uuid}",
            post(compat_add_user_client_roles).delete(compat_delete_user_client_roles),
        )
        // Groups
        .route(
            "/admin/realms/{realm}/groups",
            get(compat_list_groups).post(compat_create_group),
        )
        .route(
            "/admin/realms/{realm}/groups/{id}",
            get(compat_get_group)
                .put(compat_update_group)
                .delete(compat_delete_group),
        )
        .route(
            "/admin/realms/{realm}/groups/{id}/members",
            get(compat_list_group_members),
        )
        .route(
            "/admin/realms/{realm}/groups/{id}/role-mappings/clients/{client_uuid}",
            post(compat_add_group_client_roles).delete(compat_delete_group_client_roles),
        )
        // Clients, Roles, Scopes, Mappers, Secrets
        .route(
            "/admin/realms/{realm}/clients",
            get(compat_list_clients).post(compat_create_client),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}",
            get(compat_get_client)
                .put(compat_update_client)
                .delete(compat_delete_client),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/client-secret",
            get(compat_get_client_secret).post(compat_regenerate_client_secret),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/service-account-user",
            get(compat_get_service_account_user),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/roles",
            get(compat_list_client_roles).post(compat_create_client_role),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/roles/{role}",
            delete(compat_delete_client_role),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/roles/{role}/groups",
            get(compat_list_client_role_groups),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/roles/{role}/users",
            get(compat_list_client_role_users),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/protocol-mappers/models",
            get(compat_list_client_mappers).post(compat_create_client_mapper),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/protocol-mappers/models/{id}",
            put(compat_update_client_mapper).delete(compat_delete_client_mapper),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/default-client-scopes",
            get(compat_list_client_default_scopes),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/default-client-scopes/{scope_id}",
            put(compat_link_client_default_scope).delete(compat_unlink_client_default_scope),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/optional-client-scopes",
            get(compat_list_client_optional_scopes),
        )
        .route(
            "/admin/realms/{realm}/clients/{uuid}/optional-client-scopes/{scope_id}",
            put(compat_link_client_optional_scope).delete(compat_unlink_client_optional_scope),
        )
        .route(
            "/admin/realms/{realm}/client-scopes",
            get(compat_list_client_scopes).post(compat_create_client_scope),
        )
        .route(
            "/admin/realms/{realm}/client-scopes/{id}",
            get(compat_get_client_scope)
                .put(compat_update_client_scope)
                .delete(compat_delete_client_scope),
        )
        .route(
            "/admin/realms/{realm}/attack-detection/brute-force/users/{user_id}",
            delete(compat_clear_brute_force),
        )
        // Native registry-driven REST endpoints (`/api/v1/realms/{realm}/...`, W5)
        .route(
            "/api/v1/realms/{realm}/operations",
            get(native_list_operations),
        )
        .route("/api/v1/realms/{realm}/allowed", get(native_check_allowed))
        .route(
            "/api/v1/realms/{realm}/keys/rotate",
            post(native_rotate_keys),
        )
        .with_state(state)
}

#[derive(Clone, Debug)]
pub struct ReqInfo {
    pub method: String,
    pub path: String,
}

async fn attach_req_info(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let info = ReqInfo {
        method: req.method().as_str().to_owned(),
        path: req.uri().path().to_owned(),
    };
    let mut headers = req.headers().clone();
    if let Ok(m) = header::HeaderValue::from_str(&info.method) {
        headers.insert(header::HeaderName::from_static("x-kaisatsu-req-method"), m);
    }
    if let Ok(p) = header::HeaderValue::from_str(&info.path) {
        headers.insert(header::HeaderName::from_static("x-kaisatsu-req-path"), p);
    }
    let (mut parts, body) = req.into_parts();
    parts.headers = headers;
    parts.extensions.insert(info);
    let mut resp = next
        .run(axum::extract::Request::from_parts(parts, body))
        .await;
    if let Some(nonce) = resp.extensions().get::<String>().cloned()
        && let Ok(val) = header::HeaderValue::from_str(&nonce)
    {
        resp.headers_mut()
            .insert(header::HeaderName::from_static("dpop-nonce"), val);
    }
    resp
}

pub fn with_dpop_req_info(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(attach_req_info))
}

/// Extracts and verifies the admin token (`Bearer` or `DPoP`, either from the target realm or
/// `master`), enforces DPoP proof binding + server nonces when `cnf.jkt` or `DPoP` scheme is
/// present (RFC 9449, K6), and constructs the [`RequestCtx`] (W1, W6).
pub async fn extract_admin_ctx(
    state: &AdminState,
    mount: &Mount,
    realm_id: &str,
    headers: &HeaderMap,
) -> Result<RequestCtx> {
    let (dpop_scheme, raw_token) = kaisatsu_oidc::parse_authz_header(headers)?;

    let mut verified: Option<(Value, &str)> = None;
    for candidate_realm in [realm_id, "master"] {
        if let Ok(keys) = state.oidc.store.list_signing_keys(candidate_realm).await {
            let issuer = mount.realm_issuer(candidate_realm);
            if let Ok((_, claims)) = verify_jwt::<Value>(raw_token, &keys, Some(&issuer), None)
                && claims.get("typ").and_then(Value::as_str) == Some("Bearer")
            {
                verified = Some((claims, candidate_realm));
                break;
            }
        }
    }

    let (claims, token_realm) =
        verified.ok_or_else(|| KaisatsuError::Unauthorized("invalid admin bearer token".into()))?;

    // K6: Enforce DPoP binding across Admin Compat, Native Admin, OData, MCP, and SCIM.
    if dpop_scheme || claims.pointer("/cnf/jkt").is_some() {
        let method = headers
            .get("x-kaisatsu-req-method")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("GET");
        let path = headers
            .get("x-kaisatsu-req-path")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let htu = kaisatsu_oidc::external_htu(mount, path);
        let _ = kaisatsu_oidc::verify_resource_dpop(
            &state.oidc,
            token_realm,
            &claims,
            headers,
            method,
            &htu,
            raw_token,
            dpop_scheme,
        )
        .await?;
    }
    if let Some(jti) = claims.get("jti").and_then(Value::as_str)
        && state
            .oidc
            .store
            .is_token_revoked(token_realm, &sha256_hex(jti))
            .await?
    {
        return Err(KaisatsuError::Unauthorized(
            "invalid admin bearer token".into(),
        ));
    }
    // A token from `master` acting on another realm is Keycloak's cross-realm admin: only the
    // master `admin` role and the `{realm}-realm` client roles carry over (as `realm-management`).
    let cross_realm = token_realm != realm_id;

    let sub = claims.get("sub").and_then(Value::as_str).map(str::to_owned);
    let azp = claims.get("azp").and_then(Value::as_str).map(str::to_owned);

    let mut realm_roles: Vec<String> = claims
        .pointer("/realm_access/roles")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let mut client_roles: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(ra) = claims.get("resource_access").and_then(Value::as_object) {
        for (cid, obj) in ra {
            if let Some(arr) = obj.get("roles").and_then(Value::as_array) {
                client_roles.insert(
                    cid.clone(),
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                );
            }
        }
    }
    // Store-side role mappings come only from the realm that issued the token (never a same-id
    // person of another realm).
    if let Some(uid) = &sub
        && let Ok(Some(u)) = state
            .oidc
            .store
            .get_user_by_id(token_realm, None, uid)
            .await
    {
        for r in u.realm_roles {
            if !realm_roles.contains(&r) {
                realm_roles.push(r);
            }
        }
        for (cid, rlist) in u.client_roles {
            let entry = client_roles.entry(cid).or_default();
            for r in rlist {
                if !entry.contains(&r) {
                    entry.push(r);
                }
            }
        }
    }
    if token_realm == "master" && realm_roles.iter().any(|r| r == "admin") {
        realm_roles.push("realm-admin".into());
    }
    if cross_realm {
        realm_roles.retain(|r| r == "realm-admin");
        let per_realm = client_roles
            .remove(&format!("{realm_id}-realm"))
            .unwrap_or_default();
        client_roles.clear();
        if !per_realm.is_empty() {
            client_roles.insert("realm-management".into(), per_realm);
        }
    }

    let mut organizations: Vec<String> = claims
        .get("jc_org")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let mut org_roles: HashMap<String, Vec<String>> = HashMap::new();

    // Organization memberships of the person in this realm (a cross-realm admin has none here).
    if let Some(uid) = &sub
        && !cross_realm
    {
        if let Ok(Some(u)) = state.oidc.store.get_user_by_id(realm_id, None, uid).await
            && let Some(u_org) = u.org_id
            && !organizations.contains(&u_org)
        {
            organizations.push(u_org);
        }
        if let Ok(mships) = state
            .oidc
            .store
            .list_memberships_for_person(realm_id, uid)
            .await
        {
            for m in mships {
                if !organizations.contains(&m.org_id) {
                    organizations.push(m.org_id.clone());
                }
                org_roles.insert(m.org_id, m.org_roles);
            }
        }
    }
    if cross_realm {
        organizations.clear();
    }

    let acr = claims.get("acr").and_then(Value::as_str);
    let second_factor = acr == Some("mfa")
        || claims
            .get("amr")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("otp")));

    let mut ctx = RequestCtx::new(
        realm_id,
        Principal {
            subject: sub,
            client_id: azp,
            realm_roles,
            client_roles,
            organizations: organizations.clone(),
            org_roles,
            second_factor,
        },
    );

    if let Some(org_hdr) = headers.get("x-kaisatsu-org").and_then(|v| v.to_str().ok()) {
        ctx = ctx.with_org(org_hdr.trim());
    } else if organizations.len() == 1 && !ctx.principal.is_realm_admin() {
        ctx = ctx.with_org(organizations[0].clone());
    }

    Ok(ctx)
}

/// Role mappings of a client may be granted or removed only by someone who may write that client:
/// a realm admin, or (for an ordinary application) whoever the authorizer lets update it. The
/// admin clients (`realm-management`, `broker`, `{realm}-realm`) are realm-admin only, so neither
/// an organization admin nor a client manager can hand out admin roles (W6).
pub(crate) async fn ensure_may_map_client_roles(
    state: &AdminState,
    ctx: &RequestCtx,
    realm: &str,
    client_uuid: &str,
) -> Result<()> {
    if ctx.principal.is_realm_admin() {
        return Ok(());
    }
    let client = state
        .oidc
        .store
        .get_client_by_uuid(realm, None, client_uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    if matches!(client.client_id.as_str(), "realm-management" | "broker")
        || client.client_id.ends_with("-realm")
    {
        return Err(KaisatsuError::Forbidden(
            "admin client roles are granted by a realm admin only".into(),
        ));
    }
    check_decision(state.authorizer.check(
        ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.to_owned(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.attributes.get("managed-by").cloned(),
        },
    ))
}

/// The person a PUT/PATCH stores: Keycloak's update ignores role mappings and group membership
/// (they have their own routes, each authorized there); the service-account binding and creation
/// facts never change; only a realm admin moves a person to another organization.
pub(crate) fn merge_user_update(
    existing: &UserRepresentation,
    mut user: UserRepresentation,
    ctx: &RequestCtx,
) -> UserRepresentation {
    user.id = existing.id.clone();
    user.realm_id = existing.realm_id.clone();
    user.realm_roles = existing.realm_roles.clone();
    user.client_roles = existing.client_roles.clone();
    user.groups = existing.groups.clone();
    user.service_account_client_id = existing.service_account_client_id.clone();
    user.created_timestamp = existing.created_timestamp;
    user.transient = existing.transient;
    user.managed_by = existing.managed_by.clone();
    if user.org_id.is_none() || !ctx.principal.is_realm_admin() {
        user.org_id = existing.org_id.clone();
    }
    user
}

/// The Authorizer's decision on an application (client) and its sub-resources: mappers, scope
/// links, role holders.
fn check_application(
    state: &AdminState,
    ctx: &RequestCtx,
    client: &ClientRepresentation,
    action: Action,
) -> Result<()> {
    check_decision(state.authorizer.check(
        ctx,
        action,
        &ObjectTarget::Application {
            realm_id: ctx.realm_id.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))
}

pub(crate) fn check_decision(dec: Decision) -> Result<()> {
    if dec.allowed {
        Ok(())
    } else if !dec.visible {
        Err(KaisatsuError::NotFound("resource not found".into()))
    } else {
        Err(KaisatsuError::Forbidden(dec.reason))
    }
}

pub(crate) async fn record_admin_write(
    state: &AdminState,
    ctx: &RequestCtx,
    operation_type: &str,
    resource_type: &str,
    resource_path: &str,
    representation: Option<Value>,
) -> Result<()> {
    let mut sanitized = representation;
    if let Some(Value::Object(ref mut map)) = sanitized {
        map.remove("secret");
        map.remove("password");
        map.remove("value");
        map.remove("credentials");
    }
    state
        .oidc
        .store
        .append_admin_event(AdminEvent {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: ctx.realm_id.clone(),
            org_id: ctx.active_org_id.clone(),
            time: Utc::now().timestamp_millis(),
            operation_type: operation_type.to_owned(),
            resource_type: resource_type.to_owned(),
            resource_path: resource_path.to_owned(),
            actor_id: ctx.principal.subject.clone().unwrap_or_default(),
            actor_client_id: ctx.principal.client_id.clone(),
            representation: sanitized,
        })
        .await
}

async fn compat_create_realm(
    State(state): State<AdminState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(raw): Json<Value>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let mut realm: RealmConfig = serde_json::from_value(raw.clone())
        .map_err(|e| KaisatsuError::BadRequest(format!("invalid realm: {e}")))?;
    // keycloak-config-cli posts `{"realm": "x", ...}` without an id: the id is the realm name,
    // never the type's default id.
    if raw
        .get("realm")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(KaisatsuError::BadRequest("realm is required".into()));
    }
    if raw
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        realm.id = realm.realm.clone();
    }
    let ctx = extract_admin_ctx(&state, &mount, &realm.id, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Apply,
        &ObjectTarget::Realm {
            realm_id: realm.id.clone(),
        },
    ))?;
    if state.oidc.store.get_realm(&realm.realm).await?.is_some()
        || state.oidc.store.get_realm(&realm.id).await?.is_some()
    {
        return Err(KaisatsuError::Conflict(format!(
            "Conflict detected. See logs for details: realm {} exists",
            realm.realm
        )));
    }
    state.oidc.store.upsert_realm(realm.clone()).await?;
    record_admin_write(&state, &ctx, "CREATE", "REALM", &realm.realm, None).await?;
    Ok(StatusCode::CREATED)
}

async fn compat_get_realm(
    State(state): State<AdminState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<RealmConfig>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm_name, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Realm {
            realm_id: realm_name.clone(),
        },
    ))?;
    let realm = state
        .oidc
        .store
        .get_realm(&realm_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))?;
    Ok(Json(realm))
}

async fn compat_put_realm(
    State(state): State<AdminState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut realm): Json<RealmConfig>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm_name, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Apply,
        &ObjectTarget::Realm {
            realm_id: realm_name.clone(),
        },
    ))?;
    realm.id = realm_name.clone();
    realm.realm = realm_name.clone();
    state.oidc.store.upsert_realm(realm).await?;
    record_admin_write(&state, &ctx, "UPDATE", "REALM", &realm_name, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialImportPayload {
    #[serde(default = "default_if_exists")]
    if_resource_exists: String,
    #[serde(default)]
    users: Vec<UserRepresentation>,
    #[serde(default)]
    groups: Vec<GroupRepresentation>,
    #[serde(default)]
    clients: Vec<ClientRepresentation>,
}

fn default_if_exists() -> String {
    "FAIL".into()
}

/// `POST /admin/realms/{realm}/partialImport` (used by `charts/people/files/ensure-user-ids.sh`
/// with `ifResourceExists`: `FAIL` / `SKIP` / `OVERWRITE` to preserve PF-126 fixed user IDs, W1, W1b).
async fn compat_partial_import(
    State(state): State<AdminState>,
    Path(realm_name): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(payload): Json<PartialImportPayload>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm_name, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Apply,
        &ObjectTarget::Realm {
            realm_id: realm_name.clone(),
        },
    ))?;

    let mode = payload.if_resource_exists.to_ascii_uppercase();
    if !matches!(mode.as_str(), "FAIL" | "SKIP" | "OVERWRITE") {
        return Err(KaisatsuError::BadRequest(format!(
            "invalid ifResourceExists: {mode}"
        )));
    }
    // FAIL is all or nothing, as in Keycloak: a conflict is found before anything is written.
    if mode == "FAIL" {
        for u in &payload.users {
            let by_id = if u.id.is_empty() {
                None
            } else {
                state
                    .oidc
                    .store
                    .get_user_by_id(&realm_name, None, &u.id)
                    .await?
            };
            if by_id.is_some()
                || state
                    .oidc
                    .store
                    .get_user_by_username_or_email(&realm_name, None, &u.username)
                    .await?
                    .is_some()
            {
                return Err(KaisatsuError::Conflict(format!(
                    "partialImport FAIL: user {} already exists",
                    u.username
                )));
            }
        }
    }
    let mut added = 0u32;
    let mut overwritten = 0u32;
    let mut skipped = 0u32;

    for mut u in payload.users {
        u.realm_id = realm_name.clone();
        if u.org_id.is_none() {
            u.org_id = ctx.active_org_id.clone();
        }
        let existing_by_name = state
            .oidc
            .store
            .get_user_by_username_or_email(&realm_name, None, &u.username)
            .await?;
        let existing_by_id = if !u.id.is_empty() {
            state
                .oidc
                .store
                .get_user_by_id(&realm_name, None, &u.id)
                .await?
        } else {
            None
        };
        let existing = existing_by_id.or(existing_by_name);
        match existing {
            Some(old) if mode == "OVERWRITE" => {
                let _ = state
                    .oidc
                    .store
                    .delete_user(&realm_name, None, &old.id)
                    .await?;
                state.oidc.store.create_user(u).await?;
                overwritten += 1;
            }
            // SKIP (FAIL found its conflicts before writing).
            Some(_) => skipped += 1,
            None => {
                state.oidc.store.create_user(u).await?;
                added += 1;
            }
        }
    }

    for mut g in payload.groups {
        g.realm_id = realm_name.clone();
        state.oidc.store.upsert_group(g).await?;
        added += 1;
    }

    for mut c in payload.clients {
        c.realm_id = realm_name.clone();
        state.oidc.store.upsert_client(c).await?;
        added += 1;
    }

    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "REALM",
        &format!("{realm_name}/partialImport"),
        Some(json!({ "added": added, "overwritten": overwritten, "skipped": skipped })),
    )
    .await?;

    Ok(Json(json!({
        "added": added,
        "overwritten": overwritten,
        "skipped": skipped,
        "results": []
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub exact: Option<bool>,
    #[serde(default)]
    pub first: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
}

async fn compat_list_users(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<UserListQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<UserRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            person_id: "*".into(),
        },
    ))?;
    let allowed = ctx.allowed_orgs();
    let (first, max) = (q.first.unwrap_or(0), q.max.unwrap_or(100));
    let page = |first, max| {
        state.oidc.store.list_users(
            &realm,
            allowed.as_deref(),
            q.search.as_deref(),
            q.email.as_deref(),
            q.username.as_deref(),
            q.exact.unwrap_or(false),
            first,
            max,
        )
    };
    // Keycloak's `search` leaves service accounts out (UsersResource includeServiceAccounts
    // false), so the Portal's People page never lists `service-account-*`; `first`/`max` count
    // people only, so the store is paged until `max` of them are found.
    if q.search.is_none() || max == 0 {
        return Ok(Json(page(first, max).await?));
    }
    let batch = max.max(100);
    let (mut users, mut skip, mut offset) = (Vec::new(), first, 0);
    loop {
        let read = page(offset, batch).await?;
        let full = read.len() == batch;
        offset += batch;
        for user in read {
            if user.service_account_client_id.is_some() {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            users.push(user);
            if users.len() == max {
                return Ok(Json(users));
            }
        }
        if !full {
            return Ok(Json(users));
        }
    }
}

async fn compat_create_user(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut user): Json<UserRepresentation>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    user.realm_id = realm.clone();
    if user.org_id.is_none() {
        user.org_id = ctx.active_org_id.clone();
    }
    check_decision(state.authorizer.check(
        &ctx,
        Action::Create,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: "*".into(),
        },
    ))?;
    // As Keycloak's POST /users: no role mapping and no service-account link comes from the
    // body (an organization admin could otherwise create a realm admin); the person holds the
    // realm's default roles, `offline_access` among them (T-6301).
    let default_roles = state
        .oidc
        .store
        .get_realm(&realm)
        .await?
        .map(|r| r.default_roles)
        .unwrap_or_default();
    user.realm_roles = default_roles;
    user.client_roles.clear();
    user.groups.clear();
    user.service_account_client_id = None;
    if user.id.is_empty() || !ctx.principal.is_realm_admin() {
        user.id = uuid::Uuid::now_v7().to_string();
    }
    let created = state.oidc.store.create_user(user).await?;
    let location = format!(
        "{}/admin/realms/{realm}/users/{}",
        mount.public_base(),
        created.id
    );
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "USER",
        &format!("users/{}", created.id),
        serde_json::to_value(&created).ok(),
    )
    .await?;
    Ok((StatusCode::CREATED, [(header::LOCATION, location)], "").into_response())
}

async fn compat_get_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<UserRepresentation>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm,
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    Ok(Json(user))
}

async fn compat_update_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut user): Json<UserRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            person_id: existing.id.clone(),
        },
    ))?;
    user.id = id.clone();
    user.realm_id = realm;
    let user = merge_user_update(&existing, user, &ctx);
    let updated = state.oidc.store.update_user(user).await?;
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "USER",
        &format!("users/{id}"),
        serde_json::to_value(&updated).ok(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            person_id: existing.id.clone(),
        },
    ))?;
    state
        .oidc
        .store
        .delete_user(&realm, allowed.as_deref(), &id)
        .await?;
    record_admin_write(&state, &ctx, "DELETE", "USER", &format!("users/{id}"), None).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_execute_actions_email(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(actions): Json<Vec<String>>,
) -> Result<StatusCode> {
    let lifespan = q.get("lifespan").and_then(|s| s.parse::<i64>().ok());
    run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "send_required_actions",
        Some(&id),
        &json!({
            "actions": actions,
            "client_id": q.get("client_id"),
            "redirect_uri": q.get("redirect_uri"),
            "lifespan": lifespan,
        }),
    )
    .await
    .map(|_| StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct ResetPasswordPayload {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub temporary: bool,
}

async fn compat_reset_password(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(payload): Json<ResetPasswordPayload>,
) -> Result<StatusCode> {
    run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "credential_reset",
        Some(&id),
        &json!({ "value": payload.value, "temporary": payload.temporary }),
    )
    .await
    .map(|_| StatusCode::NO_CONTENT)
}

async fn compat_logout_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "session_revoke",
        Some(&id),
        &Value::Null,
    )
    .await
    .map(|_| StatusCode::NO_CONTENT)
}

async fn compat_list_user_credentials(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<CredentialMetadata>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let creds = state.oidc.store.list_credentials(&realm, &user.id).await?;
    Ok(Json(
        creds
            .iter()
            .filter(|c| c.credential_type != "password-history")
            .map(CredentialMetadata::from)
            .collect(),
    ))
}

async fn compat_delete_user_credential(
    State(state): State<AdminState>,
    Path((realm, id, cred_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let deleted = state
        .oidc
        .store
        .delete_credential(&realm, &user.id, &cred_id)
        .await?;
    if !deleted {
        return Err(KaisatsuError::NotFound("credential not found".into()));
    }
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CREDENTIAL",
        &format!("users/{id}/credentials/{cred_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_list_user_sessions(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<SsoSession>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Session {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let sessions = state
        .oidc
        .store
        .list_user_sso_sessions(&realm, &user.id)
        .await?;
    Ok(Json(sessions))
}

async fn compat_list_user_consents(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ConsentRecord>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let consents = state.oidc.store.list_consents(&realm, &user.id).await?;
    Ok(Json(consents))
}

async fn compat_revoke_user_consent(
    State(state): State<AdminState>,
    Path((realm, id, client_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    state
        .oidc
        .store
        .delete_consent(&realm, &user.id, &client_id)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CONSENT",
        &format!("users/{id}/consents/{client_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_list_user_groups(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<GroupRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let groups = state.oidc.store.list_user_groups(&realm, &user.id).await?;
    Ok(Json(groups))
}

async fn compat_add_user_to_group(
    State(state): State<AdminState>,
    Path((realm, id, group_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "member_add",
        Some(&id),
        &json!({ "groupId": group_id }),
    )
    .await
    .map(|_| StatusCode::NO_CONTENT)
}

async fn compat_remove_user_from_group(
    State(state): State<AdminState>,
    Path((realm, id, group_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &group_id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id,
            group_id: group.id.clone(),
            managed_by: group.managed_by,
        },
    ))?;
    state
        .oidc
        .store
        .remove_user_from_group(&realm, &id, &group_id)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "GROUP_MEMBERSHIP",
        &format!("users/{id}/groups/{group_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_get_user_composite_realm_roles(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<RoleRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id.clone(),
        },
    ))?;
    let user_groups = state.oidc.store.list_user_groups(&realm, &user.id).await?;
    let all_roles = state.oidc.store.list_roles(&realm, None).await?;
    let group_roles: Vec<String> = user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let eff_names = kaisatsu_core::claims::resolve_effective_realm_roles(
        &user.realm_roles,
        &group_roles,
        &all_roles,
    );
    let out: Vec<RoleRepresentation> = eff_names
        .into_iter()
        .map(|name| {
            all_roles
                .iter()
                .find(|r| r.name == name)
                .cloned()
                .unwrap_or_else(|| RoleRepresentation {
                    id: name.clone(),
                    realm_id: realm.clone(),
                    name,
                    ..Default::default()
                })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub exact: Option<bool>,
}

async fn compat_list_groups(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<GroupListQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<GroupRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            group_id: "*".into(),
            managed_by: None,
        },
    ))?;
    let allowed = ctx.allowed_orgs();
    let groups = state
        .oidc
        .store
        .list_groups(
            &realm,
            allowed.as_deref(),
            q.search.as_deref(),
            q.exact.unwrap_or(false),
        )
        .await?;
    Ok(Json(groups))
}

async fn compat_create_group(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut group): Json<GroupRepresentation>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    group.realm_id = realm.clone();
    if group.org_id.is_none() {
        group.org_id = ctx.active_org_id.clone();
    }
    if let Some(vals) = group.attributes.get("managed-by")
        && let Some(first) = vals.first()
    {
        group.managed_by = Some(first.clone());
    }
    check_decision(state.authorizer.check(
        &ctx,
        Action::Create,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id.clone(),
            group_id: "*".into(),
            managed_by: group.managed_by.clone(),
        },
    ))?;
    let created = state.oidc.store.upsert_group(group).await?;
    let location = format!(
        "{}/admin/realms/{realm}/groups/{}",
        mount.public_base(),
        created.id
    );
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "GROUP",
        &format!("groups/{}", created.id),
        serde_json::to_value(&created).ok(),
    )
    .await?;
    Ok((StatusCode::CREATED, [(header::LOCATION, location)], "").into_response())
}

async fn compat_get_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<GroupRepresentation>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Group {
            realm_id: realm,
            org_id: group.org_id.clone(),
            group_id: group.id.clone(),
            managed_by: group.managed_by.clone(),
        },
    ))?;
    Ok(Json(group))
}

async fn compat_update_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut group): Json<GroupRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            group_id: existing.id.clone(),
            managed_by: existing.managed_by.clone(),
        },
    ))?;
    group.id = id.clone();
    group.realm_id = realm;
    if group.path.is_empty() {
        group.path = existing.path;
    }
    // Only a realm admin moves a group to another organization.
    if group.org_id.is_none() || !ctx.principal.is_realm_admin() {
        group.org_id = existing.org_id;
    }
    if let Some(vals) = group.attributes.get("managed-by")
        && let Some(first) = vals.first()
    {
        group.managed_by = Some(first.clone());
    }
    let updated = state.oidc.store.upsert_group(group).await?;
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "GROUP",
        &format!("groups/{id}"),
        serde_json::to_value(&updated).ok(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: existing.org_id,
            group_id: existing.id,
            managed_by: existing.managed_by,
        },
    ))?;
    state
        .oidc
        .store
        .delete_group(&realm, allowed.as_deref(), &id)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "GROUP",
        &format!("groups/{id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
struct MaxQuery {
    #[serde(default)]
    pub max: Option<usize>,
}

async fn compat_list_group_members(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    Query(q): Query<MaxQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<UserRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id,
            group_id: group.id.clone(),
            managed_by: group.managed_by,
        },
    ))?;
    let members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &id, q.max.unwrap_or(1000))
        .await?;
    Ok(Json(members))
}

#[derive(Debug, Deserialize)]
struct RoleMappingItem {
    #[serde(default)]
    pub id: String,
    pub name: String,
}

async fn compat_add_user_client_roles(
    State(state): State<AdminState>,
    Path((realm, id, client_uuid)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(items): Json<Vec<RoleMappingItem>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    ensure_may_map_client_roles(&state, &ctx, &realm, &client_uuid).await?;
    for item in items {
        state
            .oidc
            .store
            .add_role_mapping(RoleMappingRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.clone(),
                org_id: user.org_id.clone(),
                principal_kind: "user".into(),
                principal_id: user.id.clone(),
                role_id: if item.id.is_empty() {
                    item.name.clone()
                } else {
                    item.id
                },
                role_name: item.name,
                client_uuid: Some(client_uuid.clone()),
            })
            .await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_ROLE_MAPPING",
        &format!("users/{id}/role-mappings/clients/{client_uuid}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_user_client_roles(
    State(state): State<AdminState>,
    Path((realm, id, client_uuid)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(items): Json<Vec<RoleMappingItem>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    ensure_may_map_client_roles(&state, &ctx, &realm, &client_uuid).await?;
    for item in items {
        state
            .oidc
            .store
            .remove_role_mapping(&realm, "user", &id, Some(&client_uuid), &item.name)
            .await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_ROLE_MAPPING",
        &format!("users/{id}/role-mappings/clients/{client_uuid}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_add_group_client_roles(
    State(state): State<AdminState>,
    Path((realm, id, client_uuid)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(items): Json<Vec<RoleMappingItem>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id.clone(),
            group_id: group.id.clone(),
            managed_by: group.managed_by,
        },
    ))?;
    ensure_may_map_client_roles(&state, &ctx, &realm, &client_uuid).await?;
    for item in items {
        state
            .oidc
            .store
            .add_role_mapping(RoleMappingRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.clone(),
                org_id: group.org_id.clone(),
                principal_kind: "group".into(),
                principal_id: group.id.clone(),
                role_id: if item.id.is_empty() {
                    item.name.clone()
                } else {
                    item.id
                },
                role_name: item.name,
                client_uuid: Some(client_uuid.clone()),
            })
            .await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_ROLE_MAPPING",
        &format!("groups/{id}/role-mappings/clients/{client_uuid}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_group_client_roles(
    State(state): State<AdminState>,
    Path((realm, id, client_uuid)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(items): Json<Vec<RoleMappingItem>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id.clone(),
            group_id: group.id.clone(),
            managed_by: group.managed_by,
        },
    ))?;
    ensure_may_map_client_roles(&state, &ctx, &realm, &client_uuid).await?;
    for item in items {
        state
            .oidc
            .store
            .remove_role_mapping(&realm, "group", &id, Some(&client_uuid), &item.name)
            .await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_ROLE_MAPPING",
        &format!("groups/{id}/role-mappings/clients/{client_uuid}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientListQuery {
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub search: Option<bool>,
}

async fn compat_list_clients(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<ClientListQuery>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ClientRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            client_id: "*".into(),
            managed_by: Some("joinedcontext".into()),
        },
    ))?;
    let allowed = ctx.allowed_orgs();
    let clients = state
        .oidc
        .store
        .list_clients(
            &realm,
            allowed.as_deref(),
            q.client_id.as_deref(),
            q.search.unwrap_or(false),
        )
        .await?;
    Ok(Json(clients))
}

async fn compat_create_client(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut client): Json<ClientRepresentation>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    client.realm_id = realm.clone();
    if client.org_id.is_none() {
        client.org_id = ctx.active_org_id.clone();
    }
    let managed = client.attributes.get("managed-by").cloned();
    check_decision(state.authorizer.check(
        &ctx,
        Action::Create,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: managed,
        },
    ))?;
    let created = state.oidc.store.upsert_client(client).await?;
    let location = format!(
        "{}/admin/realms/{realm}/clients/{}",
        mount.public_base(),
        created.id
    );
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT",
        &format!("clients/{}", created.id),
        serde_json::to_value(&created).ok(),
    )
    .await?;
    Ok((StatusCode::CREATED, [(header::LOCATION, location)], "").into_response())
}

async fn compat_get_client(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<ClientRepresentation>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Application {
            realm_id: realm,
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))?;
    Ok(Json(client))
}

async fn compat_update_client(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut client): Json<ClientRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            client_id: existing.client_id.clone(),
            managed_by: existing.managed_by.clone(),
        },
    ))?;
    client.id = uuid.clone();
    client.realm_id = realm;
    // Only a realm admin moves an application to another organization.
    if client.org_id.is_none() || !ctx.principal.is_realm_admin() {
        client.org_id = existing.org_id;
    }
    // W1 quirk-free behaviour: preserve existing default/optional scopes when PUT body omits them
    if client.default_client_scopes.is_empty() && !existing.default_client_scopes.is_empty() {
        client.default_client_scopes = existing.default_client_scopes;
    }
    // Keycloak's PUT without `protocolMappers` keeps them (they have their own routes).
    if client.protocol_mappers.is_empty() {
        client.protocol_mappers = existing.protocol_mappers;
    }
    let updated = state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "CLIENT",
        &format!("clients/{uuid}"),
        serde_json::to_value(&updated).ok(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_client(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let existing = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: existing.org_id,
            client_id: existing.client_id,
            managed_by: existing.managed_by,
        },
    ))?;
    state
        .oidc
        .store
        .delete_client(&realm, allowed.as_deref(), &uuid)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT",
        &format!("clients/{uuid}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_get_client_secret(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Application {
            realm_id: realm,
            org_id: client.org_id,
            client_id: client.client_id,
            managed_by: client.managed_by,
        },
    ))?;
    let val = client.secret.unwrap_or_default();
    Ok(Json(json!({ "type": "secret", "value": val })))
}

async fn compat_regenerate_client_secret(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))?;
    let fresh = random_handle_256();
    client.secret = Some(fresh.clone());
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "CLIENT",
        &format!("clients/{uuid}/client-secret"),
        None,
    )
    .await?;
    Ok(Json(json!({ "type": "secret", "value": fresh })))
}

async fn compat_get_service_account_user(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<UserRepresentation>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id,
            client_id: client.client_id.clone(),
            managed_by: client.managed_by,
        },
    ))?;
    let sa_name = format!("service-account-{}", client.client_id);
    let user = state
        .oidc
        .store
        .get_user_by_username_or_email(&realm, None, &sa_name)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("service account user not found".into()))?;
    Ok(Json(user))
}

async fn compat_list_client_roles(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<RoleRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id,
            client_id: client.client_id,
            managed_by: client.managed_by,
        },
    ))?;
    let roles = state.oidc.store.list_roles(&realm, Some(&uuid)).await?;
    Ok(Json(roles))
}

/// K3: writes of a client role pass on the role decision or on the client's own decision (T-2725
/// keeps `view-clients` to `managed-by: joinedcontext` clients); otherwise the role decision's
/// answer (404 when hidden).
fn client_role_write(
    state: &AdminState,
    ctx: &RequestCtx,
    action: Action,
    client: &ClientRepresentation,
    role_name: &str,
) -> Result<()> {
    let on_role = state.authorizer.check(
        ctx,
        action,
        &ObjectTarget::Role {
            realm_id: ctx.realm_id.clone(),
            org_id: client.org_id.clone(),
            role_id: role_name.to_owned(),
        },
    );
    if on_role.allowed {
        return Ok(());
    }
    let on_client = state.authorizer.check(
        ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: ctx.realm_id.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    );
    if on_client.allowed {
        return Ok(());
    }
    check_decision(on_role)
}

async fn compat_create_client_role(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut role): Json<RoleRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    // K3: a client role is written by whoever may write roles, or may write this client: the
    // T-2725 grant lets `portal-api` (`view-clients`) manage its own `managed-by: joinedcontext`
    // App clients and their roles (Keycloak's per-client manage permission), never another's.
    client_role_write(&state, &ctx, Action::Create, &client, &role.name)?;
    role.realm_id = realm.clone();
    role.org_id = client.org_id;
    role.client_role = true;
    role.container_id = Some(uuid.clone());
    let created = state.oidc.store.upsert_role(role).await?;
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_ROLE",
        &format!("clients/{uuid}/roles/{}", created.name),
        None,
    )
    .await?;
    Ok(StatusCode::CREATED)
}

async fn compat_delete_client_role(
    State(state): State<AdminState>,
    Path((realm, uuid, role_name)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    client_role_write(&state, &ctx, Action::Delete, &client, &role_name)?;
    state
        .oidc
        .store
        .delete_role(&realm, Some(&uuid), &role_name)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_ROLE",
        &format!("clients/{uuid}/roles/{role_name}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_list_client_role_groups(
    State(state): State<AdminState>,
    Path((realm, uuid, role_name)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<GroupRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Read)?;
    let group_ids = state
        .oidc
        .store
        .list_principals_with_client_role(&realm, &uuid, &role_name, "group")
        .await?;
    let mut out = Vec::new();
    for gid in group_ids {
        if let Some(g) = state
            .oidc
            .store
            .get_group(&realm, allowed.as_deref(), &gid)
            .await?
        {
            out.push(g);
        }
    }
    Ok(Json(out))
}

async fn compat_list_client_role_users(
    State(state): State<AdminState>,
    Path((realm, uuid, role_name)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<UserRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Read)?;
    let user_ids = state
        .oidc
        .store
        .list_principals_with_client_role(&realm, &uuid, &role_name, "user")
        .await?;
    let mut out = Vec::new();
    for uid in user_ids {
        if let Some(u) = state
            .oidc
            .store
            .get_user_by_id(&realm, allowed.as_deref(), &uid)
            .await?
        {
            out.push(u);
        }
    }
    Ok(Json(out))
}

async fn compat_list_client_mappers(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ProtocolMapperRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Read)?;
    Ok(Json(client.protocol_mappers))
}

async fn compat_create_client_mapper(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut mapper): Json<ProtocolMapperRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    if !kaisatsu_core::claims::is_known_mapper_kind(&mapper.protocol_mapper) {
        return Err(KaisatsuError::BadRequest(format!(
            "unknown protocolMapper kind: {}",
            mapper.protocol_mapper
        )));
    }
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))?;
    if mapper.id.is_empty() {
        mapper.id = uuid::Uuid::now_v7().to_string();
    }
    mapper.realm_id = realm;
    client.protocol_mappers.retain(|m| m.name != mapper.name);
    client.protocol_mappers.push(mapper);
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "PROTOCOL_MAPPER",
        &format!("clients/{uuid}/protocol-mappers/models"),
        None,
    )
    .await?;
    Ok(StatusCode::CREATED)
}

async fn compat_update_client_mapper(
    State(state): State<AdminState>,
    Path((realm, uuid, mapper_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut mapper): Json<ProtocolMapperRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    if !kaisatsu_core::claims::is_known_mapper_kind(&mapper.protocol_mapper) {
        return Err(KaisatsuError::BadRequest(format!(
            "unknown protocolMapper kind: {}",
            mapper.protocol_mapper
        )));
    }
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))?;
    mapper.id = mapper_id.clone();
    mapper.realm_id = realm;
    client
        .protocol_mappers
        .retain(|m| m.id != mapper_id && m.name != mapper.name);
    client.protocol_mappers.push(mapper);
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "PROTOCOL_MAPPER",
        &format!("clients/{uuid}/protocol-mappers/models/{mapper_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_client_mapper(
    State(state): State<AdminState>,
    Path((realm, uuid, mapper_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm,
            org_id: client.org_id.clone(),
            client_id: client.client_id.clone(),
            managed_by: client.managed_by.clone(),
        },
    ))?;
    client.protocol_mappers.retain(|m| m.id != mapper_id);
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "PROTOCOL_MAPPER",
        &format!("clients/{uuid}/protocol-mappers/models/{mapper_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_scope_objects(
    store: &Arc<dyn Store>,
    realm: &str,
    names: &[String],
) -> Result<Vec<ClientScopeRepresentation>> {
    let all = store.list_client_scopes(realm).await?;
    Ok(names
        .iter()
        .map(|name| {
            all.iter()
                .find(|s| &s.name == name || &s.id == name)
                .cloned()
                .unwrap_or_else(|| ClientScopeRepresentation {
                    id: name.clone(),
                    realm_id: realm.to_owned(),
                    name: name.clone(),
                    ..Default::default()
                })
        })
        .collect())
}

async fn compat_list_client_default_scopes(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ClientScopeRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Read)?;
    let scopes =
        resolve_scope_objects(&state.oidc.store, &realm, &client.default_client_scopes).await?;
    Ok(Json(scopes))
}

async fn compat_link_client_default_scope(
    State(state): State<AdminState>,
    Path((realm, uuid, scope_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Update)?;
    let scope_name = state
        .oidc
        .store
        .get_client_scope(&realm, &scope_id)
        .await?
        .map(|s| s.name)
        .unwrap_or(scope_id);
    if !client.default_client_scopes.contains(&scope_name) {
        client.default_client_scopes.push(scope_name);
    }
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_SCOPE_MAPPING",
        &format!("clients/{uuid}/default-client-scopes"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// W1: Portal's trim calls (`DELETE /clients/{uuid}/default-client-scopes/{id}`) answer 204 even
/// when kaisatsu keeps the quirk-free behaviour.
async fn compat_unlink_client_default_scope(
    State(state): State<AdminState>,
    Path((realm, uuid, scope_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Update)?;
    let scope_name = state
        .oidc
        .store
        .get_client_scope(&realm, &scope_id)
        .await?
        .map(|s| s.name)
        .unwrap_or_else(|| scope_id.clone());
    client
        .default_client_scopes
        .retain(|s| s != &scope_name && s != &scope_id);
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_SCOPE_MAPPING",
        &format!("clients/{uuid}/default-client-scopes/{scope_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_list_client_optional_scopes(
    State(state): State<AdminState>,
    Path((realm, uuid)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ClientScopeRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Read)?;
    let scopes =
        resolve_scope_objects(&state.oidc.store, &realm, &client.optional_client_scopes).await?;
    Ok(Json(scopes))
}

async fn compat_link_client_optional_scope(
    State(state): State<AdminState>,
    Path((realm, uuid, scope_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Update)?;
    let scope_name = state
        .oidc
        .store
        .get_client_scope(&realm, &scope_id)
        .await?
        .map(|s| s.name)
        .unwrap_or(scope_id);
    if !client.optional_client_scopes.contains(&scope_name) {
        client.optional_client_scopes.push(scope_name);
    }
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_SCOPE_MAPPING",
        &format!("clients/{uuid}/optional-client-scopes"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_unlink_client_optional_scope(
    State(state): State<AdminState>,
    Path((realm, uuid, scope_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client not found".into()))?;
    check_application(&state, &ctx, &client, Action::Update)?;
    let scope_name = state
        .oidc
        .store
        .get_client_scope(&realm, &scope_id)
        .await?
        .map(|s| s.name)
        .unwrap_or_else(|| scope_id.clone());
    client
        .optional_client_scopes
        .retain(|s| s != &scope_name && s != &scope_id);
    state.oidc.store.upsert_client(client).await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_SCOPE_MAPPING",
        &format!("clients/{uuid}/optional-client-scopes/{scope_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The `managed-by` mark a scope's write is judged on: only a scope without protocol mappers
/// keeps it, so the `view-clients` grant on marked scopes never reaches a token claim.
fn scope_mark(scope: &ClientScopeRepresentation) -> Option<String> {
    if scope.protocol_mappers.is_empty() {
        scope.attributes.get("managed-by").cloned()
    } else {
        None
    }
}

/// 409 when another scope of the realm (not `except`) holds `name`.
async fn scope_name_free(
    state: &AdminState,
    realm: &str,
    name: &str,
    except: Option<&str>,
) -> Result<()> {
    let taken = state
        .oidc
        .store
        .list_client_scopes(realm)
        .await?
        .into_iter()
        .any(|s| s.name == name && Some(s.id.as_str()) != except);
    if taken {
        return Err(KaisatsuError::Conflict(format!(
            "Client Scope {name} already exists"
        )));
    }
    Ok(())
}

async fn compat_list_client_scopes(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Vec<ClientScopeRepresentation>>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            scope_id: "*".into(),
            managed_by: None,
        },
    ))?;
    let scopes = state.oidc.store.list_client_scopes(&realm).await?;
    Ok(Json(scopes))
}

async fn compat_create_client_scope(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut scope): Json<ClientScopeRepresentation>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    if scope.org_id.is_none() {
        scope.org_id = ctx.active_org_id.clone();
    }
    scope.managed_by = scope.attributes.get("managed-by").cloned();
    // Authorized on the organization the scope is stored under (none = realm-wide).
    check_decision(state.authorizer.check(
        &ctx,
        Action::Create,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: scope.org_id.clone(),
            scope_id: "*".into(),
            managed_by: scope_mark(&scope),
        },
    ))?;
    // Keycloak answers 409 for a name in use; the store would replace that scope instead.
    scope_name_free(&state, &realm, &scope.name, None).await?;
    scope.realm_id = realm.clone();
    let created = state.oidc.store.upsert_client_scope(scope).await?;
    let location = format!(
        "{}/admin/realms/{realm}/client-scopes/{}",
        mount.public_base(),
        created.id
    );
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "CLIENT_SCOPE",
        &format!("client-scopes/{}", created.id),
        None,
    )
    .await?;
    Ok((StatusCode::CREATED, [(header::LOCATION, location)], "").into_response())
}

async fn compat_get_client_scope(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<ClientScopeRepresentation>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let scope = state
        .oidc
        .store
        .get_client_scope(&realm, &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client scope not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::ClientScope {
            realm_id: realm,
            org_id: scope.org_id.clone(),
            scope_id: scope.id.clone(),
            managed_by: None,
        },
    ))?;
    Ok(Json(scope))
}

async fn compat_update_client_scope(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(mut scope): Json<ClientScopeRepresentation>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    // Authorized on the stored scope's organization: a realm-wide scope (`roles`, `openid`) is
    // realm-admin business, whatever organization the caller acts for.
    let existing = state
        .oidc
        .store
        .get_client_scope(&realm, &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client scope not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            scope_id: existing.id.clone(),
            managed_by: scope_mark(&existing),
        },
    ))?;
    // The new representation is judged too: a caller may not drop the mark or add a mapper to
    // a scope it may write only because it is a bare marked scope.
    scope.managed_by = scope.attributes.get("managed-by").cloned();
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: existing.org_id.clone(),
            scope_id: existing.id.clone(),
            managed_by: scope_mark(&scope),
        },
    ))?;
    scope_name_free(&state, &realm, &scope.name, Some(&existing.id)).await?;
    scope.id = id.clone();
    scope.realm_id = realm;
    if scope.org_id.is_none() || !ctx.principal.is_realm_admin() {
        scope.org_id = existing.org_id;
    }
    state.oidc.store.upsert_client_scope(scope).await?;
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "CLIENT_SCOPE",
        &format!("client-scopes/{id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_delete_client_scope(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let existing = state
        .oidc
        .store
        .get_client_scope(&realm, &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("client scope not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            managed_by: scope_mark(&existing),
            org_id: existing.org_id,
            scope_id: existing.id,
        },
    ))?;
    state.oidc.store.delete_client_scope(&realm, &id).await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "CLIENT_SCOPE",
        &format!("client-scopes/{id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_clear_brute_force(
    State(state): State<AdminState>,
    Path((realm, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    // Authorized on the person's own organization: an organization admin never unlocks the
    // brute-force protection of somebody outside it.
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &user_id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id,
            person_id: user.id,
        },
    ))?;
    state
        .oidc
        .store
        .clear_login_failure(&realm, &format!("user:{user_id}"))
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "DELETE",
        "USER_LOGIN_FAILURE",
        &format!("attack-detection/brute-force/users/{user_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn native_list_operations(State(state): State<AdminState>) -> Json<Vec<OperationDef>> {
    Json(state.registry.operations().to_vec())
}

/// `GET /api/v1/realms/{realm}/allowed?op=<op>&kind=<kind>&id=<id>` (W5):
/// Answers "may I do this" with the same decision as the call and never reveals a hidden record
/// (same answer for a missing ID and another organization's ID).
async fn native_check_allowed(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let set = match q.get("kind").map_or("Person", String::as_str) {
        "Person" => "People",
        "Group" => "Groups",
        "Application" => "Applications",
        _ => "Realms",
    };
    let object = match q.get("id") {
        Some(id) if set != "Realms" => format!("{set}('{}')", id.replace('\'', "''")),
        _ => String::new(),
    };
    let op = q.get("op").map_or("read", String::as_str);
    let ans = run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "allowed",
        None,
        &json!({ "operation": op, "object": object }),
    )
    .await?;
    Ok(Json(ans.unwrap_or(Value::Null)))
}

#[derive(Debug, Default, Deserialize)]
struct RotateKeysPayload {
    #[serde(default = "default_alg")]
    pub algorithm: String,
    #[serde(default = "default_immediate")]
    pub immediate: bool,
}

fn default_alg() -> String {
    "ES256".into()
}
fn default_immediate() -> bool {
    true
}

async fn native_rotate_keys(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(payload): Json<RotateKeysPayload>,
) -> Result<Json<Value>> {
    let ans = run_registry_op(
        &state,
        mount_ext,
        &realm,
        &headers,
        "keys_rotate",
        None,
        &json!({ "algorithm": payload.algorithm, "immediate": payload.immediate }),
    )
    .await?;
    Ok(Json(ans.unwrap_or(Value::Null)))
}

/// Runs registry operation `name` through the shared executor (W5): REST, OData and MCP answer
/// the same.
async fn run_registry_op(
    state: &AdminState,
    mount_ext: Option<Extension<Mount>>,
    realm: &str,
    headers: &HeaderMap,
    name: &str,
    key: Option<&str>,
    params: &Value,
) -> Result<Option<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(state, &mount, realm, headers).await?;
    let op = state
        .registry
        .find(name)
        .ok_or_else(|| KaisatsuError::NotSupported(format!("unknown operation {name}")))?;
    crate::operations::execute(state, &mount, &ctx, op, key, params).await
}
