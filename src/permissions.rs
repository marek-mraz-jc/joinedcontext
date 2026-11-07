//! Roles as code, enforced (T-0526, PF-50, PF-51): the `Role` and `RoleBinding` manifests of
//! the organization repository decide who may propose, approve and delete what, per project.
//! The token contributes identity only (`sub`, e-mail, username, groups); no permission is
//! read from it. A caller without a binding reads and proposes nothing.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use jc_core::kinds::{
    Constraint, RoleBindingSpec, RoleScope, RoleSpec, Rule, ServiceAccountSpec, Verb,
};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};

/// The namespace the organization repository's `users/` manifests carry.
pub const ORG_NAMESPACE: &str = "org";

/// One rule in force, with the binding and role it came through, so a refusal can be traced.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Grant {
    pub role: String,
    pub binding: String,
    /// Where the binding that carries this rule applies: `organization`, `project:{name}` or
    /// `contextSpace:{name}`. A grant read here may have been inherited from the organization,
    /// and the page says so rather than making it look local (PF-60, PF-61).
    pub scope: String,
    /// Set when the binding is scoped to one context space: the rule then applies only to a
    /// manifest whose `spec.contextSpaceRef` names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Set when the binding is scoped to an Organization other than the installation's and the
    /// grant is read at [`ORG_NAMESPACE`]: the rule is narrowed to that Organization's own
    /// settings, bindings and groups (ADR-N-058 section 2.4), reads only what its constraints
    /// admit and never makes its holder an administrator of the installation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[schema(value_type = Object)]
    pub rule: Rule,
}

/// Something the caller may or may not do that no rule expresses as a kind and a verb, with the
/// API's own words for the refusal: the control is rendered disabled with the reason, never
/// hidden (UI-44).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Affordance {
    pub allowed: bool,
    /// Why not; absent when the caller may.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What the organization's own settings let this caller do with projects (PF-65).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectAffordances {
    /// Whether `POST /api/v1/projects` would open a project for this caller.
    pub creation: Affordance,
}

/// What one caller may do in one project: `GET /api/v1/projects/{project}/permissions/me`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Effective {
    pub project: String,
    /// The caller is in the bootstrap group of `JC_PORTAL_BOOTSTRAP_ADMINS`: everything,
    /// everywhere, so the first binding can be written into an empty repository.
    pub bootstrap: bool,
    pub grants: Vec<Grant>,
    /// Filled by the route, not by the rules: opening a project is the organization's own
    /// setting and no binding expresses it (PF-65, T-0870).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<ProjectAffordances>,
}

/// The effective permissions of the signed-in caller in `project`, right now.
/// Refuses anybody but an administrator of the organization, with a `403` that names what
/// `action` needs (UI-87). The administrator is PF-03's: `approve` and `delete` on `RoleBinding`
/// at organization scope, as the seeded `org-admin` holds them and validation health asks.
pub fn require_organization_admin(
    state: &AppState,
    identity: &Identity,
    action: &str,
) -> Result<(), ApiError> {
    if for_request(state, identity, ORG_NAMESPACE).administers_organization() {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "{action} is for organization administrators, on the Administration page (UI-87)"
    )))
}

pub fn for_request(state: &AppState, identity: &Identity, project: &str) -> Effective {
    effective(
        &state.mirror,
        &state.config.bootstrap_admins,
        identity,
        project,
        Utc::now(),
    )
}

/// Whether the caller holds a binding in force anywhere in the organization, or is in the
/// bootstrap group: who reads an organization model, since a schema carries no data (DM-75).
pub fn is_organization_member(state: &AppState, identity: &Identity) -> bool {
    in_group(identity, &state.config.bootstrap_admins)
        || !in_force(&state.mirror, identity, Utc::now()).is_empty()
}

/// Reads every `Role` and `RoleBinding` of the mirror and keeps the rules whose binding names
/// the caller, covers the project and is in force at `now`.
pub fn effective(
    mirror: &Mirror,
    bootstrap_group: &str,
    identity: &Identity,
    project: &str,
    now: DateTime<Utc>,
) -> Effective {
    if in_group(identity, bootstrap_group) {
        return Effective {
            project: project.to_owned(),
            bootstrap: true,
            grants: Vec::new(),
            projects: None,
        };
    }
    // A space binding names no project: it applies in the one project that holds the space
    // (T-4589). Asked once per space, not once per rule.
    let mut home: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut own_settings = Vec::new();
    let mut grants: Vec<Grant> = in_force(mirror, identity, now)
        .into_iter()
        .filter_map(|(reach, mut grant)| {
            grant.space = match reach {
                Reach::Organization(org) if organization_reaches(mirror, &org, project) => None,
                Reach::Organization(org) if project == ORG_NAMESPACE => {
                    own_settings.extend(narrowed_to_own_settings(mirror, &org, grant));
                    return None;
                }
                Reach::Organization(_) => return None,
                Reach::Project(p) if p == project => None,
                Reach::Project(_) => return None,
                Reach::Space(space) => {
                    let held_by = home
                        .entry(space.clone())
                        .or_insert_with(|| project_of_space(mirror, &space));
                    if held_by.as_deref() != Some(project) {
                        return None;
                    }
                    Some(space)
                }
            };
            Some(grant)
        })
        .collect();
    grants.extend(own_settings);
    let grants = without_organization_kinds(mirror, project, grants);
    Effective {
        project: project.to_owned(),
        bootstrap: false,
        grants,
        projects: None,
    }
}

impl Effective {
    /// Whether the caller administers the organization (PF-03): `approve` and `delete` on
    /// `RoleBinding`, as `org-admin` holds them, asked of the permissions at [`ORG_NAMESPACE`].
    /// What only an administrator reads asks this one question: the validation health (OPS-53)
    /// and the organization-level Endpoints page (PF-61).
    pub fn administers_organization(&self) -> bool {
        let unnarrowed = |verb: Verb| {
            self.bootstrap
                || self.grants.iter().any(|grant| {
                    grant.organization.is_none()
                        && grant.space.is_none()
                        && grant.rule.grants("RoleBinding", verb)
                })
        };
        unnarrowed(Verb::Approve) && unnarrowed(Verb::Delete)
    }

    /// Whether the caller may read anything in this project (PF-59): a binding whose scope
    /// covers it, or the bootstrap group. What is not readable is `404` and not `403`, so a
    /// project nobody bound the caller to reads like a project that is not there (R20).
    pub fn may_read_project(&self) -> bool {
        self.bootstrap || !self.grants.is_empty()
    }

    /// Whether some grant here lets the caller propose anything: what opening a workspace
    /// asks for (API/01 §22) and who a conversation spends model calls for (AG-67). Each write is
    /// still checked on its own kind (PF-82). The build lane's rule proposes nothing but its own
    /// `status.build` (AP-73), so it counts for neither.
    pub fn may_propose_anything(&self) -> bool {
        self.bootstrap
            || self.grants.iter().any(|grant| {
                !writes_status_only(&grant.rule)
                    && grant
                        .rule
                        .kinds
                        .iter()
                        .any(|kind| grant.rule.grants(kind, Verb::Propose))
            })
    }

    /// The door of a conversation: a role that may change something here, on any kind. The
    /// assistant reads and drafts as the caller, so what the conversation can do stays the
    /// caller's own grants, and a read-only role spends no model calls (AG-67, PF-74).
    pub fn check_converses(&self, project: &str) -> Result<(), ApiError> {
        if self.may_propose_anything() {
            return Ok(());
        }
        Err(ApiError::Denied(format!(
            "A conversation with the assistant needs a role with propose on some kind in project \
             {project}."
        )))
    }

    /// Whether some grant here lets the caller `verb` on some kind: what a tool that names its
    /// kind only in its input asks before the input is read (AG-63).
    pub fn may_anything(&self, verb: Verb) -> bool {
        self.bootstrap
            || self.grants.iter().any(|grant| {
                grant
                    .rule
                    .kinds
                    .iter()
                    .any(|kind| grant.rule.grants(kind, verb))
            })
    }

    /// Whether some grant here lets the caller `verb` on `kind` at all, whatever the content:
    /// the question asked before a manifest is read, so a caller without the right learns
    /// nothing of the kind's schema (T-2576, PF-50). The content is judged by [`Self::check`].
    ///
    /// A grant bound to one context space counts only for a kind that lives in a space (T-4593):
    /// for any other kind no manifest could pass [`Self::check`], and a route that asks only this
    /// question would let a steward of one space act on the whole project. A route acting on a
    /// manifest of such a kind with a space of its own asks [`Self::may_in`] with that space.
    ///
    /// A grant narrowed to one Organization's own settings opens the door to every verb but
    /// `read`: which of the kind's manifests it reads is [`Self::may_read_manifest`]'s, item by
    /// item, so a route that lists a kind after asking only this question shows none of them.
    pub fn may(&self, kind: &str, verb: Verb) -> bool {
        self.bootstrap
            || self.grants.iter().any(|grant| {
                grant.rule.grants(kind, verb)
                    && (grant.space.is_none() || lives_in_space(kind))
                    && (grant.organization.is_none() || verb != Verb::Read)
            })
    }

    /// Whether some grant reads some manifest of `kind`: [`Self::may_read`], or a grant narrowed
    /// to one Organization's own settings (ADR-N-058 section 2.4). A list or a read that asks
    /// this filters each manifest with [`Self::may_read_manifest`].
    pub fn may_read_some(&self, kind: &str) -> bool {
        self.may_read(kind)
            || self
                .grants
                .iter()
                .any(|grant| grant.organization.is_some() && grant.rule.grants(kind, Verb::Read))
    }

