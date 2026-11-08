#![forbid(unsafe_code)]
//! Token claims pipeline and protocol mapper evaluation in RI order (W1).

use crate::error::{KaisatsuError, Result};
use crate::model::{
    ClientRepresentation, ClientScopeRepresentation, GroupRepresentation,
    ProtocolMapperRepresentation, RealmConfig, RoleRepresentation, UserRepresentation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Which token or response the mapper pipeline is building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    AccessToken,
    IdToken,
    UserInfo,
    Introspection,
}

/// All protocol mapper kinds supported by kaisatsu (W1).
pub const KNOWN_MAPPER_KINDS: &[&str] = &[
    "oidc-group-membership-mapper",
    "oidc-usermodel-attribute-mapper",
    "oidc-audience-mapper",
    "oidc-amr-mapper",
    "oidc-acr-mapper",
    "oidc-usermodel-property-mapper",
    "oidc-usermodel-realm-role-mapper",
    "oidc-usermodel-client-role-mapper",
    "oidc-hardcoded-claim-mapper",
    "oidc-full-name-mapper",
    "oidc-sub-mapper",
    "oidc-AllowedWebOrigins-mapper",
    "allowed-web-origins",
    "oidc-audience-resolve-mapper",
    "oidc-sha256-pairwise-sub-mapper",
];

#[must_use]
pub fn known_mapper_kinds() -> &'static [&'static str] {
    KNOWN_MAPPER_KINDS
}

#[must_use]
pub fn is_known_mapper_kind(kind: &str) -> bool {
    KNOWN_MAPPER_KINDS.contains(&kind)
}

/// Delegation actor claim (`act`, RFC 8693 §4.1, W1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActClaim {
    pub sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<Box<ActClaim>>,
}

/// Context passed into the mapper pipeline for one token issuance.
#[derive(Debug, Clone)]
pub struct MapperContext<'a> {
    pub realm: &'a RealmConfig,
    pub issuer: &'a str,
    pub client: &'a ClientRepresentation,
    pub user: &'a UserRepresentation,
    pub user_groups: &'a [GroupRepresentation],
    pub all_realm_roles: &'a [RoleRepresentation],
    pub active_scopes: &'a [ClientScopeRepresentation],
    pub granted_scope_string: &'a str,
    pub acr: &'a str,
    pub amr: &'a [String],
    pub session_id: Option<&'a str>,
    pub nonce: Option<&'a str>,
    pub jti: &'a str,
    pub iat: i64,
    pub exp: i64,
    pub auth_time: Option<i64>,
    pub dpop_jkt: Option<&'a str>,
    pub act: Option<ActClaim>,
    pub active_org: Option<&'a str>,
}

fn mapper_applies_to(mapper: &ProtocolMapperRepresentation, kind: TokenKind) -> bool {
    match kind {
        TokenKind::AccessToken => mapper
            .config
            .get("access.token.claim")
            .is_none_or(|v| v == "true"),
        TokenKind::IdToken => mapper
            .config
            .get("id.token.claim")
            .is_some_and(|v| v == "true"),
        TokenKind::UserInfo => mapper
            .config
            .get("userinfo.token.claim")
            .is_some_and(|v| v == "true"),
        TokenKind::Introspection => mapper
            .config
            .get("introspection.token.claim")
            .or_else(|| mapper.config.get("access.token.claim"))
            .is_none_or(|v| v == "true"),
    }
}

/// Inserts a value at a dotted claim path (`realm_access.roles` -> `{"realm_access":{"roles":...}}`).
fn insert_at_claim_path(target: &mut Map<String, Value>, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        target.insert(parts[0].to_owned(), value);
        return;
    }
    let first = parts[0];
    let rest = parts[1..].join(".");
    let entry = target
        .entry(first.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(sub) = entry {
        insert_at_claim_path(sub, &rest, value);
    }
}

/// Resolves composite realm roles with cycle detection (W3).
pub fn resolve_effective_realm_roles(
    direct_roles: &[String],
    group_roles: &[String],
    all_roles: &[RoleRepresentation],
) -> Vec<String> {
    let mut visited = BTreeSet::new();
    let mut stack: Vec<String> = direct_roles
        .iter()
        .chain(group_roles.iter())
        .cloned()
        .collect();
    while let Some(role_name) = stack.pop() {
        if !visited.insert(role_name.clone()) {
            continue;
        }
        if let Some(def) = all_roles
            .iter()
            .find(|r| !r.client_role && r.name == role_name)
        {
            for child in &def.composites.realm {
                if !visited.contains(child) {
                    stack.push(child.clone());
                }
            }
        }
    }
    visited.into_iter().collect()
}

