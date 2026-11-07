#![forbid(unsafe_code)]
//! Domain and Keycloak Admin REST representations (W1, W2, W3, W7, W10).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Realm representation matching Keycloak's `RealmRepresentation` and RI's keycloak-config-cli YAML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct RealmConfig {
    pub id: String,
    pub realm: String,
    pub display_name: String,
    pub enabled: bool,
    pub not_before: i64,
    pub default_signature_algorithm: String,
    pub revoke_refresh_token: bool,
    pub refresh_token_max_reuse: u32,
    pub access_token_lifespan: i64,
    pub access_token_lifespan_for_implicit_flow: i64,
    pub sso_session_idle_timeout: i64,
    pub sso_session_max_lifespan: i64,
    pub sso_session_idle_timeout_remember_me: i64,
    pub sso_session_max_lifespan_remember_me: i64,
    pub offline_session_idle_timeout: i64,
    pub offline_session_max_lifespan_enabled: bool,
    pub offline_session_max_lifespan: i64,
    pub access_code_lifespan: i64,
    pub access_code_lifespan_user_action: i64,
    pub access_code_lifespan_login: i64,
    pub action_token_generated_by_admin_lifespan: i64,
    pub action_token_generated_by_user_lifespan: i64,
    pub admin_permissions_enabled: bool,
    pub ssl_required: String,
    pub registration_allowed: bool,
    pub registration_email_as_username: bool,
    pub remember_me: bool,
    pub verify_email: bool,
    pub login_with_email_allowed: bool,
    pub duplicate_emails_allowed: bool,
    pub reset_password_allowed: bool,
    pub edit_username_allowed: bool,
    pub brute_force_protected: bool,
    pub permanent_lockout: bool,
    pub max_failure_wait_seconds: i64,
    pub minimum_quick_login_wait_seconds: i64,
    pub wait_increment_seconds: i64,
    pub quick_login_check_milli_seconds: i64,
    pub max_delta_time_seconds: i64,
    pub failure_factor: u32,
    pub default_roles: Vec<String>,
    pub password_policy: String,
    pub otp_policy_type: String,
    pub otp_policy_algorithm: String,
    pub otp_policy_initial_counter: u64,
    pub otp_policy_digits: u32,
    pub otp_policy_look_ahead_window: u32,
    pub otp_policy_period: u64,
    pub otp_supported_applications: Vec<String>,
    pub web_authn_policy_rp_entity_name: String,
    pub web_authn_policy_signature_algorithms: Vec<String>,
    pub web_authn_policy_rp_id: String,
    pub web_authn_policy_passwordless_rp_entity_name: String,
    pub web_authn_policy_passwordless_signature_algorithms: Vec<String>,
    pub web_authn_policy_passwordless_rp_id: String,
    pub login_theme: String,
    pub account_theme: String,
    pub admin_theme: String,
    pub email_theme: String,
    pub events_enabled: bool,
    pub events_expiration: i64,
    pub events_listeners: Vec<String>,
    pub enabled_event_types: Vec<String>,
    pub admin_events_enabled: bool,
    pub admin_events_details_enabled: bool,
    pub internationalization_enabled: bool,
    pub supported_locales: Vec<String>,
    pub default_locale: String,
    pub browser_flow: String,
    pub direct_grant_flow: String,
    pub reset_credentials_flow: String,
    pub attributes: BTreeMap<String, String>,
    pub default_optional_client_scopes: Vec<String>,
    pub default_groups: Vec<String>,
    pub cimd_allowed_client_domains: Vec<String>,
    pub cimd_allowed_redirect_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_server: Option<SmtpServerConfig>,
    pub apply_checksum: Option<String>,
}

