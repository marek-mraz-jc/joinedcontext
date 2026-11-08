#![forbid(unsafe_code)]
//! OData v4.01 entity set definitions using `odata-table` (W4).
//!
//! Declares all 21 entity sets once: `Realms`, `Organizations`, `OrganizationDomains`, `People`,
//! `Memberships`, `Groups`, `GroupMembers`, `Roles`, `RoleMappings`, `Applications`,
//! `ClientScopes`, `ProtocolMappers`, `IdentityProviders`, `Credentials`, `Sessions`,
//! `ClientSessions`, `OfflineSessions`, `Consents`, `Events`, `AdminEvents`, `SigningKeys`,
//! `RequiredActions`, `BrandingBlocks`.
//! Secret fields (`passwordHash`, `secret`, `secretData`, `encryptedPrivateKey`) are never
//! declared in the entity sets, so `$filter=passwordHash ne null` and `$orderby=passwordHash`
//! fail with `UnknownProperty` (400 Bad Request, W4).

use crate::authz::{Action, Authorizer, ObjectTarget, RequestCtx};
use crate::branding::BrandingBlock;
use crate::model::*;
use odata_table::{EntityDef, Field, FieldKind, Operation, Rule, Scope, Service};
use serde_json::{Value, json};

fn req_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        nullable: false,
    }
}

fn opt_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field::new(name, kind, description)
}

fn set_rules(write_perm: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some("read".into()),
        },
        Rule {
            operation: Operation::Export,
            permission: Some("read".into()),
        },
        Rule {
            operation: Operation::Create,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Update,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Delete,
            permission: Some(write_perm.to_owned()),
        },
    ]
}

fn read_only_rules() -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some("read".into()),
        },
        Rule {
            operation: Operation::Export,
            permission: Some("read".into()),
        },
    ]
}

