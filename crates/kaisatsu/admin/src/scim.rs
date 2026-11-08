#![forbid(unsafe_code)]
//! SCIM 2.0 server (`/api/v1/realms/{realm}/scim/v2/{Users,Groups}`) for an organization's HR
//! system (W11, W12):
//! - Maps SCIM `User` and `Group` resources to `People` and `Groups` through the store and
//!   authorization check (`scim` role, scoped to one organization).
//! - Filter subset `eq`, `co`, `sw`, `and` with a strict recursion/nesting depth bound of 16
//!   (regression protection against Kanidm GHSA-2pm5-6m23-h692 and GHSA-r5fr-9gmv-jggh stack overflow).

use crate::compat::{AdminState, extract_admin_ctx, record_admin_write};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::sha256_hex;
use kaisatsu_oidc::dispatch_backchannel_logout;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const MAX_SCIM_FILTER_DEPTH: usize = 16;
pub const MAX_SCIM_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_SCIM_PATCH_OPS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScimOp {
    Eq,
    Co,
    Sw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScimExpr {
    Compare {
        attr: String,
        op: ScimOp,
        value: String,
    },
    And(Box<ScimExpr>, Box<ScimExpr>),
}

/// Parses the SCIM 2.0 filter subset (`eq`, `co`, `sw`, `and`, parentheses) with a hard depth limit
/// of [`MAX_SCIM_FILTER_DEPTH`] so deeply nested expressions never overflow the call stack (W11, W12).
pub fn parse_scim_filter(input: &str) -> Result<ScimExpr> {
    parse_scim_filter_with_depth(input.trim(), 0)
}

fn parse_scim_filter_with_depth(input: &str, depth: usize) -> Result<ScimExpr> {
    if depth > MAX_SCIM_FILTER_DEPTH {
        return Err(KaisatsuError::BadRequest(format!(
            "SCIM filter nesting depth exceeds maximum of {MAX_SCIM_FILTER_DEPTH}"
        )));
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(KaisatsuError::BadRequest("empty SCIM filter".into()));
    }

    // Strip balanced outer parentheses
    if trimmed.starts_with('(') && trimmed.ends_with(')') && is_outer_parens_balanced(trimmed) {
        return parse_scim_filter_with_depth(&trimmed[1..trimmed.len() - 1], depth + 1);
    }

    // Split top-level ` and ` (case-insensitive) outside quotes and parentheses
    if let Some((left, right)) = split_top_level_and(trimmed)? {
        let l = parse_scim_filter_with_depth(left, depth + 1)?;
        let r = parse_scim_filter_with_depth(right, depth + 1)?;
        return Ok(ScimExpr::And(Box::new(l), Box::new(r)));
    }

    // Parse `<attr> <eq|co|sw> "<value>"`
    let mut parts = trimmed.splitn(3, char::is_whitespace);
    let attr = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::BadRequest("missing SCIM filter attribute".into()))?;
    let op_str = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| KaisatsuError::BadRequest("missing SCIM filter operator".into()))?;
    let op = match op_str.to_ascii_lowercase().as_str() {
        "eq" => ScimOp::Eq,
        "co" => ScimOp::Co,
        "sw" => ScimOp::Sw,
        other => {
            return Err(KaisatsuError::BadRequest(format!(
                "unsupported SCIM filter operator: {other}"
            )));
        }
    };
    let after_op = trimmed[attr.len()..].trim_start()[op_str.len()..].trim();
    let value = after_op
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| {
            KaisatsuError::BadRequest("SCIM filter value must be double-quoted".into())
        })?;

    Ok(ScimExpr::Compare {
        attr: attr.to_owned(),
        op,
        value: value.to_owned(),
    })
}

