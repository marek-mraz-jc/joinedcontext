#![forbid(unsafe_code)]
//! Single `Authorizer` decision trait and role mapping (R9, same shape as `dataroom-core::auth`).
//!
//! - Grafana roles map at import: `Viewer -> Read`, `Editor -> Propose`, `Admin -> Approve` per folder.
//! - A query requires `Read` on the datasource.
//! - Another organization's object always returns `Decision::deny_hidden(...)`, which translates
//!   to HTTP 404 (never 403) so cross-tenant existence is never revealed.

use crate::error::CrError;
use crate::ids::{InstallationId, InstanceId, OrgId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Grafana organization/folder role mapped at import (R9, §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GrafanaRole {
    Viewer,
    Editor,
    Admin,
}

impl GrafanaRole {
    /// Maps a Grafana role onto the corresponding `daiya` [`Action`] set (R9).
    #[must_use]
    pub fn mapped_actions(self) -> Vec<Action> {
        match self {
            Self::Viewer => vec![Action::Read],
            Self::Editor => vec![Action::Read, Action::Propose],
            Self::Admin => vec![
                Action::Read,
                Action::Propose,
                Action::Approve,
                Action::Delete,
                Action::Admin,
            ],
        }
    }
}

/// Caller identity verified from the Portal session or kaisatsu bearer token (R9, §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    /// Person or workload subject ID (`None` for anonymous callers).
    pub actor_id: Option<String>,
    /// Organizations this caller is a member of.
    #[serde(default)]
    pub organizations: Vec<OrgId>,
    /// Group paths held by the caller (e.g. `["/helsinki"]`, `["platform-admin"]`).
    #[serde(default)]
    pub groups: Vec<String>,
    /// Optional raw bearer token for downstream token exchange to NGSI-LD / OData Endpoints (R3).
    #[serde(default, skip_serializing)]
    pub bearer_token: Option<String>,
    /// Read-only connector / MCP flag.
    #[serde(default)]
    pub read_only: bool,
}

impl Subject {
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            actor_id: None,
            organizations: Vec::new(),
            groups: Vec::new(),
            bearer_token: None,
            read_only: false,
        }
    }

    #[must_use]
    pub fn authenticated(
        actor_id: impl Into<String>,
        organizations: Vec<OrgId>,
        groups: Vec<String>,
    ) -> Self {
        Self {
            actor_id: Some(actor_id.into()),
            organizations,
            groups,
            bearer_token: None,
            read_only: false,
        }
    }

    /// The subject an anonymous public-dashboard query runs as (R7): a read-only member of
    /// the dashboard's organization named after that dashboard, with no groups and no token.
    /// It holds no role in any host authorizer; the query runs under a
    /// [`PublicDashboardAuthorizer`] scoped to the one dashboard.
    #[must_use]
    pub fn public_dashboard(org_id: OrgId, dashboard_uid: &str) -> Self {
        Self {
            actor_id: Some(format!("{PUBLIC_DASHBOARD_ACTOR_PREFIX}{dashboard_uid}")),
            organizations: vec![org_id],
            groups: Vec::new(),
            bearer_token: None,
            read_only: true,
        }
    }

    #[must_use]
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    #[must_use]
    pub const fn is_anonymous(&self) -> bool {
        self.actor_id.is_none()
    }

    #[must_use]
    pub fn is_member_of(&self, org_id: OrgId) -> bool {
        self.organizations.contains(&org_id)
    }

    /// All principal identifiers (`actor_id` and `groups`) for matching [`RoleBinding`]s.
    #[must_use]
    pub fn principals(&self) -> Vec<&str> {
        let mut out = Vec::new();
        if let Some(id) = &self.actor_id {
            out.push(id.as_str());
        }
        for g in &self.groups {
            out.push(g.as_str());
        }
        out
    }
}

/// Per-request context scoped to `(installation_id, org_id, instance_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestCtx {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub subject: Subject,
    pub request_id: String,
}

impl RequestCtx {
    #[must_use]
    pub fn new(
        installation_id: InstallationId,
        org_id: OrgId,
        instance_id: InstanceId,
        subject: Subject,
    ) -> Self {
        Self {
            installation_id,
            org_id,
            instance_id,
            subject,
            request_id: uuid::Uuid::now_v7().to_string(),
        }
    }

    #[must_use]
    pub fn actor_name(&self) -> &str {
        self.subject.actor_id.as_deref().unwrap_or("anonymous")
    }
}