/// Builds the complete `odata-table::Service` for kaisatsu (W4).
#[must_use]
pub fn kaisatsu_odata_service() -> Service {
    Service {
        namespace: "JoinedContext.Kaisatsu".into(),
        title: "kaisatsu OData Identity Service".into(),
        sets: vec![
            EntityDef {
                set: "Realms".into(),
                type_name: "Realm".into(),
                description: "Realms hosted by kaisatsu.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Realm ID."),
                    req_field("realm", FieldKind::String, "Realm slug."),
                    req_field("displayName", FieldKind::String, "Display name."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    req_field(
                        "defaultSignatureAlgorithm",
                        FieldKind::String,
                        "Default JWS algorithm.",
                    ),
                    req_field(
                        "accessTokenLifespan",
                        FieldKind::Integer,
                        "Access token TTL (s).",
                    ),
                    req_field(
                        "ssoSessionIdleTimeout",
                        FieldKind::Integer,
                        "SSO idle timeout (s).",
                    ),
                    req_field(
                        "ssoSessionMaxLifespan",
                        FieldKind::Integer,
                        "SSO max lifespan (s).",
                    ),
                ],
                rules: set_rules("write:Realm"),
            },
            EntityDef {
                set: "Organizations".into(),
                type_name: "Organization".into(),
                description: "Organizations inside a realm.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Organization ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    req_field("name", FieldKind::String, "Organization name."),
                    req_field("alias", FieldKind::String, "Organization alias."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    opt_field("description", FieldKind::String, "Description."),
                ],
                rules: set_rules("write:Organization"),
            },
            EntityDef {
                set: "OrganizationDomains".into(),
                type_name: "OrganizationDomain".into(),
                description: "Verified and pending domains of organizations.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Domain record ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    req_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("name", FieldKind::String, "DNS domain name."),
                    req_field(
                        "verified",
                        FieldKind::Boolean,
                        "Whether DNS TXT is verified.",
                    ),
                ],
                rules: set_rules("write:Organization"),
            },
            EntityDef {
                set: "People".into(),
                type_name: "Person".into(),
                description: "People and service-account users in the realm.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Fixed user UUID (PF-126)."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Primary organization ID."),
                    req_field("username", FieldKind::String, "Lowercase username."),
                    opt_field("email", FieldKind::String, "Email address."),
                    opt_field("firstName", FieldKind::String, "Given name."),
                    opt_field("lastName", FieldKind::String, "Family name."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    req_field("emailVerified", FieldKind::Boolean, "Email verified flag."),
                    opt_field("createdTimestamp", FieldKind::Integer, "Creation epoch ms."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "Memberships".into(),
                type_name: "Membership".into(),
                description: "Organization memberships of people.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Membership ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    req_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("person_id", FieldKind::String, "Person ID."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "Groups".into(),
                type_name: "Group".into(),
                description: "Groups and organization group subtrees.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Group ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    opt_field("parent_id", FieldKind::String, "Parent group ID."),
                    req_field("name", FieldKind::String, "Group name."),
                    req_field("path", FieldKind::String, "Full group path."),
                    opt_field("managed_by", FieldKind::String, "Managed-by marker."),
                ],
                rules: set_rules("write:Group"),
            },
            EntityDef {
                set: "GroupMembers".into(),
                type_name: "GroupMember".into(),
                description: "Group membership tuples.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Membership ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("group_id", FieldKind::String, "Group ID."),
                    req_field("user_id", FieldKind::String, "User ID."),
                ],
                rules: set_rules("write:Group"),
            },
            EntityDef {
                set: "Roles".into(),
                type_name: "Role".into(),
                description: "Realm and client roles.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Role ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("name", FieldKind::String, "Role name."),
                    opt_field("description", FieldKind::String, "Role description."),
                    req_field("composite", FieldKind::Boolean, "Composite flag."),
                    req_field("clientRole", FieldKind::Boolean, "Client role flag."),
                    opt_field(
                        "containerId",
                        FieldKind::String,
                        "Client UUID for client roles.",
                    ),
                ],
                rules: set_rules("write:Role"),
            },
            EntityDef {
                set: "RoleMappings".into(),
                type_name: "RoleMapping".into(),
                description: "Role assignments to users and groups.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Role mapping ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("principal_kind", FieldKind::String, "user or group."),
                    req_field("principal_id", FieldKind::String, "User or group ID."),
                    req_field("role_id", FieldKind::String, "Role ID."),
                    req_field("role_name", FieldKind::String, "Role name."),
                    opt_field(
                        "client_uuid",
                        FieldKind::String,
                        "Client UUID if client role.",
                    ),
                ],
                rules: set_rules("write:Role"),
            },
            EntityDef {
                set: "Applications".into(),
                type_name: "Application".into(),
                description: "OIDC clients (applications and workloads).".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Client internal UUID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("clientId", FieldKind::String, "OIDC client_id."),
                    opt_field("name", FieldKind::String, "Display name."),
                    opt_field("description", FieldKind::String, "Description."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    req_field("publicClient", FieldKind::Boolean, "Public client flag."),
                    req_field("bearerOnly", FieldKind::Boolean, "Bearer-only flag."),
                    req_field(
                        "standardFlowEnabled",
                        FieldKind::Boolean,
                        "Code flow enabled.",
                    ),
                    req_field(
                        "directAccessGrantsEnabled",
                        FieldKind::Boolean,
                        "Password grant enabled.",
                    ),
                    req_field(
                        "serviceAccountsEnabled",
                        FieldKind::Boolean,
                        "Service account enabled.",
                    ),
                    req_field(
                        "clientAuthenticatorType",
                        FieldKind::String,
                        "Authenticator type.",
                    ),
                    opt_field("managed_by", FieldKind::String, "Managed-by marker."),
                ],
                rules: set_rules("write:Application"),
            },
            EntityDef {
                set: "ClientScopes".into(),
                type_name: "ClientScope".into(),
                description: "OIDC client scopes.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Scope ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("name", FieldKind::String, "Scope name."),
                    opt_field("description", FieldKind::String, "Description."),
                    req_field("protocol", FieldKind::String, "Protocol."),
                ],
                rules: set_rules("write:ClientScope"),
            },
            EntityDef {
                set: "ProtocolMappers".into(),
                type_name: "ProtocolMapper".into(),
                description: "Protocol mappers on clients and client scopes.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Mapper ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("name", FieldKind::String, "Mapper name."),
                    req_field("protocol", FieldKind::String, "Protocol."),
                    req_field("protocolMapper", FieldKind::String, "Mapper kind."),
                    req_field("consentRequired", FieldKind::Boolean, "Consent required."),
                ],
                rules: set_rules("write:Application"),
            },
            EntityDef {
                set: "IdentityProviders".into(),
                type_name: "IdentityProvider".into(),
                description: "Upstream OIDC and Kubernetes identity providers.".into(),
                key: "internalId".into(),
                fields: vec![
                    req_field("internalId", FieldKind::String, "Internal ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("alias", FieldKind::String, "IdP alias."),
                    req_field("providerId", FieldKind::String, "Provider type."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    req_field("trustEmail", FieldKind::Boolean, "Trust email flag."),
                ],
                rules: set_rules("write:Realm"),
            },
            EntityDef {
                set: "Credentials".into(),
                type_name: "Credential".into(),
                description: "Credential metadata only (never hashes or secrets, W4).".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Credential ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "Owner user ID."),
                    req_field(
                        "type",
                        FieldKind::String,
                        "Credential type (password, otp, webauthn).",
                    ),
                    opt_field("userLabel", FieldKind::String, "User label."),
                    req_field("createdDate", FieldKind::Integer, "Created epoch ms."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "Sessions".into(),
                type_name: "Session".into(),
                description: "Active SSO browser sessions.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Session ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "User ID."),
                    req_field("username", FieldKind::String, "Username."),
                    req_field("ipAddress", FieldKind::String, "Client IP."),
                    req_field("start", FieldKind::Integer, "Start epoch ms."),
                    req_field("lastAccess", FieldKind::Integer, "Last access epoch ms."),
                    req_field("acr", FieldKind::String, "Authentication context class."),
                ],
                rules: set_rules("write:Session"),
            },
            EntityDef {
                set: "ClientSessions".into(),
                type_name: "ClientSession".into(),
                description: "Per-client sessions inside SSO sessions.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Client session ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field(
                        "sso_session_id",
                        FieldKind::String,
                        "Parent SSO session ID.",
                    ),
                    req_field("client_id", FieldKind::String, "Client ID."),
                    req_field("user_id", FieldKind::String, "User ID."),
                ],
                rules: read_only_rules(),
            },
            EntityDef {
                set: "OfflineSessions".into(),
                type_name: "OfflineSession".into(),
                description: "Offline refresh sessions.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Offline session ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "User ID."),
                    req_field("client_id", FieldKind::String, "Client ID."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                    req_field("last_access", FieldKind::DateTime, "Last access timestamp."),
                ],
                rules: set_rules("write:Session"),
            },
            EntityDef {
                set: "Consents".into(),
                type_name: "Consent".into(),
                description: "Granted user consents per client.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Consent ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "User ID."),
                    req_field("clientId", FieldKind::String, "Client ID."),
                    req_field("createdDate", FieldKind::Integer, "Created epoch ms."),
                    req_field("lastUpdatedDate", FieldKind::Integer, "Updated epoch ms."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "Events".into(),
                type_name: "Event".into(),
                description: "Authentication and token audit events.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Event ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("time", FieldKind::Integer, "Event epoch ms."),
                    req_field("type", FieldKind::String, "Event type."),
                    opt_field("clientId", FieldKind::String, "Client ID."),
                    opt_field("userId", FieldKind::String, "User ID."),
                    req_field("ipAddress", FieldKind::String, "Source IP."),
                    opt_field("error", FieldKind::String, "Error code if failed."),
                ],
                rules: read_only_rules(),
            },
            EntityDef {
                set: "AdminEvents".into(),
                type_name: "AdminEvent".into(),
                description: "Admin write audit events.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Admin event ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("time", FieldKind::Integer, "Event epoch ms."),
                    req_field("operationType", FieldKind::String, "Operation type."),
                    req_field("resourceType", FieldKind::String, "Resource type."),
                    req_field("resourcePath", FieldKind::String, "Resource path."),
                    req_field("actorId", FieldKind::String, "Actor ID."),
                ],
                rules: read_only_rules(),
            },
            EntityDef {
                set: "SigningKeys".into(),
                type_name: "SigningKey".into(),
                description: "Public signing key metadata and rotation dates (W4, W10).".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Key record ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    req_field("kid", FieldKind::String, "JWK key ID."),
                    req_field(
                        "algorithm",
                        FieldKind::String,
                        "Algorithm (ES256, RS256, EdDSA).",
                    ),
                    req_field(
                        "status",
                        FieldKind::String,
                        "Status (ACTIVE, PASSIVE, RETIRED).",
                    ),
                    req_field("priority", FieldKind::Integer, "Key priority."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                    opt_field("activate_at", FieldKind::DateTime, "Activation timestamp."),
                    opt_field("expires_at", FieldKind::DateTime, "Retirement timestamp."),
                ],
                rules: read_only_rules(),
            },
            EntityDef {
                set: "RequiredActions".into(),
                type_name: "RequiredAction".into(),
                description: "Configured required actions in the realm.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Required action ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    req_field("alias", FieldKind::String, "Action alias."),
                    req_field("name", FieldKind::String, "Display name."),
                    req_field("providerId", FieldKind::String, "Provider ID."),
                    req_field("enabled", FieldKind::Boolean, "Enabled flag."),
                    req_field("defaultAction", FieldKind::Boolean, "Default action flag."),
                    req_field("priority", FieldKind::Integer, "Priority."),
                ],
                rules: set_rules("write:Realm"),
            },
            EntityDef {
                set: "BrandingBlocks".into(),
                type_name: "BrandingBlock".into(),
                description: "Per-realm and per-organization branding blocks.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Branding block ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("instanceName", FieldKind::String, "Instance name."),
                    req_field("shortName", FieldKind::String, "Short name."),
                    req_field(
                        "organisation",
                        FieldKind::String,
                        "Organisation display name.",
                    ),
                    req_field(
                        "primaryForeground",
                        FieldKind::String,
                        "Computed WCAG foreground.",
                    ),
                ],
                rules: set_rules("write:Organization"),
            },
        ],
    }
}