fn is_outer_parens_balanced(s: &str) -> bool {
    let mut depth = 0i32;
    let mut in_quotes = false;
    for (idx, ch) in s.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '(' if !in_quotes => depth += 1,
            ')' if !in_quotes => {
                depth -= 1;
                if depth == 0 && idx + ch.len_utf8() < s.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_top_level_and(s: &str) -> Result<Option<(&str, &str)>> {
    let bytes = s.as_bytes();
    let mut paren_depth = 0usize;
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'(' if !in_quotes => {
                paren_depth += 1;
                if paren_depth > MAX_SCIM_FILTER_DEPTH {
                    return Err(KaisatsuError::BadRequest(
                        "SCIM filter parenthesis depth exceeded".into(),
                    ));
                }
            }
            b')' if !in_quotes => {
                if paren_depth == 0 {
                    return Err(KaisatsuError::BadRequest(
                        "unbalanced ')' in SCIM filter".into(),
                    ));
                }
                paren_depth -= 1;
            }
            b' ' if !in_quotes && paren_depth == 0 && i + 5 <= bytes.len() => {
                let slice = &s[i..i + 5];
                if slice.eq_ignore_ascii_case(" and ") {
                    return Ok(Some((&s[..i], &s[i + 5..])));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(None)
}

pub fn eval_scim_user(expr: &ScimExpr, u: &UserRepresentation) -> bool {
    match expr {
        ScimExpr::And(a, b) => eval_scim_user(a, u) && eval_scim_user(b, u),
        ScimExpr::Compare { attr, op, value } => {
            let field_val = match attr.to_ascii_lowercase().as_str() {
                "username" => u.username.as_str(),
                "emails.value" | "email" => u.email.as_deref().unwrap_or_default(),
                "name.givenname" | "givenname" => u.first_name.as_deref().unwrap_or_default(),
                "name.familyname" | "familyname" => u.last_name.as_deref().unwrap_or_default(),
                "id" => u.id.as_str(),
                _ => return false,
            };
            let f_low = field_val.to_ascii_lowercase();
            let v_low = value.to_ascii_lowercase();
            match op {
                ScimOp::Eq => f_low == v_low,
                ScimOp::Co => f_low.contains(&v_low),
                ScimOp::Sw => f_low.starts_with(&v_low),
            }
        }
    }
}

pub fn eval_scim_group(expr: &ScimExpr, g: &GroupRepresentation) -> bool {
    match expr {
        ScimExpr::And(a, b) => eval_scim_group(a, g) && eval_scim_group(b, g),
        ScimExpr::Compare { attr, op, value } => {
            let field_val = match attr.to_ascii_lowercase().as_str() {
                "displayname" | "name" => g.name.as_str(),
                "id" => g.id.as_str(),
                _ => return false,
            };
            let f_low = field_val.to_ascii_lowercase();
            let v_low = value.to_ascii_lowercase();
            match op {
                ScimOp::Eq => f_low == v_low,
                ScimOp::Co => f_low.contains(&v_low),
                ScimOp::Sw => f_low.starts_with(&v_low),
            }
        }
    }
}

pub fn scim_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/api/v1/realms/{realm}/scim/v2/Users",
            get(scim_list_users).post(scim_create_user),
        )
        .route(
            "/api/v1/realms/{realm}/scim/v2/Users/{id}",
            get(scim_get_user)
                .put(scim_put_user)
                .patch(scim_patch_user)
                .delete(scim_delete_user),
        )
        .route(
            "/api/v1/realms/{realm}/scim/v2/Groups",
            get(scim_list_groups).post(scim_create_group),
        )
        .route(
            "/api/v1/realms/{realm}/scim/v2/Groups/{id}",
            get(scim_get_group)
                .put(scim_put_group)
                .patch(scim_patch_group)
                .delete(scim_delete_group),
        )
        .with_state(state)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScimUserPayload {
    #[serde(default)]
    pub schemas: Vec<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    pub user_name: String,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub name: Option<ScimName>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub nick_name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub emails: Vec<ScimEmail>,
    #[serde(default)]
    pub roles: Vec<Value>,
    #[serde(default)]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScimName {
    #[serde(default)]
    pub formatted: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub middle_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScimEmail {
    pub value: String,
    #[serde(default, rename = "type")]
    pub email_type: Option<String>,
    #[serde(default)]
    pub primary: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScimGroupMember {
    pub value: String,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default, rename = "$ref")]
    pub member_ref: Option<String>,
    #[serde(default, rename = "type")]
    pub member_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
// `schemas`, `externalId` and `meta` are accepted as RFC 7643 §4.2 allows and not stored.
#[allow(dead_code)]
struct ScimGroupPayload {
    #[serde(default)]
    pub schemas: Vec<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub members: Vec<ScimGroupMember>,
    #[serde(default)]
    pub meta: Option<Value>,
}

/// RFC 7644 §3.5.2 SCIM `PatchOp` request body (`deny_unknown_fields`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScimPatchRequest {
    pub schemas: Vec<String>,
    #[serde(rename = "Operations", alias = "operations")]
    pub operations: Vec<ScimPatchOp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScimPatchOp {
    pub op: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

/// Builds a RFC 7644 §3.12 SCIM error response with `scimType` (`invalidPath`, `noTarget`,
/// `mutability`, `invalidValue`, `tooMany`).
fn scim_error_response(status: StatusCode, scim_type: Option<&str>, detail: &str) -> Response {
    let mut body = json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:Error"],
        "status": status.as_u16().to_string(),
        "detail": detail,
    });
    if let Some(st) = scim_type {
        body["scimType"] = Value::String(st.to_owned());
    }
    (
        status,
        [(header::CONTENT_TYPE, "application/scim+json")],
        body.to_string(),
    )
        .into_response()
}

#[allow(clippy::result_large_err)] // the error is the finished SCIM answer
fn parse_scim_body<T: serde::de::DeserializeOwned>(
    bytes: &Bytes,
) -> std::result::Result<T, Response> {
    if bytes.len() > MAX_SCIM_BODY_BYTES {
        return Err(scim_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            Some("invalidValue"),
            "SCIM request body exceeds 1 MiB limit",
        ));
    }
    serde_json::from_slice(bytes).map_err(|e| {
        scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            &format!("invalid SCIM JSON payload: {e}"),
        )
    })
}

fn user_etag(u: &UserRepresentation) -> String {
    let material = format!(
        "{}:{}:{}:{:?}:{:?}:{:?}:{}",
        u.id,
        u.username,
        u.enabled,
        u.email,
        u.first_name,
        u.last_name,
        u.attributes
            .get("externalId")
            .and_then(|v| v.first())
            .map_or("", String::as_str)
    );
    format!("W/\"{}\"", &sha256_hex(&material)[..16])
}

fn group_etag(g: &GroupRepresentation, members: &[UserRepresentation]) -> String {
    let mut mids: Vec<&str> = members.iter().map(|m| m.id.as_str()).collect();
    mids.sort_unstable();
    let material = format!("{}:{}:{}", g.id, g.name, mids.join(","));
    format!("W/\"{}\"", &sha256_hex(&material)[..16])
}

#[allow(clippy::result_large_err)] // the error is the finished SCIM answer
fn check_if_match(headers: &HeaderMap, current_etag: &str) -> std::result::Result<(), Response> {
    if let Some(if_match) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        let trimmed = if_match.trim();
        if trimmed != "*" && trimmed != current_etag {
            return Err(scim_error_response(
                StatusCode::PRECONDITION_FAILED,
                None,
                "If-Match ETag precondition failed",
            ));
        }
    }
    Ok(())
}