    /// Whether the caller may read `kind` here (PF-59). `propose` on a kind implies `read` on
    /// it, which is what keeps a role written before the verb working (jc-core `Rule::grants`).
    pub fn may_read(&self, kind: &str) -> bool {
        self.may(kind, Verb::Read)
    }

    /// Whether a role of this caller names `field` in a constraint on `propose` of `kind`
    /// (AP-73): a constraint on a `status.*` field means that role writes the field and nobody
    /// else does.
    ///
    /// The bootstrap group is not a shortcut here. `status` is the platform's own computation
    /// and `status.build` is the build lane's alone, so the one writer is the one role — an
    /// administrator who wants it binds themselves to that role in the open.
    pub fn may_write_status_field(&self, kind: &str, field: &str) -> bool {
        self.grants.iter().any(|grant| {
            grant.rule.grants(kind, Verb::Propose)
                && grant.rule.constraints.iter().any(|c| c.field == field)
        })
    }

    /// Whether the caller may read this one manifest (PF-59, PF-60): a grant that reads the
    /// kind, and — when the binding is scoped to one context space — a manifest of that space.
    /// This is what an organization-level list filters with, item by item.
    ///
    /// A ContextSpace is in its own space: a reader bound to `air` reads the `air` manifest and
    /// no other space's (T-5698).
    pub fn may_read_manifest(&self, kind: &str, manifest: &Value) -> bool {
        let space = if kind == "ContextSpace" {
            manifest
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            space_ref(manifest)
        };
        self.may_read_in(kind, space.as_deref())
            || self.grants.iter().any(|grant| {
                grant.organization.is_some()
                    && grant.rule.grants(kind, Verb::Read)
                    && grant
                        .rule
                        .constraints
                        .iter()
                        .all(|c| satisfied(c, Some(manifest)))
            })
    }

    /// [`Self::may_read_manifest`] for a stored envelope (T-5698): every door that hands out a
    /// manifest of a kind asks this per item, since [`Self::may_read`] is true for a grant bound
    /// to one space and says nothing about the others. A grant that reads the kind in the whole
    /// project answers without serializing the envelope.
    pub fn may_read_envelope(
        &self,
        kind: &str,
        envelope: &crate::resource::ResourceEnvelope,
    ) -> bool {
        self.bootstrap
            || self.grants.iter().any(|grant| {
                grant.space.is_none()
                    && grant.organization.is_none()
                    && grant.rule.grants(kind, Verb::Read)
            })
            || serde_json::to_value(envelope)
                .is_ok_and(|manifest| self.may_read_manifest(kind, &manifest))
    }

    /// The context spaces this caller's grants reach: `None` when one grant is the project's or
    /// wider, else the spaces the bindings are scoped to. What a space's events (activity) are
    /// shown by (T-4876).
    pub fn spaces_reached(&self) -> Option<Vec<String>> {
        if self.bootstrap || self.grants.iter().any(|grant| grant.space.is_none()) {
            return None;
        }
        let spaces: BTreeSet<String> = self
            .grants
            .iter()
            .filter_map(|grant| grant.space.clone())
            .collect();
        Some(spaces.into_iter().collect())
    }

    /// [`Self::may_read_manifest`] for a manifest known only by its kind and its context space
    /// (`None` for a kind that lives outside one), as a draft event carries them.
    pub fn may_read_in(&self, kind: &str, space: Option<&str>) -> bool {
        self.may_in(kind, Verb::Read, space)
    }

    /// Whether a grant lets the caller `verb` on `kind` in `space`: a project-wide grant, or one
    /// bound to that very space. What a steward of one space may do there, and nowhere else.
    pub fn may_in(&self, kind: &str, verb: Verb, space: Option<&str>) -> bool {
        if self.bootstrap {
            return true;
        }
        self.grants.iter().any(|grant| {
            grant.rule.grants(kind, verb)
                && (grant.organization.is_none() || verb != Verb::Read)
                && match &grant.space {
                    None => true,
                    Some(bound) => space == Some(bound.as_str()),
                }
        })
    }

    /// `Ok` when a grant allows `verb` on `kind` for `target` (the whole manifest as JSON, when
    /// there is one); a 403 that names the missing verb or the violated constraint otherwise.
    pub fn check(&self, kind: &str, verb: Verb, target: Option<&Value>) -> Result<(), ApiError> {
        if self.bootstrap {
            return Ok(());
        }
        // An App that declares destinations on the internet lets data out as surely as a public
        // one, so its approval is judged as a public App's: publisher or org-admin (AP-134,
        // PF-71). The seeded steward's approve is constrained to a non-public visibility.
        let egress_declared = kind == "App" && verb == Verb::Approve && declares_egress(target);
        let judged_as_public = egress_declared.then(|| {
            let mut judged = target.cloned().unwrap_or_default();
            judged["spec"]["visibility"] = Value::from("public");
            judged
        });
        let target = judged_as_public.as_ref().or(target);
        let space_of_target = target.and_then(space_ref);
        let mut violation: Option<String> = None;
        for grant in &self.grants {
            let rule = &grant.rule;
            // `Rule::grants`, so `propose` holds `read` here as `may` says it does: org-admin
            // lists no `read` on RoleBinding and still reads a project's members (T-6266, PF-59).
            if !rule.grants(kind, verb) {
                continue;
            }
            // The build lane's rule writes `status.build` and authorizes no other proposal: its
            // one write is judged by `may_write_status_field` and the unchanged spec (AP-73).
            if verb == Verb::Propose && writes_status_only(rule) {
                continue;
            }
            if let Some(space) = &grant.space {
                if space_of_target.as_deref() != Some(space.as_str()) {
                    continue;
                }
            }
            match rule.constraints.iter().find(|c| !satisfied(c, target)) {
                None => return Ok(()),
                Some(c) => {
                    violation.get_or_insert_with(|| {
                        format!(
                            "{} does not satisfy role {} ({})",
                            c.field,
                            grant.role,
                            describe(c)
                        )
                    });
                }
            }
        }
        // Letting data out to the public is a right of its own, and the one refusal that says
        // which role holds it: every door goes through this check, so the Approvals page, the
        // operations registry and the assistant all say the same sentence (EP-76, PF-71, PF-72).
        if kind == "Endpoint" && verb == Verb::Approve && audience_of(target) == Some("public") {
            return Err(ApiError::Denied(
                "approving a public Endpoint needs publisher, the role whose approve is \
                 constrained to a public audience; org-admin holds it too (EP-76, PF-71)"
                    .to_owned(),
            ));
        }
        if egress_declared {
            return Err(ApiError::Denied(
                "approving an App that declares spec.egress needs publisher, the role whose \
                 approve is constrained to a public visibility; org-admin holds it too (AP-134, \
                 PF-71)"
                    .to_owned(),
            ));
        }
        if kind == "App"
            && verb == Verb::Approve
            && target
                .and_then(|t| t.pointer("/spec/visibility"))
                .and_then(Value::as_str)
                == Some("public")
        {
            return Err(ApiError::Denied(
                "approving a public App needs publisher, the role whose approve is constrained \
                 to a public visibility; org-admin holds it too (AP-120, PF-71)"
                    .to_owned(),
            ));
        }
        Err(ApiError::Denied(violation.unwrap_or_else(|| {
            format!(
                "no role grants {} on {kind} in project {} (PF-50)",
                verb_name(verb),
                self.project
            )
        })))
    }
}

/// Whether an App manifest declares a destination beyond its endpoint (AP-134).
fn declares_egress(target: Option<&Value>) -> bool {
    target
        .and_then(|t| t.pointer("/spec/egress"))
        .and_then(Value::as_array)
        .is_some_and(|egress| !egress.is_empty())
}

/// Where a binding applies.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reach {
    Organization(String),
    Project(String),
    Space(String),
}

impl Reach {
    /// What `permissions/me` calls this scope (PF-61).
    fn name(&self) -> String {
        match self {
            Self::Organization(_) => "organization".to_owned(),
            Self::Project(project) => format!("project:{project}"),
            Self::Space(space) => format!("contextSpace:{space}"),
        }
    }

    fn of(scope: &RoleScope) -> Option<Self> {
        match (&scope.organization, &scope.project, &scope.context_space) {
            (Some(organization), _, _) => Some(Self::Organization(organization.clone())),
            (_, Some(project), _) => Some(Self::Project(project.clone())),
            (_, _, Some(space)) => Some(Self::Space(space.clone())),
            _ => None,
        }
    }

    /// Whether a grant held here also holds at `target`: an Organization covers its own projects
    /// and their spaces, the installation's also every Organization (it hands a new one its first
    /// administrator, ADR-N-058 section 2.4), a project its own context spaces.
    fn covers(&self, target: &Self, mirror: &Mirror) -> bool {
        match (self, target) {
            (Self::Organization(held), Self::Organization(wanted)) => {
                held == wanted || organization_reaches(mirror, held, ORG_NAMESPACE)
            }
            (Self::Organization(held), Self::Project(project)) => {
                organization_reaches(mirror, held, project)
            }
            (Self::Organization(held), Self::Space(space)) => project_of_space(mirror, space)
                .is_some_and(|project| organization_reaches(mirror, held, &project)),
            (Self::Project(held), Self::Project(wanted)) => held == wanted,
            (Self::Project(held), Self::Space(space)) => {
                mirror.get(held, "ContextSpace", space).is_some()
            }
            (Self::Space(held), Self::Space(wanted)) => held == wanted,
            _ => false,
        }
    }
}

/// Whether the repository holds more than one Organization (ADR-N-058): with one, every person
/// of the realm is that Organization's.
pub fn several_organizations(mirror: &Mirror) -> bool {
    organizations(mirror).len() > 1
}

