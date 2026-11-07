#![forbid(unsafe_code)]
//! Keycloak Admin REST compatibility layer (`/admin/realms/{realm}/...`) and registry-driven
//! native REST endpoints (`/api/v1/realms/{realm}/...`), W1, W5, W6.
//!
//! Every write decision goes through [`wardkeep_core::Authorizer::check`] and leaves an
//! [`wardkeep_core::AdminEvent`] row without secrets (W6). Cross-organization calls receive 404,
//! never 403.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use chrono::Utc;
use jc_router::Mount;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use wardkeep_core::*;
use wardkeep_crypto::{random_handle_256, verify_jwt};
use wardkeep_oidc::{OidcState, dispatch_backchannel_logout};

#[derive(Clone)]
pub struct AdminState {
    pub oidc: OidcState,
    pub authorizer: Arc<dyn Authorizer>,
    pub registry: Arc<OperationRegistry>,
}

impl AdminState {
    #[must_use]
    pub fn new(oidc: OidcState, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            oidc,
            authorizer,
            registry: Arc::new(OperationRegistry::new()),
        }
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

/// Extracts and verifies the admin bearer token (either from the target realm or `master`) and
/// constructs the [`RequestCtx`] (W1, W6).
pub async fn extract_admin_ctx(
    state: &AdminState,
    mount: &Mount,
    realm_id: &str,
    headers: &HeaderMap,
) -> Result<RequestCtx> {
    let raw_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WardkeepError::Unauthorized("missing admin bearer token".into()))?;

    let mut verified_claims: Option<Value> = None;
    for candidate_realm in [realm_id, "master"] {
        if let Ok(keys) = state.oidc.store.list_signing_keys(candidate_realm).await {
            let issuer = mount.realm_issuer(candidate_realm);
            if let Ok((_, claims)) = verify_jwt::<Value>(raw_token, &keys, Some(&issuer), None)
                && claims.get("typ").and_then(Value::as_str) == Some("Bearer")
            {
                verified_claims = Some(claims);
                break;
            }
        }
    }

    let claims = verified_claims
        .ok_or_else(|| WardkeepError::Unauthorized("invalid admin bearer token".into()))?;

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

