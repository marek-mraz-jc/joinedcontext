#![forbid(unsafe_code)]
//! SCIM 2.0 server (`/api/v1/realms/{realm}/scim/v2/{Users,Groups}`) for an organization's HR
//! system (W11, W12):
//! - Maps SCIM `User` and `Group` resources to `People` and `Groups` through the store and
//!   authorization check (`scim` role, scoped to one organization).
//! - Filter subset `eq`, `co`, `sw`, `and` with a strict recursion/nesting depth bound of 16
//!   (regression protection against Kanidm GHSA-2pm5-6m23-h692 and GHSA-r5fr-9gmv-jggh stack overflow).

use crate::compat::{AdminState, extract_admin_ctx};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use jc_router::Mount;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wardkeep_core::*;

pub const MAX_SCIM_FILTER_DEPTH: usize = 16;

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
        return Err(WardkeepError::BadRequest(format!(
            "SCIM filter nesting depth exceeds maximum of {MAX_SCIM_FILTER_DEPTH}"
        )));
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(WardkeepError::BadRequest("empty SCIM filter".into()));
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
        .ok_or_else(|| WardkeepError::BadRequest("missing SCIM filter attribute".into()))?;
    let op_str = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WardkeepError::BadRequest("missing SCIM filter operator".into()))?;
    let op = match op_str.to_ascii_lowercase().as_str() {
        "eq" => ScimOp::Eq,
        "co" => ScimOp::Co,
        "sw" => ScimOp::Sw,
        other => {
            return Err(WardkeepError::BadRequest(format!(
                "unsupported SCIM filter operator: {other}"
            )));
        }
    };
    let after_op = trimmed[attr.len()..].trim_start()[op_str.len()..].trim();
    let value = after_op
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| {
            WardkeepError::BadRequest("SCIM filter value must be double-quoted".into())
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
                    return Err(WardkeepError::BadRequest(
                        "SCIM filter parenthesis depth exceeded".into(),
                    ));
                }
            }
            b')' if !in_quotes => {
                if paren_depth == 0 {
                    return Err(WardkeepError::BadRequest(
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
            get(scim_get_user).delete(scim_delete_user),
        )
        .route(
            "/api/v1/realms/{realm}/scim/v2/Groups",
            get(scim_list_groups).post(scim_create_group),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScimUserPayload {
    pub user_name: String,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub name: Option<ScimName>,
    #[serde(default)]
    pub emails: Vec<ScimEmail>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScimName {
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ScimEmail {
    pub value: String,
}

fn to_scim_user_json(u: &UserRepresentation) -> Value {
    json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "id": u.id,
        "userName": u.username,
        "active": u.enabled,
        "name": {
            "givenName": u.first_name,
            "familyName": u.last_name,
        },
        "emails": u.email.as_ref().map(|e| vec![json!({"value": e, "primary": true})]).unwrap_or_default(),
    })
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
        return Err(WardkeepError::Forbidden(dec.reason));
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

async fn scim_create_user(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Json(payload): Json<ScimUserPayload>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let target = ObjectTarget::Person {
        realm_id: realm.clone(),
        org_id: ctx.active_org_id.clone(),
        person_id: "*".into(),
    };
    let dec = state.authorizer.check(&ctx, Action::Create, &target);
    if !dec.allowed {
        return Err(WardkeepError::Forbidden(dec.reason));
    }
    let email = payload
        .emails
        .first()
        .map(|e| e.value.clone())
        .or_else(|| Some(payload.user_name.clone()));
    let created = state
        .oidc
        .store
        .create_user(UserRepresentation {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm,
            org_id: ctx.active_org_id,
            username: payload.user_name,
            email,
            first_name: payload.name.as_ref().and_then(|n| n.given_name.clone()),
            last_name: payload.name.as_ref().and_then(|n| n.family_name.clone()),
            enabled: payload.active.unwrap_or(true),
            email_verified: true,
            ..Default::default()
        })
        .await?;
    Ok((StatusCode::CREATED, Json(to_scim_user_json(&created))).into_response())
}

async fn scim_get_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<Json<Value>> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let user = state
        .oidc
        .store
        .get_user_by_id(&realm, allowed.as_deref(), &id)
        .await?
        .ok_or_else(|| WardkeepError::NotFound("user not found".into()))?;
    Ok(Json(to_scim_user_json(&user)))
}

async fn scim_delete_user(
    State(state): State<AdminState>,
    Path((realm, id)): Path<(String, String)>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Result<StatusCode> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let allowed = ctx.allowed_orgs();
    let deleted = state
        .oidc
        .store
        .delete_user(&realm, allowed.as_deref(), &id)
        .await?;
    if !deleted {
        return Err(WardkeepError::NotFound("user not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScimGroupPayload {
    pub display_name: String,
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
    let resources: Vec<Value> = groups
        .into_iter()
        .filter(|g| filter.as_ref().is_none_or(|f| eval_scim_group(f, g)))
        .map(|g| {
            json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
                "id": g.id,
                "displayName": g.name,
            })
        })
        .collect();
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
    Json(payload): Json<ScimGroupPayload>,
) -> Result<Response> {
    let mount = state.oidc.mount_from(mount_ext.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(&state, &mount, &realm, &headers).await?;
    let path = match &ctx.active_org_id {
        Some(org) => format!("/{org}/{}", payload.display_name),
        None => format!("/{}", payload.display_name),
    };
    let grp = state
        .oidc
        .store
        .upsert_group(GroupRepresentation {
            id: uuid::Uuid::now_v7().to_string(),
            realm_id: realm,
            org_id: ctx.active_org_id,
            name: payload.display_name.clone(),
            path,
            ..Default::default()
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
            "id": grp.id,
            "displayName": grp.name,
        })),
    )
        .into_response())
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