fn to_scim_user_json(u: &UserRepresentation) -> Value {
    let etag = user_etag(u);
    let emails: Vec<Value> = if let Some(raw_emails) = u
        .attributes
        .get("scim.emails")
        .and_then(|v| v.first())
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
    {
        raw_emails
    } else {
        u.email
            .as_ref()
            .map(|e| vec![json!({"value": e, "type": "work", "primary": true})])
            .unwrap_or_default()
    };
    let mut out = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "id": u.id,
        "userName": u.username,
        "active": u.enabled,
        "name": {
            "givenName": u.first_name,
            "familyName": u.last_name,
        },
        "emails": emails,
        "meta": {
            "resourceType": "User",
            "version": etag,
        }
    });
    if let Some(ext) = u.attributes.get("externalId").and_then(|v| v.first()) {
        out["externalId"] = Value::String(ext.clone());
    }
    if let Some(nick) = u.attributes.get("nickName").and_then(|v| v.first()) {
        out["nickName"] = Value::String(nick.clone());
    }
    out
}

fn to_scim_group_json(g: &GroupRepresentation, members: &[UserRepresentation]) -> Value {
    let etag = group_etag(g, members);
    let member_list: Vec<Value> = members
        .iter()
        .map(|m| {
            json!({
                "value": m.id,
                "display": m.username,
            })
        })
        .collect();
    json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
        "id": g.id,
        "displayName": g.name,
        "members": member_list,
        "meta": {
            "resourceType": "Group",
            "version": etag,
        }
    })
}

fn scim_json_with_etag(status: StatusCode, body: Value) -> Response {
    let etag = body
        .pointer("/meta/version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut resp = (
        status,
        [(header::CONTENT_TYPE, "application/scim+json")],
        body.to_string(),
    )
        .into_response();
    if !etag.is_empty()
        && let Ok(val) = header::HeaderValue::from_str(&etag)
    {
        resp.headers_mut().insert(header::ETAG, val);
    }
    resp
}

async fn scim_list_users(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let target = ObjectTarget::Person {
        realm_id: realm.clone(),
        org_id: ctx.active_org_id.clone(),
        person_id: "*".into(),
    };
    let dec = state.authorizer.check(&ctx, Action::Read, &target);
    if !dec.allowed {
        return Err(KaisatsuError::Forbidden(dec.reason));
    }
    let filter = match q.get("filter") {
        Some(f) => Some(parse_scim_filter(f)?),
        None => None,
    };
    let allowed = ctx.allowed_orgs();
    let users = state
        .oidc
        .store
        .list_users(&realm, allowed.as_deref(), None, None, None, false, 0, 1000)
        .await?;
    let resources: Vec<Value> = users
        .into_iter()
        .filter(|u| filter.as_ref().is_none_or(|f| eval_scim_user(f, u)))
        .map(|u| to_scim_user_json(&u))
        .collect();
    Ok(Json(json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": resources.len(),
        "Resources": resources,
    })))
}

async fn run_op(
    state: &AdminState,
    mount: &Mount,
    ctx: &RequestCtx,
    name: &str,
    key: Option<&str>,
    params: &Value,
) -> Result<Option<Value>> {
    let op = state
        .registry
        .find(name)
        .ok_or_else(|| KaisatsuError::NotSupported(name.to_owned()))?;
    crate::operations::execute(state, mount, ctx, op, key, params).await
}