/// Verb checked by [`Authorizer::check`] (R9).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Propose,
    Approve,
    Delete,
    Admin,
}

impl Action {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Propose => "propose",
            Self::Approve => "approve",
            Self::Delete => "delete",
            Self::Admin => "admin",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "read" | "view" => Some(Self::Read),
            "propose" | "edit" | "create" | "update" => Some(Self::Propose),
            "approve" | "apply" => Some(Self::Approve),
            "delete" => Some(Self::Delete),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_write(self) -> bool {
        !matches!(self, Self::Read)
    }
}

/// Object targeted by an authorization check (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ObjectTarget {
    Organization {
        org_id: OrgId,
    },
    Folder {
        org_id: OrgId,
        folder_uid: String,
    },
    Dashboard {
        org_id: OrgId,
        dashboard_uid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_uid: Option<String>,
    },
    Datasource {
        org_id: OrgId,
        datasource_uid: String,
    },
    LibraryPanel {
        org_id: OrgId,
        panel_uid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_uid: Option<String>,
    },
    PublicDashboard {
        org_id: OrgId,
        dashboard_uid: String,
    },
}

impl ObjectTarget {
    #[must_use]
    pub const fn org_id(&self) -> OrgId {
        match self {
            Self::Organization { org_id }
            | Self::Folder { org_id, .. }
            | Self::Dashboard { org_id, .. }
            | Self::Datasource { org_id, .. }
            | Self::LibraryPanel { org_id, .. }
            | Self::PublicDashboard { org_id, .. } => *org_id,
        }
    }

    #[must_use]
    pub fn folder_uid(&self) -> Option<&str> {
        match self {
            Self::Folder { folder_uid, .. } => Some(folder_uid.as_str()),
            Self::Dashboard { folder_uid, .. } | Self::LibraryPanel { folder_uid, .. } => {
                folder_uid.as_deref()
            }
            _ => None,
        }
    }
}

/// Decision returned by [`Authorizer::check`] (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub allowed: bool,
    pub reason: String,
    /// When `false` on a denial, the caller must receive 404 (not 403) so existence is hidden.
    pub visible: bool,
}

impl Decision {
    #[must_use]
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            allowed: true,
            reason: reason.into(),
            visible: true,
        }
    }

    #[must_use]
    pub fn deny_visible(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: true,
        }
    }

    #[must_use]
    pub fn deny_hidden(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            visible: false,
        }
    }

    /// Converts this decision into `Result<(), CrError>`, mapping hidden denials to 404 (R9).
    pub fn into_result(self, not_found_msg: impl Into<String>) -> Result<(), CrError> {
        if self.allowed {
            Ok(())
        } else if !self.visible {
            Err(CrError::NotFound(not_found_msg.into()))
        } else {
            Err(CrError::Forbidden(self.reason))
        }
    }
}

/// Single authorization decision interface (R9).
pub trait Authorizer: Send + Sync {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision;
}

/// Actor id prefix of [`Subject::public_dashboard`].
pub const PUBLIC_DASHBOARD_ACTOR_PREFIX: &str = "public-dashboard:";

/// Least-privilege authorizer of an anonymous public-dashboard query (R7): read on the one
/// dashboard and on the datasources its saved panel names, in its organization, for the
/// subject of that dashboard. Every other check is a hidden denial (404).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicDashboardAuthorizer {
    pub org_id: OrgId,
    pub dashboard_uid: String,
    pub datasource_uids: std::collections::BTreeSet<String>,
}

impl PublicDashboardAuthorizer {
    #[must_use]
    pub fn new(
        org_id: OrgId,
        dashboard_uid: impl Into<String>,
        datasource_uids: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            org_id,
            dashboard_uid: dashboard_uid.into(),
            datasource_uids: datasource_uids.into_iter().collect(),
        }
    }
}

impl Authorizer for PublicDashboardAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        let expected_actor = format!("{PUBLIC_DASHBOARD_ACTOR_PREFIX}{}", self.dashboard_uid);
        if action != Action::Read
            || ctx.org_id != self.org_id
            || object.org_id() != self.org_id
            || ctx.subject.actor_id.as_deref() != Some(expected_actor.as_str())
        {
            return Decision::deny_hidden(
                "public dashboards are read-only and scoped to one dashboard",
            );
        }
        let allowed = match object {
            ObjectTarget::Dashboard { dashboard_uid, .. }
            | ObjectTarget::PublicDashboard { dashboard_uid, .. } => {
                *dashboard_uid == self.dashboard_uid
            }
            ObjectTarget::Datasource { datasource_uid, .. } => {
                self.datasource_uids.contains(datasource_uid)
            }
            ObjectTarget::Organization { .. }
            | ObjectTarget::Folder { .. }
            | ObjectTarget::LibraryPanel { .. } => false,
        };
        if allowed {
            Decision::allow("saved query of a public dashboard")
        } else {
            Decision::deny_hidden("not part of this public dashboard")
        }
    }
}