/// Builds the `odata_table::Scope` for `ctx` and `set_name` before `$filter` runs (W4, W6).
pub fn scope_for_context(ctx: &RequestCtx, authorizer: &dyn Authorizer, set_name: &str) -> Scope {
    let mut perms = Vec::new();
    let sample_org = ctx.active_org_id.clone();

    let read_target = match set_name {
        "Realms" | "IdentityProviders" | "SigningKeys" | "RequiredActions" => ObjectTarget::Realm {
            realm_id: ctx.realm_id.clone(),
        },
        "Organizations" | "OrganizationDomains" | "BrandingBlocks" => ObjectTarget::Organization {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone().unwrap_or_default(),
        },
        "People" | "Memberships" | "Credentials" | "Consents" => ObjectTarget::Person {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            person_id: "*".into(),
        },
        "Groups" | "GroupMembers" => ObjectTarget::Group {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            group_id: "*".into(),
            managed_by: Some("joinedcontext".into()),
        },
        "Roles" | "RoleMappings" => ObjectTarget::Role {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            role_id: "*".into(),
        },
        "Applications" | "ProtocolMappers" => ObjectTarget::Application {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            client_id: "*".into(),
            managed_by: Some("joinedcontext".into()),
        },
        "ClientScopes" => ObjectTarget::ClientScope {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            scope_id: "*".into(),
        },
        "Sessions" | "ClientSessions" | "OfflineSessions" => ObjectTarget::Session {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
            person_id: "*".into(),
        },
        _ => ObjectTarget::Audit {
            realm_id: ctx.realm_id.clone(),
            org_id: sample_org.clone(),
        },
    };

    if authorizer.check(ctx, Action::Read, &read_target).allowed {
        perms.push("read");
    }
    if authorizer.check(ctx, Action::Update, &read_target).allowed {
        perms.push("write:Realm");
        perms.push("write:Organization");
        perms.push("write:Person");
        perms.push("write:Group");
        perms.push("write:Role");
        perms.push("write:Application");
        perms.push("write:ClientScope");
        perms.push("write:Session");
    }

    let mut scope = Scope::default().holding(&perms);
    if set_name == "Realms" {
        scope = scope.and(&["id"], vec![Value::String(ctx.realm_id.clone())]);
    } else {
        scope = scope.and(&["realm_id"], vec![Value::String(ctx.realm_id.clone())]);
    }

    let org_scoped_sets = [
        "Organizations",
        "OrganizationDomains",
        "People",
        "Memberships",
        "Groups",
        "GroupMembers",
        "Roles",
        "RoleMappings",
        "Applications",
        "ClientScopes",
        "ProtocolMappers",
        "IdentityProviders",
        "Credentials",
        "Sessions",
        "ClientSessions",
        "OfflineSessions",
        "Consents",
        "Events",
        "AdminEvents",
        "BrandingBlocks",
    ];
    if org_scoped_sets.contains(&set_name)
        && let Some(allowed) = ctx.allowed_orgs()
    {
        let field = if set_name == "Organizations" {
            "id"
        } else {
            "org_id"
        };
        scope = scope.and(&[field], allowed.into_iter().map(Value::String).collect());
    }

    scope
}