/// Creates a person through the registry executor, so SCIM, REST, OData and MCP make the same
/// decision and write the same audit row. The address is not marked verified: SCIM carries no
/// proof of it, and a verified address is what account linking trusts.
async fn scim_create_user(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(payload): Json<ScimUserPayload>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    if payload.user_name.trim().is_empty() {
        return Err(KaisatsuError::BadRequest("userName is required".into()));
    }
    let email = payload
        .emails
        .first()
        .map(|e| e.value.clone())
        .or_else(|| Some(payload.user_name.clone()));
    let person = UserRepresentation {
        realm_id: realm.clone(),
        org_id: ctx.active_org_id.clone(),
        username: payload.user_name,
        email,
        first_name: payload.name.as_ref().and_then(|n| n.given_name.clone()),
        last_name: payload.name.as_ref().and_then(|n| n.family_name.clone()),
        enabled: payload.active.unwrap_or(true),
        email_verified: false,
        ..Default::default()
    };
    let body =
        serde_json::to_value(&person).map_err(|e| KaisatsuError::BadRequest(e.to_string()))?;
    let created = run_op(&state, &mount, &ctx, "person_create", None, &body)
        .await?
        .ok_or_else(|| KaisatsuError::Storage("person_create returned nothing".into()))?;
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| KaisatsuError::Storage("created person has no id".into()))?;
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, None, id)
        .await?
        .ok_or_else(|| KaisatsuError::Storage("created person vanished".into()))?;
    if let Some(ext) = payload.external_id {
        let mut u = user;
        u.attributes.insert("externalId".into(), vec![ext]);
        let u = state.oidc.store.update_user(u).await?;
        return Ok(scim_json_with_etag(
            StatusCode::CREATED,
            to_scim_user_json(&u),
        ));
    }
    Ok(scim_json_with_etag(
        StatusCode::CREATED,
        to_scim_user_json(&user),
    ))
}

async fn scim_get_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let user = crate::operations::visible_person(&state, &ctx, &id).await?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &crate::operations::person_target(&ctx, &user),
    );
    if !dec.allowed {
        // Same answer as a missing person: no probing who exists (W5).
        return Err(KaisatsuError::NotFound("user not found".into()));
    }
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_user_json(&user),
    ))
}

/// RFC 7644 §3.5.1 `PUT /Users/{id}` (K14): replaces mutable attributes, enforces `If-Match`
/// and `id` immutability (`mutability`), revokes sessions when `active=false`, and returns 404
/// outside the caller's scope.
async fn scim_put_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Bytes,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let mut existing = crate::operations::visible_person(&state, &ctx, &id).await?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Update,
        &crate::operations::person_target(&ctx, &existing),
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("user not found".into()));
    }
    if let Err(resp) = check_if_match(&headers, &user_etag(&existing)) {
        return Ok(resp);
    }
    let payload: ScimUserPayload = match parse_scim_body(&body) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if let Some(ref body_id) = payload.id
        && body_id != &existing.id
    {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("mutability"),
            "Attribute 'id' is immutable and cannot be changed",
        ));
    }
    if payload.user_name.trim().is_empty() {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "userName is required",
        ));
    }

    let email_before = existing.email.clone();
    existing.username = payload.user_name.trim().to_ascii_lowercase();
    existing.first_name = payload.name.as_ref().and_then(|n| n.given_name.clone());
    existing.last_name = payload.name.as_ref().and_then(|n| n.family_name.clone());
    if let Some(active) = payload.active {
        existing.enabled = active;
    }
    if !payload.emails.is_empty() {
        let primary_email = payload
            .emails
            .iter()
            .find(|e| e.primary == Some(true))
            .or_else(|| payload.emails.first())
            .map(|e| e.value.clone());
        existing.email = primary_email;
        existing.attributes.insert(
            "scim.emails".into(),
            vec![serde_json::to_string(&payload.emails).unwrap_or_default()],
        );
    }
    if let Some(ext) = payload.external_id {
        existing.attributes.insert("externalId".into(), vec![ext]);
    }
    if let Some(nick) = payload.nick_name {
        existing.attributes.insert("nickName".into(), vec![nick]);
    }
    // SCIM carries no proof of an address (see create): a changed address is unverified, or a
    // verified address account linking trusts could be pointed at someone else's mailbox.
    if existing.email != email_before {
        existing.email_verified = false;
    }

    let updated = state.oidc.store.update_user(existing).await?;
    if !updated.enabled {
        end_sessions_for_deactivated_user(&state, &mount, &realm, &updated.id).await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "USER",
        &format!("scim/Users/{id}"),
        None,
    )
    .await?;
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_user_json(&updated),
    ))
}

/// RFC 7644 §3.5.2 `PATCH /Users/{id}` (K14): applies `add`, `remove`, and `replace` operations
/// sequentially and atomically, supporting plain paths (`active`, `userName`, `name.givenName`,
/// `name.familyName`, `nickName`) and `valuePath` filters (`emails[type eq "work"].value`).
async fn scim_patch_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Bytes,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let mut user = crate::operations::visible_person(&state, &ctx, &id).await?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Update,
        &crate::operations::person_target(&ctx, &user),
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("user not found".into()));
    }
    if let Err(resp) = check_if_match(&headers, &user_etag(&user)) {
        return Ok(resp);
    }
    let patch: ScimPatchRequest = match parse_scim_body(&body) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !patch
        .schemas
        .iter()
        .any(|s| s == "urn:ietf:params:scim:api:messages:2.0:PatchOp")
    {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "PATCH request must include PatchOp schema",
        ));
    }
    if patch.operations.is_empty() || patch.operations.len() > MAX_SCIM_PATCH_OPS {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "PATCH Operations count must be 1..=100",
        ));
    }

    let email_before = user.email.clone();
    for op in &patch.operations {
        if let Err(resp) = apply_user_patch_op(&mut user, op) {
            return Ok(resp);
        }
    }
    if user.email != email_before {
        user.email_verified = false;
    }

    let updated = state.oidc.store.update_user(user).await?;
    if !updated.enabled {
        end_sessions_for_deactivated_user(&state, &mount, &realm, &updated.id).await?;
    }
    record_admin_write(
        &state,
        &ctx,
        "UPDATE",
        "USER",
        &format!("scim/Users/{id}"),
        None,
    )
    .await?;
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_user_json(&updated),
    ))
}

