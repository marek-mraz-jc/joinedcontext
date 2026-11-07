#![forbid(unsafe_code)]
//! Single operation registry (W5), shaped so it can move to `crates/shared/jc-ops` unchanged.

use crate::authz::Action;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub entity_set: Option<&'static str>,
    pub action: Action,
    pub permission: &'static str,
    pub audit_kind: &'static str,
    pub rest_method: &'static str,
    pub rest_path: &'static str,
    pub keycloak_compat_path: Option<&'static str>,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone)]
pub struct OperationRegistry {
    ops: Vec<OperationDef>,
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut ops = Vec::new();
        let obj_schema = |props: Value, req: &[&str]| {
            json!({
                "type": "object",
                "properties": props,
                "required": req,
            })
        };

        // 1. Entity set CRUD + list operations
        for (set, singular, perm) in [
            ("Realms", "realm", "realm-admin"),
            ("Organizations", "organization", "org-admin"),
            ("OrganizationDomains", "domain", "org-admin"),
            ("People", "person", "people-manager"),
            ("Memberships", "membership", "people-manager"),
            ("Groups", "group", "people-manager"),
            ("GroupMembers", "group_member", "people-manager"),
            ("Roles", "role", "application-manager"),
            ("RoleMappings", "role_mapping", "people-manager"),
            ("Applications", "application", "application-manager"),
            ("ClientScopes", "client_scope", "application-manager"),
            ("ProtocolMappers", "protocol_mapper", "application-manager"),
            ("IdentityProviders", "identity_provider", "realm-admin"),
            ("Credentials", "credential", "people-manager"),
            ("Sessions", "session", "people-manager"),
            ("ClientSessions", "client_session", "auditor"),
            ("OfflineSessions", "offline_session", "people-manager"),
            ("Consents", "consent", "people-manager"),
            ("Events", "event", "auditor"),
            ("AdminEvents", "admin_event", "auditor"),
            ("SigningKeys", "signing_key", "realm-admin"),
            ("RequiredActions", "required_action", "realm-admin"),
            ("BrandingBlocks", "branding", "org-admin"),
        ] {
            ops.push(OperationDef {
                name: Box::leak(format!("wk_{singular}_list").into_boxed_str()),
                title: Box::leak(format!("List {set}").into_boxed_str()),
                description: Box::leak(format!("List and filter {set} via OData.").into_boxed_str()),
                entity_set: Some(set),
                action: Action::Read,
                permission: perm,
                audit_kind: "READ",
                rest_method: "GET",
                rest_path: Box::leak(format!("/api/v1/realms/{{realm}}/odata/{set}").into_boxed_str()),
                keycloak_compat_path: None,
                input_schema: obj_schema(
                    json!({
                        "realm": { "type": "string", "description": "Realm name" },
                        "filter": { "type": "string", "description": "OData $filter expression" }
                    }),
                    &["realm"],
                ),
                output_schema: json!({ "type": "object" }),
            });
        }