impl Default for RealmConfig {
    fn default() -> Self {
        let mut attributes = BTreeMap::new();
        attributes.insert("acr.loa.map".into(), r#"{"pwd":1,"mfa":2}"#.into());
        Self {
            id: "joinedcontext".into(),
            realm: "joinedcontext".into(),
            display_name: "joinedcontext".into(),
            enabled: true,
            not_before: 0,
            default_signature_algorithm: "ES256".into(),
            revoke_refresh_token: true,
            refresh_token_max_reuse: 0,
            access_token_lifespan: 300,
            access_token_lifespan_for_implicit_flow: 900,
            sso_session_idle_timeout: 3600,
            sso_session_max_lifespan: 36000,
            sso_session_idle_timeout_remember_me: 0,
            sso_session_max_lifespan_remember_me: 0,
            offline_session_idle_timeout: 2_592_000,
            offline_session_max_lifespan_enabled: true,
            offline_session_max_lifespan: 5_184_000,
            access_code_lifespan: 60,
            access_code_lifespan_user_action: 300,
            access_code_lifespan_login: 1800,
            action_token_generated_by_admin_lifespan: 43200,
            action_token_generated_by_user_lifespan: 300,
            admin_permissions_enabled: true,
            ssl_required: "external".into(),
            registration_allowed: false,
            registration_email_as_username: true,
            remember_me: false,
            verify_email: true,
            login_with_email_allowed: true,
            duplicate_emails_allowed: false,
            reset_password_allowed: true,
            edit_username_allowed: true,
            brute_force_protected: true,
            permanent_lockout: false,
            max_failure_wait_seconds: 900,
            minimum_quick_login_wait_seconds: 60,
            wait_increment_seconds: 60,
            quick_login_check_milli_seconds: 1000,
            max_delta_time_seconds: 43200,
            failure_factor: 10,
            default_roles: vec!["offline_access".into(), "uma_authorization".into()],
            password_policy: "length(12) and upperCase(1) and lowerCase(1) and digits(1) and notUsername(undefined) and notEmail(undefined) and passwordHistory(5)".into(),
            otp_policy_type: "totp".into(),
            otp_policy_algorithm: "HmacSHA256".into(),
            otp_policy_initial_counter: 0,
            otp_policy_digits: 6,
            otp_policy_look_ahead_window: 1,
            otp_policy_period: 30,
            otp_supported_applications: vec!["FreeOTP".into(), "Google Authenticator".into()],
            web_authn_policy_rp_entity_name: "keycloak".into(),
            web_authn_policy_signature_algorithms: vec!["ES256".into()],
            web_authn_policy_rp_id: String::new(),
            web_authn_policy_passwordless_rp_entity_name: "keycloak".into(),
            web_authn_policy_passwordless_signature_algorithms: vec!["ES256".into()],
            web_authn_policy_passwordless_rp_id: String::new(),
            login_theme: "joinedcontext".into(),
            account_theme: "joinedcontext".into(),
            admin_theme: "keycloak".into(),
            email_theme: "keycloak".into(),
            events_enabled: true,
            events_expiration: 2_592_000,
            events_listeners: vec!["jboss-logging".into()],
            enabled_event_types: Vec::new(),
            admin_events_enabled: true,
            admin_events_details_enabled: true,
            internationalization_enabled: true,
            supported_locales: vec!["en".into(), "sk".into(), "cs".into(), "de".into()],
            default_locale: "en".into(),
            browser_flow: "custom browser".into(),
            direct_grant_flow: "custom direct grant".into(),
            reset_credentials_flow: "reset credentials".into(),
            attributes,
            default_optional_client_scopes: vec!["connector".into(), "offline_access".into()],
            default_groups: vec!["/platform-readers".into()],
            cimd_allowed_client_domains: vec!["claude.ai".into(), "vscode.dev".into()],
            cimd_allowed_redirect_domains: vec![
                "claude.ai".into(),
                "vscode.dev".into(),
                "code.visualstudio.com".into(),
                "localhost".into(),
                "127.0.0.1".into(),
            ],
            smtp_server: None,
            apply_checksum: None,
        }
    }
}

impl RealmConfig {
    /// Parses `acr.loa.map` attribute (`{"pwd":1,"mfa":2}`).
    pub fn acr_loa_map(&self) -> BTreeMap<String, i32> {
        self.attributes
            .get("acr.loa.map")
            .and_then(|s| serde_json::from_str::<BTreeMap<String, i32>>(s).ok())
            .unwrap_or_else(|| BTreeMap::from([("pwd".into(), 1), ("mfa".into(), 2)]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct SmtpServerConfig {
    pub host: String,
    pub port: String,
    pub from: String,
    pub user: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub starttls: String,
    pub auth: String,
    pub ssl: String,
}

impl Default for SmtpServerConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: "587".into(),
            from: String::new(),
            user: String::new(),
            password: String::new(),
            starttls: "true".into(),
            auth: "true".into(),
            ssl: "false".into(),
        }
    }
}

/// Organization inside a realm (W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: String,
    pub realm_id: String,
    pub name: String,
    pub alias: String,
    pub enabled: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub domains: Vec<OrganizationDomain>,
    #[serde(default)]
    pub attributes: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub managed_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationDomain {
    pub id: String,
    pub realm_id: String,
    pub org_id: String,
    pub name: String,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_token: Option<String>,
}

/// Membership of a person in an organization (W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub id: String,
    pub realm_id: String,
    pub org_id: String,
    pub person_id: String,
    #[serde(default)]
    pub org_roles: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Person (`UserRepresentation` in Keycloak Admin REST, W1, W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct UserRepresentation {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    pub enabled: bool,
    pub email_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_timestamp: Option<i64>,
    #[serde(default)]
    pub required_actions: Vec<String>,
    #[serde(default)]
    pub attributes: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub realm_roles: Vec<String>,
    #[serde(default)]
    pub client_roles: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    /// Transient user (I3, W3): not persisted in normal user lists.
    #[serde(default)]
    pub transient: bool,
}

impl Default for UserRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            username: String::new(),
            email: None,
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: false,
            created_timestamp: None,
            required_actions: Vec::new(),
            attributes: HashMap::new(),
            realm_roles: Vec::new(),
            client_roles: HashMap::new(),
            groups: Vec::new(),
            service_account_client_id: None,
            managed_by: None,
            transient: false,
        }
    }
}

