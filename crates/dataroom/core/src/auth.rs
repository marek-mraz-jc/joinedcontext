#![forbid(unsafe_code)]
//! Identity, request context, and single `Authorizer` decision trait (C4, C5, C16).

use crate::model::{CatalogueId, Dataset, OrgId, Publisher, Visibility};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

/// Constant-time token comparison (00-contract.md Security).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Global identity subject (C4, C5): one person or workload, with memberships in one or more
/// organizations. Never stored in dataroom tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    /// Person ID or service account ID; `None` for anonymous callers.
    pub actor_id: Option<String>,
    /// Organizations this person or workload belongs to.
    #[serde(default)]
    pub organizations: Vec<OrgId>,
    /// Group IDs held in the active organization.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Client ID (for connectors or service accounts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Whether this subject is a read-only connector.
    #[serde(default)]
    pub read_only: bool,
}

impl Subject {
    /// An anonymous caller.
    pub fn anonymous() -> Self {
        Self {
            actor_id: None,
            organizations: Vec::new(),
            groups: Vec::new(),
            client_id: None,
            read_only: false,
        }
    }

    /// A person or workload with membership in `orgs` and `groups`.
    pub fn authenticated(
        actor_id: impl Into<String>,
        organizations: Vec<OrgId>,
        groups: Vec<String>,
    ) -> Self {
        Self {
            actor_id: Some(actor_id.into()),
            organizations,
            groups,
            client_id: None,
            read_only: false,
        }
    }

    /// Whether the caller is anonymous.
    pub fn is_anonymous(&self) -> bool {
        self.actor_id.is_none()
    }

    /// Whether the caller is a member of `org`.
    pub fn is_member_of(&self, org: OrgId) -> bool {
        self.organizations.contains(&org)
    }

    /// All principal identifiers (`actor_id` and `groups`) for matching grants.
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

/// Verified per-request context acting in one `(org_id, catalogue_id)` (C5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestCtx {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub subject: Subject,
    pub request_id: String,
}

impl RequestCtx {
    pub fn new(org_id: OrgId, catalogue_id: CatalogueId, subject: Subject) -> Self {
        Self {
            org_id,
            catalogue_id,
            subject,
            request_id: uuid::Uuid::now_v7().to_string(),
        }
    }

    pub fn anonymous(org_id: OrgId, catalogue_id: CatalogueId) -> Self {
        Self::new(org_id, catalogue_id, Subject::anonymous())
    }

    pub fn actor_name(&self) -> &str {
        self.subject.actor_id.as_deref().unwrap_or("anonymous")
    }
}

/// Verb checked by the `Authorizer`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Propose,
    Approve,
    Delete,
    Harvest,
    Admin,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Propose => "propose",
            Self::Approve => "approve",
            Self::Delete => "delete",
            Self::Harvest => "harvest",
            Self::Admin => "admin",
        }
    }

    pub const fn is_write(self) -> bool {
        !matches!(self, Self::Read)
    }
}

/// Object targeted by an authorization check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ObjectTarget {
    Catalogue {
        org_id: OrgId,
        catalogue_id: CatalogueId,
    },
    Publisher {
        org_id: OrgId,
        catalogue_id: CatalogueId,
        publisher_id: String,
        #[serde(default)]
        editor_grants: Vec<String>,
    },
    Dataset {
        org_id: OrgId,
        catalogue_id: CatalogueId,
        dataset_id: String,
        publisher_id: String,
        visibility: Visibility,
        #[serde(default)]
        restricted_grants: Vec<String>,
        #[serde(default)]
        publisher_editor_grants: Vec<String>,
    },
    HarvestSource {
        org_id: OrgId,
        catalogue_id: CatalogueId,
        source_id: String,
    },
    Profile {
        org_id: OrgId,
        catalogue_id: CatalogueId,
        profile_id: String,
    },
    Plugin {
        org_id: OrgId,
        catalogue_id: CatalogueId,
        plugin_name: String,
    },
}