        // 2. Specialized identity and self-service operations (W5)
        let extra = [
            (
                "realm_apply",
                "Apply Realm Configuration",
                "Declaratively applies JSON/YAML realm configuration (W1b).",
                Action::Apply,
                "realm-apply",
                "REALM_APPLY",
                "POST",
                "/api/v1/realms/{realm}/apply",
                Some("/admin/realms/{realm}/partialImport"),
            ),
            (
                "realm_export",
                "Export Realm Configuration",
                "Exports the realm configuration in keycloak-config-cli format (W1b).",
                Action::Read,
                "realm-admin",
                "REALM_EXPORT",
                "GET",
                "/api/v1/realms/{realm}/export",
                None,
            ),
            (
                "person_create",
                "Create Person",
                "Creates a person in the realm/organization.",
                Action::Create,
                "people-manager",
                "USER_CREATE",
                "POST",
                "/api/v1/realms/{realm}/people",
                Some("/admin/realms/{realm}/users"),
            ),
            (
                "person_update",
                "Update Person",
                "Updates a person in the realm/organization.",
                Action::Update,
                "people-manager",
                "USER_UPDATE",
                "PUT",
                "/api/v1/realms/{realm}/people/{id}",
                Some("/admin/realms/{realm}/users/{id}"),
            ),
            (
                "person_delete",
                "Delete Person",
                "Deletes a person in the realm/organization.",
                Action::Delete,
                "people-manager",
                "USER_DELETE",
                "DELETE",
                "/api/v1/realms/{realm}/people/{id}",
                Some("/admin/realms/{realm}/users/{id}"),
            ),
            (
                "member_add",
                "Add Member to Group",
                "Adds a person to a group.",
                Action::Update,
                "people-manager",
                "GROUP_MEMBERSHIP_CREATE",
                "PUT",
                "/api/v1/realms/{realm}/people/{id}/groups/{group_id}",
                Some("/admin/realms/{realm}/users/{id}/groups/{group_id}"),
            ),
            (
                "role_map",
                "Map Role",
                "Maps a realm or client role to a person or group.",
                Action::Update,
                "people-manager",
                "ROLE_MAPPING_CREATE",
                "POST",
                "/api/v1/realms/{realm}/role-mappings",
                Some("/admin/realms/{realm}/users/{id}/role-mappings/clients/{client_uuid}"),
            ),
            (
                "credential_reset",
                "Reset Credential",
                "Sets a temporary password or removes second-factor credentials.",
                Action::Update,
                "people-manager",
                "RESET_PASSWORD",
                "PUT",
                "/api/v1/realms/{realm}/people/{id}/reset-password",
                Some("/admin/realms/{realm}/users/{id}/reset-password"),
            ),
            (
                "send_required_actions",
                "Send Required Actions Email",
                "Sends a signed execute-actions link by email.",
                Action::Update,
                "people-manager",
                "EXECUTE_ACTIONS_EMAIL",
                "PUT",
                "/api/v1/realms/{realm}/people/{id}/execute-actions-email",
                Some("/admin/realms/{realm}/users/{id}/execute-actions-email"),
            ),
            (
                "session_revoke",
                "Revoke Sessions",
                "Signs a person out of all sessions and triggers back-channel logout.",
                Action::Delete,
                "people-manager",
                "USER_LOGOUT",
                "POST",
                "/api/v1/realms/{realm}/people/{id}/logout",
                Some("/admin/realms/{realm}/users/{id}/logout"),
            ),
            (
                "keys_rotate",
                "Rotate Signing Keys",
                "Generates a new active signing key and keeps the previous key for verification (W10).",
                Action::Update,
                "realm-admin",
                "KEYS_ROTATE",
                "POST",
                "/api/v1/realms/{realm}/keys/rotate",
                None,
            ),
            (
                "domain_verify",
                "Verify Organization Domain",
                "Verifies an organization domain by DNS TXT token.",
                Action::Update,
                "org-admin",
                "DOMAIN_VERIFY",
                "POST",
                "/api/v1/realms/{realm}/organizations/{org_id}/domains/{domain}/verify",
                None,
            ),
            (
                "totp_enrol",
                "Enrol TOTP",
                "Enrols a TOTP HmacSHA256 authenticator for the caller.",
                Action::SelfService,
                "self",
                "UPDATE_TOTP",
                "POST",
                "/api/v1/realms/{realm}/me/totp",
                None,
            ),
            (
                "passkey_register",
                "Register Passkey",
                "Registers a WebAuthn passkey credential for the caller.",
                Action::SelfService,
                "self",
                "WEBAUTHN_REGISTER",
                "POST",
                "/api/v1/realms/{realm}/me/passkeys",
                None,
            ),
            (
                "recovery_codes_generate",
                "Generate Recovery Codes",
                "Generates 10 single-use hashed recovery codes for the caller (W7).",
                Action::SelfService,
                "self",
                "RECOVERY_CODES_GENERATE",
                "POST",
                "/api/v1/realms/{realm}/me/recovery-codes",
                None,
            ),
            (
                "consent_revoke",
                "Revoke Consent",
                "Revokes a person's consent and offline sessions for a client.",
                Action::Delete,
                "self",
                "REVOKE_GRANT",
                "DELETE",
                "/api/v1/realms/{realm}/people/{id}/consents/{client_id}",
                Some("/admin/realms/{realm}/users/{id}/consents/{client_id}"),
            ),
            (
                "allowed",
                "Check Operation Permission",
                "Answers whether the caller may perform an operation on an object without revealing hidden records (W5).",
                Action::Read,
                "self",
                "ALLOWED_CHECK",
                "GET",
                "/api/v1/realms/{realm}/allowed",
                None,
            ),
        ];

        for (name, title, description, action, permission, audit_kind, rest_method, rest_path, compat) in extra {
            ops.push(OperationDef {
                name,
                title,
                description,
                entity_set: None,
                action,
                permission,
                audit_kind,
                rest_method,
                rest_path,
                keycloak_compat_path: compat,
                input_schema: obj_schema(
                    json!({
                        "realm": { "type": "string", "description": "Realm name" },
                        "id": { "type": "string", "description": "Target object ID" },
                        "payload": { "type": "object", "description": "Operation payload" }
                    }),
                    &["realm"],
                ),
                output_schema: json!({ "type": "object" }),
            });
        }

        Self { ops }
    }

    pub fn operations(&self) -> &[OperationDef] {
        &self.ops
    }

    pub fn find(&self, name: &str) -> Option<&OperationDef> {
        self.ops.iter().find(|op| op.name == name)
    }
}