/// Group (`GroupRepresentation` in Keycloak Admin REST, W1, W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupRepresentation {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub attributes: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub realm_roles: Vec<String>,
    #[serde(default)]
    pub client_roles: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub sub_groups: Vec<GroupRepresentation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

impl Default for GroupRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            parent_id: None,
            name: String::new(),
            path: String::new(),
            attributes: HashMap::new(),
            realm_roles: Vec::new(),
            client_roles: HashMap::new(),
            sub_groups: Vec::new(),
            managed_by: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberRecord {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub group_id: String,
    pub user_id: String,
}

/// Role (`RoleRepresentation` in Keycloak Admin REST, W1, W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct RoleRepresentation {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub composite: bool,
    pub client_role: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(default)]
    pub composites: RoleComposites,
    #[serde(default)]
    pub attributes: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

impl Default for RoleRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            name: String::new(),
            description: None,
            composite: false,
            client_role: false,
            container_id: None,
            composites: RoleComposites::default(),
            attributes: HashMap::new(),
            managed_by: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct RoleComposites {
    #[serde(default)]
    pub realm: Vec<String>,
    #[serde(default)]
    pub client: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleMappingRecord {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// `"user"` or `"group"`
    pub principal_kind: String,
    pub principal_id: String,
    pub role_id: String,
    pub role_name: String,
    /// `None` for realm roles, `Some(client_uuid)` for client roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_uuid: Option<String>,
}

/// Protocol mapper (`ProtocolMapperRepresentation`, W1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtocolMapperRepresentation {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub name: String,
    pub protocol: String,
    pub protocol_mapper: String,
    pub consent_required: bool,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

impl Default for ProtocolMapperRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            name: String::new(),
            protocol: "openid-connect".into(),
            protocol_mapper: String::new(),
            consent_required: false,
            config: BTreeMap::new(),
        }
    }
}

/// Client Scope (`ClientScopeRepresentation`, W1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientScopeRepresentation {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub protocol: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub protocol_mappers: Vec<ProtocolMapperRepresentation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

impl Default for ClientScopeRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            name: String::new(),
            description: None,
            protocol: "openid-connect".into(),
            attributes: BTreeMap::new(),
            protocol_mappers: Vec::new(),
            managed_by: None,
        }
    }
}

impl ClientScopeRepresentation {
    pub fn is_parameterized(&self) -> bool {
        self.attributes
            .get("is.parameterized.scope")
            .is_some_and(|v| v == "true")
    }

    pub fn parameterized_regexp(&self) -> Option<&str> {
        self.attributes
            .get("parameterized.scope.regexp")
            .map(String::as_str)
    }

    pub fn include_in_token_scope(&self) -> bool {
        self.attributes
            .get("include.in.token.scope")
            .map_or(true, |v| v != "false")
    }
}

/// Application / OIDC Client (`ClientRepresentation`, W1, W3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientRepresentation {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enabled: bool,
    pub protocol: String,
    pub public_client: bool,
    pub bearer_only: bool,
    pub consent_required: bool,
    pub standard_flow_enabled: bool,
    pub implicit_flow_enabled: bool,
    pub direct_access_grants_enabled: bool,
    pub service_accounts_enabled: bool,
    pub authorization_services_enabled: bool,
    pub frontchannel_logout: bool,
    pub full_scope_allowed: bool,
    pub client_authenticator_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub web_origins: Vec<String>,
    #[serde(default)]
    pub default_client_scopes: Vec<String>,
    #[serde(default)]
    pub optional_client_scopes: Vec<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub protocol_mappers: Vec<ProtocolMapperRepresentation>,
    /// Never serialized into OData or general responses; stored hashed/encrypted in the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    #[serde(default)]
    pub is_cimd: bool,
}

