#![forbid(unsafe_code)]
//! Authorization of admin and self-service calls (W6): `Authorizer` trait matching dataroom's
//! shape (`check(ctx, action, object) -> Decision { allowed, reason, visible }`).
//! Admin calls on another organization answer 404 (`visible: false`), never 403.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    /// Subject (`sub`), or `None` for anonymous.
    pub subject: Option<String>,
    /// Client ID (`azp`).
    pub client_id: Option<String>,
    /// Realm roles held by the caller.
    #[serde(default)]
    pub realm_roles: Vec<String>,
    /// Client roles by client ID (`realm-management`, etc.).
    #[serde(default)]
    pub client_roles: std::collections::HashMap<String, Vec<String>>,
    /// Organizations the caller belongs to (`jc_org`).
    #[serde(default)]
    pub organizations: Vec<String>,
    /// Organization-scoped roles (`org_id -> [roles]`).
    #[serde(default)]
    pub org_roles: std::collections::HashMap<String, Vec<String>>,
    /// Whether the caller proved a second factor in this session.
    #[serde(default)]
    pub second_factor: bool,
}

impl Principal {
    pub fn anonymous() -> Self {
        Self {
            subject: None,
            client_id: None,
            realm_roles: Vec::new(),
            client_roles: std::collections::HashMap::new(),
            organizations: Vec::new(),
            org_roles: std::collections::HashMap::new(),
            second_factor: false,
        }
    }

    pub fn realm_admin(subject: impl Into<String>) -> Self {
        Self {
            subject: Some(subject.into()),
            client_id: Some("admin-cli".into()),
            realm_roles: vec!["realm-admin".into(), "admin".into()],
            client_roles: std::collections::HashMap::new(),
            organizations: Vec::new(),
            org_roles: std::collections::HashMap::new(),
            second_factor: true,
        }
    }

    pub fn is_realm_admin(&self) -> bool {
        if self
            .realm_roles
            .iter()
            .any(|r| matches!(r.as_str(), "realm-admin" | "admin" | "platform-admin"))
        {
            return true;
        }
        self.client_roles
            .get("realm-management")
            .is_some_and(|roles| roles.iter().any(|r| r == "realm-admin"))
    }

    pub fn is_realm_applier(&self) -> bool {
        self.is_realm_admin()
            || self.realm_roles.iter().any(|r| r == "realm-apply")
            || self
                .client_roles
                .get("realm-management")
                .is_some_and(|roles| roles.iter().any(|r| r == "manage-realm"))
    }

    pub fn has_realm_mgmt_role(&self, role: &str) -> bool {
        self.is_realm_admin()
            || self.realm_roles.iter().any(|r| r == role)
            || self
                .client_roles
                .get("realm-management")
                .is_some_and(|roles| roles.iter().any(|r| r == role))
    }