pub fn realm_to_row(r: &RealmConfig) -> Value {
    json!({
        "id": r.id,
        "realm": r.realm,
        "displayName": r.display_name,
        "enabled": r.enabled,
        "defaultSignatureAlgorithm": r.default_signature_algorithm,
        "accessTokenLifespan": r.access_token_lifespan,
        "ssoSessionIdleTimeout": r.sso_session_idle_timeout,
        "ssoSessionMaxLifespan": r.sso_session_max_lifespan,
    })
}

pub fn org_to_row(o: &Organization) -> Value {
    json!({
        "id": o.id,
        "realm_id": o.realm_id,
        "name": o.name,
        "alias": o.alias,
        "enabled": o.enabled,
        "description": o.description,
    })
}

pub fn org_domain_to_row(d: &OrganizationDomain) -> Value {
    json!({
        "id": d.id,
        "realm_id": d.realm_id,
        "org_id": d.org_id,
        "name": d.name,
        "verified": d.verified,
    })
}

pub fn person_to_row(u: &UserRepresentation) -> Value {
    json!({
        "id": u.id,
        "realm_id": u.realm_id,
        "org_id": u.org_id,
        "username": u.username,
        "email": u.email,
        "firstName": u.first_name,
        "lastName": u.last_name,
        "enabled": u.enabled,
        "emailVerified": u.email_verified,
        "createdTimestamp": u.created_timestamp,
    })
}