#[allow(clippy::result_large_err)] // the error is the finished SCIM answer
fn apply_user_patch_op(
    user: &mut UserRepresentation,
    op: &ScimPatchOp,
) -> std::result::Result<(), Response> {
    let op_kind = op.op.to_ascii_lowercase();
    if !matches!(op_kind.as_str(), "add" | "remove" | "replace") {
        return Err(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "unsupported PATCH op",
        ));
    }
    let path_str = op.path.as_deref().map(str::trim).unwrap_or("");
    if op_kind == "remove" && path_str.is_empty() {
        // RFC 7644 §3.5.2.2: If "path" is unspecified on remove, fail with 400 scimType="noTarget".
        return Err(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("noTarget"),
            "remove operation requires a path",
        ));
    }

    // Omitted path on add/replace: value is an object of attributes
    if path_str.is_empty() {
        let Some(Value::Object(map)) = &op.value else {
            return Err(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidValue"),
                "omitted path requires an object value",
            ));
        };
        for (k, v) in map {
            let sub_op = ScimPatchOp {
                op: op_kind.clone(),
                path: Some(k.clone()),
                value: Some(v.clone()),
            };
            apply_user_patch_op(user, &sub_op)?;
        }
        return Ok(());
    }

    let lower_path = path_str.to_ascii_lowercase();
    if lower_path == "id" || lower_path.starts_with("meta.") {
        return Err(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("mutability"),
            "attribute is readOnly or immutable",
        ));
    }

    // Handle valuePath on `emails[...]`: e.g. `emails[type eq "work"].value` or `emails[type eq "work"]`
    if let Some(rest) = path_str
        .strip_prefix("emails[")
        .or_else(|| path_str.strip_prefix("Emails["))
    {
        let Some((filter_str, tail)) = rest.split_once(']') else {
            return Err(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidPath"),
                "unclosed bracket in emails valuePath",
            ));
        };
        let filter_expr = parse_scim_filter(filter_str).map_err(|_| {
            scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidPath"),
                "invalid valuePath filter in emails",
            )
        })?;
        let mut list: Vec<Value> = user
            .attributes
            .get("scim.emails")
            .and_then(|v| v.first())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| {
                user.email
                    .as_ref()
                    .map(|e| vec![json!({"value": e, "type": "work", "primary": true})])
                    .unwrap_or_default()
            });

        let sub_attr = tail.strip_prefix('.').unwrap_or("");
        if op_kind == "remove" {
            let before = list.len();
            list.retain(|item| !eval_json_item_filter(&filter_expr, item));
            if list.len() == before {
                return Err(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("noTarget"),
                    "no matching email entry found",
                ));
            }
        } else {
            let mut matched_any = false;
            for item in &mut list {
                if eval_json_item_filter(&filter_expr, item) {
                    matched_any = true;
                    if sub_attr.is_empty() {
                        if let Some(val) = &op.value {
                            *item = val.clone();
                        }
                    } else if let (Some(obj), Some(val)) = (item.as_object_mut(), &op.value) {
                        obj.insert(sub_attr.to_owned(), val.clone());
                    }
                }
            }
            if !matched_any {
                return Err(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("noTarget"),
                    "no matching email entry for replace",
                ));
            }
        }
        user.email = list
            .iter()
            .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true))
            .or_else(|| list.first())
            .and_then(|e| e.get("value").and_then(Value::as_str))
            .map(str::to_owned);
        user.attributes.insert(
            "scim.emails".into(),
            vec![serde_json::to_string(&list).unwrap_or_default()],
        );
        return Ok(());
    }

    match lower_path.as_str() {
        "username" => {
            if op_kind == "remove" {
                return Err(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("mutability"),
                    "userName is required and cannot be removed",
                ));
            }
            let s = op
                .value
                .as_ref()
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    scim_error_response(
                        StatusCode::BAD_REQUEST,
                        Some("invalidValue"),
                        "userName must be a non-empty string",
                    )
                })?;
            user.username = s.trim().to_ascii_lowercase();
        }
        "active" => {
            let b = op.value.as_ref().and_then(Value::as_bool).ok_or_else(|| {
                scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("invalidValue"),
                    "active must be a boolean",
                )
            })?;
            user.enabled = b;
        }
        "name.givenname" | "givenname" => {
            user.first_name = if op_kind == "remove" {
                None
            } else {
                op.value.as_ref().and_then(Value::as_str).map(str::to_owned)
            };
        }
        "name.familyname" | "familyname" => {
            user.last_name = if op_kind == "remove" {
                None
            } else {
                op.value.as_ref().and_then(Value::as_str).map(str::to_owned)
            };
        }
        "name" => {
            if op_kind == "remove" {
                user.first_name = None;
                user.last_name = None;
            } else if let Some(obj) = op.value.as_ref().and_then(Value::as_object) {
                if let Some(gn) = obj.get("givenName").and_then(Value::as_str) {
                    user.first_name = Some(gn.to_owned());
                }
                if let Some(fn_) = obj.get("familyName").and_then(Value::as_str) {
                    user.last_name = Some(fn_.to_owned());
                }
            }
        }
        "nickname" => {
            if op_kind == "remove" {
                user.attributes.remove("nickName");
            } else if let Some(s) = op.value.as_ref().and_then(Value::as_str) {
                user.attributes
                    .insert("nickName".into(), vec![s.to_owned()]);
            }
        }
        "externalid" => {
            if op_kind == "remove" {
                user.attributes.remove("externalId");
            } else if let Some(s) = op.value.as_ref().and_then(Value::as_str) {
                user.attributes
                    .insert("externalId".into(), vec![s.to_owned()]);
            }
        }
        "emails" => {
            if op_kind == "remove" {
                user.email = None;
                user.attributes.remove("scim.emails");
            } else if let Some(arr) = op.value.as_ref().and_then(Value::as_array) {
                let mut current: Vec<Value> = if op_kind == "add" {
                    user.attributes
                        .get("scim.emails")
                        .and_then(|v| v.first())
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                current.extend(arr.iter().cloned());
                user.email = current
                    .iter()
                    .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true))
                    .or_else(|| current.first())
                    .and_then(|e| e.get("value").and_then(Value::as_str))
                    .map(str::to_owned);
                user.attributes.insert(
                    "scim.emails".into(),
                    vec![serde_json::to_string(&current).unwrap_or_default()],
                );
            }
        }
        _ => {
            return Err(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidPath"),
                &format!("unknown SCIM User attribute path: {path_str}"),
            ));
        }
    }
    Ok(())
}