/// The Organizations of the repository by name (PF-121).
pub fn organizations(mirror: &Mirror) -> Vec<(String, Value)> {
    mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .map(|env| (env.metadata.name, env.spec))
        .collect()
}

/// The installation's Organization (PF-121, ADR-N-058 section 2.2): the only one, else the only
/// one that sets `spec.installation`; none when two claim it or none does. The mirror does not
/// keep the file a manifest came from, so a legacy `org.yaml` beside other Organizations counts
/// only when it sets `spec.installation` too.
pub fn installation_organization(mirror: &Mirror) -> Option<String> {
    let all = organizations(mirror);
    if let [(only, _)] = all.as_slice() {
        return Some(only.clone());
    }
    let mut claims = all
        .into_iter()
        .filter(|(_, spec)| spec.get("installation") == Some(&Value::Bool(true)));
    match (claims.next(), claims.next()) {
        (Some((one, _)), None) => Some(one),
        _ => None,
    }
}

/// The Organization that owns `project`: its Project's `spec.organizationRef`, or the
/// installation's for a project without one (ADR-N-058 section 2.3).
pub fn organization_of(mirror: &Mirror, project: &str) -> Option<String> {
    mirror
        .get(ORG_NAMESPACE, "Project", project)
        .and_then(|env| {
            env.spec
                .get("organizationRef")
                .and_then(crate::resource::reference_name)
                .map(str::to_owned)
        })
        .or_else(|| installation_organization(mirror))
}

/// The `Organization` manifest that owns `project` ([`organization_of`]): its policies, limits and
/// domain are the ones that hold there, never another Organization's (ADR-N-058 section 2.3).
pub fn organization_manifest_of(
    mirror: &Mirror,
    project: &str,
) -> Option<crate::resource::ResourceEnvelope> {
    mirror.get(
        ORG_NAMESPACE,
        "Organization",
        &organization_of(mirror, project)?,
    )
}

/// The installation's `Organization` manifest ([`installation_organization`]), whose settings
/// hold for what belongs to no project yet, such as opening one (ADR-N-058 section 2.2).
pub fn installation_organization_manifest(
    mirror: &Mirror,
) -> Option<crate::resource::ResourceEnvelope> {
    mirror.get(
        ORG_NAMESPACE,
        "Organization",
        &installation_organization(mirror)?,
    )
}

/// Whether a binding scoped to `organization` applies in `project` (PF-123, ADR-N-058 section
/// 2.4). With one Organization it applies everywhere, as before several existed. With several, the
/// organization's own namespace ([`ORG_NAMESPACE`]: bindings, groups, brokers, platform settings)
/// is the installation Organization's alone, and a project is its Organization's alone: deny by
/// default, so a binding naming an Organization that owns nothing grants nothing.
fn organization_reaches(mirror: &Mirror, organization: &str, project: &str) -> bool {
    if organizations(mirror).len() <= 1 {
        return true;
    }
    let owner = if project == ORG_NAMESPACE || project.is_empty() {
        installation_organization(mirror)
    } else {
        organization_of(mirror, project)
    };
    owner.as_deref() == Some(organization)
}

/// In a project of an Organization other than the installation's, a grant reaches the project's
/// own kinds alone (PF-123, ADR-N-058 section 2.2): the installation-wide manifests (brokers,
/// environments, platform settings, bindings, groups, every `Organization`) are judged at
/// [`ORG_NAMESPACE`] by the installation's administrators, so a change filed in such a project
/// carries none of them past its approver.
fn without_organization_kinds(mirror: &Mirror, project: &str, grants: Vec<Grant>) -> Vec<Grant> {
    if project == ORG_NAMESPACE
        || organizations(mirror).len() <= 1
        || organization_of(mirror, project) == installation_organization(mirror)
    {
        return grants;
    }
    grants
        .into_iter()
        .filter_map(|mut grant| {
            grant
                .rule
                .kinds
                .retain(|kind| !crate::resource::belongs_to_the_organization(kind, Some(project)));
            (!grant.rule.kinds.is_empty()).then_some(grant)
        })
        .collect()
}

/// The kinds a non-installation Organization's administrator keeps at [`ORG_NAMESPACE`]
/// (ADR-N-058 section 2.4): its own `Organization` manifest, the bindings inside it and the groups
/// those bindings name. Roles, brokers, environments and platform settings stay the installation's.
const OWN_SETTINGS_KINDS: [&str; 3] = ["Organization", "RoleBinding", "Group"];

fn one_of(field: &str, values: Vec<String>) -> Constraint {
    Constraint {
        field: field.to_owned(),
        one_of: values,
        not_in: Vec::new(),
        equals: None,
        pattern: None,
    }
}

fn none_of(field: &str, values: Vec<String>) -> Constraint {
    Constraint {
        one_of: Vec::new(),
        not_in: values,
        ..one_of(field, Vec::new())
    }
}

/// A grant of a binding scoped to `organization`, not the installation's, read at
/// [`ORG_NAMESPACE`]: its rules on [`OWN_SETTINGS_KINDS`], each confined by constraints to what
/// stays inside that Organization (ADR-N-058 section 2.4, T-5248).
///
/// - `Organization`: `metadata.name` is `organization`, `spec.installation` is never set, and no
///   `delete` (an Organization's projects would point at nothing).
/// - `RoleBinding`: `spec.scope` names `organization`, one of its projects or one of their spaces
///   (one rule each, so any one of them admits it; a level it owns nothing of gets no rule).
/// - `Group`: any name but one a binding outside `organization` names, so a group whose members
///   decide a second Organization's access is not changed from here.
///
/// An Organization that is not in the repository is given nothing.
fn narrowed_to_own_settings(mirror: &Mirror, organization: &str, grant: Grant) -> Vec<Grant> {
    if mirror
        .get(ORG_NAMESPACE, "Organization", organization)
        .is_none()
    {
        return Vec::new();
    }
    let own = Reach::Organization(organization.to_owned());
    let mut narrowed = Vec::new();
    let mut push = |kind: &str, verbs: Vec<Verb>, extra: Vec<Constraint>| {
        if verbs.is_empty() {
            return;
        }
        let mut constraints = grant.rule.constraints.clone();
        constraints.extend(extra);
        narrowed.push(Grant {
            organization: Some(organization.to_owned()),
            rule: Rule {
                kinds: vec![kind.to_owned()],
                verbs,
                constraints,
            },
            ..grant.clone()
        });
    };
    for kind in OWN_SETTINGS_KINDS {
        if !grant.rule.kinds.iter().any(|k| k == kind) {
            continue;
        }
        let verbs = grant.rule.verbs.clone();
        match kind {
            "Organization" => push(
                kind,
                verbs.into_iter().filter(|v| *v != Verb::Delete).collect(),
                vec![
                    one_of("metadata.name", vec![organization.to_owned()]),
                    none_of("spec.installation", vec!["true".to_owned()]),
                ],
            ),
            "RoleBinding" => {
                let projects: Vec<String> = mirror
                    .list(ORG_NAMESPACE, "Project", &ListOptions::default())
                    .items
                    .into_iter()
                    .map(|env| env.metadata.name)
                    .filter(|project| organization_reaches(mirror, organization, project))
                    .collect();
                let spaces: Vec<String> = mirror
                    .matching(|env| {
                        env.kind == "ContextSpace"
                            && env
                                .metadata
                                .namespace
                                .as_ref()
                                .is_some_and(|ns| projects.contains(ns))
                    })
                    .into_iter()
                    .map(|env| env.metadata.name)
                    .filter(|space| own.covers(&Reach::Space(space.clone()), mirror))
                    .collect();
                // A scope that names two levels applies at the widest (`Reach::of`): no rule
                // admits a second Organization or a foreign project beside an own one.
                let others: Vec<String> = organizations(mirror)
                    .into_iter()
                    .map(|(name, _)| name)
                    .filter(|name| name != organization)
                    .collect();
                let foreign_projects: Vec<String> = mirror
                    .list(ORG_NAMESPACE, "Project", &ListOptions::default())
                    .items
                    .into_iter()
                    .map(|env| env.metadata.name)
                    .filter(|project| !projects.contains(project))
                    .collect();
                let scopes = [
                    ("spec.scope.organization", vec![organization.to_owned()]),
                    ("spec.scope.project", projects),
                    ("spec.scope.contextSpace", spaces),
                ];
                for (field, values) in scopes {
                    if !values.is_empty() {
                        push(
                            kind,
                            verbs.clone(),
                            vec![
                                one_of(field, values),
                                none_of("spec.scope.organization", others.clone()),
                                none_of("spec.scope.project", foreign_projects.clone()),
                            ],
                        );
                    }
                }
            }
            _ => {
                let mut elsewhere: BTreeSet<String> = mirror
                    .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
                    .items
                    .into_iter()
                    .filter_map(|env| serde_json::from_value::<RoleBindingSpec>(env.spec).ok())
                    .filter(|binding| {
                        Reach::of(&binding.scope).is_none_or(|reach| !own.covers(&reach, mirror))
                    })
                    .flat_map(|binding| binding.subjects.into_iter().filter_map(|s| s.group))
                    .collect();
                // A Policy grants its data to every token holding a group it names (gateway PDP).
                elsewhere.extend(
                    mirror
                        .matching(|env| {
                            env.kind == "Policy"
                                && !env.metadata.namespace.as_deref().is_some_and(|project| {
                                    organization_reaches(mirror, organization, project)
                                })
                        })
                        .into_iter()
                        .flat_map(|env| policy_groups(&env.spec)),
                );
                // An App grants its roles to the groups its access names, and an annotated Group
                // is one App's default group (AP-118): both stay with that App's administrators.
                elsewhere.extend(
                    mirror
                        .matching(|env| {
                            env.kind == "App"
                                && !env.metadata.namespace.as_deref().is_some_and(|project| {
                                    organization_reaches(mirror, organization, project)
                                })
                        })
                        .into_iter()
                        .flat_map(|env| app_groups(&env.spec)),
                );
                elsewhere.extend(
                    mirror
                        .list(ORG_NAMESPACE, "Group", &ListOptions::default())
                        .items
                        .into_iter()
                        .filter(|env| {
                            env.metadata
                                .annotations
                                .contains_key(crate::apps::reconciler::APP_LABEL)
                        })
                        .map(|env| env.metadata.name),
                );
                push(
                    kind,
                    verbs,
                    vec![
                        Constraint {
                            pattern: Some(format!("{}-.+", regex::escape(organization))),
                            ..one_of("metadata.name", Vec::new())
                        },
                        none_of("metadata.name", elsewhere.into_iter().collect()),
                    ],
                );
            }
        }
    }
    narrowed
}