/// Role binding rule for [`PolicyAuthorizer`] (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleBinding {
    pub org_id: OrgId,
    /// Principal (actor ID or group path).
    pub principal: String,
    pub actions: Vec<Action>,
    /// Optional folder scope (`None` means organization-wide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_uid: Option<String>,
    /// Optional datasource scope (`None` means all datasources in the org).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_uid: Option<String>,
}

impl RoleBinding {
    /// Creates a role binding from a Grafana role (`Viewer -> read`, `Editor -> propose`, `Admin -> approve`).
    #[must_use]
    pub fn from_grafana_role(
        org_id: OrgId,
        principal: impl Into<String>,
        role: GrafanaRole,
        folder_uid: Option<String>,
    ) -> Self {
        Self {
            org_id,
            principal: principal.into(),
            actions: role.mapped_actions(),
            folder_uid,
            datasource_uid: None,
        }
    }
}

/// Policy-backed [`Authorizer`] implementation (R9).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyAuthorizer {
    pub bindings: Vec<RoleBinding>,
}

impl PolicyAuthorizer {
    #[must_use]
    pub fn new(bindings: Vec<RoleBinding>) -> Self {
        Self { bindings }
    }

    /// Permissive authorizer for a single organization (grants all actions to members of `org_id`).
    #[must_use]
    pub fn allow_org_members(org_id: OrgId, principal: impl Into<String>) -> Self {
        Self {
            bindings: vec![RoleBinding::from_grafana_role(
                org_id,
                principal,
                GrafanaRole::Admin,
                None,
            )],
        }
    }
}

impl Authorizer for PolicyAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        // 1. Cross-organization object is ALWAYS a hidden denial (404, never 403, R9).
        if object.org_id() != ctx.org_id || !ctx.subject.is_member_of(object.org_id()) {
            return Decision::deny_hidden("object is outside the caller's organization");
        }

        // 2. Read-only connectors cannot perform writes.
        if action.is_write() && ctx.subject.read_only {
            return Decision::deny_visible("caller is read-only");
        }

        let principals = ctx.subject.principals();
        let target_folder = object.folder_uid();
        let target_ds = match object {
            ObjectTarget::Datasource { datasource_uid, .. } => Some(datasource_uid.as_str()),
            _ => None,
        };

        let can_read = self.bindings.iter().any(|b| {
            b.org_id == ctx.org_id
                && principals.contains(&b.principal.as_str())
                && (b.actions.contains(&Action::Read) || b.actions.contains(&Action::Admin))
                && match (&b.folder_uid, target_folder) {
                    (None, _) => true,
                    (Some(bf), Some(tf)) => bf == tf,
                    (Some(_), None) => false,
                }
                && match (&b.datasource_uid, target_ds) {
                    (None, _) => true,
                    (Some(bds), Some(tds)) => bds == tds,
                    (Some(_), None) => false,
                }
        });

        if !can_read {
            return Decision::deny_hidden("caller cannot read the target resource");
        }

        if action == Action::Read {
            return Decision::allow("granted read");
        }

        let matched = self.bindings.iter().any(|b| {
            b.org_id == ctx.org_id
                && principals.contains(&b.principal.as_str())
                && (b.actions.contains(&action)
                    || b.actions.contains(&Action::Admin)
                    || (action == Action::Propose && b.actions.contains(&Action::Approve)))
                && match (&b.folder_uid, target_folder) {
                    (None, _) => true,
                    (Some(bf), Some(tf)) => bf == tf,
                    (Some(_), None) => false,
                }
                && match (&b.datasource_uid, target_ds) {
                    (None, _) => true,
                    (Some(bds), Some(tds)) => bds == tds,
                    (Some(_), None) => false,
                }
        });

        if matched {
            Decision::allow(format!("granted {}", action.as_str()))
        } else {
            Decision::deny_visible(format!("missing {} permission", action.as_str()))
        }
    }
}