impl Default for ClientRepresentation {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            org_id: None,
            client_id: String::new(),
            name: None,
            description: None,
            enabled: true,
            protocol: "openid-connect".into(),
            public_client: false,
            bearer_only: false,
            consent_required: false,
            standard_flow_enabled: true,
            implicit_flow_enabled: false,
            direct_access_grants_enabled: false,
            service_accounts_enabled: false,
            authorization_services_enabled: false,
            frontchannel_logout: false,
            full_scope_allowed: true,
            client_authenticator_type: "client-secret".into(),
            root_url: None,
            base_url: None,
            redirect_uris: Vec::new(),
            web_origins: Vec::new(),
            default_client_scopes: vec![
                "basic".into(),
                "email".into(),
                "profile".into(),
                "roles".into(),
                "web-origins".into(),
            ],
            optional_client_scopes: vec![
                "address".into(),
                "phone".into(),
                "microprofile-jwt".into(),
            ],
            attributes: BTreeMap::new(),
            protocol_mappers: Vec::new(),
            secret: None,
            managed_by: None,
            is_cimd: false,
        }
    }
}

/// Upstream Identity Provider (`IdentityProviderRepresentation`, W1, W11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct IdentityProviderRepresentation {
    pub internal_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub alias: String,
    pub provider_id: String,
    pub enabled: bool,
    pub trust_email: bool,
    pub first_broker_login_flow_alias: String,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

impl Default for IdentityProviderRepresentation {
    fn default() -> Self {
        Self {
            internal_id: String::new(),
            realm_id: String::new(),
            org_id: None,
            alias: String::new(),
            provider_id: "oidc".into(),
            enabled: true,
            trust_email: false,
            first_broker_login_flow_alias: "first broker login".into(),
            config: BTreeMap::new(),
            managed_by: None,
        }
    }
}

/// Stored credential record (internal). Never returned directly over OData or Admin REST.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CredentialRecord {
    pub id: String,
    pub realm_id: String,
    pub user_id: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    pub user_label: Option<String>,
    pub created_date: i64,
    /// PHC string or encrypted TOTP/WebAuthn secret, or Keycloak `secretData` JSON.
    pub secret_data: String,
    /// Keycloak `credentialData` JSON (`{"subType":"totp","digits":6,"period":30,"algorithm":"HmacSHA256"}` etc.).
    pub credential_data: String,
    pub temporary: bool,
}

impl Default for CredentialRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            user_id: String::new(),
            credential_type: "password".into(),
            user_label: None,
            created_date: 0,
            secret_data: String::new(),
            credential_data: String::new(),
            temporary: false,
        }
    }
}

/// Public credential metadata (`CredentialRepresentation` without secrets, W1, W4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetadata {
    pub id: String,
    pub realm_id: String,
    pub user_id: String,
    #[serde(rename = "type")]
    pub credential_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_label: Option<String>,
    pub created_date: i64,
}

impl From<&CredentialRecord> for CredentialMetadata {
    fn from(c: &CredentialRecord) -> Self {
        Self {
            id: c.id.clone(),
            realm_id: c.realm_id.clone(),
            user_id: c.user_id.clone(),
            credential_type: c.credential_type.clone(),
            user_label: c.user_label.clone(),
            created_date: c.created_date,
        }
    }
}

/// Required action provider configuration (`RequiredActionProviderRepresentation`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct RequiredActionConfig {
    pub id: String,
    pub realm_id: String,
    pub alias: String,
    pub name: String,
    pub provider_id: String,
    pub enabled: bool,
    pub default_action: bool,
    pub priority: i32,
}

impl Default for RequiredActionConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            realm_id: String::new(),
            alias: "CONFIGURE_TOTP".into(),
            name: "Configure OTP".into(),
            provider_id: "CONFIGURE_TOTP".into(),
            enabled: true,
            default_action: false,
            priority: 10,
        }
    }
}