    // Also load direct role mappings and memberships from store for service accounts and org admins
    if let Some(uid) = &sub {
        for candidate_realm in [realm_id, "master"] {
            if let Ok(Some(u)) = state
                .oidc
                .store
                .get_user_by_id(candidate_realm, None, uid)
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
                if let Some(u_org) = u.org_id
                    && !organizations.contains(&u_org)
                {
                    organizations.push(u_org);
                }
            }
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

    if let Some(org_hdr) = headers.get("x-wardkeep-org").and_then(|v| v.to_str().ok()) {
        ctx = ctx.with_org(org_hdr.trim());
    } else if organizations.len() == 1 && !ctx.principal.is_realm_admin() {
        ctx = ctx.with_org(organizations[0].clone());
    }

    Ok(ctx)
}

fn check_decision(dec: Decision) -> Result<()> {
    if dec.allowed {
        Ok(())
    } else if !dec.visible {
        Err(WardkeepError::NotFound("resource not found".into()))
    } else {
        Err(WardkeepError::Forbidden(dec.reason))
    }
}

async fn record_admin_write(
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
    Json(mut realm): Json<RealmConfig>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    if realm.id.is_empty() {
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
        return Err(WardkeepError::Conflict(format!(
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
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
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
        match (existing, mode.as_str()) {
            (Some(_), "FAIL") => {
                return Err(WardkeepError::Conflict(format!(
                    "partialImport FAIL: user {} already exists",
                    u.username
                )));
            }
            (Some(_), "SKIP") => {
                skipped += 1;
            }
            (Some(old), "OVERWRITE") => {
                let _ = state
                    .oidc
                    .store
                    .delete_user(&realm_name, None, &old.id)
                    .await?;
                state.oidc.store.create_user(u).await?;
                overwritten += 1;
            }
            (None, _) => {
                state.oidc.store.create_user(u).await?;
                added += 1;
            }
            _ => {
                return Err(WardkeepError::BadRequest(format!(
                    "invalid ifResourceExists: {mode}"
                )));
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
    let users = state
        .oidc
        .store
        .list_users(
            &realm,
            allowed.as_deref(),
            q.search.as_deref(),
            q.email.as_deref(),
            q.username.as_deref(),
            q.exact.unwrap_or(false),
            q.first.unwrap_or(0),
            q.max.unwrap_or(100),
        )
        .await?;
    Ok(Json(users))
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
    if user.org_id.is_none() {
        user.org_id = existing.org_id;
    }
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
    Query(_q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(actions): Json<Vec<String>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    for a in &actions {
        if !user.required_actions.contains(a) {
            user.required_actions.push(a.clone());
        }
    }
    state.oidc.store.update_user(user).await?;
    let realm_cfg = state
        .oidc
        .store
        .get_realm(&realm)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
    // Per Portal `people.rs:365`: if SMTP is not configured, answer 500 so the Portal falls back
    // to a temporary password (`PF-92`).
    if realm_cfg.smtp_server.is_none() {
        return Err(WardkeepError::Storage(
            "SMTP is not configured on this realm".into(),
        ));
    }
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "USER",
        &format!("users/{id}/execute-actions-email"),
        Some(json!({ "actions": actions })),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
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
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    let realm_cfg = state
        .oidc
        .store
        .get_realm(&realm)
        .await?
        .unwrap_or_default();
    let creds = state.oidc.store.list_credentials(&realm, &user.id).await?;
    let history: Vec<CredentialRecord> = creds
        .into_iter()
        .filter(|c| c.credential_type.starts_with("password"))
        .collect();
    wardkeep_crypto::check_password_policy(
        &realm_cfg.password_policy,
        &payload.value,
        &user.username,
        user.email.as_deref(),
        &history,
    )?;
    let phc = wardkeep_crypto::hash_password(&payload.value)?;
    state
        .oidc
        .store
        .upsert_credential(CredentialRecord {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.clone(),
            user_id: user.id,
            credential_type: "password".into(),
            user_label: Some("Password".into()),
            created_date: Utc::now().timestamp_millis(),
            secret_data: phc,
            credential_data: String::new(),
            temporary: payload.temporary,
        })
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "USER",
        &format!("users/{id}/reset-password"),
        Some(json!({ "temporary": payload.temporary })),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compat_logout_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Session {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
    let realm_cfg = state
        .oidc
        .store
        .get_realm(&realm)
        .await?
        .unwrap_or_default();
    let ended = state
        .oidc
        .store
        .delete_user_sso_sessions(&realm, &user.id)
        .await?;
    for sso in &ended {
        let _ = dispatch_backchannel_logout(&state.oidc, &mount, &realm_cfg, sso).await;
    }
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "USER",
        &format!("users/{id}/logout"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        return Err(WardkeepError::NotFound("credential not found".into()));
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    let group = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &group_id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: group.org_id.clone(),
            group_id: group.id.clone(),
            managed_by: group.managed_by.clone(),
        },
    ))?;
    state
        .oidc
        .store
        .add_user_to_group(&realm, &user.id, &group.id)
        .await?;
    record_admin_write(
        &state,
        &ctx,
        "CREATE",
        "GROUP_MEMBERSHIP",
        &format!("users/{id}/groups/{group_id}"),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
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
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
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
    let eff_names = wardkeep_core::claims::resolve_effective_realm_roles(
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
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
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
    if group.org_id.is_none() {
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
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: user.org_id.clone(),
            person_id: user.id.clone(),
        },
    ))?;
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            person_id: id.clone(),
        },
    ))?;
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
        .ok_or_else(|| WardkeepError::NotFound("group not found".into()))?;
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            group_id: id.clone(),
            managed_by: None,
        },
    ))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
    if client.org_id.is_none() {
        client.org_id = existing.org_id;
    }
    // W1 quirk-free behaviour: preserve existing default/optional scopes when PUT body omits them
    if client.default_client_scopes.is_empty() && !existing.default_client_scopes.is_empty() {
        client.default_client_scopes = existing.default_client_scopes;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("service account user not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id.clone(),
            client_id: client.client_id,
            managed_by: client.managed_by,
        },
    ))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Application {
            realm_id: realm.clone(),
            org_id: client.org_id,
            client_id: client.client_id,
            managed_by: client.managed_by,
        },
    ))?;
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
    let _client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
    let _client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
    if !wardkeep_core::claims::is_known_mapper_kind(&mapper.protocol_mapper) {
        return Err(WardkeepError::BadRequest(format!(
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
    let allowed = ctx.allowed_orgs();
    let mut client = state
        .oidc
        .store
        .get_client_by_uuid(&realm, allowed.as_deref(), &uuid)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
/// when wardkeep keeps the quirk-free behaviour.
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
        .ok_or_else(|| WardkeepError::NotFound("client not found".into()))?;
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Create,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            scope_id: "*".into(),
        },
    ))?;
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
    let _ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let scope = state
        .oidc
        .store
        .get_client_scope(&realm, &id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("client scope not found".into()))?;
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            scope_id: id.clone(),
        },
    ))?;
    scope.id = id.clone();
    scope.realm_id = realm;
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::ClientScope {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            scope_id: id.clone(),
        },
    ))?;
    let deleted = state.oidc.store.delete_client_scope(&realm, &id).await?;
    if !deleted {
        return Err(WardkeepError::NotFound("client scope not found".into()));
    }
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
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Person {
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            person_id: user_id.clone(),
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
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let action = match q.get("op").map_or("read", String::as_str) {
        "create" => Action::Create,
        "update" => Action::Update,
        "delete" => Action::Delete,
        "apply" => Action::Apply,
        _ => Action::Read,
    };
    let kind = q.get("kind").map_or("Person", String::as_str);
    let id = q.get("id").map(String::as_str);
    let allowed_orgs = ctx.allowed_orgs();

    // Look up the object within the caller's allowed_orgs so another organization's ID and a
    // non-existent ID produce the exact same result (W5).
    let (exists_and_visible, target) = match (kind, id) {
        ("Person", Some(uid)) => {
            let found = state
                .oidc
                .store
                .get_user_by_id(&realm, allowed_orgs.as_deref(), uid)
                .await?;
            (
                found.is_some(),
                ObjectTarget::Person {
                    realm_id: realm,
                    org_id: found.and_then(|u| u.org_id).or(ctx.active_org_id.clone()),
                    person_id: uid.to_owned(),
                },
            )
        }
        ("Group", Some(gid)) => {
            let found = state
                .oidc
                .store
                .get_group(&realm, allowed_orgs.as_deref(), gid)
                .await?;
            (
                found.is_some(),
                ObjectTarget::Group {
                    realm_id: realm,
                    org_id: found
                        .as_ref()
                        .and_then(|g| g.org_id.clone())
                        .or(ctx.active_org_id.clone()),
                    group_id: gid.to_owned(),
                    managed_by: found.and_then(|g| g.managed_by),
                },
            )
        }
        ("Application", Some(uuid)) => {
            let found = state
                .oidc
                .store
                .get_client_by_uuid(&realm, allowed_orgs.as_deref(), uuid)
                .await?;
            (
                found.is_some(),
                ObjectTarget::Application {
                    realm_id: realm,
                    org_id: found
                        .as_ref()
                        .and_then(|c| c.org_id.clone())
                        .or(ctx.active_org_id.clone()),
                    client_id: uuid.to_owned(),
                    managed_by: found.and_then(|c| c.managed_by),
                },
            )
        }
        _ => (true, ObjectTarget::Realm { realm_id: realm }),
    };

    if id.is_some() && !exists_and_visible {
        return Ok(Json(json!({
            "allowed": false,
            "reason": "resource not found",
            "notFound": true,
        })));
    }

    let dec = state.authorizer.check(&ctx, action, &target);
    if !dec.visible {
        return Ok(Json(json!({
            "allowed": false,
            "reason": "resource not found",
            "notFound": true,
        })));
    }
    Ok(Json(json!({
        "allowed": dec.allowed,
        "reason": dec.reason,
        "notFound": false,
    })))
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
) -> Result<Json<SigningKeyMetadata>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    check_decision(state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Realm {
            realm_id: realm.clone(),
        },
    ))?;
    let realm_cfg = state
        .oidc
        .store
        .get_realm(&realm)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("realm not found".into()))?;
    let mut existing = state.oidc.store.list_signing_keys(&realm).await?;
    let new_key = wardkeep_crypto::KeyManager::rotate(
        &realm,
        &state.oidc.kek,
        &mut existing,
        &payload.algorithm,
        realm_cfg.access_token_lifespan,
        payload.immediate,
    )?;
    for k in existing {
        state.oidc.store.upsert_signing_key(k).await?;
    }
    state.oidc.store.upsert_signing_key(new_key.clone()).await?;
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "KEYS",
        "keys/rotate",
        Some(json!({ "kid": new_key.kid, "algorithm": new_key.algorithm })),
    )
    .await?;
    Ok(Json(SigningKeyMetadata::from(&new_key)))
}
