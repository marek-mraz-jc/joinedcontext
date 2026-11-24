//! The Keycloak groups the repository owns (PF-62, PF-63, T-0866).
//!
//! A `Group` manifest is the membership; Keycloak is where it takes effect. This wave is the
//! only writer of the groups it manages: each one carries the attribute `managed-by:
//! joinedcontext`, and a group without that attribute — the bootstrap administrators' group
//! included — is never read for drift, never written and never removed.
//!
//! What somebody changed in the console between two runs is overwritten and reported, so the
//! repository stays the answer to "who is in this group" and the Access page can show that the
//! two disagreed (PF-63, PF-51).

use crate::config::ClientAuth;
use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::GroupSpec;
use jc_time::TimestampTz;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::permissions::{
    is_person_id, organization_of, organization_scoped_people, project_members, ORG_NAMESPACE,
};
use crate::store::{ListOptions, Mirror};

/// The attribute that says a group is this platform's to write (PF-63).
pub const MANAGED_BY: &str = "managed-by";
pub const MANAGED_VALUE: &str = "joinedcontext";

/// The attributes the realm's `jc_org` and `jc_groups` mappers read (ADR-N-067 2.2).
const ORGANIZATION_ATTRIBUTE: &str = "jc_org";
/// The attribute the realm's `jc_org_id` mapper reads: the Organization's id (EP-186), which the
/// gateway sends to the broker for its Organization check (EP-188).
const ORGANIZATION_ID_ATTRIBUTE: &str = "jc_org_id";
const GROUPS_ATTRIBUTE: &str = "jc_groups";

/// The attribute kaisatsu reads on a person's groups to ask them for a second factor
/// (`kaisatsu_oidc::ORG_SECOND_FACTOR_ATTRIBUTE`, T-6894): written `required` on an
/// Organization's group while its manifest sets `spec.policies.signIn.secondFactor: required`.
pub const SECOND_FACTOR_ATTRIBUTE: &str = "jc.secondFactor";

/// The ending of an Organization's group of bound people, [`organization_group`]. A project,
/// Group or Organization name is DNS-1123, so no name of those can end in it.
const ORGANIZATION_GROUP_ENDING: &str = "_bound";

/// The root group whose members hold `jc_org` of `organization` by an Organization-scoped
/// RoleBinding (ADR-N-067 2.2, T-6470).
pub fn organization_group(organization: &str) -> String {
    format!("{organization}{ORGANIZATION_GROUP_ENDING}")
}

/// The Organization an [`organization_group`] gives its members.
pub fn organization_of_group(group: &str) -> Option<&str> {
    group
        .strip_suffix(ORGANIZATION_GROUP_ENDING)
        .filter(|organization| !organization.is_empty())
}

/// A group the platform derives, by Keycloak user id, and what it adds to its members' tokens.
/// A project's `{slug}-readers` and `{slug}-writers` (PF-87) hold whoever a binding reaches,
/// vendors included, so they name the group (`jc_groups`) and never the Organization (`jc_org`):
/// that comes from `/{organization}/members` or from [`organization_group`] (T-6470).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedGroup {
    pub name: String,
    pub members: BTreeSet<String>,
    pub jc_org: Option<String>,
    /// The id of the Organization in `jc_org`, from its manifest's `spec.domain` (EP-186);
    /// `None` when the domain gives no id.
    pub jc_org_id: Option<String>,
    pub jc_groups: Option<String>,
    /// The Organization in `jc_org` asks every member for a second factor (T-6895).
    pub second_factor: bool,
}

impl DerivedGroup {
    /// The Organization the group belongs to: the one it grants (`jc_org`), or the one whose
    /// project it serves (the first segment of `jc_groups`).
    pub fn organization(&self) -> Option<&str> {
        self.jc_org.as_deref().or_else(|| {
            let path = self.jc_groups.as_deref()?.strip_prefix('/')?;
            path.split_once('/').map(|(organization, _)| organization)
        })
    }
}

/// Whether the Organization manifest `organization` sets `spec.policies.signIn.secondFactor:
/// required`. Only `spec.policies` is read. A block that does not parse (a manifest that skipped
/// the propose check: a direct git edit, an older Portal) counts as required, so a broken
/// manifest never loosens the rule (T-6895).
fn requires_second_factor(mirror: &Mirror, organization: &str) -> bool {
    use jc_core::kinds::{OrganizationPolicies, SecondFactor};
    let Some(policies) = mirror
        .get(ORG_NAMESPACE, "Organization", organization)
        .and_then(|org| org.spec.get("policies").cloned())
    else {
        return false;
    };
    match serde_json::from_value::<OrganizationPolicies>(policies) {
        Ok(policies) => policies.sign_in.second_factor == Some(SecondFactor::Required),
        Err(why) => {
            tracing::warn!(
                %organization,
                %why,
                "Organization policies do not parse: second factor kept as required"
            );
            true
        }
    }
}

/// The id of the Organization manifest `organization` (`sk_bbsk` for `bbsk.sk`, EP-186), as the
/// gateway derives a tenant's Organization (PF-130): from its `spec.domain` alone.
pub(crate) fn organization_id(mirror: &Mirror, organization: &str) -> Option<String> {
    let org = mirror.get(ORG_NAMESPACE, "Organization", organization)?;
    let domain = org.spec.get("domain")?.as_str()?;
    jc_core::names::OrganizationId::from_domain(domain)
        .map(|id| id.as_str().to_owned())
        .ok()
}

