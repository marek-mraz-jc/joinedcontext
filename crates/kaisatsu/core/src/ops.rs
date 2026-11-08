#![forbid(unsafe_code)]
//! Single operation registry (W5), shaped so it can move to `crates/shared/jc-ops` unchanged.

use crate::authz::Action;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How an operation appears in the one OData 4.01 service (W4, W5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ODataKind {
    /// Served by `GET {set}` (query) on the entity set named [`ODataOperation::name`].
    EntitySetRead,
    /// Served by `POST {set}`, `PATCH {set}(key)` or `DELETE {set}(key)`.
    EntitySetWrite,
    /// A CSDL `Action` (POST), bound or unbound.
    Action,
    /// A CSDL `Function` (GET, no side effects), bound or unbound.
    Function,
}

/// One parameter of an OData action or function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataParam {
    pub name: &'static str,
    /// `Edm.String`, `Edm.Boolean`, `Collection(Edm.String)`, `Edm.Untyped`, ...
    pub edm_type: &'static str,
    pub nullable: bool,
}

/// The OData face of a registry operation, declared with it so `$metadata`, the OData routes and
/// MCP come from the same declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataOperation {
    pub kind: ODataKind,
    /// CSDL name (`ResetCredentials`) or, for entity set operations, the set (`People`).
    pub name: &'static str,
    /// Entity type the action/function is bound to (`Person`), `None` when unbound.
    pub binding: Option<&'static str>,
    /// Bound to the collection (`Collection(SigningKey)`) rather than one entity.
    pub binding_collection: bool,
    pub params: Vec<ODataParam>,
    /// `None` for an action without a result (answers 204).
    pub return_type: Option<&'static str>,
}

const fn p(name: &'static str, edm_type: &'static str, nullable: bool) -> ODataParam {
    ODataParam {
        name,
        edm_type,
        nullable,
    }
}

fn bound(
    kind: ODataKind,
    name: &'static str,
    binding: &'static str,
    params: Vec<ODataParam>,
    return_type: Option<&'static str>,
) -> ODataOperation {
    ODataOperation {
        kind,
        name,
        binding: Some(binding),
        binding_collection: false,
        params,
        return_type,
    }
}