fn eval_json_item_filter(expr: &ScimExpr, item: &Value) -> bool {
    match expr {
        ScimExpr::And(a, b) => eval_json_item_filter(a, item) && eval_json_item_filter(b, item),
        ScimExpr::Compare { attr, op, value } => {
            let field_val = item
                .get(attr)
                .or_else(|| item.get(attr.to_ascii_lowercase()))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let f_low = field_val.to_ascii_lowercase();
            let v_low = value.to_ascii_lowercase();
            match op {
                ScimOp::Eq => f_low == v_low,
                ScimOp::Co => f_low.contains(&v_low),
                ScimOp::Sw => f_low.starts_with(&v_low),
            }
        }
    }
}

async fn end_sessions_for_deactivated_user(
    state: &AdminState,
    mount: &Mount,
    realm: &str,
    user_id: &str,
) -> Result<()> {
    let realm_cfg = state.oidc.store.get_realm(realm).await?.unwrap_or_default();
    let ended = state
        .oidc
        .store
        .delete_user_sso_sessions(realm, user_id)
        .await?;
    for sso in &ended {
        let _ = dispatch_backchannel_logout(&state.oidc, mount, &realm_cfg, sso).await;
    }
    Ok(())
}

async fn scim_delete_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    run_op(
        &state,
        &mount,
        &ctx,
        "person_delete",
        Some(&id),
        &Value::Null,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn group_target(ctx: &RequestCtx, group_id: &str) -> ObjectTarget {
    ObjectTarget::Group {
        realm_id: ctx.realm_id.clone(),
        org_id: ctx.active_org_id.clone(),
        group_id: group_id.to_owned(),
        managed_by: None,
    }
}

async fn scim_list_groups(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let dec = state
        .authorizer
        .check(&ctx, Action::Read, &group_target(&ctx, "*"));
    if !dec.allowed {
        return Err(KaisatsuError::Forbidden(dec.reason));
    }
    let filter = match q.get("filter") {
        Some(f) => Some(parse_scim_filter(f)?),
        None => None,
    };
    let allowed = ctx.allowed_orgs();
    let groups = state
        .oidc
        .store
        .list_groups(&realm, allowed.as_deref(), None, false)
        .await?;
    let mut resources = Vec::new();
    for g in groups
        .into_iter()
        .filter(|g| filter.as_ref().is_none_or(|f| eval_scim_group(f, g)))
    {
        let members = state
            .oidc
            .store
            .list_group_members(&realm, allowed.as_deref(), &g.id, 1000)
            .await
            .unwrap_or_default();
        resources.push(to_scim_group_json(&g, &members));
    }
    Ok(Json(json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": resources.len(),
        "Resources": resources,
    })))
}