/// The two groups of a project: who reads it and who writes it.
fn teams_of(slug: &str) -> [String; 2] {
    [format!("{slug}-readers"), format!("{slug}-writers")]
}

/// Every registered project's derived groups, and the group of each of their Organizations, as
/// `mirror` holds its bindings at `now`: a binding taken away or run out empties its person from
/// them, and only a binding that reaches the project counts, so none of another Organization
/// grants its Organization here.
pub fn derived_groups(
    mirror: &Mirror,
    projects: &BTreeMap<String, String>,
    now: TimestampTz,
) -> Vec<DerivedGroup> {
    let of_projects = projects.keys().flat_map(|slug| {
        let members = project_members(mirror, slug, now);
        let organization = organization_of(mirror, slug);
        let [readers, writers] = teams_of(slug);
        [(readers, members.readers), (writers, members.writers)].map(|(name, members)| {
            let jc_groups = organization.as_ref().map(|org| format!("/{org}/{name}"));
            DerivedGroup {
                name,
                members: people_only(members),
                jc_org: None,
                jc_org_id: None,
                jc_groups,
                second_factor: false,
            }
        })
    });
    // An Organization that asks for a second factor has its group even before its first
    // project, so its members are asked from their first binding on (T-6895).
    let organizations: BTreeSet<String> = projects
        .keys()
        .filter_map(|slug| organization_of(mirror, slug))
        .chain(
            mirror
                .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
                .items
                .into_iter()
                .map(|org| org.metadata.name)
                .filter(|name| requires_second_factor(mirror, name)),
        )
        .collect();
    let of_organizations = organizations.into_iter().map(|organization| DerivedGroup {
        name: organization_group(&organization),
        members: people_only(organization_scoped_people(mirror, &organization, now)),
        second_factor: requires_second_factor(mirror, &organization),
        jc_org_id: organization_id(mirror, &organization),
        jc_org: Some(organization),
        jc_groups: None,
    });
    of_projects.chain(of_organizations).collect()
}

/// The people of a binding's subjects: a `service-account-<client>` subject reads with its own
/// client and joins no group, and left in it would hold every group of its Organization at
/// "not named by their Keycloak user id" (T-6514).
fn people_only(members: BTreeSet<String>) -> BTreeSet<String> {
    members
        .into_iter()
        .filter(|member| !member.starts_with("service-account-"))
        .collect()
}

/// What one run did with one group, in the shape the mirror and the feed both carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupOutcome {
    pub name: String,
    /// Empty when the realm already matched the manifest.
    pub drift: Vec<String>,
    /// A member the realm has no user for: the binding takes effect at their first login
    /// (PF-04), so this is a warning and never a failure.
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