impl ObjectTarget {
    pub fn org_id(&self) -> OrgId {
        match self {
            Self::Catalogue { org_id, .. }
            | Self::Publisher { org_id, .. }
            | Self::Dataset { org_id, .. }
            | Self::HarvestSource { org_id, .. }
            | Self::Profile { org_id, .. }
            | Self::Plugin { org_id, .. } => *org_id,
        }
    }

    pub fn catalogue_id(&self) -> CatalogueId {
        match self {
            Self::Catalogue { catalogue_id, .. }
            | Self::Publisher { catalogue_id, .. }
            | Self::Dataset { catalogue_id, .. }
            | Self::HarvestSource { catalogue_id, .. }
            | Self::Profile { catalogue_id, .. }
            | Self::Plugin { catalogue_id, .. } => *catalogue_id,
        }
    }

    pub fn from_dataset(dataset: &Dataset, publisher: Option<&Publisher>) -> Self {
        Self::Dataset {
            org_id: dataset.org_id,
            catalogue_id: dataset.catalogue_id,
            dataset_id: dataset.id.clone(),
            publisher_id: dataset.publisher_id.clone(),
            visibility: dataset.visibility,
            restricted_grants: dataset.restricted_grants.clone(),
            publisher_editor_grants: publisher
                .map(|p| p.editor_grants.clone())
                .unwrap_or_default(),
        }
    }

    pub fn from_publisher(publisher: &Publisher) -> Self {
        Self::Publisher {
            org_id: publisher.org_id,
            catalogue_id: publisher.catalogue_id,
            publisher_id: publisher.id.clone(),
            editor_grants: publisher.editor_grants.clone(),
        }
    }
}

/// Decision returned by [`Authorizer::check`] (C4, C11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub allowed: bool,
    pub reason: String,
    /// When `false` on a denial, the caller may not learn that the object exists (returns 404).
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

/// Single authorization decision interface (C4).
pub trait Authorizer: Send + Sync {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision;
}

/// Rule entry for the standalone/local [`PolicyAuthorizer`] (C4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantRule {
    pub org_id: OrgId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalogue_id: Option<CatalogueId>,
    /// Principal ID (person ID or group ID).
    pub principal: String,
    /// Allowed actions (`read`, `propose`, `approve`, `delete`, `harvest`, `admin`).
    pub actions: Vec<Action>,
    /// Optional publisher restriction (`None` means catalogue-wide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_id: Option<String>,
}

/// Local policy-backed [`Authorizer`] for standalone use and tests (C4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyAuthorizer {
    pub rules: Vec<GrantRule>,
}

impl PolicyAuthorizer {
    pub fn new(rules: Vec<GrantRule>) -> Self {
        Self { rules }
    }

    pub fn with_rule(mut self, rule: GrantRule) -> Self {
        self.rules.push(rule);
        self
    }

    fn holds_rule(&self, ctx: &RequestCtx, action: Action, publisher_id: Option<&str>) -> bool {
        let principals = ctx.subject.principals();
        self.rules.iter().any(|rule| {
            rule.org_id == ctx.org_id
                && rule.catalogue_id.is_none_or(|cid| cid == ctx.catalogue_id)
                && principals.contains(&rule.principal.as_str())
                && (rule.actions.contains(&action) || rule.actions.contains(&Action::Admin))
                && match (&rule.publisher_id, publisher_id) {
                    (None, _) => true,
                    (Some(rp), Some(tp)) => rp == tp,
                    (Some(_), None) => false,
                }
        })
    }
}

