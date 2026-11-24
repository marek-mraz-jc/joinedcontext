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
            EntityDef {
                set: "WebAuthnCredentials".into(),
                type_name: "WebAuthnCredential".into(),
                description: "Registered WebAuthn passkeys and security keys (metadata only)."
                    .into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Credential record ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "Owner user ID."),
                    req_field(
                        "credentialId",
                        FieldKind::String,
                        "Base64url credential ID.",
                    ),
                    req_field("coseAlg", FieldKind::Integer, "COSE algorithm identifier."),
                    req_field("signCount", FieldKind::Integer, "Signature counter."),
                    req_field(
                        "uvInitialized",
                        FieldKind::Boolean,
                        "User verification initialized.",
                    ),
                    req_field(
                        "backupEligible",
                        FieldKind::Boolean,
                        "Backup eligibility (BE).",
                    ),
                    req_field("backupState", FieldKind::Boolean, "Backup state (BS)."),
                    req_field("attestationType", FieldKind::String, "Attestation type."),
                    req_field("rpId", FieldKind::String, "Relying Party ID."),
                    opt_field("userLabel", FieldKind::String, "User label."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "EmailOutbox".into(),
                type_name: "EmailOutboxItem".into(),
                description: "Transactional e-mail outbox rows (without secret payloads).".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Outbox item ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "Target user ID."),
                    req_field("toAddress", FieldKind::String, "Recipient e-mail address."),
                    req_field("locale", FieldKind::String, "Message locale."),
                    req_field("templateKind", FieldKind::String, "Template kind."),
                    req_field("status", FieldKind::String, "Delivery status."),
                    req_field("attempts", FieldKind::Integer, "Delivery attempt count."),
                    opt_field("lastError", FieldKind::String, "Last delivery error."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                ],
                rules: set_rules("write:Person"),
            },
            EntityDef {
                set: "IdentityProviderLinks".into(),
                type_name: "IdentityProviderLink".into(),
                description: "Upstream identity provider links for people.".into(),
                key: "id".into(),
                fields: vec![
                    req_field("id", FieldKind::String, "Link ID."),
                    req_field("realm_id", FieldKind::String, "Realm ID."),
                    opt_field("org_id", FieldKind::String, "Organization ID."),
                    req_field("user_id", FieldKind::String, "Local user ID."),
                    req_field("idpAlias", FieldKind::String, "Identity provider alias."),
                    req_field("externalUserId", FieldKind::String, "Upstream subject ID."),
                    opt_field("externalUsername", FieldKind::String, "Upstream username."),
                    req_field("created_at", FieldKind::DateTime, "Created timestamp."),
                ],
                rules: set_rules("write:Person"),
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
        "People"
        | "Memberships"
        | "Credentials"
        | "Consents"
        | "WebAuthnCredentials"
        | "EmailOutbox"
        | "IdentityProviderLinks" => ObjectTarget::Person {
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
            managed_by: None,
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
        "WebAuthnCredentials",
        "EmailOutbox",
        "IdentityProviderLinks",
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

pub fn webauthn_credential_to_row(w: &WebAuthnCredentialRecord) -> Value {
    json!({
        "id": w.id,
        "realm_id": w.realm_id,
        "org_id": w.org_id,
        "user_id": w.user_id,
        "credentialId": w.credential_id,
        "coseAlg": w.cose_alg,
        "signCount": w.sign_count,
        "uvInitialized": w.uv_initialized,
        "backupEligible": w.backup_eligible,
        "backupState": w.backup_state,
        "attestationType": w.attestation_type,
        "rpId": w.rp_id,
        "userLabel": w.user_label,
        "created_at": w.created_at.to_rfc3339(),
    })
}

pub fn email_outbox_to_row(e: &EmailOutboxRecord) -> Value {
    json!({
        "id": e.id,
        "realm_id": e.realm_id,
        "org_id": e.org_id,
        "user_id": e.user_id,
        "toAddress": e.to_address,
        "locale": e.locale,
        "templateKind": e.template_kind,
        "status": e.status,
        "attempts": e.attempts,
        "lastError": e.last_error,
        "created_at": e.created_at.to_rfc3339(),
    })
}

pub fn idp_link_to_row(l: &IdentityProviderLinkRecord) -> Value {
    json!({
        "id": l.id,
        "realm_id": l.realm_id,
        "org_id": l.org_id,
        "user_id": l.user_id,
        "idpAlias": l.idp_alias,
        "externalUserId": l.external_user_id,
        "externalUsername": l.external_username,
        "created_at": l.created_at.to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jc_time::TimestampTz;
    use std::collections::{BTreeMap, HashMap, HashSet};

    fn s(v: &str) -> String {
        v.to_owned()
    }

    /// One row per entity set, each built from a record whose optional fields are empty.
    fn rows() -> Vec<(&'static str, Value)> {
        let now = TimestampTz::now();
        vec![
            ("Realms", realm_to_row(&RealmConfig::default())),
            (
                "Organizations",
                org_to_row(&Organization {
                    id: s("o"),
                    realm_id: s("r"),
                    name: s("n"),
                    alias: s("a"),
                    enabled: true,
                    description: String::new(),
                    domains: Vec::new(),
                    attributes: HashMap::new(),
                    managed_by: None,
                }),
            ),
            (
                "OrganizationDomains",
                org_domain_to_row(&OrganizationDomain {
                    id: s("d"),
                    realm_id: s("r"),
                    org_id: s("o"),
                    name: s("example.org"),
                    verified: false,
                    verification_token: None,
                    challenge_hash: None,
                    challenge_expires_at: None,
                }),
            ),
            ("People", person_to_row(&UserRepresentation::default())),
            (
                "Memberships",
                membership_to_row(&Membership {
                    id: s("m"),
                    realm_id: s("r"),
                    org_id: s("o"),
                    person_id: s("p"),
                    org_roles: vec![s("org-admin")],
                    created_at: now,
                }),
            ),
            ("Groups", group_to_row(&GroupRepresentation::default())),
            (
                "GroupMembers",
                group_member_to_row(&GroupMemberRecord {
                    id: s("gm"),
                    realm_id: s("r"),
                    org_id: None,
                    group_id: s("g"),
                    user_id: s("u"),
                }),
            ),
            ("Roles", role_to_row(&RoleRepresentation::default())),
            (
                "RoleMappings",
                role_mapping_to_row(&RoleMappingRecord {
                    id: s("rm"),
                    realm_id: s("r"),
                    org_id: None,
                    principal_kind: s("user"),
                    principal_id: s("u"),
                    role_id: s("role"),
                    role_name: s("viewer"),
                    client_uuid: None,
                }),
            ),
            (
                "Applications",
                client_to_row(&ClientRepresentation::default()),
            ),
            (
                "ClientScopes",
                scope_to_row(&ClientScopeRepresentation::default()),
            ),
            (
                "ProtocolMappers",
                mapper_to_row(&ProtocolMapperRepresentation::default()),
            ),
            (
                "IdentityProviders",
                idp_to_row(&IdentityProviderRepresentation::default()),
            ),
            (
                "Credentials",
                credential_to_row(&CredentialRecord::default(), None),
            ),
            (
                "Sessions",
                session_to_row(&SsoSession {
                    id: s("ss"),
                    realm_id: s("r"),
                    org_id: None,
                    handle_hash: s("h"),
                    user_id: s("u"),
                    username: s("ada"),
                    ip_address: s("127.0.0.1"),
                    user_agent: s("ua"),
                    start: 1,
                    last_access: 2,
                    auth_time: 1,
                    acr: s("1"),
                    amr: vec![s("pwd")],
                    clients: HashMap::new(),
                    idle_expires_at: now,
                    max_expires_at: now,
                }),
            ),
            (
                "ClientSessions",
                client_session_to_row(&ClientSession {
                    id: s("cs"),
                    realm_id: s("r"),
                    org_id: None,
                    sso_session_id: s("ss"),
                    client_id: s("c"),
                    user_id: s("u"),
                    scopes: vec![s("openid")],
                    created_at: now,
                    last_access: now,
                }),
            ),
            (
                "OfflineSessions",
                offline_session_to_row(&OfflineSession {
                    id: s("os"),
                    realm_id: s("r"),
                    org_id: None,
                    user_id: s("u"),
                    client_id: s("c"),
                    scopes: Vec::new(),
                    created_at: now,
                    last_access: now,
                    idle_expires_at: now,
                    max_expires_at: now,
                }),
            ),
            (
                "Consents",
                consent_to_row(&ConsentRecord {
                    id: s("co"),
                    realm_id: s("r"),
                    org_id: None,
                    user_id: s("u"),
                    client_id: s("c"),
                    granted_client_scopes: Vec::new(),
                    created_date: 1,
                    last_updated_date: 2,
                }),
            ),
            (
                "Events",
                event_to_row(&AuditEvent {
                    id: s("e"),
                    realm_id: s("r"),
                    org_id: None,
                    time: 1,
                    event_type: s("LOGIN"),
                    client_id: None,
                    user_id: None,
                    session_id: None,
                    ip_address: s("127.0.0.1"),
                    error: None,
                    details: BTreeMap::new(),
                }),
            ),
            (
                "AdminEvents",
                admin_event_to_row(&AdminEvent {
                    id: s("ae"),
                    realm_id: s("r"),
                    org_id: None,
                    time: 1,
                    operation_type: s("CREATE"),
                    resource_type: s("USER"),
                    resource_path: s("users/u"),
                    actor_id: s("admin"),
                    actor_client_id: None,
                    representation: None,
                }),
            ),
            (
                "SigningKeys",
                signing_key_to_row(&SigningKeyRecord {
                    id: s("k"),
                    realm_id: s("r"),
                    kid: s("kid"),
                    algorithm: s("ES256"),
                    status: s("active"),
                    priority: 1,
                    public_jwk_json: s("{}"),
                    encrypted_private_key: vec![1, 2, 3],
                    created_at: now,
                    activate_at: None,
                    expires_at: None,
                }),
            ),
            (
                "RequiredActions",
                required_action_to_row(&RequiredActionConfig::default()),
            ),
            ("BrandingBlocks", branding_to_row(&BrandingBlock::default())),
            (
                "WebAuthnCredentials",
                webauthn_credential_to_row(&WebAuthnCredentialRecord {
                    id: s("w"),
                    realm_id: s("r"),
                    org_id: None,
                    user_id: s("u"),
                    user_handle: s("uh"),
                    credential_id: s("cid"),
                    public_key_cose_json: s("{}"),
                    cose_alg: -7,
                    sign_count: 0,
                    uv_initialized: true,
                    backup_eligible: false,
                    backup_state: false,
                    transports: vec![s("usb")],
                    attestation_type: s("none"),
                    rp_id: s("example.org"),
                    user_label: None,
                    created_at: now,
                    last_used_at: None,
                }),
            ),
            (
                "EmailOutbox",
                email_outbox_to_row(&EmailOutboxRecord {
                    id: s("eo"),
                    realm_id: s("r"),
                    org_id: None,
                    user_id: s("u"),
                    to_address: s("ada@example.org"),
                    locale: s("en"),
                    template_kind: s("verify-email"),
                    action_jti: s("jti"),
                    actions: Vec::new(),
                    client_id: None,
                    redirect_uri: None,
                    lifespan_secs: 60,
                    issuer: s("https://id.example.org"),
                    status: s("queued"),
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: now,
                    created_at: now,
                    sent_at: None,
                }),
            ),
            (
                "IdentityProviderLinks",
                idp_link_to_row(&IdentityProviderLinkRecord {
                    id: s("l"),
                    realm_id: s("r"),
                    org_id: None,
                    user_id: s("u"),
                    idp_alias: s("google"),
                    external_user_id: s("x"),
                    external_username: None,
                    created_at: now,
                }),
            ),
        ]
    }

    #[test]
    fn every_row_holds_exactly_the_fields_its_entity_set_declares_with_their_kinds() {
        let service = kaisatsu_odata_service();
        assert_eq!(service.namespace, "JoinedContext.Kaisatsu");
        let rows = rows();
        let declared: HashSet<&str> = service.sets.iter().map(|d| d.set.as_str()).collect();
        let built: HashSet<&str> = rows.iter().map(|(set, _)| *set).collect();
        assert_eq!(declared, built, "one row builder per entity set");
        assert_eq!(service.sets.len(), 26);
        for (set, row) in rows {
            let def = service.sets.iter().find(|d| d.set == set).expect("set");
            let obj = row.as_object().expect("each row is an object");
            let fields: HashSet<&str> = def.fields.iter().map(|f| f.name.as_str()).collect();
            let keys: HashSet<&str> = obj.keys().map(String::as_str).collect();
            assert_eq!(fields, keys, "{set}: row keys are the declared fields");
            assert!(fields.contains(def.key.as_str()), "{set}: key is a field");
            for f in &def.fields {
                let v = &obj[&f.name];
                if f.name == def.key {
                    assert!(!f.nullable, "{set}.{}: a key is never nullable", f.name);
                }
                let ok = match (&f.kind, v) {
                    (_, Value::Null) => f.nullable,
                    (FieldKind::DateTime, Value::String(t)) => t.parse::<TimestampTz>().is_ok(),
                    (FieldKind::Integer, Value::Number(n)) => n.is_i64() || n.is_u64(),
                    (kind, v) => matches!(
                        (kind, v),
                        (FieldKind::String, Value::String(_))
                            | (FieldKind::Boolean, Value::Bool(_))
                    ),
                };
                assert!(ok, "{set}.{}: {v} does not fit {:?}", f.name, f.kind);
            }
        }
    }

    #[test]
    fn no_entity_set_declares_a_secret_field() {
        for def in kaisatsu_odata_service().sets {
            for f in &def.fields {
                assert!(
                    !matches!(
                        f.name.as_str(),
                        "passwordHash" | "secret" | "secretData" | "encryptedPrivateKey"
                    ) && !f.name.to_ascii_lowercase().contains("secret"),
                    "{}.{} is a secret",
                    def.set,
                    f.name
                );
            }
        }
    }

    #[test]
    fn read_needs_read_and_writes_need_the_sets_write_permission() {
        let expected: &[(&str, Option<&str>)] = &[
            ("Realms", Some("write:Realm")),
            ("Organizations", Some("write:Organization")),
            ("OrganizationDomains", Some("write:Organization")),
            ("People", Some("write:Person")),
            ("Memberships", Some("write:Person")),
            ("Groups", Some("write:Group")),
            ("GroupMembers", Some("write:Group")),
            ("Roles", Some("write:Role")),
            ("RoleMappings", Some("write:Role")),
            ("Applications", Some("write:Application")),
            ("ClientScopes", Some("write:ClientScope")),
            ("ProtocolMappers", Some("write:Application")),
            ("IdentityProviders", Some("write:Realm")),
            ("Credentials", Some("write:Person")),
            ("Sessions", Some("write:Session")),
            ("ClientSessions", None),
            ("OfflineSessions", Some("write:Session")),
            ("Consents", Some("write:Person")),
            ("Events", None),
            ("AdminEvents", None),
            ("SigningKeys", None),
            ("RequiredActions", Some("write:Realm")),
            ("BrandingBlocks", Some("write:Organization")),
            ("WebAuthnCredentials", Some("write:Person")),
            ("EmailOutbox", Some("write:Person")),
            ("IdentityProviderLinks", Some("write:Person")),
        ];
        let service = kaisatsu_odata_service();
        assert_eq!(service.sets.len(), expected.len());
        for (set, write) in expected {
            let def = service.sets.iter().find(|d| d.set == *set).expect("set");
            let mut want = vec![
                (Operation::Read, Some(s("read"))),
                (Operation::Export, Some(s("read"))),
            ];
            if let Some(w) = write {
                for op in [Operation::Create, Operation::Update, Operation::Delete] {
                    want.push((op, Some(s(w))));
                }
            }
            let got: Vec<_> = def
                .rules
                .iter()
                .map(|r| (r.operation, r.permission.clone()))
                .collect();
            assert_eq!(got, want, "{set}");
        }
    }

    /// Grants what `allow` lists and remembers each target it was asked about.
    struct Recording {
        allow: Vec<Action>,
        asked: std::sync::Mutex<Vec<(Action, ObjectTarget)>>,
    }

    impl Authorizer for Recording {
        fn check(
            &self,
            _: &RequestCtx,
            action: Action,
            object: &ObjectTarget,
        ) -> crate::authz::Decision {
            self.asked
                .lock()
                .expect("lock")
                .push((action, object.clone()));
            if self.allow.contains(&action) {
                crate::authz::Decision::allow("test")
            } else {
                crate::authz::Decision::deny_hidden("test")
            }
        }
    }

    fn recording(allow: &[Action]) -> Recording {
        Recording {
            allow: allow.to_vec(),
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Each set asks the authorizer about its own kind of object (a sample of the active
    /// organization), holds `read` and the write permissions by the answers, and is scoped to the
    /// realm and the caller's organizations by the right field.
    #[test]
    fn a_sets_scope_asks_about_its_object_kind_and_filters_by_realm_and_organization() {
        let principal = crate::authz::Principal {
            subject: Some("u".into()),
            organizations: vec!["hel".into()],
            org_roles: HashMap::from([("hel".into(), vec!["org-admin".into()])]),
            ..crate::authz::Principal::anonymous()
        };
        let ctx = RequestCtx::new("dev", principal).with_org("hel");
        let kind = |set: &str| {
            let auth = recording(&[]);
            let scope = scope_for_context(&ctx, &auth, set);
            assert!(!scope.holds("read"), "{set}");
            let asked = auth.asked.lock().expect("lock").clone();
            assert_eq!(asked.len(), 2, "{set}: read and update");
            assert_eq!((asked[0].0, asked[1].0), (Action::Read, Action::Update));
            assert_eq!(asked[0].1, asked[1].1);
            serde_json::to_value(&asked[0].1).expect("json")
        };
        for (sets, want) in [
            (
                &[
                    "Realms",
                    "IdentityProviders",
                    "SigningKeys",
                    "RequiredActions",
                ][..],
                json!({"kind": "realm", "realm_id": "dev"}),
            ),
            (
                &["Organizations", "OrganizationDomains", "BrandingBlocks"][..],
                json!({"kind": "organization", "realm_id": "dev", "org_id": "hel"}),
            ),
            (
                &[
                    "People",
                    "Memberships",
                    "Credentials",
                    "Consents",
                    "WebAuthnCredentials",
                    "EmailOutbox",
                    "IdentityProviderLinks",
                ][..],
                json!({"kind": "person", "realm_id": "dev", "org_id": "hel", "person_id": "*"}),
            ),
            (
                &["Sessions", "ClientSessions", "OfflineSessions"][..],
                json!({"kind": "session", "realm_id": "dev", "org_id": "hel", "person_id": "*"}),
            ),
            (
                &["Events", "AdminEvents"][..],
                json!({"kind": "audit", "realm_id": "dev", "org_id": "hel"}),
            ),
        ] {
            for set in sets {
                assert_eq!(kind(set), want, "{set}");
            }
        }
        for (set, k, id_field) in [
            ("Groups", "group", "group_id"),
            ("GroupMembers", "group", "group_id"),
            ("Roles", "role", "role_id"),
            ("RoleMappings", "role", "role_id"),
            ("Applications", "application", "client_id"),
            ("ProtocolMappers", "application", "client_id"),
            ("ClientScopes", "clientScope", "scope_id"),
        ] {
            let got = kind(set);
            assert_eq!(got["kind"], k, "{set}");
            assert_eq!(got[id_field], "*", "{set}");
            assert_eq!(got["org_id"], "hel", "{set}");
        }

        let reader = recording(&[Action::Read]);
        let scope = scope_for_context(&ctx, &reader, "People");
        assert!(scope.holds("read") && !scope.holds("write:Person"));
        let writer = recording(&[Action::Read, Action::Update]);
        let scope = scope_for_context(&ctx, &writer, "People");
        for p in [
            "read",
            "write:Realm",
            "write:Person",
            "write:Session",
            "write:ClientScope",
        ] {
            assert!(scope.holds(p), "{p}");
        }
        let paths = |set: &str| -> Vec<(Vec<String>, Vec<Value>)> {
            scope_for_context(&ctx, &writer, set)
                .restrictions
                .into_iter()
                .map(|r| (r.path, r.any_of))
                .collect()
        };
        let dev = || vec![json!("dev")];
        let hel = || vec![json!("hel")];
        assert_eq!(paths("Realms"), [(vec!["id".to_owned()], dev())]);
        assert_eq!(paths("SigningKeys"), [(vec!["realm_id".to_owned()], dev())]);
        assert_eq!(
            paths("Organizations"),
            [
                (vec!["realm_id".to_owned()], dev()),
                (vec!["id".to_owned()], hel())
            ]
        );
        assert_eq!(
            paths("People"),
            [
                (vec!["realm_id".to_owned()], dev()),
                (vec!["org_id".to_owned()], hel())
            ]
        );
        let admin = RequestCtx::new("dev", crate::authz::Principal::realm_admin("a"));
        let wide: Vec<_> = scope_for_context(&admin, &writer, "People")
            .restrictions
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(
            wide,
            [vec!["realm_id".to_owned()]],
            "no organization filter realm-wide"
        );
    }
}