impl GroupOutcome {
    fn of(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            drift: Vec::new(),
            warnings: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct KcGroup {
    id: String,
    name: String,
    #[serde(default)]
    attributes: std::collections::HashMap<String, Vec<String>>,
}

impl KcGroup {
    fn managed(&self) -> bool {
        self.attributes
            .get(MANAGED_BY)
            .is_some_and(|values| values.iter().any(|v| v == MANAGED_VALUE))
    }
}

#[derive(Debug, Deserialize)]
struct KcUser {
    id: String,
}

/// The realm's groups, as this Portal's own client may write them.
pub struct GroupSync {
    admin: super::app_clients::Admin,
    /// Where a run records the groups it may not write, for the write doors (AP-115).
    foreign: Option<std::sync::Arc<super::foreign::ForeignNames>>,
    /// The user ids the realm answered 404 for, and when: a manifest naming an id the realm
    /// lacks asked the realm again on every run (jc-smoke-logs: ~600 `GET /users/{id}` a minute
    /// at a 10 s sync). Asked again once [`ABSENT_FOR`] has passed.
    absent: std::sync::Mutex<BTreeMap<String, std::time::Instant>>,
    /// The Organization whose own realm this is (T-7144): only its derived groups and its
    /// `/{organization}/members` are written there, never a Group manifest or another
    /// Organization's group. `None`: the installation realm.
    organization: Option<String>,
    /// The operators' group (`JC_PORTAL_BOOTSTRAP_ADMINS`, T-7129 §4): its members change only
    /// by an operator Change a second operator approved, never by a Group manifest.
    operators: Option<String>,
}

/// How long a user the realm lacks is taken as still missing before the realm is asked again.
const ABSENT_FOR: std::time::Duration = std::time::Duration::from_secs(300);

impl GroupSync {
    /// Makes each run record the realm's unmanaged group names where the write doors read them.
    pub fn with_foreign(mut self, foreign: std::sync::Arc<super::foreign::ForeignNames>) -> Self {
        self.foreign = Some(foreign);
        self
    }

    /// `None` when no admin client is configured: the groups are then read from the repository
    /// and written nowhere, which is what a Portal without the credential does.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth) -> Option<Self> {
        Some(Self {
            admin: super::app_clients::Admin::new(issuer, client_id, auth)?,
            foreign: None,
            absent: Default::default(),
            organization: None,
            operators: None,
        })
    }

    /// Never writes `group`, the operators' group, from a Group manifest (T-7129 §4, T-7139).
    pub fn with_operators(mut self, group: &str) -> Self {
        self.operators = Some(group.to_owned());
        self
    }

    /// Makes this the sync of `organization`'s own realm (T-7144).
    pub fn for_organization(mut self, organization: &str) -> Self {
        self.organization = Some(organization.to_owned());
        self
    }

    /// Brings the realm to what the manifests say, and answers what it had to change.
    ///
    /// The mirror is the repository as this run loaded it; only `Group` manifests of the
    /// organization namespace are read, and only groups carrying the managed attribute are
    /// written or removed.
    pub async fn converge(&self, mirror: &Mirror) -> Vec<GroupOutcome> {
        self.converge_with(mirror, &[]).await
    }

    /// [`Self::converge`], and besides the manifests the groups the platform derives: a
    /// project's `{slug}-readers` and `{slug}-writers`, which its bindings fill (PF-87). They are
    /// managed like a manifest's group and removed with the project; a `Group` manifest of the
    /// same name wins, and the clash is the outcome's error.
    pub async fn converge_with(
        &self,
        mirror: &Mirror,
        generated: &[DerivedGroup],
    ) -> Vec<GroupOutcome> {
        let token = match self.admin.token().await {
            Ok(token) => token,
            Err(err) => {
                return vec![GroupOutcome {
                    name: "*".to_owned(),
                    drift: Vec::new(),
                    warnings: Vec::new(),
                    error: Some(err),
                }]
            }
        };

        let existing: Vec<KcGroup> = match self
            .admin
            .get_all(&token, "/groups?briefRepresentation=false")
            .await
        {
            Ok(groups) => groups,
            Err(err) => {
                return vec![GroupOutcome {
                    name: "*".to_owned(),
                    drift: Vec::new(),
                    warnings: Vec::new(),
                    error: Some(err),
                }]
            }
        };

        if let Some(foreign) = self.foreign.as_ref() {
            foreign.set_groups(
                existing
                    .iter()
                    .filter(|group| !group.managed())
                    .map(|group| group.name.clone())
                    .collect(),
            );
        }

        let mut outcomes = Vec::new();
        let mut wanted: BTreeSet<String> = BTreeSet::new();
        // The Group manifests are the installation's: an Organization's realm holds its own
        // derived groups and its members group alone.
        let manifests = match self.organization.as_deref() {
            Some(organization) => {
                wanted.insert(organization.to_owned());
                outcomes.push(
                    self.keep_members_group(&token, organization, &existing)
                        .await,
                );
                Vec::new()
            }
            None => {
                mirror
                    .list(ORG_NAMESPACE, "Group", &ListOptions::default())
                    .items
            }
        };
        for envelope in manifests {
            let name = envelope.metadata.name.clone();
            wanted.insert(name.clone());
            if self.operators.as_deref() == Some(name.as_str()) {
                let mut outcome = GroupOutcome::of(&name);
                outcome.error = Some(format!(
                    "'{name}' is the installation's operators' group: its members change only by \
                     an operator Change a second operator approves (OperatorAdd, OperatorRemove), \
                     never by a Group manifest. Remove the manifest or rename it"
                ));
                outcomes.push(outcome);
                continue;
            }
            let spec: GroupSpec = match serde_json::from_value(envelope.spec) {
                Ok(spec) => spec,
                Err(err) => {
                    let mut outcome = GroupOutcome::of(&name);
                    outcome.error = Some(format!("the manifest does not parse: {err}"));
                    outcomes.push(outcome);
                    continue;
                }
            };
            let held = existing.iter().find(|group| group.name == name);
            let members = spec
                .members
                .iter()
                .map(|member| member.user.trim().to_owned())
                .collect();
            outcomes.push(self.converge_one(&token, &name, &members, None, held).await);
        }
        let generated = generated.iter().filter(|derived| {
            self.organization.is_none() || derived.organization() == self.organization.as_deref()
        });
        for derived in generated {
            let name = &derived.name;
            if !wanted.insert(name.clone()) {
                let mut outcome = GroupOutcome::of(name);
                outcome.error = Some(format!(
                    "a Group manifest declares '{name}', the name of a project's forge group \
                     (PF-87); the manifest decides its members, so the project's bindings do not \
                     reach the forge through it. Rename the manifest"
                ));
                outcomes.push(outcome);
                continue;
            }
            let held = existing.iter().find(|group| &group.name == name);
            outcomes.push(
                self.converge_one(&token, name, &derived.members, Some(derived), held)
                    .await,
            );
        }

        // A managed group the repository no longer declares is this wave's to remove; one
        // without the attribute belongs to whoever made it (PF-63).
        // Before the source has loaded once (Mirror::hold_removal) nothing goes: a fresh
        // installation's seed declares no group, and a delete drops the group's members too.
        for group in existing.iter().filter(|g| g.managed()) {
            if wanted.contains(&group.name) || mirror.hold_removal("Group", &group.name) {
                continue;
            }
            let mut outcome = GroupOutcome::of(&group.name);
            match self
                .admin
                .send(
                    &token,
                    reqwest::Method::DELETE,
                    &format!("/groups/{}", group.id),
                    None,
                )
                .await
            {
                Ok(_) => outcome.drift.push(
                    "the group was managed here and no manifest declares it any more; it was \
                     removed from the realm"
                        .to_owned(),
                ),
                Err(err) => outcome.error = Some(err),
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    /// `/{organization}/members` in the Organization's own realm, where an invitation puts a new
    /// person (ADR-N-067 §2.2): the root group is made, managed, when the realm lacks it, and
    /// the subgroup under it. Its members are the invitations' and are never pruned here.
    async fn keep_members_group(
        &self,
        token: &str,
        organization: &str,
        existing: &[KcGroup],
    ) -> GroupOutcome {
        let mut outcome = GroupOutcome::of(&format!("{organization}/members"));
        let root = match existing.iter().find(|group| group.name == organization) {
            Some(group) => group.id.clone(),
            None => match self.create(token, organization, None).await {
                Ok(id) => {
                    outcome.drift.push(format!(
                        "the realm had no group '{organization}'; it was created"
                    ));
                    id
                }
                Err(err) => {
                    outcome.error = Some(err);
                    return outcome;
                }
            },
        };
        let children: Vec<KcGroup> = match self
            .admin
            .get_all(
                token,
                &format!("/groups/{root}/children?briefRepresentation=true"),
            )
            .await
        {
            Ok(children) => children,
            Err(err) => {
                outcome.error = Some(err);
                return outcome;
            }
        };
        if children.iter().any(|child| child.name == "members") {
            return outcome;
        }
        match self
            .admin
            .send(
                token,
                reqwest::Method::POST,
                &format!("/groups/{root}/children"),
                Some(&json!({ "name": "members" })),
            )
            .await
        {
            Ok(_) => outcome.drift.push(format!(
                "the realm had no '/{organization}/members'; it was created"
            )),
            Err(err) => outcome.error = Some(err),
        }
        outcome
    }

    /// Creates the managed top-level group `name` and answers its id.
    async fn create(
        &self,
        token: &str,
        name: &str,
        derived: Option<&DerivedGroup>,
    ) -> Result<String, String> {
        self.admin
            .send(
                token,
                reqwest::Method::POST,
                "/groups",
                Some(&json!({ "name": name, "attributes": attributes_of(derived) })),
            )
            .await?;
        let found = self
            .admin
            .get::<Value>(token, &format!("/groups?search={name}&exact=true"))
            .await
            .and_then(|v| serde_json::from_value::<Vec<KcGroup>>(v).map_err(|e| e.to_string()))?;
        found
            .into_iter()
            .find(|group| group.name == name)
            .map(|group| group.id)
            .ok_or_else(|| "the realm did not return the group it just created".to_owned())
    }

    async fn converge_one(
        &self,
        token: &str,
        name: &str,
        wanted: &BTreeSet<String>,
        derived: Option<&DerivedGroup>,
        held: Option<&KcGroup>,
    ) -> GroupOutcome {
        let mut outcome = GroupOutcome::of(name);
        let id = match held {
            Some(group) if !group.managed() => {
                // Somebody else's group under the same name: it is not ours to write, and
                // saying so is the whole of the report (PF-63).
                outcome.error = Some(format!(
                    "the realm has a group '{name}' without the {MANAGED_BY}: {MANAGED_VALUE} \
                     attribute; the platform never writes a group it does not manage"
                ));
                return outcome;
            }
            Some(group) => group.id.clone(),
            None => match self.create(token, name, derived).await {
                Ok(id) => {
                    outcome
                        .drift
                        .push(format!("the realm had no group '{name}'; it was created"));
                    id
                }
                Err(err) => {
                    outcome.error = Some(err);
                    return outcome;
                }
            },
        };

        self.keep_claims(token, held, derived, &mut outcome).await;

        // A manifest written before ADR-N-061 names members by e-mail. Matching those against
        // the realm's ids would take every such person out, so the group is left as the realm
        // holds it until the repository's migration Change names them by id. No value is
        // echoed: the outcome reaches the logs and the feed.
        let unnamed = wanted.iter().filter(|member| !is_person_id(member)).count();
        if unnamed > 0 {
            outcome.error = Some(format!(
                "{unnamed} member(s) of '{name}' are not named by their Keycloak user id \
                 (ADR-N-061), so the group is left as the realm holds it; merge the migration \
                 Change that names them by id (People, Name people by id)"
            ));
            return outcome;
        }

        // Who is in the group now, by Keycloak user id, against who the manifest says: a member
        // without an e-mail is a member like any other (PF-62).
        let held: BTreeSet<String> = match self
            .admin
            .get_all::<KcUser>(token, &format!("/groups/{id}/members"))
            .await
        {
            Ok(members) => members.into_iter().map(|user| user.id).collect(),
            Err(err) => {
                outcome.error = Some(err);
                return outcome;
            }
        };
        for user in wanted.difference(&held) {
            match self.user_exists(token, user).await {
                Ok(true) => {
                    match self
                        .admin
                        .send(
                            token,
                            reqwest::Method::PUT,
                            &format!("/users/{user}/groups/{id}"),
                            None,
                        )
                        .await
                    {
                        Ok(_) => outcome
                            .drift
                            .push(format!("{user} belongs in the group and was added")),
                        Err(err) => outcome.error = Some(err),
                    }
                }
                // Not an error: the account may come back, or the manifest is fixed next.
                Ok(false) => outcome.warnings.push(format!(
                    "{user} belongs in the group and the realm has no user with that id"
                )),
                Err(err) => outcome.error = Some(err),
            }
        }

        for user in held.difference(wanted) {
            match self
                .admin
                .send(
                    token,
                    reqwest::Method::DELETE,
                    &format!("/users/{user}/groups/{id}"),
                    None,
                )
                .await
            {
                Ok(_) => outcome.drift.push(format!(
                    "{user} was in the realm's group and does not belong in it; the \
                     membership was removed"
                )),
                Err(err) => outcome.error = Some(err),
            }
        }
        outcome
    }

    /// Brings a derived group already in the realm to the claims it should give: a group made
    /// before T-6461 carries none, one of T-6461 still names the Organization to every person a
    /// binding reaches (T-6470), and a project can move to another one. A manifest's group is
    /// left as it is.
    async fn keep_claims(
        &self,
        token: &str,
        held: Option<&KcGroup>,
        derived: Option<&DerivedGroup>,
        outcome: &mut GroupOutcome,
    ) {
        let (Some(group), Some(derived)) = (held, derived) else {
            return;
        };
        let attributes = attributes_of(Some(derived));
        let says = |key: &str| group.attributes.get(key).map(|values| json!(values));
        if [
            ORGANIZATION_ATTRIBUTE,
            ORGANIZATION_ID_ATTRIBUTE,
            GROUPS_ATTRIBUTE,
            SECOND_FACTOR_ATTRIBUTE,
        ]
        .iter()
        .all(|key| says(key).as_ref() == attributes.get(*key))
        {
            return;
        }
        let body = json!({ "name": group.name, "attributes": attributes });
        let sent = self
            .admin
            .send(
                token,
                reqwest::Method::PUT,
                &format!("/groups/{}", group.id),
                Some(&body),
            )
            .await;
        match sent {
            Ok(_) => outcome.drift.push(
                "the group's Organization claims or sign-in rule were not what its project and \
                 Organization say; they are now"
                    .to_owned(),
            ),
            Err(err) => outcome.error = Some(err),
        }
    }

    /// Whether the realm has a user with this id; a 404 is `false`, any other refusal an error.
    async fn user_exists(&self, token: &str, user: &str) -> Result<bool, String> {
        let recently_absent = |absent: &BTreeMap<String, std::time::Instant>| {
            absent.get(user).is_some_and(|at| at.elapsed() < ABSENT_FOR)
        };
        if recently_absent(&self.absent.lock().unwrap_or_else(|e| e.into_inner())) {
            return Ok(false);
        }
        let exists = self.admin.exists(token, &format!("/users/{user}")).await?;
        let mut absent = self.absent.lock().unwrap_or_else(|e| e.into_inner());
        if exists {
            absent.remove(user);
        } else {
            absent.insert(user.to_owned(), std::time::Instant::now());
        }
        Ok(exists)
    }
}

/// What a group the platform writes carries: the managed mark and, for a derived group, the
/// claims it gives. The path in `jc_groups` is a name, not the group's place in the realm: the
/// group is a root group so that the forge's team map finds it (PF-87).
fn attributes_of(derived: Option<&DerivedGroup>) -> Value {
    let mut attributes = json!({ MANAGED_BY: [MANAGED_VALUE] });
    if let Some(organization) = derived.and_then(|group| group.jc_org.as_ref()) {
        attributes[ORGANIZATION_ATTRIBUTE] = json!([organization]);
    }
    if let Some(id) = derived.and_then(|group| group.jc_org_id.as_ref()) {
        attributes[ORGANIZATION_ID_ATTRIBUTE] = json!([id]);
    }
    if let Some(path) = derived.and_then(|group| group.jc_groups.as_ref()) {
        attributes[GROUPS_ATTRIBUTE] = json!([path]);
    }
    if derived.is_some_and(|group| group.second_factor) {
        attributes[SECOND_FACTOR_ATTRIBUTE] = json!(["required"]);
    }
    attributes
}

/// The few characters an address may carry that a query string may not (`+` above all).
pub(super) fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' | '@' => c.to_string(),
            other => other
                .to_string()
                .as_bytes()
                .iter()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect()
}

/// What the Access page reads: the drift and the warnings of one run, on the Group manifest
/// itself, so a person sees the disagreement where the membership is (PF-62, UI-44).
pub fn record(mirror: &Mirror, outcomes: &[GroupOutcome]) {
    for outcome in outcomes {
        let Some(mut envelope) = mirror.get(ORG_NAMESPACE, "Group", &outcome.name) else {
            continue;
        };
        let Some(status) = envelope.status.as_mut() else {
            continue;
        };
        if let Some(error) = &outcome.error {
            status.phase = crate::resource::Phase::Error;
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "False",
                "RealmRefused",
                error,
            )];
        } else if !outcome.drift.is_empty() {
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "True",
                "DriftCorrected",
                &outcome.drift.join("; "),
            )];
        } else if !outcome.warnings.is_empty() {
            status.conditions = vec![super::streams::make_condition(
                "GroupSynced",
                "True",
                "MemberUnknown",
                &outcome.warnings.join("; "),
            )];
        }
        mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_safe_in_a_query_string() {
        assert_eq!(urlencoding("jana+test@hel.fi"), "jana%2Btest@hel.fi");
        assert_eq!(urlencoding("jana@hel.fi"), "jana@hel.fi");
    }