impl Authorizer for PolicyAuthorizer {
    fn check(&self, ctx: &RequestCtx, action: Action, object: &ObjectTarget) -> Decision {
        // 1. Cross-tenant or cross-catalogue boundary is always a hidden denial.
        if object.org_id() != ctx.org_id || object.catalogue_id() != ctx.catalogue_id {
            return Decision::deny_hidden(
                "object is outside the request's organization or catalogue",
            );
        }

        // 2. Read-only connectors cannot perform writes.
        if action.is_write() && ctx.subject.read_only {
            return Decision::deny_visible("connector is read-only");
        }

        // 3. Writes require membership in the active organization.
        if action.is_write() && !ctx.subject.is_member_of(ctx.org_id) {
            return Decision::deny_visible("caller is not a member of the organization");
        }

        match object {
            ObjectTarget::Catalogue { .. } => {
                if action == Action::Read {
                    return Decision::allow("catalogue metadata is readable");
                }
                if self.holds_rule(ctx, action, None) {
                    Decision::allow("granted on catalogue")
                } else {
                    Decision::deny_visible(format!(
                        "missing {} permission on catalogue",
                        action.as_str()
                    ))
                }
            }
            ObjectTarget::Publisher {
                publisher_id,
                editor_grants,
                ..
            } => {
                if action == Action::Read {
                    return Decision::allow("publishers are readable");
                }
                let principals = ctx.subject.principals();
                let has_editor_grant = matches!(action, Action::Propose | Action::Approve)
                    && editor_grants
                        .iter()
                        .any(|g| principals.contains(&g.as_str()));
                if has_editor_grant || self.holds_rule(ctx, action, Some(publisher_id)) {
                    Decision::allow("granted on publisher")
                } else {
                    Decision::deny_visible(format!(
                        "missing {} permission on publisher {publisher_id}",
                        action.as_str()
                    ))
                }
            }
            ObjectTarget::Dataset {
                publisher_id,
                visibility,
                restricted_grants,
                publisher_editor_grants,
                ..
            } => {
                let principals = ctx.subject.principals();
                let is_publisher_editor = ctx.subject.is_member_of(ctx.org_id)
                    && (publisher_editor_grants
                        .iter()
                        .any(|g| principals.contains(&g.as_str()))
                        || self.holds_rule(ctx, Action::Propose, Some(publisher_id)));

                let can_read = match visibility {
                    Visibility::Public => true,
                    Visibility::Internal => {
                        ctx.subject.is_member_of(ctx.org_id)
                            && (is_publisher_editor
                                || self.holds_rule(ctx, Action::Read, Some(publisher_id))
                                || self.holds_rule(ctx, Action::Read, None))
                    }
                    Visibility::Restricted => {
                        ctx.subject.is_member_of(ctx.org_id)
                            && (is_publisher_editor
                                || restricted_grants
                                    .iter()
                                    .any(|g| principals.contains(&g.as_str()))
                                || self.holds_rule(ctx, Action::Admin, None))
                    }
                };

                if action == Action::Read {
                    return if can_read {
                        Decision::allow("dataset is readable")
                    } else {
                        Decision::deny_hidden("dataset is not visible to caller")
                    };
                }

                if !can_read {
                    return Decision::deny_hidden("dataset is not visible to caller");
                }

                if (matches!(action, Action::Propose | Action::Approve | Action::Delete)
                    && is_publisher_editor)
                    || self.holds_rule(ctx, action, Some(publisher_id))
                {
                    Decision::allow("granted on dataset's publisher")
                } else {
                    Decision::deny_visible(format!(
                        "missing {} permission for publisher {publisher_id}",
                        action.as_str()
                    ))
                }
            }
            ObjectTarget::HarvestSource { .. } => {
                if !ctx.subject.is_member_of(ctx.org_id) {
                    return Decision::deny_hidden(
                        "harvest sources require organization membership",
                    );
                }
                if self.holds_rule(ctx, action, None) || self.holds_rule(ctx, Action::Harvest, None)
                {
                    Decision::allow("granted on harvest source")
                } else {
                    Decision::deny_visible(format!(
                        "missing {} permission on harvest source",
                        action.as_str()
                    ))
                }
            }
            ObjectTarget::Profile { .. } | ObjectTarget::Plugin { .. } => {
                if action == Action::Read {
                    return Decision::allow("profiles and plugins are readable");
                }
                if self.holds_rule(ctx, action, None) || self.holds_rule(ctx, Action::Admin, None) {
                    Decision::allow("granted on catalogue profile/plugin")
                } else {
                    Decision::deny_visible(format!(
                        "missing {} permission on profile/plugin",
                        action.as_str()
                    ))
                }
            }
        }
    }
}