/// Evaluates built-in scope claims and every protocol mapper on the active scopes and client
/// in RI order (W1).
pub fn evaluate_mappers(ctx: &MapperContext<'_>, kind: TokenKind) -> Result<Map<String, Value>> {
    let mut claims = Map::new();

    if kind != TokenKind::UserInfo {
        claims.insert("iss".into(), Value::String(ctx.issuer.to_owned()));
        claims.insert("sub".into(), Value::String(ctx.user.id.clone()));
        claims.insert("azp".into(), Value::String(ctx.client.client_id.clone()));
        claims.insert("jti".into(), Value::String(ctx.jti.to_owned()));
        claims.insert("iat".into(), json!(ctx.iat));
        claims.insert("exp".into(), json!(ctx.exp));
        let typ = match kind {
            TokenKind::AccessToken | TokenKind::Introspection => "Bearer",
            TokenKind::IdToken => "ID",
            TokenKind::UserInfo => "UserInfo",
        };
        claims.insert("typ".into(), Value::String(typ.into()));
        if let Some(sid) = ctx.session_id {
            claims.insert("sid".into(), Value::String(sid.to_owned()));
        }
        if let Some(auth_time) = ctx.auth_time {
            claims.insert("auth_time".into(), json!(auth_time));
        }
        if kind == TokenKind::AccessToken || kind == TokenKind::Introspection {
            claims.insert(
                "scope".into(),
                Value::String(ctx.granted_scope_string.to_owned()),
            );
        }
        if kind == TokenKind::IdToken
            && let Some(nonce) = ctx.nonce
        {
            claims.insert("nonce".into(), Value::String(nonce.to_owned()));
        }
        if !ctx.acr.is_empty() {
            claims.insert("acr".into(), Value::String(ctx.acr.to_owned()));
        }
        if !ctx.amr.is_empty() {
            claims.insert(
                "amr".into(),
                Value::Array(ctx.amr.iter().cloned().map(Value::String).collect()),
            );
        }
    } else {
        claims.insert("sub".into(), Value::String(ctx.user.id.clone()));
    }

    // Standard built-in scope claims (`profile`, `email`, `roles`, `acr`, `basic`)
    let scope_names: BTreeSet<&str> = ctx
        .active_scopes
        .iter()
        .map(|s| s.name.as_str())
        .chain(ctx.granted_scope_string.split_whitespace())
        .collect();

    if (scope_names.contains("profile") || scope_names.contains("basic"))
        && !ctx.user.username.is_empty()
    {
        claims.insert(
            "preferred_username".into(),
            Value::String(ctx.user.username.clone()),
        );
    }
    if scope_names.contains("profile") {
        if let Some(first) = &ctx.user.first_name {
            claims.insert("given_name".into(), Value::String(first.clone()));
        }
        if let Some(last) = &ctx.user.last_name {
            claims.insert("family_name".into(), Value::String(last.clone()));
        }
        let full = format!(
            "{} {}",
            ctx.user.first_name.as_deref().unwrap_or_default(),
            ctx.user.last_name.as_deref().unwrap_or_default()
        )
        .trim()
        .to_owned();
        if !full.is_empty() {
            claims.insert("name".into(), Value::String(full));
        }
    }
    if scope_names.contains("email") {
        if let Some(email) = &ctx.user.email {
            claims.insert("email".into(), Value::String(email.clone()));
        }
        claims.insert(
            "email_verified".into(),
            Value::Bool(ctx.user.email_verified),
        );
    }

    let group_realm_roles: Vec<String> = ctx
        .user_groups
        .iter()
        .flat_map(|g| g.realm_roles.iter().cloned())
        .collect();
    let effective_realm_roles = resolve_effective_realm_roles(
        &ctx.user.realm_roles,
        &group_realm_roles,
        ctx.all_realm_roles,
    );

    if scope_names.contains("roles")
        && ctx.client.full_scope_allowed
        && kind == TokenKind::AccessToken
    {
        if !effective_realm_roles.is_empty() {
            insert_at_claim_path(
                &mut claims,
                "realm_access.roles",
                json!(effective_realm_roles),
            );
        }
        if !ctx.user.client_roles.is_empty() {
            let mut ra = Map::new();
            for (c, r) in &ctx.user.client_roles {
                if !r.is_empty() {
                    ra.insert(c.clone(), json!({ "roles": r }));
                }
            }
            if !ra.is_empty() {
                claims.insert("resource_access".into(), Value::Object(ra));
            }
        }
    }

    let mut audiences: Vec<String> = Vec::new();
    if kind == TokenKind::IdToken {
        audiences.push(ctx.client.client_id.clone());
    }

    // Collect mappers in RI order: active client scopes first, then client's own protocolMappers.
    let all_mappers = ctx
        .active_scopes
        .iter()
        .flat_map(|s| s.protocol_mappers.iter())
        .chain(ctx.client.protocol_mappers.iter());

    for mapper in all_mappers {
        if !is_known_mapper_kind(&mapper.protocol_mapper) {
            return Err(KaisatsuError::BadRequest(format!(
                "unknown protocol mapper kind: {}",
                mapper.protocol_mapper
            )));
        }
        if !mapper_applies_to(mapper, kind) {
            continue;
        }
        match mapper.protocol_mapper.as_str() {
            "oidc-audience-mapper" => {
                if let Some(aud) = mapper
                    .config
                    .get("included.custom.audience")
                    .or_else(|| mapper.config.get("included.client.audience"))
                    .filter(|s| !s.is_empty())
                    && !audiences.contains(aud)
                {
                    audiences.push(aud.clone());
                }
            }
            "oidc-group-membership-mapper" => {
                let claim_name = mapper
                    .config
                    .get("claim.name")
                    .map_or("groups", String::as_str);
                let full_path = mapper.config.get("full.path").is_none_or(|v| v == "true");
                let mut groups: Vec<String> = ctx
                    .user_groups
                    .iter()
                    .map(|g| {
                        if full_path {
                            if g.path.starts_with('/') {
                                g.path.clone()
                            } else {
                                format!("/{}", g.path)
                            }
                        } else {
                            g.name.clone()
                        }
                    })
                    .collect();
                for path in &ctx.user.groups {
                    let formatted = if full_path {
                        if path.starts_with('/') {
                            path.clone()
                        } else {
                            format!("/{path}")
                        }
                    } else {
                        path.rsplit('/').next().unwrap_or(path).to_owned()
                    };
                    if !groups.contains(&formatted) {
                        groups.push(formatted);
                    }
                }
                groups.sort();
                groups.dedup();
                insert_at_claim_path(&mut claims, claim_name, json!(groups));
            }
            "oidc-usermodel-attribute-mapper" => {
                let attr_name = mapper
                    .config
                    .get("user.attribute")
                    .or_else(|| mapper.config.get("claim.name"))
                    .cloned()
                    .unwrap_or_default();
                let claim_name = mapper
                    .config
                    .get("claim.name")
                    .cloned()
                    .unwrap_or_else(|| attr_name.clone());
                let aggregate = mapper
                    .config
                    .get("aggregate.attrs")
                    .is_some_and(|v| v == "true");
                let multivalued = mapper
                    .config
                    .get("multivalued")
                    .is_some_and(|v| v == "true");

                let mut vals: Vec<String> = Vec::new();
                // Group attributes feed `jc_org` and `jc_groups` (ADR-N-067 2.2)
                for g in ctx.user_groups {
                    if let Some(gv) = g.attributes.get(&attr_name) {
                        for item in gv {
                            if !vals.contains(item) {
                                vals.push(item.clone());
                            }
                        }
                    }
                }
                if aggregate || vals.is_empty() {
                    // Only allow managed user attributes; `jc_org` and `jc_groups` come from groups
                    // unless explicitly present on a transient user.
                    if (ctx.user.transient || !matches!(attr_name.as_str(), "jc_org" | "jc_groups"))
                        && let Some(uv) = ctx.user.attributes.get(&attr_name)
                    {
                        for item in uv {
                            if !vals.contains(item) {
                                vals.push(item.clone());
                            }
                        }
                    }
                }
                if attr_name == "jc_org"
                    && let Some(pinned_org) = ctx.active_org
                    && !vals.iter().any(|v| v == pinned_org)
                {
                    vals.push(pinned_org.to_owned());
                }
                if !vals.is_empty() {
                    let val = if multivalued {
                        json!(vals)
                    } else {
                        json!(vals[0])
                    };
                    insert_at_claim_path(&mut claims, &claim_name, val);
                }
            }
            "oidc-amr-mapper" => {
                claims.insert(
                    "amr".into(),
                    Value::Array(ctx.amr.iter().cloned().map(Value::String).collect()),
                );
            }
            "oidc-acr-mapper" => {
                if !ctx.acr.is_empty() {
                    claims.insert("acr".into(), Value::String(ctx.acr.to_owned()));
                }
            }
            "oidc-usermodel-realm-role-mapper" => {
                let claim_name = mapper
                    .config
                    .get("claim.name")
                    .map_or("realm_access.roles", String::as_str);
                insert_at_claim_path(&mut claims, claim_name, json!(effective_realm_roles));
            }
            "oidc-usermodel-client-role-mapper" => {
                let target_client = mapper
                    .config
                    .get("usermodel.clientRoleMapping.clientId")
                    .map_or(ctx.client.client_id.as_str(), String::as_str);
                let claim_name = mapper
                    .config
                    .get("claim.name")
                    .cloned()
                    .unwrap_or_else(|| format!("resource_access.{target_client}.roles"));
                let roles = ctx
                    .user
                    .client_roles
                    .get(target_client)
                    .cloned()
                    .unwrap_or_default();
                if !roles.is_empty() {
                    insert_at_claim_path(&mut claims, &claim_name, json!(roles));
                }
            }
            "oidc-usermodel-property-mapper" => {
                let prop = mapper
                    .config
                    .get("user.attribute")
                    .map_or("", String::as_str);
                let claim_name = mapper.config.get("claim.name").map_or(prop, String::as_str);
                let val = match prop {
                    "id" => Some(ctx.user.id.clone()),
                    "username" => Some(ctx.user.username.clone()),
                    "email" => ctx.user.email.clone(),
                    "firstName" => ctx.user.first_name.clone(),
                    "lastName" => ctx.user.last_name.clone(),
                    _ => None,
                };
                if let Some(v) = val {
                    insert_at_claim_path(&mut claims, claim_name, Value::String(v));
                }
            }
            "oidc-hardcoded-claim-mapper" => {
                if let (Some(claim_name), Some(claim_val)) = (
                    mapper.config.get("claim.name"),
                    mapper.config.get("claim.value"),
                ) {
                    let json_type = mapper
                        .config
                        .get("jsonType.label")
                        .map_or("String", String::as_str);
                    let val = match json_type {
                        "boolean" => Value::Bool(claim_val == "true"),
                        "int" | "long" => claim_val
                            .parse::<i64>()
                            .map_or_else(|_| Value::String(claim_val.clone()), |n| json!(n)),
                        "JSON" => serde_json::from_str(claim_val)
                            .unwrap_or_else(|_| Value::String(claim_val.clone())),
                        _ => Value::String(claim_val.clone()),
                    };
                    insert_at_claim_path(&mut claims, claim_name, val);
                }
            }
            "oidc-full-name-mapper" => {
                let full = format!(
                    "{} {}",
                    ctx.user.first_name.as_deref().unwrap_or_default(),
                    ctx.user.last_name.as_deref().unwrap_or_default()
                )
                .trim()
                .to_owned();
                if !full.is_empty() {
                    claims.insert("name".into(), Value::String(full));
                }
            }
            "oidc-sub-mapper" | "oidc-sha256-pairwise-sub-mapper" => {
                claims.insert("sub".into(), Value::String(ctx.user.id.clone()));
            }
            "allowed-web-origins" | "oidc-AllowedWebOrigins-mapper" => {
                if !ctx.client.web_origins.is_empty() {
                    claims.insert("allowed-origins".into(), json!(ctx.client.web_origins));
                }
            }
            "oidc-audience-resolve-mapper" => {
                for c in ctx.user.client_roles.keys() {
                    if !audiences.contains(c) {
                        audiences.push(c.clone());
                    }
                }
            }
            _ => {}
        }
    }

    if kind != TokenKind::UserInfo && !audiences.is_empty() {
        let aud_value = if audiences.len() == 1 {
            Value::String(audiences.remove(0))
        } else {
            Value::Array(audiences.into_iter().map(Value::String).collect())
        };
        claims.insert("aud".into(), aud_value);
    }

    if let Some(jkt) = ctx.dpop_jkt
        && (kind == TokenKind::AccessToken || kind == TokenKind::Introspection)
    {
        claims.insert("cnf".into(), json!({ "jkt": jkt }));
    }

    if let Some(act) = &ctx.act
        && let Ok(v) = serde_json::to_value(act)
    {
        claims.insert("act".into(), v);
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn evaluates_groups_jc_org_jc_groups_and_audience_mappers_in_ri_order() {
        let realm = RealmConfig::default();
        let client = ClientRepresentation {
            client_id: "edge".into(),
            protocol_mappers: vec![
                ProtocolMapperRepresentation {
                    name: "groups".into(),
                    protocol_mapper: "oidc-group-membership-mapper".into(),
                    config: BTreeMap::from([
                        ("claim.name".into(), "groups".into()),
                        ("full.path".into(), "true".into()),
                        ("access.token.claim".into(), "true".into()),
                    ]),
                    ..Default::default()
                },
                ProtocolMapperRepresentation {
                    name: "jc_org".into(),
                    protocol_mapper: "oidc-usermodel-attribute-mapper".into(),
                    config: BTreeMap::from([
                        ("user.attribute".into(), "jc_org".into()),
                        ("claim.name".into(), "jc_org".into()),
                        ("multivalued".into(), "true".into()),
                        ("aggregate.attrs".into(), "true".into()),
                        ("access.token.claim".into(), "true".into()),
                    ]),
                    ..Default::default()
                },
                ProtocolMapperRepresentation {
                    name: "jc_groups".into(),
                    protocol_mapper: "oidc-usermodel-attribute-mapper".into(),
                    config: BTreeMap::from([
                        ("user.attribute".into(), "jc_groups".into()),
                        ("claim.name".into(), "jc_groups".into()),
                        ("multivalued".into(), "true".into()),
                        ("aggregate.attrs".into(), "true".into()),
                        ("access.token.claim".into(), "true".into()),
                    ]),
                    ..Default::default()
                },
                ProtocolMapperRepresentation {
                    name: "portal-api-audience".into(),
                    protocol_mapper: "oidc-audience-mapper".into(),
                    config: BTreeMap::from([
                        ("included.client.audience".into(), "portal-api".into()),
                        ("access.token.claim".into(), "true".into()),
                    ]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let user = UserRepresentation {
            id: "de300000-0000-4000-8000-00000000000f".into(),
            username: "demo.orgmember@hel.fi".into(),
            email: Some("demo.orgmember@hel.fi".into()),
            email_verified: true,
            ..Default::default()
        };
        let group = GroupRepresentation {
            id: "g1".into(),
            name: "members".into(),
            path: "/helsinki/members".into(),
            attributes: HashMap::from([
                ("jc_org".into(), vec!["helsinki".into()]),
                ("jc_groups".into(), vec!["/helsinki/members".into()]),
            ]),
            ..Default::default()
        };
        let ctx = MapperContext {
            realm: &realm,
            issuer: "https://idm.example.test/realms/dev",
            client: &client,
            user: &user,
            user_groups: &[group],
            all_realm_roles: &[],
            active_scopes: &[],
            granted_scope_string: "openid profile email",
            acr: "mfa",
            amr: &["pwd".into(), "otp".into()],
            session_id: Some("s1"),
            nonce: None,
            jti: "j1",
            iat: 1000,
            exp: 1300,
            auth_time: Some(1000),
            dpop_jkt: None,
            act: None,
            active_org: None,
        };
        let claims = evaluate_mappers(&ctx, TokenKind::AccessToken).expect("claims");
        assert_eq!(claims.get("groups"), Some(&json!(["/helsinki/members"])));
        assert_eq!(claims.get("jc_org"), Some(&json!(["helsinki"])));
        assert_eq!(claims.get("jc_groups"), Some(&json!(["/helsinki/members"])));
        assert_eq!(claims.get("aud"), Some(&json!("portal-api")));
        assert_eq!(claims.get("acr"), Some(&json!("mfa")));
        assert_eq!(claims.get("amr"), Some(&json!(["pwd", "otp"])));
    }
}