pub fn membership_to_row(m: &Membership) -> Value {
    json!({
        "id": m.id,
        "realm_id": m.realm_id,
        "org_id": m.org_id,
        "person_id": m.person_id,
        "created_at": m.created_at.to_rfc3339(),
    })
}

pub fn group_to_row(g: &GroupRepresentation) -> Value {
    json!({
        "id": g.id,
        "realm_id": g.realm_id,
        "org_id": g.org_id,
        "parent_id": g.parent_id,
        "name": g.name,
        "path": g.path,
        "managed_by": g.managed_by,
    })
}

pub fn group_member_to_row(m: &GroupMemberRecord) -> Value {
    json!({
        "id": m.id,
        "realm_id": m.realm_id,
        "org_id": m.org_id,
        "group_id": m.group_id,
        "user_id": m.user_id,
    })
}

pub fn role_to_row(r: &RoleRepresentation) -> Value {
    json!({
        "id": r.id,
        "realm_id": r.realm_id,
        "org_id": r.org_id,
        "name": r.name,
        "description": r.description,
        "composite": r.composite,
        "clientRole": r.client_role,
        "containerId": r.container_id,
    })
}

pub fn role_mapping_to_row(rm: &RoleMappingRecord) -> Value {
    json!({
        "id": rm.id,
        "realm_id": rm.realm_id,
        "org_id": rm.org_id,
        "principal_kind": rm.principal_kind,
        "principal_id": rm.principal_id,
        "role_id": rm.role_id,
        "role_name": rm.role_name,
        "client_uuid": rm.client_uuid,
    })
}

pub fn client_to_row(c: &ClientRepresentation) -> Value {
    json!({
        "id": c.id,
        "realm_id": c.realm_id,
        "org_id": c.org_id,
        "clientId": c.client_id,
        "name": c.name,
        "description": c.description,
        "enabled": c.enabled,
        "publicClient": c.public_client,
        "bearerOnly": c.bearer_only,
        "standardFlowEnabled": c.standard_flow_enabled,
        "directAccessGrantsEnabled": c.direct_access_grants_enabled,
        "serviceAccountsEnabled": c.service_accounts_enabled,
        "clientAuthenticatorType": c.client_authenticator_type,
        "managed_by": c.managed_by,
    })
}

pub fn scope_to_row(s: &ClientScopeRepresentation) -> Value {
    json!({
        "id": s.id,
        "realm_id": s.realm_id,
        "org_id": s.org_id,
        "name": s.name,
        "description": s.description,
        "protocol": s.protocol,
    })
}