async fn scim_create_group(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Bytes,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let dec = state
        .authorizer
        .check(&ctx, Action::Create, &group_target(&ctx, "*"));
    if !dec.allowed {
        return Err(KaisatsuError::Forbidden(dec.reason));
    }
    let payload: ScimGroupPayload = match parse_scim_body(&body) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    let name = payload.display_name.trim();
    if name.is_empty() || name.contains('/') {
        return Err(KaisatsuError::BadRequest(
            "displayName must be non-empty and must not contain '/'".into(),
        ));
    }
    let path = match &ctx.active_org_id {
        Some(org) => format!("/{org}/{name}"),
        None => format!("/{name}"),
    };
    let grp = state
        .oidc
        .store
        .upsert_group(GroupRepresentation {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm.clone(),
            org_id: ctx.active_org_id.clone(),
            name: name.to_owned(),
            path,
            ..Default::default()
        })
        .await?;

    let allowed = ctx.allowed_orgs();
    for m in &payload.members {
        if state
            .oidc
            .store
            .get_user_by_id(&realm, allowed.as_deref(), &m.value)
            .await?
            .is_none()
        {
            return Err(KaisatsuError::NotFound("member user not found".into()));
        }
        state
            .oidc
            .store
            .add_user_to_group(&realm, &m.value, &grp.id)
            .await?;
    }
    let members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    Ok(scim_json_with_etag(
        StatusCode::CREATED,
        to_scim_group_json(&grp, &members),
    ))
}

async fn scim_get_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let grp = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: grp.org_id.clone(),
            group_id: grp.id.clone(),
            managed_by: grp.managed_by.clone(),
        },
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("group not found".into()));
    }
    let members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_group_json(&grp, &members),
    ))
}

/// RFC 7644 §3.5.1 `PUT /Groups/{id}` (K14): replaces group `displayName` and `members`.
async fn scim_put_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Bytes,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut grp = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: grp.org_id.clone(),
            group_id: grp.id.clone(),
            managed_by: grp.managed_by.clone(),
        },
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("group not found".into()));
    }
    let cur_members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    if let Err(resp) = check_if_match(&headers, &group_etag(&grp, &cur_members)) {
        return Ok(resp);
    }
    let payload: ScimGroupPayload = match parse_scim_body(&body) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if let Some(ref body_id) = payload.id
        && body_id != &grp.id
    {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("mutability"),
            "Attribute 'id' is immutable",
        ));
    }
    let name = payload.display_name.trim();
    if name.is_empty() || name.contains('/') {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "displayName must be non-empty and must not contain '/'",
        ));
    }
    for m in &payload.members {
        if state
            .oidc
            .store
            .get_user_by_id(&realm, allowed.as_deref(), &m.value)
            .await?
            .is_none()
        {
            return Err(KaisatsuError::NotFound("member user not found".into()));
        }
    }

    grp.name = name.to_owned();
    grp.path = match &grp.org_id {
        Some(org) => format!("/{org}/{name}"),
        None => format!("/{name}"),
    };
    let grp = state.oidc.store.upsert_group(grp).await?;

    for old in &cur_members {
        state
            .oidc
            .store
            .remove_user_from_group(&realm, &old.id, &grp.id)
            .await?;
    }
    for m in &payload.members {
        state
            .oidc
            .store
            .add_user_to_group(&realm, &m.value, &grp.id)
            .await?;
    }
    let new_members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_group_json(&grp, &new_members),
    ))
}