    pub fn has_org_role(&self, org_id: &str, role: &str) -> bool {
        self.organizations.iter().any(|o| o == org_id)
            && self
                .org_roles
                .get(org_id)
                .is_some_and(|roles| roles.iter().any(|r| r == role || r == "org-admin"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestCtx {
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_org_id: Option<String>,
    pub principal: Principal,
    pub request_id: String,
}

impl RequestCtx {
    pub fn new(realm_id: impl Into<String>, principal: Principal) -> Self {
        Self {
            realm_id: realm_id.into(),
            active_org_id: None,
            principal,
            request_id: uuid::Uuid::now_v7().to_string(),
        }
    }

    pub fn with_org(mut self, org_id: impl Into<String>) -> Self {
        self.active_org_id = Some(org_id.into());
        self
    }

    /// Allowed organizations for this request (`None` = realm-wide access).
    pub fn allowed_orgs(&self) -> Option<Vec<String>> {
        if self.principal.is_realm_admin()
            || self.principal.has_realm_mgmt_role("manage-users")
            || self.principal.has_realm_mgmt_role("view-clients")
            || self.principal.has_realm_mgmt_role("query-groups")
            || self.principal.has_realm_mgmt_role("query-users")
        {
            if let Some(org) = &self.active_org_id {
                return Some(vec![org.clone()]);
            }
            return None;
        }
        if let Some(org) = &self.active_org_id {
            if self.principal.organizations.contains(org) {
                return Some(vec![org.clone()]);
            }
            return Some(Vec::new());
        }
        Some(self.principal.organizations.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    Apply,
    SelfService,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Apply => "apply",
            Self::SelfService => "self",
        }
    }

    pub const fn is_write(self) -> bool {
        !matches!(self, Self::Read)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ObjectTarget {
    Realm {
        realm_id: String,
    },
    Organization {
        realm_id: String,
        org_id: String,
    },
    Person {
        realm_id: String,
        org_id: Option<String>,
        person_id: String,
    },
    Group {
        realm_id: String,
        org_id: Option<String>,
        group_id: String,
        managed_by: Option<String>,
    },
    Role {
        realm_id: String,
        org_id: Option<String>,
        role_id: String,
    },
    Application {
        realm_id: String,
        org_id: Option<String>,
        client_id: String,
        managed_by: Option<String>,
    },
    ClientScope {
        realm_id: String,
        org_id: Option<String>,
        scope_id: String,
    },
    Session {
        realm_id: String,
        org_id: Option<String>,
        person_id: String,
    },
    Audit {
        realm_id: String,
        org_id: Option<String>,
    },
}

impl ObjectTarget {
    pub fn realm_id(&self) -> &str {
        match self {
            Self::Realm { realm_id }
            | Self::Organization { realm_id, .. }
            | Self::Person { realm_id, .. }
            | Self::Group { realm_id, .. }
            | Self::Role { realm_id, .. }
            | Self::Application { realm_id, .. }
            | Self::ClientScope { realm_id, .. }
            | Self::Session { realm_id, .. }
            | Self::Audit { realm_id, .. } => realm_id,
        }
    }

    pub fn org_id(&self) -> Option<&str> {
        match self {
            Self::Realm { .. } => None,
            Self::Organization { org_id, .. } => Some(org_id.as_str()),
            Self::Person { org_id, .. }
            | Self::Group { org_id, .. }
            | Self::Role { org_id, .. }
            | Self::Application { org_id, .. }
            | Self::ClientScope { org_id, .. }
            | Self::Session { org_id, .. }
            | Self::Audit { org_id, .. } => org_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub allowed: bool,
    pub reason: String,
    /// When `false`, the caller must receive 404 NotFound so cross-organization existence never leaks (W6).
    pub visible: bool,
}

impl Decision {
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            allowed: true,
            reason: reason.into(),
            visible: true,
        }
    }

    pub fn deny_visible(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: true,
        }
    }

    pub fn deny_hidden(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: false,
        }
    }
}

/// Single authorization decision interface (W6).
pub trait Authorizer: Send + Sync {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision;
}

/// Default local implementation reading realm roles and organization roles (W6):
/// `realm-admin`, `org-admin`, `people-manager`, `application-manager`, `auditor`, `self`,
/// plus Keycloak's `realm-management` client roles (`manage-users`, `view-clients`, `query-groups`, `query-users`, `scim`).
#[derive(Debug, Clone, Default)]
pub struct LocalAuthorizer;

impl LocalAuthorizer {
    fn check_org_scope(&self, ctx: &RequestCtx, target_org: Option<&str>) -> Option<Decision> {
        if ctx.principal.is_realm_admin() {
            if let (Some(active), Some(target)) = (ctx.active_org_id.as_deref(), target_org) {
                if active != target {
                    return Some(Decision::deny_hidden("outside active organization"));
                }
            }
            return None;
        }
        // Realm-wide service accounts (`portal-reconciler`, `portal-api`) when no org is pinned
        if ctx.active_org_id.is_none()
            && (ctx.principal.has_realm_mgmt_role("manage-users")
                || ctx.principal.has_realm_mgmt_role("view-clients")
                || ctx.principal.has_realm_mgmt_role("query-groups")
                || ctx.principal.has_realm_mgmt_role("query-users"))
        {
            return None;
        }
        let Some(target) = target_org else {
            return Some(Decision::deny_hidden("realm-level object hidden from org caller"));
        };
        if let Some(active) = ctx.active_org_id.as_deref() {
            if active != target {
                return Some(Decision::deny_hidden("outside active organization"));
            }
        }
        if !ctx.principal.organizations.iter().any(|o| o == target) {
            return Some(Decision::deny_hidden("object belongs to another organization"));
        }
        None
    }
}

impl Authorizer for LocalAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        if object.realm_id() != ctx.realm_id {
            return Decision::deny_hidden("object is in another realm");
        }
        if ctx.principal.subject.is_none() {
            return Decision::deny_hidden("unauthenticated");
        }

        // Self-service checks on own profile, credentials, sessions, consents
        match object {
            ObjectTarget::Person { person_id, .. } | ObjectTarget::Session { person_id, .. } => {
                if ctx.principal.subject.as_deref() == Some(person_id.as_str())
                    && matches!(action, Action::Read | Action::SelfService | Action::Update | Action::Delete)
                {
                    return Decision::allow("self-service on own account");
                }
            }
            _ => {}
        }

        if let Some(denied) = self.check_org_scope(ctx, object.org_id()) {
            return denied;
        }

        if ctx.principal.is_realm_admin() {
            return Decision::allow("realm-admin");
        }

        let org_id = object.org_id().or(ctx.active_org_id.as_deref()).unwrap_or_default();

        match object {
            ObjectTarget::Realm { .. } => {
                if action == Action::Apply && ctx.principal.is_realm_applier() {
                    Decision::allow("realm-apply")
                } else if action == Action::Read && ctx.principal.is_realm_applier() {
                    Decision::allow("realm-read")
                } else {
                    Decision::deny_visible("realm-admin or realm-apply required")
                }
            }
            ObjectTarget::Organization { org_id, .. } => {
                if ctx.principal.has_org_role(org_id, "org-admin") {
                    Decision::allow("org-admin")
                } else if action == Action::Read && ctx.principal.organizations.contains(org_id) {
                    Decision::allow("org member read")
                } else {
                    Decision::deny_hidden("not allowed on organization")
                }
            }
            ObjectTarget::Person { .. } => {
                let can_write = ctx.principal.has_realm_mgmt_role("manage-users")
                    || ctx.principal.has_realm_mgmt_role("scim")
                    || ctx.principal.has_org_role(org_id, "org-admin")
                    || ctx.principal.has_org_role(org_id, "people-manager");
                let can_read = can_write
                    || ctx.principal.has_realm_mgmt_role("query-users")
                    || ctx.principal.has_realm_mgmt_role("view-users")
                    || ctx.principal.has_org_role(org_id, "auditor");
                if (action == Action::Read && can_read) || (action.is_write() && can_write) {
                    Decision::allow("granted on person")
                } else {
                    Decision::deny_hidden("insufficient role for person")
                }
            }
            ObjectTarget::Group { .. } => {
                let can_write = ctx.principal.has_realm_mgmt_role("manage-users")
                    || ctx.principal.has_realm_mgmt_role("scim")
                    || ctx.principal.has_org_role(org_id, "org-admin")
                    || ctx.principal.has_org_role(org_id, "people-manager");
                let can_read = can_write || ctx.principal.has_realm_mgmt_role("query-groups");
                if (action == Action::Read && can_read) || (action.is_write() && can_write) {
                    Decision::allow("granted on group")
                } else {
                    Decision::deny_hidden("insufficient role for group")
                }
            }
            ObjectTarget::Role { .. } | ObjectTarget::ClientScope { .. } => {
                let can_write = ctx.principal.has_realm_mgmt_role("manage-clients")
                    || ctx.principal.has_realm_mgmt_role("view-clients")
                    || ctx.principal.has_org_role(org_id, "org-admin")
                    || ctx.principal.has_org_role(org_id, "application-manager");
                let can_read = can_write || ctx.principal.has_realm_mgmt_role("query-groups");
                if (action == Action::Read && can_read) || (action.is_write() && can_write) {
                    Decision::allow("granted on role/scope")
                } else {
                    Decision::deny_hidden("insufficient role for role/scope")
                }
            }
            ObjectTarget::Application { managed_by, .. } => {
                // T-2725 compatibility: `portal-api` (`view-clients`) may write only clients with
                // `managed-by: joinedcontext` (or newly created ones).
                let is_portal_client_mgr = ctx.principal.has_realm_mgmt_role("view-clients");
                let can_write = ctx.principal.has_realm_mgmt_role("manage-clients")
                    || ctx.principal.has_org_role(org_id, "org-admin")
                    || ctx.principal.has_org_role(org_id, "application-manager")
                    || (is_portal_client_mgr
                        && managed_by.as_deref().unwrap_or("joinedcontext") == "joinedcontext");
                let can_read = can_write || is_portal_client_mgr;
                if (action == Action::Read && can_read) || (action.is_write() && can_write) {
                    Decision::allow("granted on application")
                } else {
                    Decision::deny_hidden("insufficient role for application")
                }
            }
            ObjectTarget::Session { .. } | ObjectTarget::Audit { .. } => {
                let can_write = ctx.principal.has_realm_mgmt_role("manage-users")
                    || ctx.principal.has_org_role(org_id, "org-admin")
                    || ctx.principal.has_org_role(org_id, "people-manager");
                let can_read = can_write
                    || ctx.principal.has_org_role(org_id, "auditor")
                    || ctx.principal.has_realm_mgmt_role("view-events");
                if (action == Action::Read && can_read) || (action.is_write() && can_write) {
                    Decision::allow("granted on session/audit")
                } else {
                    Decision::deny_hidden("insufficient role for session/audit")
                }
            }
        }
    }
}