pub fn mapper_to_row(m: &ProtocolMapperRepresentation) -> Value {
    json!({
        "id": m.id,
        "realm_id": m.realm_id,
        "org_id": m.org_id,
        "name": m.name,
        "protocol": m.protocol,
        "protocolMapper": m.protocol_mapper,
        "consentRequired": m.consent_required,
    })
}

pub fn idp_to_row(idp: &IdentityProviderRepresentation) -> Value {
    json!({
        "internalId": idp.internal_id,
        "realm_id": idp.realm_id,
        "org_id": idp.org_id,
        "alias": idp.alias,
        "providerId": idp.provider_id,
        "enabled": idp.enabled,
        "trustEmail": idp.trust_email,
    })
}

pub fn credential_to_row(c: &CredentialRecord, org_id: Option<&str>) -> Value {
    json!({
        "id": c.id,
        "realm_id": c.realm_id,
        "org_id": org_id,
        "user_id": c.user_id,
        "type": c.credential_type,
        "userLabel": c.user_label,
        "createdDate": c.created_date,
    })
}

pub fn session_to_row(s: &SsoSession) -> Value {
    json!({
        "id": s.id,
        "realm_id": s.realm_id,
        "org_id": s.org_id,
        "user_id": s.user_id,
        "username": s.username,
        "ipAddress": s.ip_address,
        "start": s.start,
        "lastAccess": s.last_access,
        "acr": s.acr,
    })
}

pub fn client_session_to_row(cs: &ClientSession) -> Value {
    json!({
        "id": cs.id,
        "realm_id": cs.realm_id,
        "org_id": cs.org_id,
        "sso_session_id": cs.sso_session_id,
        "client_id": cs.client_id,
        "user_id": cs.user_id,
    })
}

pub fn offline_session_to_row(os: &OfflineSession) -> Value {
    json!({
        "id": os.id,
        "realm_id": os.realm_id,
        "org_id": os.org_id,
        "user_id": os.user_id,
        "client_id": os.client_id,
        "created_at": os.created_at.to_rfc3339(),
        "last_access": os.last_access.to_rfc3339(),
    })
}

pub fn consent_to_row(c: &ConsentRecord) -> Value {
    json!({
        "id": c.id,
        "realm_id": c.realm_id,
        "org_id": c.org_id,
        "user_id": c.user_id,
        "clientId": c.client_id,
        "createdDate": c.created_date,
        "lastUpdatedDate": c.last_updated_date,
    })
}

pub fn event_to_row(e: &AuditEvent) -> Value {
    json!({
        "id": e.id,
        "realm_id": e.realm_id,
        "org_id": e.org_id,
        "time": e.time,
        "type": e.event_type,
        "clientId": e.client_id,
        "userId": e.user_id,
        "ipAddress": e.ip_address,
        "error": e.error,
    })
}

pub fn admin_event_to_row(ae: &AdminEvent) -> Value {
    json!({
        "id": ae.id,
        "realm_id": ae.realm_id,
        "org_id": ae.org_id,
        "time": ae.time,
        "operationType": ae.operation_type,
        "resourceType": ae.resource_type,
        "resourcePath": ae.resource_path,
        "actorId": ae.actor_id,
    })
}

pub fn signing_key_to_row(k: &SigningKeyRecord) -> Value {
    json!({
        "id": k.id,
        "realm_id": k.realm_id,
        "kid": k.kid,
        "algorithm": k.algorithm,
        "status": k.status,
        "priority": k.priority,
        "created_at": k.created_at.to_rfc3339(),
        "activate_at": k.activate_at.map(|d| d.to_rfc3339()),
        "expires_at": k.expires_at.map(|d| d.to_rfc3339()),
    })
}

pub fn required_action_to_row(ra: &RequiredActionConfig) -> Value {
    json!({
        "id": ra.id,
        "realm_id": ra.realm_id,
        "alias": ra.alias,
        "name": ra.name,
        "providerId": ra.provider_id,
        "enabled": ra.enabled,
        "defaultAction": ra.default_action,
        "priority": ra.priority,
    })
}

pub fn branding_to_row(b: &BrandingBlock) -> Value {
    json!({
        "id": b.id,
        "realm_id": b.realm_id,
        "org_id": b.org_id,
        "instanceName": b.instance_name,
        "shortName": b.short_name,
        "organisation": b.organisation,
        "primaryForeground": b.primary_foreground,
    })
}