/// RFC 7644 §3.5.2 `PATCH /Groups/{id}` (K14): supports `add`, `remove`, and `replace` on
/// `displayName`, `members`, and `members[value eq "<id>"]`.
async fn scim_patch_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    body: Bytes,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let mut grp = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Update,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: grp.org_id.clone(),
            group_id: grp.id.clone(),
            managed_by: grp.managed_by.clone(),
        },
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("group not found".into()));
    }
    let cur_members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    if let Err(resp) = check_if_match(&headers, &group_etag(&grp, &cur_members)) {
        return Ok(resp);
    }

    let patch: ScimPatchRequest = match parse_scim_body(&body) {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !patch
        .schemas
        .iter()
        .any(|s| s == "urn:ietf:params:scim:api:messages:2.0:PatchOp")
    {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "PATCH request must include PatchOp schema",
        ));
    }
    if patch.operations.is_empty() || patch.operations.len() > MAX_SCIM_PATCH_OPS {
        return Ok(scim_error_response(
            StatusCode::BAD_REQUEST,
            Some("invalidValue"),
            "PATCH Operations count must be 1..=100",
        ));
    }

    let mut member_ids: Vec<String> = cur_members.iter().map(|u| u.id.clone()).collect();
    for op in &patch.operations {
        let op_kind = op.op.to_ascii_lowercase();
        if !matches!(op_kind.as_str(), "add" | "remove" | "replace") {
            return Ok(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidValue"),
                "unsupported PATCH op",
            ));
        }
        let path_str = op.path.as_deref().map(str::trim).unwrap_or("");
        if op_kind == "remove" && path_str.is_empty() {
            return Ok(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("noTarget"),
                "remove operation requires a path",
            ));
        }
        let lower_path = path_str.to_ascii_lowercase();
        if lower_path == "id" {
            return Ok(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("mutability"),
                "id is immutable",
            ));
        } else if lower_path == "displayname"
            || (path_str.is_empty()
                && op
                    .value
                    .as_ref()
                    .and_then(|v| v.get("displayName"))
                    .is_some())
        {
            if op_kind == "remove" {
                return Ok(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("mutability"),
                    "displayName is required and cannot be removed",
                ));
            }
            let name_val = if lower_path == "displayname" {
                op.value.as_ref().and_then(Value::as_str)
            } else {
                op.value
                    .as_ref()
                    .and_then(|v| v.get("displayName"))
                    .and_then(Value::as_str)
            };
            let Some(name) = name_val
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.contains('/'))
            else {
                return Ok(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("invalidValue"),
                    "invalid displayName",
                ));
            };
            grp.name = name.to_owned();
            grp.path = match &grp.org_id {
                Some(org) => format!("/{org}/{name}"),
                None => format!("/{name}"),
            };
        } else if lower_path == "members" {
            if op_kind == "remove" {
                member_ids.clear();
            } else {
                let Some(arr) = op.value.as_ref().and_then(Value::as_array) else {
                    return Ok(scim_error_response(
                        StatusCode::BAD_REQUEST,
                        Some("invalidValue"),
                        "members value must be an array",
                    ));
                };
                if op_kind == "replace" {
                    member_ids.clear();
                }
                for item in arr {
                    let Some(uid) = item.get("value").and_then(Value::as_str) else {
                        return Ok(scim_error_response(
                            StatusCode::BAD_REQUEST,
                            Some("invalidValue"),
                            "member item missing value",
                        ));
                    };
                    if state
                        .oidc
                        .store
                        .get_user_by_id(&realm, allowed.as_deref(), uid)
                        .await?
                        .is_none()
                    {
                        return Err(KaisatsuError::NotFound("member user not found".into()));
                    }
                    if !member_ids.iter().any(|m| m == uid) {
                        member_ids.push(uid.to_owned());
                    }
                }
            }
        } else if let Some(rest) = path_str
            .strip_prefix("members[")
            .or_else(|| path_str.strip_prefix("Members["))
        {
            let Some(filter_str) = rest.strip_suffix(']') else {
                return Ok(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("invalidPath"),
                    "unclosed bracket in members valuePath",
                ));
            };
            let expr = match parse_scim_filter(filter_str) {
                Ok(e) => e,
                Err(_) => {
                    return Ok(scim_error_response(
                        StatusCode::BAD_REQUEST,
                        Some("invalidPath"),
                        "invalid filter in members valuePath",
                    ));
                }
            };
            if op_kind == "remove" {
                member_ids.retain(|uid| !eval_json_item_filter(&expr, &json!({ "value": uid })));
            } else {
                return Ok(scim_error_response(
                    StatusCode::BAD_REQUEST,
                    Some("invalidPath"),
                    "unsupported operation on members valuePath",
                ));
            }
        } else {
            return Ok(scim_error_response(
                StatusCode::BAD_REQUEST,
                Some("invalidPath"),
                &format!("unknown SCIM Group attribute path: {path_str}"),
            ));
        }
    }

    let grp = state.oidc.store.upsert_group(grp).await?;
    for old in &cur_members {
        if !member_ids.contains(&old.id) {
            state
                .oidc
                .store
                .remove_user_from_group(&realm, &old.id, &grp.id)
                .await?;
        }
    }
    for uid in &member_ids {
        if !cur_members.iter().any(|u| &u.id == uid) {
            state
                .oidc
                .store
                .add_user_to_group(&realm, uid, &grp.id)
                .await?;
        }
    }
    let updated_members = state
        .oidc
        .store
        .list_group_members(&realm, allowed.as_deref(), &grp.id, 1000)
        .await?;
    Ok(scim_json_with_etag(
        StatusCode::OK,
        to_scim_group_json(&grp, &updated_members),
    ))
}

async fn scim_delete_group(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let grp = state
        .oidc
        .store
        .get_group(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("group not found".into()))?;
    let dec = state.authorizer.check(
        &ctx,
        Action::Delete,
        &ObjectTarget::Group {
            realm_id: realm.clone(),
            org_id: grp.org_id,
            group_id: grp.id,
            managed_by: grp.managed_by,
        },
    );
    if !dec.allowed {
        return Err(KaisatsuError::NotFound("group not found".into()));
    }
    state
        .oidc
        .store
        .delete_group(&realm, allowed.as_deref(), &id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scim_filter_parses_eq_co_sw_and_and_refuses_deep_recursion_stack_overflow() {
        let f = parse_scim_filter(r#"userName sw "alice" and emails.value co "@hel.fi""#).unwrap();
        let u = UserRepresentation {
            id: "u1".into(),
            username: "alice.smith@hel.fi".into(),
            email: Some("alice.smith@hel.fi".into()),
            ..Default::default()
        };
        assert!(eval_scim_user(&f, &u));

        // Regression test for Kanidm GHSA-2pm5-6m23-h692 / GHSA-r5fr-9gmv-jggh: deeply nested
        // filter must return 400 BadRequest instead of overflowing the stack.
        let deep = format!("{}userName eq \"a\"{}", "(".repeat(200), ")".repeat(200));
        let err = parse_scim_filter(&deep).expect_err("deep filter must be refused");
        assert_eq!(err.status_code(), 400);
    }
}