    #[test]
    fn only_a_group_carrying_the_attribute_is_ours() {
        let ours = KcGroup {
            id: "1".into(),
            name: "city-leads".into(),
            attributes: [(MANAGED_BY.to_owned(), vec![MANAGED_VALUE.to_owned()])]
                .into_iter()
                .collect(),
        };
        let theirs = KcGroup {
            id: "2".into(),
            name: "platform-admins".into(),
            attributes: Default::default(),
        };
        assert!(ours.managed());
        assert!(!theirs.managed());
    }

    use crate::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const REALM: &str = "/admin/realms/bb";
    const LEAD: &str = "00000000-0000-4000-8000-00000000000a";
    const LEFT: &str = "00000000-0000-4000-8000-00000000000d";

    fn status() -> Status {
        Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }
    }

    fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: Some(status()),
        }
    }

    fn mirror_of(items: Vec<ResourceEnvelope>) -> Mirror {
        let mirror = Mirror::new();
        for item in items {
            mirror.upsert(item);
        }
        mirror
    }

    fn members(ids: &[&str]) -> Value {
        json!({ "members": ids.iter().map(|id| json!({ "user": id })).collect::<Vec<_>>() })
    }

    fn managed(id: &str, name: &str) -> Value {
        json!({ "id": id, "name": name, "attributes": { MANAGED_BY: [MANAGED_VALUE] } })
    }

    /// A realm handing out a token and listing `groups`; each test mounts the rest.
    async fn realm(groups: Value) -> MockServer {
        let keycloak = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
            )
            .mount(&keycloak)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REALM}/groups")))
            .and(query_param("briefRepresentation", "false"))
            .respond_with(ResponseTemplate::new(200).set_body_json(groups))
            .mount(&keycloak)
            .await;
        keycloak
    }

    async fn answer(keycloak: &MockServer, verb: &str, at: &str, status: u16, body: Value) {
        Mock::given(method(verb))
            .and(path(format!("{REALM}{at}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(keycloak)
            .await;
    }

    fn sync(keycloak: &MockServer) -> GroupSync {
        GroupSync::new(
            &format!("{}/realms/bb", keycloak.uri()),
            "portal-reconciler".to_owned(),
            ClientAuth::Secret("s3cret".into()),
        )
        .expect("a realm issuer")
    }

    fn only(outcomes: &[GroupOutcome], name: &str) -> GroupOutcome {
        outcomes
            .iter()
            .find(|outcome| outcome.name == name)
            .cloned()
            .expect("an outcome of that name")
    }

    #[test]
    fn an_organization_group_names_its_organization_and_nothing_else_does() {
        assert_eq!(organization_group("bbsk"), "bbsk_bound");
        assert_eq!(organization_of_group("bbsk_bound"), Some("bbsk"));
        assert_eq!(
            organization_of_group("_bound"),
            None,
            "an empty Organization"
        );
        assert_eq!(organization_of_group("bbsk-readers"), None);
    }

    /// The second-factor rule reads the Organization's policies alone: none is not required,
    /// `required` is, and a block that does not parse counts as required and says so.
    #[tokio::test]
    async fn a_second_factor_is_required_by_policy_or_by_a_policy_that_does_not_parse() {
        let mirror = mirror_of(vec![
            manifest("Organization", "plain", json!({ "domain": "plain.sk" })),
            manifest(
                "Organization",
                "strict",
                json!({ "policies": { "signIn": { "secondFactor": "required" } } }),
            ),
            manifest(
                "Organization",
                "broken",
                json!({ "policies": { "signIn": { "secondFactor": 7 } } }),
            ),
        ]);
        assert!(!requires_second_factor(&mirror, "plain"));
        assert!(!requires_second_factor(&mirror, "nobody"));
        assert!(requires_second_factor(&mirror, "strict"));
        let (required, log) =
            super::super::logged(async { requires_second_factor(&mirror, "broken") }).await;
        assert!(required);
        assert!(log.contains("Organization policies do not parse"), "{log}");
        assert_eq!(
            organization_id(&mirror, "plain").as_deref(),
            Some("sk_plain")
        );
        assert_eq!(organization_id(&mirror, "strict"), None, "no domain, no id");
        assert_eq!(organization_id(&mirror, "nobody"), None);
    }

    /// A derived group of an Organization carries its id and its second-factor rule; a manifest's
    /// group carries the managed mark alone.
    #[test]
    fn a_derived_group_carries_every_claim_it_gives() {
        let derived = DerivedGroup {
            name: "bbsk_bound".into(),
            members: BTreeSet::new(),
            jc_org: Some("bbsk".into()),
            jc_org_id: Some("sk_bbsk".into()),
            jc_groups: None,
            second_factor: true,
        };
        assert_eq!(
            attributes_of(Some(&derived)),
            json!({
                MANAGED_BY: [MANAGED_VALUE],
                ORGANIZATION_ATTRIBUTE: ["bbsk"],
                ORGANIZATION_ID_ATTRIBUTE: ["sk_bbsk"],
                SECOND_FACTOR_ATTRIBUTE: ["required"],
            })
        );
        assert_eq!(attributes_of(None), json!({ MANAGED_BY: [MANAGED_VALUE] }));
        assert_eq!(derived.organization(), Some("bbsk"));
        let project = DerivedGroup {
            jc_org: None,
            jc_groups: Some("noslash".into()),
            ..derived.clone()
        };
        assert_eq!(
            project.organization(),
            None,
            "a path without its leading slash names none"
        );
    }

    /// A manifest that does not parse is reported and written nowhere; a managed group no manifest
    /// declares whose delete the realm refuses is an error, never a silent keep.
    #[tokio::test]
    async fn a_broken_manifest_and_a_refused_removal_are_each_an_outcome() {
        let keycloak = realm(json!([managed("g9", "old-team")])).await;
        answer(&keycloak, "DELETE", "/groups/g9", 500, json!({})).await;
        let mirror = mirror_of(vec![manifest(
            "Group",
            "city-leads",
            json!({ "members": "everyone" }),
        )]);
        let outcomes = sync(&keycloak).converge(&mirror).await;
        let broken = only(&outcomes, "city-leads");
        assert!(
            broken
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("the manifest does not parse: ")),
            "{broken:?}"
        );
        assert_eq!(
            only(&outcomes, "old-team").error.as_deref(),
            Some("DELETE /groups/g9: 500 Internal Server Error")
        );
    }

    /// The realm listing refused: one outcome for the whole wave and nothing written.
    #[tokio::test]
    async fn a_refused_group_listing_fails_the_wave() {
        let keycloak = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "access_token": "t", "expires_in": 60 })),
            )
            .mount(&keycloak)
            .await;
        answer(&keycloak, "GET", "/groups", 403, json!({})).await;
        let outcomes = sync(&keycloak)
            .converge(&mirror_of(vec![manifest(
                "Group",
                "city-leads",
                members(&[LEAD]),
            )]))
            .await;
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert_eq!(outcomes[0].name, "*");
        assert!(
            outcomes[0]
                .error
                .as_deref()
                .is_some_and(|e| e.contains("403")),
            "{outcomes:?}"
        );
    }

    /// In an Organization's realm `/{organization}/members` is kept: made when missing, left when
    /// there, and each refused step is the outcome's error.
    #[tokio::test]
    async fn an_organizations_members_group_is_kept_and_each_refusal_said() {
        // Both there: nothing to do.
        let keycloak = realm(json!([managed("root", "bbsk")])).await;
        answer(
            &keycloak,
            "GET",
            "/groups/root/children",
            200,
            json!([{ "id": "m", "name": "members" }]),
        )
        .await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        assert_eq!(
            only(&outcomes, "bbsk/members"),
            GroupOutcome::of("bbsk/members")
        );

        // The root missing and its create refused.
        let keycloak = realm(json!([])).await;
        answer(&keycloak, "POST", "/groups", 409, json!({})).await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        assert_eq!(
            only(&outcomes, "bbsk/members").error.as_deref(),
            Some("POST /groups: 409 Conflict")
        );

        // The root made, but the realm does not find it again.
        let keycloak = realm(json!([])).await;
        answer(&keycloak, "POST", "/groups", 201, json!({})).await;
        Mock::given(method("GET"))
            .and(path(format!("{REALM}/groups")))
            .and(query_param("exact", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{ "id": "x", "name": "bbsk-other" }])),
            )
            .mount(&keycloak)
            .await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        assert_eq!(
            only(&outcomes, "bbsk/members").error.as_deref(),
            Some("the realm did not return the group it just created")
        );

        // The root made, and the search answers something that is not a list of groups.
        let keycloak = realm(json!([])).await;
        answer(&keycloak, "POST", "/groups", 201, json!({})).await;
        Mock::given(method("GET"))
            .and(path(format!("{REALM}/groups")))
            .and(query_param("exact", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "x" })))
            .mount(&keycloak)
            .await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        let error = only(&outcomes, "bbsk/members").error.unwrap_or_default();
        assert!(error.contains("expected a sequence"), "{error}");

        // The children not listed, then the subgroup's create refused.
        let keycloak = realm(json!([managed("root", "bbsk")])).await;
        answer(&keycloak, "GET", "/groups/root/children", 500, json!({})).await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        assert!(only(&outcomes, "bbsk/members")
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("GET /groups/root/children: 500")));
        let keycloak = realm(json!([managed("root", "bbsk")])).await;
        answer(&keycloak, "GET", "/groups/root/children", 200, json!([])).await;
        answer(&keycloak, "POST", "/groups/root/children", 403, json!({})).await;
        let outcomes = sync(&keycloak)
            .for_organization("bbsk")
            .converge(&Mirror::new())
            .await;
        assert_eq!(
            only(&outcomes, "bbsk/members").error.as_deref(),
            Some("POST /groups/root/children: 403 Forbidden")
        );
    }

    /// Each refused call of one group's convergence is that group's error: its create, its
    /// members' listing, a member's add, the user lookup, a membership's removal, a claim update.
    #[tokio::test]
    async fn each_refused_step_of_a_group_is_its_error() {
        let mirror = mirror_of(vec![manifest("Group", "city-leads", members(&[LEAD]))]);

        let keycloak = realm(json!([])).await;
        answer(&keycloak, "POST", "/groups", 500, json!({})).await;
        let outcome = only(&sync(&keycloak).converge(&mirror).await, "city-leads");
        assert_eq!(
            outcome.error.as_deref(),
            Some("POST /groups: 500 Internal Server Error")
        );

        let keycloak = realm(json!([managed("g1", "city-leads")])).await;
        answer(&keycloak, "GET", "/groups/g1/members", 502, json!({})).await;
        let outcome = only(&sync(&keycloak).converge(&mirror).await, "city-leads");
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("GET /groups/g1/members: 502")),
            "{outcome:?}"
        );

        let keycloak = realm(json!([managed("g1", "city-leads")])).await;
        answer(
            &keycloak,
            "GET",
            "/groups/g1/members",
            200,
            json!([{ "id": LEFT }]),
        )
        .await;
        answer(
            &keycloak,
            "GET",
            &format!("/users/{LEAD}"),
            200,
            json!({ "id": LEAD }),
        )
        .await;
        answer(
            &keycloak,
            "PUT",
            &format!("/users/{LEAD}/groups/g1"),
            403,
            json!({}),
        )
        .await;
        answer(
            &keycloak,
            "DELETE",
            &format!("/users/{LEFT}/groups/g1"),
            500,
            json!({}),
        )
        .await;
        let sync_one = sync(&keycloak);
        let outcome = only(&sync_one.converge(&mirror).await, "city-leads");
        // The removal runs after the add: its refusal is the last word.
        assert_eq!(
            outcome.error.as_deref(),
            Some(format!("DELETE /users/{LEFT}/groups/g1: 500 Internal Server Error").as_str())
        );
        assert!(outcome.drift.is_empty(), "{outcome:?}");

        let keycloak = realm(json!([managed("g1", "city-leads")])).await;
        answer(&keycloak, "GET", "/groups/g1/members", 200, json!([])).await;
        answer(&keycloak, "GET", &format!("/users/{LEAD}"), 500, json!({})).await;
        let outcome = only(&sync(&keycloak).converge(&mirror).await, "city-leads");
        assert_eq!(
            outcome.error.as_deref(),
            Some(format!("GET /users/{LEAD}: 500 Internal Server Error").as_str())
        );

        // A derived group whose claims went stale, and the realm refuses the update.
        let keycloak = realm(json!([managed("g2", "bbsk_bound")])).await;
        answer(&keycloak, "PUT", "/groups/g2", 403, json!({})).await;
        answer(&keycloak, "GET", "/groups/g2/members", 200, json!([])).await;
        let derived = DerivedGroup {
            name: "bbsk_bound".into(),
            members: BTreeSet::new(),
            jc_org: Some("bbsk".into()),
            jc_org_id: None,
            jc_groups: None,
            second_factor: false,
        };
        let outcomes = sync(&keycloak)
            .converge_with(&Mirror::new(), &[derived])
            .await;
        assert_eq!(
            only(&outcomes, "bbsk_bound").error.as_deref(),
            Some("PUT /groups/g2: 403 Forbidden")
        );
    }

    /// A user the realm lacks is asked for once per [`ABSENT_FOR`], and a cache poisoned by a
    /// panicking thread still answers rather than taking the wave down.
    #[tokio::test]
    async fn an_absent_user_is_asked_once_and_a_poisoned_cache_still_answers() {
        let keycloak = realm(json!([])).await;
        Mock::given(method("GET"))
            .and(path(format!("{REALM}/users/{LEAD}")))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&keycloak)
            .await;
        let groups = sync(&keycloak);
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = groups.absent.lock().expect("not poisoned yet");
            panic!("a thread dies holding the cache");
        }));
        assert!(poison.is_err() && groups.absent.is_poisoned());
        assert_eq!(groups.user_exists("t", LEAD).await, Ok(false));
        assert_eq!(
            groups.user_exists("t", LEAD).await,
            Ok(false),
            "answered from the cache"
        );
    }

    /// The Access page reads a run on the Group manifest: a refusal is an Error phase, a warning
    /// a MemberUnknown condition; an outcome without a manifest, or a manifest without a status,
    /// is left alone.
    #[test]
    fn a_runs_outcome_is_recorded_on_its_manifest() {
        let mut bare = manifest("Group", "bare", members(&[]));
        bare.status = None;
        let mirror = mirror_of(vec![
            manifest("Group", "refused", members(&[])),
            manifest("Group", "warned", members(&[])),
            manifest("Group", "clean", members(&[])),
            bare,
        ]);
        let outcome = |name: &str, error: Option<&str>, warnings: &[&str]| GroupOutcome {
            name: name.into(),
            drift: Vec::new(),
            warnings: warnings.iter().map(|w| (*w).to_owned()).collect(),
            error: error.map(str::to_owned),
        };
        record(
            &mirror,
            &[
                outcome("refused", Some("POST /groups: 409 Conflict"), &[]),
                outcome("warned", None, &["a has no user", "b has no user"]),
                outcome("bare", Some("ignored"), &[]),
                outcome("nobody", Some("ignored"), &[]),
                outcome("clean", None, &[]),
            ],
        );
        let condition = |name: &str| {
            let status = mirror
                .get(ORG_NAMESPACE, "Group", name)
                .and_then(|g| g.status)
                .expect("a status");
            (
                status.phase,
                status.conditions.first().cloned().expect("a condition"),
            )
        };
        let (phase, refused) = condition("refused");
        assert_eq!(phase, Phase::Error);
        assert_eq!(
            (refused.status.as_str(), refused.reason.as_deref()),
            ("False", Some("RealmRefused"))
        );
        assert_eq!(
            refused.message.as_deref(),
            Some("POST /groups: 409 Conflict")
        );
        let (phase, warned) = condition("warned");
        assert_eq!(phase, Phase::Live);
        assert_eq!(warned.reason.as_deref(), Some("MemberUnknown"));
        assert_eq!(
            warned.message.as_deref(),
            Some("a has no user; b has no user")
        );
        assert!(mirror
            .get(ORG_NAMESPACE, "Group", "bare")
            .expect("kept")
            .status
            .is_none());
        assert!(mirror.get(ORG_NAMESPACE, "Group", "nobody").is_none());
        let clean = mirror
            .get(ORG_NAMESPACE, "Group", "clean")
            .and_then(|g| g.status)
            .expect("a status");
        assert_eq!(
            (clean.phase, clean.conditions.len()),
            (Phase::Live, 0),
            "a clean run changes nothing"
        );
    }
}