/// Signing key metadata and encrypted private key material (W4, W10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningKeyRecord {
    pub id: String,
    pub realm_id: String,
    pub kid: String,
    /// `"ES256"`, `"RS256"`, `"EdDSA"`
    pub algorithm: String,
    /// `"ACTIVE"`, `"PASSIVE"`, `"RETIRED"`
    pub status: String,
    pub priority: i32,
    pub public_jwk_json: String,
    /// AES-256-GCM encrypted PKCS#8 DER under the installation KEK.
    #[serde(skip_serializing)]
    pub encrypted_private_key: Vec<u8>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activate_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Public representation of a signing key for OData (`SigningKeys`, W4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SigningKeyMetadata {
    pub id: String,
    pub realm_id: String,
    pub kid: String,
    pub algorithm: String,
    pub status: String,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activate_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl From<&SigningKeyRecord> for SigningKeyMetadata {
    fn from(k: &SigningKeyRecord) -> Self {
        Self {
            id: k.id.clone(),
            realm_id: k.realm_id.clone(),
            kid: k.kid.clone(),
            algorithm: k.algorithm.clone(),
            status: k.status.clone(),
            priority: k.priority,
            created_at: k.created_at,
            activate_at: k.activate_at,
            expires_at: k.expires_at,
        }
    }
}

/// In-flight authentication session (`auth_session`, 30 min, W2, W7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub handle_hash: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub response_mode: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acr_requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_locales: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_jkt: Option<String>,
    pub csrf_token: String,
    pub step_state_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default)]
    pub amr: Vec<String>,
    pub acr_achieved: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Single-use authorization code or PAR request URI (`single_use`, 60 s, W2, W12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationCodeRecord {
    pub code_hash: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub client_id: String,
    pub user_id: String,
    pub sso_session_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    pub acr: String,
    pub amr: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_jkt: Option<String>,
    pub auth_time: i64,
    pub consumed: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Pushed Authorization Request (PAR, RFC 9126, 60 s single-use, W1, W12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParRequestRecord {
    pub request_uri_hash: String,
    pub realm_id: String,
    pub client_id: String,
    pub params: BTreeMap<String, String>,
    pub consumed: bool,
    pub expires_at: DateTime<Utc>,
}

/// Persistent SSO session (`sso_session`, `UserSessionRepresentation`, W1, W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoSession {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing)]
    pub handle_hash: String,
    pub user_id: String,
    pub username: String,
    pub ip_address: String,
    pub start: i64,
    pub last_access: i64,
    pub auth_time: i64,
    pub acr: String,
    pub amr: Vec<String>,
    #[serde(default)]
    pub clients: HashMap<String, String>,
    pub idle_expires_at: DateTime<Utc>,
    pub max_expires_at: DateTime<Utc>,
}

/// Per-client session inside an SSO session (`client_session`, W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientSession {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub sso_session_id: String,
    pub client_id: String,
    pub user_id: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
}

/// Offline session (`offline_session`, 30 d idle / 60 d max per RI, W1, W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OfflineSession {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub user_id: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
    pub idle_expires_at: DateTime<Utc>,
    pub max_expires_at: DateTime<Utc>,
}

/// Refresh token family for rotation and reuse detection (W1, W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshTokenFamily {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub sso_session_id: String,
    pub user_id: String,
    pub client_id: String,
    pub scope: String,
    pub acr: String,
    pub amr: Vec<String>,
    pub auth_time: i64,
    pub rotation_counter: u32,
    pub current_jti_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_jkt: Option<String>,
    pub offline: bool,
    pub revoked: bool,
    pub expires_at: DateTime<Utc>,
}

/// Revoked token or single-use JTI/OTP marker (`revoked_token`, W2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokedTokenRecord {
    pub jti_hash: String,
    pub realm_id: String,
    pub expires_at: DateTime<Utc>,
}

/// Brute-force login failure record (`login_failure`, W2, W7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginFailureRecord {
    pub id: String,
    pub realm_id: String,
    /// User ID or `"ip:{addr}"`
    pub target_key: String,
    pub num_failures: u32,
    pub last_failure_ms: i64,
    pub last_ip_failure: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_until: Option<DateTime<Utc>>,
    pub window_started_at: DateTime<Utc>,
}

/// User consent (`Consent`, W1, W2, W4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRecord {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub user_id: String,
    pub client_id: String,
    pub granted_client_scopes: Vec<String>,
    pub created_date: i64,
    pub last_updated_date: i64,
}

/// Login/protocol audit event (`EVENT_ENTITY`, W4, W12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub time: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub ip_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
}

/// Admin write audit event (`ADMIN_EVENT_ENTITY`, W4, W6, W12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminEvent {
    pub id: String,
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    pub time: i64,
    pub operation_type: String,
    pub resource_type: String,
    pub resource_path: String,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Value>,
}