/// The OData face of each non-list registry operation (W5).
fn odata_for(name: &str) -> ODataOperation {
    use ODataKind::{Action as A, EntitySetWrite, Function as F};
    const U: Option<&str> = Some("Edm.Untyped");
    match name {
        "realm_apply" => bound(
            A,
            "Apply",
            "Realm",
            vec![
                p("configuration", "Edm.Untyped", false),
                p("dryRun", "Edm.Boolean", true),
                p("prune", "Edm.Boolean", true),
            ],
            U,
        ),
        "realm_export" => bound(
            F,
            "Export",
            "Realm",
            vec![p("withUsers", "Edm.Boolean", true)],
            U,
        ),
        "member_add" => bound(
            A,
            "AddToGroup",
            "Person",
            vec![p("groupId", "Edm.String", false)],
            None,
        ),
        "role_map" => bound(
            A,
            "MapClientRoles",
            "Person",
            vec![
                p("clientUuid", "Edm.String", false),
                p("roles", "Collection(Edm.String)", false),
            ],
            None,
        ),
        "credential_reset" => bound(
            A,
            "ResetCredentials",
            "Person",
            vec![
                p("value", "Edm.String", false),
                p("temporary", "Edm.Boolean", true),
            ],
            None,
        ),
        "send_required_actions" => bound(
            A,
            "SendRequiredActions",
            "Person",
            vec![p("actions", "Collection(Edm.String)", false)],
            None,
        ),
        "session_revoke" => bound(A, "RevokeSessions", "Person", Vec::new(), None),
        "keys_rotate" => ODataOperation {
            kind: A,
            name: "Rotate",
            binding: Some("SigningKey"),
            binding_collection: true,
            params: vec![
                p("algorithm", "Edm.String", true),
                p("immediate", "Edm.Boolean", true),
            ],
            return_type: U,
        },
        "domain_request_verification" => bound(
            A,
            "RequestVerification",
            "OrganizationDomain",
            Vec::new(),
            U,
        ),
        "domain_verify" => bound(A, "Verify", "OrganizationDomain", Vec::new(), U),
        "totp_enrol" => bound(
            A,
            "EnrolTotp",
            "Person",
            vec![
                p("secret", "Edm.String", true),
                p("code", "Edm.String", true),
            ],
            U,
        ),
        "passkey_register" => bound(
            A,
            "RegisterPasskey",
            "Person",
            vec![
                p("credential", "Edm.Untyped", true),
                p("challenge", "Edm.String", true),
                p("label", "Edm.String", true),
            ],
            U,
        ),
        "passkey_delete" => bound(A, "DeletePasskey", "WebAuthnCredential", Vec::new(), None),
        "outbox_retry" => bound(A, "Retry", "EmailOutboxItem", Vec::new(), U),
        "recovery_codes_generate" => bound(
            A,
            "GenerateRecoveryCodes",
            "Person",
            Vec::new(),
            Some("Collection(Edm.String)"),
        ),
        "consent_revoke" => bound(A, "Revoke", "Consent", Vec::new(), None),
        "allowed" => ODataOperation {
            kind: F,
            name: "Allowed",
            binding: None,
            binding_collection: false,
            params: vec![
                p("operation", "Edm.String", false),
                p("object", "Edm.String", true),
            ],
            return_type: U,
        },
        // person_create / person_update / person_delete: writes on the People set.
        _ => ODataOperation {
            kind: EntitySetWrite,
            name: "People",
            binding: None,
            binding_collection: false,
            params: Vec::new(),
            return_type: None,
        },
    }
}

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
    pub odata: ODataOperation,
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
            ("WebAuthnCredentials", "webauthn_credential", "people-manager"),
            ("EmailOutbox", "email_outbox", "people-manager"),
            ("IdentityProviderLinks", "identity_provider_link", "people-manager"),
        ] {
            ops.push(OperationDef {
                name: Box::leak(format!("wk_{singular}_list").into_boxed_str()),
                title: Box::leak(format!("List {set}").into_boxed_str()),
                description: Box::leak(
                    format!("List and filter {set} via OData.").into_boxed_str(),
                ),
                entity_set: Some(set),
                action: Action::Read,
                permission: perm,
                audit_kind: "READ",
                rest_method: "GET",
                rest_path: Box::leak(
                    format!("/api/v1/realms/{{realm}}/odata/{set}").into_boxed_str(),
                ),
                keycloak_compat_path: None,
                input_schema: obj_schema(
                    json!({
                        "realm": { "type": "string", "description": "Realm name" },
                        "filter": { "type": "string", "description": "OData $filter expression" }
                    }),
                    &["realm"],
                ),
                output_schema: json!({ "type": "object" }),
                odata: ODataOperation {
                    kind: ODataKind::EntitySetRead,
                    name: set,
                    binding: None,
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: None,
                },
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
                "domain_request_verification",
                "Request Organization Domain Verification",
                "Issues a DNS TXT verification challenge for an organization domain.",
                Action::Update,
                "org-admin",
                "DOMAIN_REQUEST_VERIFICATION",
                "POST",
                "/api/v1/realms/{realm}/organizations/{org_id}/domains/{domain}/request-verification",
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
                "passkey_delete",
                "Delete Passkey",
                "Deletes a registered WebAuthn passkey credential.",
                Action::Delete,
                "self",
                "WEBAUTHN_DELETE",
                "DELETE",
                "/api/v1/realms/{realm}/me/passkeys/{id}",
                None,
            ),
            (
                "outbox_retry",
                "Retry Outbox Email",
                "Resets a failed e-mail outbox row to pending for immediate retry.",
                Action::Update,
                "people-manager",
                "EMAIL_OUTBOX_RETRY",
                "POST",
                "/api/v1/realms/{realm}/email-outbox/{id}/retry",
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

        for (
            name,
            title,
            description,
            action,
            permission,
            audit_kind,
            rest_method,
            rest_path,
            compat,
        ) in extra
        {
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
                odata: odata_for(name),
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

    /// The bound or unbound action/function called `odata_name` (qualified or not) on
    /// `binding` (`None` for unbound), as the OData router resolves a URL segment.
    pub fn find_odata(&self, odata_name: &str, binding: Option<&str>) -> Option<&OperationDef> {
        let short = odata_name.rsplit('.').next().unwrap_or(odata_name);
        self.ops.iter().find(|op| {
            matches!(op.odata.kind, ODataKind::Action | ODataKind::Function)
                && op.odata.name == short
                && op.odata.binding == binding
        })
    }
}