/// The groups a Policy names as assignee, alone or in one of its `rules[]`.
fn policy_groups(spec: &Value) -> Vec<String> {
    let rules = spec.get("rules").and_then(Value::as_array);
    std::iter::once(spec)
        .chain(rules.into_iter().flatten())
        .filter_map(|holder| holder.get("assignee"))
        .filter(|assignee| assignee.get("kind").and_then(Value::as_str) == Some("group"))
        .filter_map(|assignee| {
            assignee
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// The groups an App's `spec.access` gives a role to.
fn app_groups(spec: &Value) -> Vec<String> {
    spec.get("access")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("subjects").and_then(Value::as_array))
        .flatten()
        .filter_map(|subject| subject.get("group").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// The roles of one namespace: the organization's `users/roles/`, or one project's own (PF-68).
fn roles(mirror: &Mirror, namespace: &str) -> Vec<(String, RoleSpec)> {
    mirror
        .list(namespace, "Role", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| match serde_json::from_value::<RoleSpec>(env.spec) {
            Ok(spec) => Some((env.metadata.name, spec)),
            Err(e) => {
                tracing::warn!(role = %env.metadata.name, error = %e, "Role in the mirror does not parse; it grants nothing");
                None
            }
        })
        .collect()
}

/// The project a context space belongs to: where a binding scoped to that space applies, and
/// whose roles it may reach (PF-69). A space name is unique in the organization (PF-44, PF-76);
/// a mirror holding it in two projects breaks that, and such a binding then applies nowhere
/// rather than in both (T-4589).
/// The project a binding's scope lies in (S-27): its project, or the project holding its context
/// space. None for an organization scope, which only the organization decides.
pub fn binding_project(mirror: &Mirror, spec: &Value) -> Option<String> {
    let binding: RoleBindingSpec = serde_json::from_value(spec.clone()).ok()?;
    match Reach::of(&binding.scope)? {
        Reach::Project(project) => Some(project),
        Reach::Space(space) => project_of_space(mirror, &space),
        Reach::Organization(_) => None,
    }
}

fn project_of_space(mirror: &Mirror, space: &str) -> Option<String> {
    let mut holders: Vec<String> = mirror
        .matching(|env| env.kind == "ContextSpace" && env.metadata.name == space)
        .into_iter()
        .filter_map(|env| env.metadata.namespace)
        .collect();
    holders.sort_unstable();
    holders.dedup();
    match holders.as_slice() {
        [project] => Some(project.clone()),
        [] => None,
        _ => {
            tracing::warn!(space, projects = ?holders, "context space name held by more than one project; a binding scoped to it grants nothing");
            None
        }
    }
}

/// The role a binding names, looked up where the binding reaches it: the organization's roles
/// first, then the roles of the project its scope names (PF-68, PF-69).
///
/// A name in both places is refused by `jcctl validate` before the manifest ever lands, so the
/// organization's copy winning here is a tie that cannot happen, not a precedence rule.
fn role_of(
    mirror: &Mirror,
    organization: &[(String, RoleSpec)],
    reach: &Reach,
    name: &str,
) -> Option<(String, RoleSpec)> {
    if let Some((found, spec)) = organization.iter().find(|(role, _)| role == name) {
        return Some((found.clone(), spec.clone()));
    }
    let project = match reach {
        Reach::Organization(_) => return None,
        Reach::Project(project) => project.clone(),
        Reach::Space(space) => project_of_space(mirror, space)?,
    };
    roles(mirror, &project)
        .into_iter()
        .find(|(role, _)| role == name)
}

/// Every rule a binding in force at `now` gives the caller, with where the binding applies.
fn in_force(mirror: &Mirror, identity: &Identity, now: DateTime<Utc>) -> Vec<(Reach, Grant)> {
    let organization = roles(mirror, ORG_NAMESPACE);
    let mut grants = Vec::new();
    for env in mirror.kind_where(ORG_NAMESPACE, "RoleBinding", |env| {
        names_caller(identity, &env.spec)
    }) {
        let binding = match serde_json::from_value::<RoleBindingSpec>(env.spec) {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!(binding = %env.metadata.name, error = %e, "RoleBinding in the mirror does not parse; it grants nothing");
                continue;
            }
        };
        if !binding.subjects.iter().any(|s| is_subject(identity, s)) {
            continue;
        }
        if binding.validity.as_ref().is_some_and(|v| !v.contains(now)) {
            continue;
        }
        let Some(reach) = Reach::of(&binding.scope) else {
            continue;
        };
        let Some((role_name, role)) = role_of(mirror, &organization, &reach, &binding.role) else {
            tracing::warn!(binding = %env.metadata.name, role = %binding.role, "RoleBinding names a Role it does not reach");
            continue;
        };
        for rule in &role.rules {
            grants.push((
                reach.clone(),
                Grant {
                    role: role_name.clone(),
                    binding: env.metadata.name.clone(),
                    scope: reach.name(),
                    space: None,
                    organization: None,
                    rule: rule.clone(),
                },
            ));
        }
    }
    grants.extend(account_grants(mirror, &organization, identity, now));
    grants
}

/// Whether a RoleBinding's spec names the caller, read in place: an organization holds a
/// binding per person and project, so parsing only the caller's own keeps a call's rights
/// computation from growing with everybody else's (T-5978). A subject that does not parse names
/// nobody; the binding is parsed whole, and judged again, only when one names the caller.
fn names_caller(identity: &Identity, spec: &Value) -> bool {
    spec.get("subjects")
        .and_then(Value::as_array)
        .is_some_and(|subjects| {
            subjects.iter().any(|subject| {
                names(
                    identity,
                    subject.get("user").and_then(Value::as_str),
                    subject.get("group").and_then(Value::as_str),
                )
            })
        })
}

/// Whether one of a ServiceAccount's roles names a `Role` (§2a), so the account acts on the
/// Portal and its client's tokens carry the Portal's audience (PF-47).
pub(crate) fn account_holds_portal_role(mirror: &Mirror, spec: &ServiceAccountSpec) -> bool {
    let organization = roles(mirror, ORG_NAMESPACE);
    spec.roles.iter().any(|granted| {
        Reach::of(&granted.scope)
            .is_some_and(|reach| role_of(mirror, &organization, &reach, &granted.role).is_some())
    })
}

/// Whether a bearer token is a `ServiceAccount`'s own, a workload with no person behind it.
pub fn is_service_account(mirror: &Mirror, identity: &Identity) -> bool {
    account_of(mirror, identity).is_some()
}

/// The account a bearer token speaks for (PF-46, PF-49): its `azp` is the derived client id of
/// exactly one `ServiceAccount`, and its user is that client's own service account, so a person
/// who signed in through some client is never mistaken for it. An id two accounts derive is
/// nobody's, as at the gateway (T-1454).
fn account_of(mirror: &Mirror, identity: &Identity) -> Option<(String, ServiceAccountSpec)> {
    use jc_core::kinds::service_account::keycloak_client_id;
    let client = identity.client.as_deref()?;
    if !identity
        .username
        .eq_ignore_ascii_case(&format!("service-account-{client}"))
    {
        return None;
    }
    let mut matches = mirror.namespaces().into_iter().flat_map(|namespace| {
        mirror
            .list(&namespace, "ServiceAccount", &ListOptions::default())
            .items
            .into_iter()
            .filter(move |env| keycloak_client_id(&namespace, &env.metadata.name) == client)
    });
    let env = matches.next()?;
    if matches.next().is_some() {
        tracing::warn!(
            client,
            "two ServiceAccounts derive this Keycloak client id; it grants nothing"
        );
        return None;
    }
    match serde_json::from_value::<ServiceAccountSpec>(env.spec) {
        Ok(spec) => Some((env.metadata.name, spec)),
        Err(e) => {
            tracing::warn!(account = %env.metadata.name, error = %e, "ServiceAccount in the mirror does not parse; it grants nothing");
            None
        }
    }
}

/// The rules of the caller's own `ServiceAccount`: each of its `roles` that names a `Role`, on
/// the scope beside it, without `approve` (PF-58: a workload proposes, a person approves). Its
/// other roles are the gateway's templates and grant nothing here (CC-60).
fn account_grants(
    mirror: &Mirror,
    organization: &[(String, RoleSpec)],
    identity: &Identity,
    now: DateTime<Utc>,
) -> Vec<(Reach, Grant)> {
    let Some((account, spec)) = account_of(mirror, identity) else {
        return Vec::new();
    };
    let mut grants = Vec::new();
    // A grant outside its window grants nothing, before the Portal removes it (AG-131).
    for granted in spec.roles.iter().filter(|granted| granted.in_force(now)) {
        let Some(reach) = Reach::of(&granted.scope) else {
            continue;
        };
        let Some((role_name, role)) = role_of(mirror, organization, &reach, &granted.role) else {
            continue;
        };
        for rule in role.rules {
            let verbs: Vec<Verb> = rule
                .verbs
                .iter()
                .copied()
                .filter(|verb| *verb != Verb::Approve)
                .collect();
            if verbs.is_empty() {
                continue;
            }
            grants.push((
                reach.clone(),
                Grant {
                    role: role_name.clone(),
                    binding: format!("serviceaccount/{account}"),
                    scope: reach.name(),
                    space: None,
                    organization: None,
                    rule: Rule { verbs, ..rule },
                },
            ));
        }
    }
    grants
}

/// Whether the person `person` names (as a RoleBinding or a Group names people) may read the
/// space `space` of `project` as the space routes decide it for a caller: a binding in force
/// names them, directly or through a `Group` manifest's members (API/01 §35, T-3106).
pub fn may_read_space(
    mirror: &Mirror,
    project: &str,
    space: &str,
    person: &str,
    now: DateTime<Utc>,
) -> bool {
    let identity = named_person(mirror, person);
    // No bootstrap group: a person named in a comment is judged by their bindings alone.
    let granted = effective(mirror, "", &identity, project, now);
    granted.may_read_project() && granted.may_read_in("ContextSpace", Some(space))
}

/// The identity of the person `person` names (an e-mail, as a RoleBinding or a Group names
/// people), with the `Group` manifests whose members name them as groups.
pub fn named_person(mirror: &Mirror, person: &str) -> Identity {
    let groups = mirror
        .list(ORG_NAMESPACE, "Group", &ListOptions::default())
        .items
        .into_iter()
        .filter(|env| {
            serde_json::from_value::<jc_core::kinds::GroupSpec>(env.spec.clone()).is_ok_and(
                |spec| {
                    spec.members
                        .iter()
                        .any(|m| m.user.eq_ignore_ascii_case(person))
                },
            )
        })
        .map(|env| env.metadata.name)
        .collect();
    Identity {
        client: None,
        subject: person.to_owned(),
        username: person.to_owned(),
        email: Some(person.to_owned()),
        name: None,
        roles: Vec::new(),
        groups,
        second_factor: false,
    }
}

/// Who reaches one project's repository in the forge, by lower-case identifier (PF-87): the
/// people a binding in force at the organization or at the project lets read some kind, and
/// those it lets propose, approve or delete. A `group` subject counts through the members of its
/// `Group` manifest. A binding scoped to one context space counts for nobody: the repository
/// holds every space of the project, so a clone would reach past the grant.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectMembers {
    pub readers: BTreeSet<String>,
    pub writers: BTreeSet<String>,
    /// Those it lets approve some kind: who reviews a Change of the project (PF-50, CC-91).
    pub approvers: BTreeSet<String>,
}

pub fn project_members(mirror: &Mirror, project: &str, now: DateTime<Utc>) -> ProjectMembers {
    members_where(mirror, now, |reach| match reach {
        Reach::Organization(organization) => organization_reaches(mirror, organization, project),
        Reach::Project(held) => held == project,
        Reach::Space(_) => false,
    })
}

/// Whom a binding in force at `now` scoped to `organization` itself reads or writes for. Such a
/// binding is the repository's word that the person is of the Organization (ADR-N-067 2.2); one
/// on a project is not, and neither is the reach of another Organization's binding.
pub fn organization_scoped_people(
    mirror: &Mirror,
    organization: &str,
    now: DateTime<Utc>,
) -> BTreeSet<String> {
    let members = members_where(
        mirror,
        now,
        |reach| matches!(reach, Reach::Organization(held) if held == organization),
    );
    members.readers.into_iter().chain(members.writers).collect()
}

/// [`ProjectMembers`] by every binding in force at `now` whose scope `counts`.
fn members_where(
    mirror: &Mirror,
    now: DateTime<Utc>,
    counts: impl Fn(&Reach) -> bool,
) -> ProjectMembers {
    let organization = roles(mirror, ORG_NAMESPACE);
    let groups = group_members(mirror);
    let mut members = ProjectMembers::default();
    for env in mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
    {
        let Ok(binding) = serde_json::from_value::<RoleBindingSpec>(env.spec) else {
            continue;
        };
        if binding.validity.as_ref().is_some_and(|v| !v.contains(now)) {
            continue;
        }
        let Some(reach) = Reach::of(&binding.scope).filter(&counts) else {
            continue;
        };
        let Some((_, role)) = role_of(mirror, &organization, &reach, &binding.role) else {
            continue;
        };
        let grants = |verb: Verb| {
            role.rules
                .iter()
                .any(|rule| rule.kinds.iter().any(|kind| rule.grants(kind, verb)))
        };
        let reads = grants(Verb::Read);
        let approves = grants(Verb::Approve);
        let writes = [Verb::Propose, Verb::Delete].into_iter().any(grants) || approves;
        if !reads && !writes {
            continue;
        }
        for person in people_of(&groups, &binding.subjects) {
            if reads {
                members.readers.insert(person.clone());
            }
            if approves {
                members.approvers.insert(person.clone());
            }
            if writes {
                members.writers.insert(person);
            }
        }
    }
    members
}

/// The members of each `Group` manifest of the organization, by its name (PF-62).
pub fn group_members(mirror: &Mirror) -> BTreeMap<String, Vec<String>> {
    mirror
        .list(ORG_NAMESPACE, "Group", &ListOptions::default())
        .items
        .into_iter()
        .map(|env| {
            let members = serde_json::from_value::<jc_core::kinds::GroupSpec>(env.spec)
                .map(|spec| spec.members.into_iter().map(|m| m.user).collect())
                .unwrap_or_default();
            (env.metadata.name, members)
        })
        .collect()
}

/// The people `subjects` name, by lower-case identifier: each `user`, and each member of a
/// `group` subject's `Group` manifest.
pub fn people_of(
    groups: &BTreeMap<String, Vec<String>>,
    subjects: &[jc_core::kinds::Subject],
) -> BTreeSet<String> {
    subjects
        .iter()
        .flat_map(|subject| {
            let through = subject
                .group
                .as_ref()
                .and_then(|group| groups.get(group))
                .into_iter()
                .flatten();
            subject.user.iter().chain(through)
        })
        .map(|person| person.trim().to_ascii_lowercase())
        .filter(|person| !person.is_empty())
        .collect()
}

/// Everyone a binding in force at `now` names, at any scope: who [`is_organization_member`]
/// admits, besides the bootstrap group, which no manifest lists (DM-75, AP-173).
pub fn organization_people(mirror: &Mirror, now: DateTime<Utc>) -> BTreeSet<String> {
    let groups = group_members(mirror);
    mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| serde_json::from_value::<RoleBindingSpec>(env.spec).ok())
        .filter(|binding| binding.validity.as_ref().is_none_or(|v| v.contains(now)))
        .flat_map(|binding| people_of(&groups, &binding.subjects))
        .collect()
}

/// Every `group` subject of a `RoleBinding` or `ServiceAccount` names a `Group` manifest of the
/// organization (PF-62, PF-64). A binding to a group nobody declared matches nobody and says
/// nothing about it, which is the silence this refusal replaces. The bootstrap administrators
/// are not a subject — they are the platform setting of PF-52 — so nothing here touches them.
fn subjects_name_a_group(mirror: &Mirror, manifest: &Value) -> Result<(), ApiError> {
    let kind = manifest.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind != "RoleBinding" && kind != "ServiceAccount" {
        return Ok(());
    }
    let named = manifest
        .pointer("/spec/subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|subject| subject.get("group").and_then(Value::as_str));
    for group in named {
        if mirror.get(ORG_NAMESPACE, "Group", group).is_none() {
            let declared: Vec<String> = mirror
                .list(ORG_NAMESPACE, "Group", &ListOptions::default())
                .items
                .into_iter()
                .map(|env| env.metadata.name)
                .collect();
            return Err(ApiError::BadRequest(format!(
                "spec.subjects names the group '{group}', and no Group manifest declares it; \
                 propose the group first, or a binding to it matches nobody (PF-62, PF-64). \
                 Declared: {}",
                if declared.is_empty() {
                    "none".to_owned()
                } else {
                    declared.join(", ")
                }
            )));
        }
    }
    Ok(())
}

/// A change to the organization's access, as the PF-03 guard reads it: a manifest written, or
/// one removed.
pub enum AccessChange<'a> {
    Write(&'a Value),
    Remove {
        kind: &'a str,
        namespace: &'a str,
        name: &'a str,
    },
}

/// PF-03: the organization keeps at least one administrator. A change that would take the last
/// one away is refused before anything is written or merged, on every door that writes or removes
/// a `Role` or `RoleBinding`.
///
/// An administrator is who can hand access out and take it back: a binding at organization scope,
/// in force and naming somebody, whose organization role grants `approve` and `delete` on
/// `RoleBinding` with no constraint. The role is read by what it grants rather than by the name
/// `org-admin`, because the taxonomy is a seed an organization extends (PF-56). An organization
/// that has none yet (it runs on the bootstrap group) is not held to it: the guard stops the last
/// one from going, it does not demand a first.
pub fn keeps_an_administrator(mirror: &Mirror, change: AccessChange<'_>) -> Result<(), ApiError> {
    keeps_an_administrator_after(mirror, &[change])
}

/// [`keeps_an_administrator`] for several access changes that land together, judged on the state
/// after all of them: two removals that each leave an administrator can together leave none.
pub fn keeps_an_administrator_after(
    mirror: &Mirror,
    changes: &[AccessChange<'_>],
) -> Result<(), ApiError> {
    let mut roles: BTreeMap<String, Value> = mirror
        .list(ORG_NAMESPACE, "Role", &ListOptions::default())
        .items
        .into_iter()
        .map(|env| (env.metadata.name, env.spec))
        .collect();
    let mut bindings: BTreeMap<String, Value> = mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
        .into_iter()
        .map(|env| (env.metadata.name, env.spec))
        .collect();
    let now = Utc::now();
    let installation = installation_scope(mirror);
    let before = administrators(&roles, &bindings, installation.as_ref(), now);
    if before.is_empty() {
        return Ok(());
    }
    let mut touched = false;
    for change in changes {
        let (kind, namespace, name, written) = match change {
            AccessChange::Write(manifest) => (
                manifest
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                manifest
                    .pointer("/metadata/namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(ORG_NAMESPACE),
                manifest
                    .pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                manifest.get("spec").cloned(),
            ),
            AccessChange::Remove {
                kind,
                namespace,
                name,
            } => (*kind, *namespace, *name, None),
        };
        if !matches!(kind, "Role" | "RoleBinding") || !matches!(namespace, ORG_NAMESPACE | "") {
            continue;
        }
        touched = true;
        let table = if kind == "Role" {
            &mut roles
        } else {
            &mut bindings
        };
        match written {
            Some(spec) => table.insert(name.to_owned(), spec),
            None => table.remove(name),
        };
    }
    if touched && administrators(&roles, &bindings, installation.as_ref(), now).is_empty() {
        return Err(ApiError::Conflict(format!(
            "this change would leave the organization without an administrator: after it, no \
             binding at organization scope grants approve and delete on RoleBinding (today: {}). \
             Bind another person to such a role first, then make this change (PF-03)",
            before.join(", ")
        )));
    }
    Ok(())
}

/// Every person an administrator binding names, directly or through a `Group` they are in, by
/// lower-case e-mail (PF-03): who would be left to run the organization.
pub fn administrator_people(mirror: &Mirror) -> BTreeSet<String> {
    let table = |kind: &str| -> BTreeMap<String, Value> {
        mirror
            .list(ORG_NAMESPACE, kind, &ListOptions::default())
            .items
            .into_iter()
            .map(|env| (env.metadata.name, env.spec))
            .collect()
    };
    let roles = table("Role");
    let bindings = table("RoleBinding");
    let groups = table("Group");
    let mut people = BTreeSet::new();
    for name in administrators(
        &roles,
        &bindings,
        installation_scope(mirror).as_ref(),
        Utc::now(),
    ) {
        let Some(binding) = bindings
            .get(&name)
            .and_then(|spec| serde_json::from_value::<RoleBindingSpec>(spec.clone()).ok())
        else {
            continue;
        };
        for subject in binding.subjects {
            if let Some(user) = subject.user {
                people.insert(user.to_ascii_lowercase());
            }
            if let Some(group) = subject.group.and_then(|group| groups.get(&group).cloned()) {
                let members = serde_json::from_value::<jc_core::kinds::GroupSpec>(group)
                    .map(|spec| spec.members)
                    .unwrap_or_default();
                people.extend(members.into_iter().map(|m| m.user.to_ascii_lowercase()));
            }
        }
    }
    people
}

/// Which `spec.scope.organization` makes an administrator of the installation: any name while the
/// repository holds one Organization (as before ADR-N-058), else the installation Organization's
/// alone, `Some(None)` when none is (PF-121). `None` means any name counts.
fn installation_scope(mirror: &Mirror) -> Option<Option<String>> {
    (organizations(mirror).len() > 1).then(|| installation_organization(mirror))
}

/// The organization bindings that make somebody an administrator, by name (PF-03). With several
/// Organizations only the installation's count: another Organization's administrator keeps none
/// of the installation running (ADR-N-058 section 2.2).
fn administrators(
    roles: &BTreeMap<String, Value>,
    bindings: &BTreeMap<String, Value>,
    installation: Option<&Option<String>>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let administers = |role: &str| {
        roles
            .get(role)
            .and_then(|spec| serde_json::from_value::<RoleSpec>(spec.clone()).ok())
            .is_some_and(|spec| {
                spec.rules.iter().any(|rule| {
                    rule.constraints.is_empty()
                        && rule.kinds.iter().any(|kind| kind == "RoleBinding")
                        && rule.verbs.contains(&Verb::Approve)
                        && rule.verbs.contains(&Verb::Delete)
                })
            })
    };
    bindings
        .iter()
        .filter_map(|(name, spec)| {
            let binding: RoleBindingSpec = serde_json::from_value(spec.clone()).ok()?;
            let in_force = binding.validity.as_ref().is_none_or(|v| v.contains(now));
            let counts = match installation {
                None => binding.scope.organization.is_some(),
                Some(installation) => {
                    installation.is_some() && binding.scope.organization == *installation
                }
            };
            (counts && !binding.subjects.is_empty() && in_force && administers(&binding.role))
                .then(|| name.clone())
        })
        .collect()
}

/// Nobody grants above their own rights (PF-52, AG-77): every verb on every kind a proposed
/// `Role` (on the organization), `RoleBinding` (on its scope) or `ServiceAccount` (through each of
/// its roles the organization defines, on that role's scope) would grant must be one `identity`
/// holds there, under no constraint the new rule drops. `who` is "proposer" or "approver"; any
/// other kind and the bootstrap group pass. `before` is the manifest the change replaces: a
/// ServiceAccount whose grants are all among its grants there only takes access away, which the
/// kind's own `propose`/`approve` already decides (T-5016).
pub fn within_own_rights(
    state: &AppState,
    identity: &Identity,
    manifest: &Value,
    before: Option<&Value>,
    who: &str,
) -> Result<(), ApiError> {
    let mirror = &state.mirror;
    // PF-64: a subject that names a group names a `Group` manifest. Checked here because this
    // is the gate every door to a `users/` manifest passes through — the resource route, an
    // import, a blueprint and an approval.
    subjects_name_a_group(mirror, manifest)?;
    replaces_only_own_settings(state, identity, manifest, who)?;
    keeps_an_administrator(mirror, AccessChange::Write(manifest))?;
    let spec = manifest.get("spec").cloned().unwrap_or(Value::Null);
    let unreadable = |e: serde_json::Error| ApiError::BadRequest(format!("spec: {e}"));
    let no_scope = || {
        ApiError::BadRequest("spec.scope names no organization, project or context space".into())
    };
    let organization = roles(mirror, ORG_NAMESPACE);
    // The role a manifest names, read where that manifest reaches it: a binding at project
    // scope may name the project's own role, an organization one may not (PF-69).
    let rules_at = |name: &str, target: &Reach| {
        role_of(mirror, &organization, target, name).map(|(_, spec)| spec.rules)
    };
    let (noun, grants): (&str, Vec<(Vec<Rule>, Reach)>) =
        match manifest.get("kind").and_then(Value::as_str) {
            Some("Role") => {
                let role: RoleSpec = serde_json::from_value(spec).map_err(unreadable)?;
                // A role of a project is measured against what its proposer holds in that project,
                // an organization role against what they hold organization-wide (PF-68).
                let namespace = manifest
                    .pointer("/metadata/namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(ORG_NAMESPACE);
                let at = match namespace {
                    ORG_NAMESPACE | "" => {
                        Reach::Organization(installation_organization(mirror).unwrap_or_default())
                    }
                    project => Reach::Project(project.to_owned()),
                };
                ("role", vec![(role.rules, at)])
            }
            Some("RoleBinding") => {
                let binding: RoleBindingSpec = serde_json::from_value(spec).map_err(unreadable)?;
                let target = Reach::of(&binding.scope).ok_or_else(no_scope)?;
                let rules = rules_at(&binding.role, &target).ok_or_else(|| {
                    ApiError::Denied(format!(
                        "no role {} is defined where this binding applies; propose the role \
                     before a binding to it (PF-52, PF-69)",
                        binding.role
                    ))
                })?;
                ("binding", vec![(rules, target)])
            }
            // A service account's other roles are the gateway's role templates, which grant data
            // access through Policies and nothing here (CC-60).
            Some("ServiceAccount") => {
                let account: ServiceAccountSpec =
                    serde_json::from_value(spec).map_err(unreadable)?;
                let mut grants = Vec::new();
                let narrows = before.is_some_and(|before| only_drops_grants(before, &account));
                for granted in account.roles.iter().filter(|_| !narrows) {
                    let target = Reach::of(&granted.scope).ok_or_else(no_scope)?;
                    if let Some(rules) = rules_at(&granted.role, &target) {
                        grants.push((rules, target));
                    }
                }
                ("service account", grants)
            }
            _ => return Ok(()),
        };
    if in_group(identity, &state.config.bootstrap_admins) {
        return Ok(());
    }
    let held = in_force(mirror, identity, Utc::now());
    let mut missing: Vec<String> = Vec::new();
    for (rules, target) in &grants {
        for rule in rules {
            for kind in &rule.kinds {
                for verb in &rule.verbs {
                    // `Rule::grants`, so `propose` holds `read` here as it does at every door:
                    // an administrator who proposes a kind may hand out reading it (PF-59).
                    let holds = held.iter().any(|(reach, grant)| {
                        reach.covers(target, mirror)
                            && grant.rule.grants(kind, *verb)
                            && grant
                                .rule
                                .constraints
                                .iter()
                                .all(|c| rule.constraints.contains(c))
                    });
                    let item = format!("{} on {kind}", verb_name(*verb));
                    if !holds && !missing.contains(&item) {
                        missing.push(item);
                    }
                }
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "a {noun} may not grant more than its {who} holds: missing {} (PF-52)",
        missing.join(", ")
    )))
}

/// A caller whose only rights on an [`OWN_SETTINGS_KINDS`] manifest are narrowed to their own
/// Organization changes one already in the repository only when that one is theirs as well:
/// rewriting the scope of another Organization's binding into their own would take its access
/// away (ADR-N-058 section 2.4, T-5248). The manifest replaced is read from the mirror, so every
/// door (the resource route, an import, a workspace, an approval) is judged alike.
fn replaces_only_own_settings(
    state: &AppState,
    identity: &Identity,
    manifest: &Value,
    who: &str,
) -> Result<(), ApiError> {
    let text = |path: &str| manifest.pointer(path).and_then(Value::as_str);
    let (Some(kind), Some(name)) = (text("/kind"), text("/metadata/name")) else {
        return Ok(());
    };
    if !OWN_SETTINGS_KINDS.contains(&kind)
        || !matches!(text("/metadata/namespace"), None | Some(ORG_NAMESPACE | ""))
    {
        return Ok(());
    }
    let verb = if who == "approver" {
        Verb::Approve
    } else {
        Verb::Propose
    };
    let effective = for_request(state, identity, ORG_NAMESPACE);
    let narrowed_only = !effective.bootstrap
        && effective.grants.iter().any(|g| g.organization.is_some())
        && !effective
            .grants
            .iter()
            .any(|g| g.organization.is_none() && g.rule.grants(kind, verb));
    if !narrowed_only {
        return Ok(());
    }
    // An App's default group carries that App's roles (AP-118): only the installation names one.
    if kind == "Group"
        && manifest
            .pointer("/metadata/annotations")
            .and_then(|annotations| annotations.get(crate::apps::reconciler::APP_LABEL))
            .is_some()
    {
        return Err(ApiError::Denied(format!(
            "metadata.annotations: the Group {name} may not name an App; an App's default group \
             is the installation administrators' to make (ADR-N-058, AP-118)"
        )));
    }
    let Some(replaced) = state.mirror.get(ORG_NAMESPACE, kind, name) else {
        return Ok(());
    };
    let replaced = serde_json::to_value(replaced).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check(kind, verb, Some(&replaced)).map_err(|_| {
        ApiError::Denied(format!(
            "{kind} {name} as it stands belongs to another Organization or to the installation; \
             its administrators change it (ADR-N-058, PF-123)"
        ))
    })
}

/// Whether every grant of `head` is one `before` held, each matched once and compared whole, so a
/// widened grant (a wider scope, more types or operations, a longer window) counts as new.
fn only_drops_grants(before: &Value, head: &ServiceAccountSpec) -> bool {
    let Some(mut held) = before
        .get("spec")
        .and_then(|spec| serde_json::from_value::<ServiceAccountSpec>(spec.clone()).ok())
        .map(|spec| spec.roles)
    else {
        return false;
    };
    head.roles.iter().all(|grant| {
        held.iter()
            .position(|kept| kept == grant)
            .map(|at| held.remove(at))
            .is_some()
    })
}

fn in_group(identity: &Identity, group: &str) -> bool {
    // Keycloak realm roles count as groups here: the dev realm assigns people through them
    // and a realm role, like a group, says who somebody is, not what they may do (PF-50).
    identity.groups.iter().any(|g| g == group) || identity.roles.iter().any(|r| r == group)
}

/// A Keycloak user id as ADR-N-061 writes a person into a manifest: a lower-case hyphenated
/// UUID, the token's `sub`.
pub fn is_person_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, len)| {
            group.len() == len
                && group
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn is_subject(identity: &Identity, subject: &jc_core::kinds::Subject) -> bool {
    names(identity, subject.user.as_deref(), subject.group.as_deref())
}

/// Whether a `user` a manifest writes (a `Group` member, a binding subject) is `identity`: by
/// Keycloak id (ADR-N-061), or by the e-mail or username an older manifest wrote. The one
/// matcher for a person everywhere a manifest names one.
pub fn names_user(identity: &Identity, user: &str) -> bool {
    names(identity, Some(user), None)
}

/// [`is_subject`] over a subject's `user` and `group`.
fn names(identity: &Identity, user: Option<&str>, group: Option<&str>) -> bool {
    if let Some(user) = user {
        // A person's id matches their `sub` and nothing else: a username or an address that
        // spells somebody's id is not them (ADR-N-061). Any other value is an e-mail an older
        // manifest wrote, or a `service-account-<client>` user.
        if is_person_id(user) {
            return identity.subject == user;
        }
        return identity
            .email
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(user))
            || identity.username.eq_ignore_ascii_case(user)
            || identity.subject == user;
    }
    group.is_some_and(|g| in_group(identity, g))
}

/// The audience of the manifest under approval, when it has one (EP-14).
fn audience_of(target: Option<&Value>) -> Option<&str> {
    target?.pointer("/spec/audience")?.as_str()
}

/// Whether a manifest of `kind` names its context space in `spec.contextSpaceRef`, or is the
/// space itself: the kinds a binding scoped to one space can reach (T-4593).
pub fn lives_in_space(kind: &str) -> bool {
    matches!(
        kind,
        "ContextSpace"
            | "Endpoint"
            | "Policy"
            | "Subscription"
            | "ContextSourceRegistration"
            | "DataModel"
            | "Mapping"
            | "ModelProjection"
            | "DataOffer"
    )
}

/// `spec.contextSpaceRef` of a manifest, written bare or as `{ name }`.
pub fn space_ref(target: &Value) -> Option<String> {
    target.get("spec").and_then(spec_space)
}

/// The space a manifest's `spec` names by `contextSpaceRef`, as a name or a reference.
pub fn spec_space(spec: &Value) -> Option<String> {
    let value = spec.get("contextSpaceRef")?;
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The value at a dotted path of the manifest, as text; `None` when absent or structured.
fn field_text(target: Option<&Value>, path: &str) -> Option<String> {
    let mut cursor = target?;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    match cursor {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Whether `rule` is the build lane's: `propose` on `App` constrained to `status.build`, which
/// names the field's writer and carries no operator (AP-73, jc-core `Rule::writes_status_only`).
fn writes_status_only(rule: &Rule) -> bool {
    rule.constraints.iter().any(|c| c.field == "status.build")
}

/// jc-core reads the operators, so the Portal and the forge's `roles.rego` agree (PF-51).
fn satisfied(constraint: &Constraint, target: Option<&Value>) -> bool {
    constraint.holds(field_text(target, &constraint.field).as_deref())
}

fn describe(constraint: &Constraint) -> String {
    if let Some(expected) = &constraint.equals {
        format!("must equal {expected}")
    } else if let Some(pattern) = &constraint.pattern {
        format!("must match {pattern}")
    } else if !constraint.one_of.is_empty() {
        format!("must be one of {}", constraint.one_of.join(", "))
    } else {
        format!("must not be one of {}", constraint.not_in.join(", "))
    }
}

pub fn verb_name(verb: Verb) -> &'static str {
    verb.as_str()
}

/// Where one grant of a ServiceAccount stands at `now` (AG-131).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum GrantState {
    /// In force: no window, or `now` inside it.
    Active,
    /// Its `notBefore` is still ahead.
    Pending,
    /// Its `notAfter` has passed: it grants nothing and the Portal removes it.
    Expired,
}

/// One entry of a ServiceAccount's `spec.roles` as its Access tab reads it (AG-130): the role and
/// scope, the window and where it stands, and the Portal rules its `Role` carries, without
/// `approve`, which no account holds (PF-58). A role that names no `Role` (a gateway template,
/// CC-60) carries no Portal rule and acts at the gateway alone.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountGrant {
    /// The position in `spec.roles`, which a revoke names with the role (AG-132).
    pub index: usize,
    pub role: String,
    /// `organization`, `project:{name}` or `contextSpace:{name}`.
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub not_after: Option<DateTime<Utc>>,
    pub state: GrantState,
    pub rules: Vec<AccountRule>,
}

/// One rule of the `Role` behind an [`AccountGrant`]: the jc-core [`Rule`] with `approve` taken
/// out, typed for the OpenAPI document so the Access tab reads `kinds` and `verbs` (AG-130).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountRule {
    /// Manifest kinds the rule covers, e.g. `Pipeline`.
    pub kinds: Vec<String>,
    /// Verbs granted on those kinds, e.g. `propose`; never `approve`.
    #[schema(value_type = Vec<String>)]
    pub verbs: Vec<Verb>,
    /// Constraints every covered manifest must satisfy, as the Role writes them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Object>)]
    pub constraints: Vec<Constraint>,
}

/// Every grant of `spec` at `now`, in manifest order (AG-130).
pub fn account_access(
    mirror: &Mirror,
    spec: &ServiceAccountSpec,
    now: DateTime<Utc>,
) -> Vec<AccountGrant> {
    let organization = roles(mirror, ORG_NAMESPACE);
    spec.roles
        .iter()
        .enumerate()
        .map(|(index, granted)| {
            let window = granted.validity.clone().unwrap_or_default();
            let state = match (window.not_before, window.not_after) {
                (Some(from), _) if now < from => GrantState::Pending,
                (_, Some(to)) if to < now => GrantState::Expired,
                _ => GrantState::Active,
            };
            let reach = Reach::of(&granted.scope);
            let rules = reach
                .as_ref()
                .and_then(|reach| role_of(mirror, &organization, reach, &granted.role))
                .map(|(_, role)| {
                    role.rules
                        .into_iter()
                        .filter_map(|rule| {
                            let verbs: Vec<Verb> = rule
                                .verbs
                                .iter()
                                .copied()
                                .filter(|v| *v != Verb::Approve)
                                .collect();
                            (!verbs.is_empty()).then_some(AccountRule {
                                kinds: rule.kinds,
                                verbs,
                                constraints: rule.constraints,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            AccountGrant {
                index,
                role: granted.role.clone(),
                scope: reach.map(|reach| reach.name()).unwrap_or_default(),
                not_before: window.not_before,
                not_after: window.not_after,
                state,
                rules,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        }
    }

    /// ADR-N-067 2.2 (T-6408): Keycloak writes groups as full paths and the session keeps them
    /// with one leading `/` trimmed, so a member of `/helsinki/members` is in no RoleBinding
    /// subject group `members`; the root group `/helsinki` still reads `helsinki`.
    #[test]
    fn an_organizations_subgroup_is_in_no_binding_group_of_its_bare_name() {
        let member = Identity {
            client: None,
            subject: "f:1:ana".to_owned(),
            username: "ana".to_owned(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["helsinki".to_owned(), "helsinki/members".to_owned()],
            second_factor: false,
        };
        assert!(in_group(&member, "helsinki"));
        assert!(!in_group(&member, "members"));
    }

    #[test]
    fn a_person_id_subject_matches_their_sub_and_no_username_or_address_spelling_it() {
        let id = "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e";
        let subject = jc_core::kinds::Subject {
            user: Some(id.to_owned()),
            ..Default::default()
        };
        let person = |sub: &str, username: &str, email: &str| Identity {
            client: None,
            subject: sub.to_owned(),
            username: username.to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        };
        assert!(is_subject(&person(id, "jana", "jana@hel.fi"), &subject));
        assert!(!is_subject(&person("other", id, "x@hel.fi"), &subject));
        assert!(!is_subject(&person("other", "x", id), &subject));
        // An address an older manifest wrote still names its person until the migration lands.
        let legacy = jc_core::kinds::Subject {
            user: Some("Jana@hel.fi".to_owned()),
            ..Default::default()
        };
        assert!(is_subject(&person(id, "jana", "jana@hel.fi"), &legacy));
        assert!(is_person_id(id));
        assert!(!is_person_id("jana@hel.fi"));
        assert!(!is_person_id(&id.to_uppercase()));
        assert!(!is_person_id("2f1c6b1e7a4d4c559e0e3b8f1a2d4c6e"));
    }

    #[test]
    fn a_binding_spec_is_read_in_place_for_its_subjects() {
        let jana = Identity {
            client: None,
            subject: "sub-jana".to_owned(),
            username: "jana".to_owned(),
            email: Some("jana@hel.fi".to_owned()),
            name: None,
            roles: Vec::new(),
            groups: vec!["doprava-team".to_owned()],
            second_factor: false,
        };
        let spec = |subjects: Value| serde_json::json!({ "subjects": subjects, "role": "viewer" });
        assert!(names_caller(
            &jana,
            &spec(serde_json::json!([{ "user": "JANA@hel.fi" }]))
        ));
        assert!(names_caller(
            &jana,
            &spec(serde_json::json!([{ "user": "x@hel.fi" }, { "group": "doprava-team" }]))
        ));
        assert!(!names_caller(
            &jana,
            &spec(serde_json::json!([{ "user": "x@hel.fi" }]))
        ));
        assert!(!names_caller(
            &jana,
            &spec(serde_json::json!([{ "user": 7 }]))
        ));
        assert!(!names_caller(
            &jana,
            &serde_json::json!({ "subjects": "jana" })
        ));
    }

    #[test]
    fn only_a_subset_of_the_grants_held_before_drops_grants() {
        let grant = |project: &str| serde_json::json!({ "role": "editor", "scope": { "project": project } });
        let account = |roles: Value| {
            serde_json::json!({ "spec": { "owner": { "user": "jana@hel.fi" }, "purpose": "ci",
                                          "roles": roles, "credentials": [] } })
        };
        let spec = |roles: Value| -> ServiceAccountSpec {
            serde_json::from_value(account(roles)["spec"].clone()).expect("spec")
        };
        let before = account(serde_json::json!([grant("a"), grant("b")]));
        assert!(only_drops_grants(
            &before,
            &spec(serde_json::json!([grant("b")]))
        ));
        assert!(only_drops_grants(
            &before,
            &spec(serde_json::json!([grant("a"), grant("b")]))
        ));
        // A grant held once cannot be counted twice, a new scope is new, a widened grant is new.
        assert!(!only_drops_grants(
            &before,
            &spec(serde_json::json!([grant("a"), grant("a")]))
        ));
        assert!(!only_drops_grants(
            &before,
            &spec(serde_json::json!([grant("c")]))
        ));
        let mut wider = grant("a");
        wider["types"] = serde_json::json!(["Vehicle"]);
        let typed = account(serde_json::json!([wider]));
        assert!(!only_drops_grants(
            &typed,
            &spec(serde_json::json!([grant("a")]))
        ));
        // Nothing to compare with measures every grant.
        assert!(!only_drops_grants(
            &Value::Null,
            &spec(serde_json::json!([grant("a")]))
        ));
    }

    /// T-6464: propose on any kind opens a conversation; reading does not, and the build lane's
    /// rule, which writes `status.build` alone (AP-73), does not either.
    #[test]
    fn a_conversation_needs_a_real_propose_on_some_kind() {
        let mirror = Mirror::new();
        for (role, rule) in [
            (
                "builder",
                serde_json::json!({ "kinds": ["App"], "verbs": ["propose"],
                                    "constraints": [{ "field": "status.build" }] }),
            ),
            (
                "reader",
                serde_json::json!({ "kinds": ["App", "Endpoint"], "verbs": ["read"] }),
            ),
            (
                "editor",
                serde_json::json!({ "kinds": ["Endpoint"], "verbs": ["read", "propose"] }),
            ),
        ] {
            mirror.upsert(org("Role", role, serde_json::json!({ "rules": [rule] })));
            mirror.upsert(org(
                "RoleBinding",
                &format!("{role}-here"),
                serde_json::json!({ "subjects": [{ "user": format!("{role}@hel.fi") }],
                                    "role": role, "scope": { "project": "espoo" } }),
            ));
        }
        for (role, converses) in [("builder", false), ("reader", false), ("editor", true)] {
            let who = Identity {
                client: None,
                subject: format!("f:1:{role}"),
                username: format!("{role}@hel.fi"),
                email: Some(format!("{role}@hel.fi")),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            };
            let grants = effective(&mirror, "portal-approver", &who, "espoo", Utc::now());
            assert_eq!(grants.check_converses("espoo").is_ok(), converses, "{role}");
        }
    }

    #[test]
    fn the_organization_is_everyone_a_binding_in_force_names_each_once() {
        let mirror = Mirror::new();
        mirror.upsert(org(
            "Group",
            "inspectors",
            serde_json::json!({ "members": [{ "user": "Jana@hel.fi" }, { "user": "vera@hel.fi" }] }),
        ));
        mirror.upsert(org(
            "RoleBinding",
            "inspectors-here",
            serde_json::json!({ "subjects": [{ "group": "inspectors" }, { "user": "jana@hel.fi" }],
                                "role": "viewer", "scope": { "project": "espoo" } }),
        ));
        mirror.upsert(org(
            "RoleBinding",
            "lapsed",
            serde_json::json!({ "subjects": [{ "user": "otto@hel.fi" }], "role": "viewer",
                                "scope": { "organization": "hel" },
                                "validity": { "notAfter": "2020-01-01T00:00:00Z" } }),
        ));
        let people = organization_people(&mirror, Utc::now());
        assert_eq!(
            people.into_iter().collect::<Vec<_>>(),
            ["jana@hel.fi", "vera@hel.fi"],
            "a group's members count through it, a lapsed binding counts for nobody"
        );
        let groups = group_members(&mirror);
        let subjects: Vec<jc_core::kinds::Subject> =
            serde_json::from_value(serde_json::json!([{ "group": "nobody" }, { "user": "  " }]))
                .expect("subjects");
        assert!(people_of(&groups, &subjects).is_empty());
    }

    #[test]
    fn a_project_scoped_administrator_reads_what_its_role_proposes_and_nothing_elsewhere() {
        let mirror = Mirror::new();
        mirror.upsert(org(
            "Role",
            "org-admin",
            serde_json::json!({ "rules": [{ "kinds": ["RoleBinding", "Pipeline"],
                                            "verbs": ["propose", "approve", "delete"] }] }),
        ));
        mirror.upsert(org(
            "RoleBinding",
            "owner",
            serde_json::json!({ "subjects": [{ "user": "owner@hel.fi" }], "role": "org-admin",
                                "scope": { "project": "t-full" } }),
        ));
        let owner = Identity {
            client: None,
            subject: "sub-owner".to_owned(),
            username: "owner".to_owned(),
            email: Some("owner@hel.fi".to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        };
        let at = |project: &str| effective(&mirror, "no-bootstrap", &owner, project, Utc::now());
        // `jc_member_list` asks read on RoleBinding: propose implies it, as `may` already says.
        assert!(at("t-full").check("RoleBinding", Verb::Read, None).is_ok());
        assert!(at("t-full").check("Pipeline", Verb::Propose, None).is_ok());
        assert!(at("t-full").check("Endpoint", Verb::Read, None).is_err());
        assert!(at("espoo").check("RoleBinding", Verb::Read, None).is_err());
    }
}
