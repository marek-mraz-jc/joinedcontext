//! A person's connectors (AG-142..AG-147, ADR-N-064, API/14): a grant one person gives one AI
//! client over part of the platform, served as an MCP server at `/c/{id}` (`crate::mcp::connector`).
//!
//! The record names segments (presets of tools and kinds) and a scope (projects, and inside one
//! the spaces, Endpoints or pipelines it reaches). Nothing here is a right of its own: every call
//! is the owner's, narrowed by the record, and the owner's live bindings decide again at each call
//! (`ops::permitted`), so a role the owner lost drops from every connector at its next call.
//!
//! Two storage arms like every store here: `Postgres` over migration `0051_connectors.sql`, and
//! `Memory` when the Portal runs without a database.

use std::sync::Mutex;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use jc_time::TimestampTz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use jc_core::envelope::Kind as _;
use jc_core::kinds::delegation_policy::{validate_kind_verbs, verbs_allow};
use jc_core::kinds::{AboveCeiling, ConnectorVerb, DelegationPolicySpec, KindVerbs, Segment};

use crate::activity::{odt_to_ts, ts_to_odt};
use crate::error::ApiError;
use crate::ops::Operation;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::is_dns1123;

/// Live connectors one person may hold (T-5950).
pub const MAX_PER_PERSON: usize = 20;
/// Scope items and allowed clients one connector may name.
const MAX_SCOPE: usize = 50;
const MAX_CLIENTS: usize = 10;
/// Audit rows one activity page carries, and how long a row is kept.
pub const CALLS_PAGE: i64 = 200;
/// The audit rows the memory arm keeps: without a database nothing outlives the process anyway.
const CALLS_IN_MEMORY: usize = 10_000;
/// How long the audit is kept unless `JC_PORTAL_AUDIT_RETENTION_DAYS` says otherwise (AUD-04).
pub const AUDIT_RETENTION_DAYS: u32 = 400;
/// The verdict of the row written before a call runs; alone, it says the outcome is unknown.
pub const STARTED: &str = "started";
/// Connectors the organization overview lists, live ones first (T-5975).
const OVERVIEW_PAGE: i64 = 500;
/// Refused calls in a day that flag a connector on the overview: a client probing past its scope.
pub const DENIALS_FLAG: i64 = 10;

/// A segment (API/14 §1, Architecture/07 §11): the read tools it lists and the kinds those tools
/// may name. A segment that proposes lists `jc_change_plan` (a dry run, nothing saved); the one
/// write, `jc_change_apply`, comes with the connector's `writes` switch alone (AG-149, T-5956).
/// Cross-project tools (the organization's people, projects, every Endpoint) are in none; a
/// segment's lookups read inside the project only (`jc_member_list`, T-6039; `jc_catalog_search`
/// matches the call's project and the public open-data catalogue, EP-67, T-6194). The UI tools come
/// with a paired Portal tab, whatever the segments (T-5967).
pub const SEGMENTS: &[(&str, &[&str], &[&str])] = &[
    (
        "data-read",
        &[
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_kpi_compute",
            "jc_data_query",
            "jc_render",
            "jc_catalog_search",
            "jc_attribute_explain",
        ],
        &["Endpoint", "ContextSpace", "DataModel"],
    ),
    (
        "context",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_model_source_get",
            "jc_model_sdm_search",
            "jc_model_sdm_import",
            "jc_model_generate",
            "jc_endpoint_preview",
            "jc_space_governance",
        ],
        &[
            "ContextSpace",
            "DataModel",
            "Endpoint",
            "Policy",
            "ModelProjection",
            "Subscription",
        ],
    ),
    (
        "pipeline",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_pipeline_metrics",
            "jc_pipeline_logs",
            "jc_pipeline_sample_source",
            "jc_pipeline_try_mapping",
            "jc_pipeline_test",
            "jc_syncsource_status",
            "jc_mqtt_captured",
            "jc_mqtt_test_publish",
            "jc_pipeline_inspect",
            "jc_space_governance",
        ],
        &[
            "Pipeline",
            "DataSource",
            "MqttBroker",
            "SyncSource",
            "ContextSpace",
            "DataModel",
            "Endpoint",
            "Subscription",
        ],
    ),
    (
        "access",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_member_list",
            "jc_access_simulate",
            "jc_access_explore",
        ],
        &["Role", "RoleBinding", "ServiceAccount", "Policy", "Group"],
    ),
    (
        "publish",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_ckan_status",
            "jc_model_source_get",
            "jc_kpi_compute",
            "jc_catalogue_draft",
            "jc_endpoint_preview",
        ],
        &["Endpoint", "CkanInstance", "DataModel"],
    ),
    (
        "apps",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_run_list",
            "jc_run_get",
        ],
        &["App", "Dashboard", "Layer"],
    ),
    (
        "operate",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_activity_list",
            "jc_pipeline_metrics",
            "jc_pipeline_logs",
            "jc_syncsource_status",
            "jc_change_list",
            "jc_change_get",
            "jc_change_history",
            "jc_pipeline_inspect",
        ],
        &["Pipeline", "SyncSource"],
    ),
    (
        "knowledge",
        &[
            "jc_change_plan",
            "jc_change_revert_plan",
            "jc_schema_get",
            "jc_resource_list",
            "jc_resource_get",
            "jc_knowledge_status",
            "jc_knowledge_search",
        ],
        &["KnowledgeSource", "AssistantDeployment"],
    ),
];

/// The plan and apply tools (API/14 §4.3).
pub const PLAN: &str = "jc_change_plan";
pub const APPLY: &str = "jc_change_apply";
/// Plans the undo of a merged change; a plan like any other (T-5974).
pub const REVERT: &str = "jc_change_revert_plan";

/// `admin` is every segment above.
pub const ADMIN: &str = "admin";
/// Runs an approved Pipeline now: it writes data, so like [`APPLY`] it comes with the `writes`
/// switch alone, to a connector whose segments reach Pipeline (T-6028).
pub const RUN: &str = "jc_pipeline_run";
/// Pauses or resumes one SyncSource's loop: it changes what the platform pulls with no Change a
/// person approves, so like [`RUN`] it comes with the `writes` switch alone, to a connector whose
/// segments reach SyncSource, and needs `edit` on it (T-6533).
pub const PAUSE: &str = "jc_syncsource_pause";
/// Writes entities through one Endpoint: like [`APPLY`], it comes with the `writes` switch alone.
pub const DATA_WRITE: &str = crate::ops::data_write::NAME;
/// Starts the builder on an App: it spends the person's quota and writes a branch, so like
/// [`APPLY`] it comes with the `writes` switch alone, to a connector whose segments reach App.
pub const BUILD: &str = "jc_run_create";

/// Whether `segment` is one a connector may name.
pub fn is_segment(segment: &str) -> bool {
    segment == ADMIN || SEGMENTS.iter().any(|(s, _, _)| *s == segment)
}

/// The connector a Portal assistant conversation runs as (T-5958, GOAL-connectors §6): the
/// person's own, one segment, the run's project, no writes, never stored. Its [`Connector::access`]
/// narrows every call the conversation makes exactly as `/c/{id}` narrows a client's.
pub fn of_conversation(run_id: &str, owner: &str, project: &str, segment: &str) -> Connector {
    let now = TimestampTz::now();
    Connector {
        id: format!("conversation-{run_id}"),
        name: format!("assistant {segment}"),
        owner: owner.to_owned(),
        segments: vec![segment.to_owned()],
        scope: vec![ScopeItem {
            project: project.to_owned(),
            space: None,
            endpoint: None,
            pipeline: None,
            access: ItemAccess::Read,
            personal_data: false,
        }],
        kinds: None,
        writes: false,
        autonomous: false,
        expires_at: now + jiff::SignedDuration::from_hours(1),
        allowed_clients: Vec::new(),
        client_is_boundary: false,
        state: "active".to_owned(),
        above_ceiling: Vec::new(),
        approval: None,
        narrowed_by: Vec::new(),
        created_at: now,
        granted_at: now,
        over: Vec::new(),
        last_used_at: None,
        url: String::new(),
        limits: PERSON_LIMITS,
    }
}

/// One value a request asks above the ceiling, kept beside its sentence: an approval covers these
/// values and nothing else (AG-150, T-7541). It names no policy, so a renamed policy changes none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Over {
    Segment(String),
    Project(String),
    /// `None` or `*`: every Endpoint of the project.
    Endpoint {
        project: String,
        endpoint: Option<String>,
    },
    Verb {
        kind: String,
        verb: ConnectorVerb,
    },
    /// No kinds list: every change the segments reach.
    Kinds,
    Writes,
    /// Approving its own changes in this project.
    Autonomous(String),
    /// Until this second of the Unix clock.
    Until(i64),
    Client(String),
    /// No client list: any client.
    AnyClient,
}

/// What a request asks above the ceiling: the values an approval would cover, and one sentence
/// per field for the people who read it.
#[derive(Debug, Clone, Default)]
pub struct Above {
    values: Vec<Over>,
    pub lines: Vec<String>,
}

impl Above {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The ceiling one person's connectors live under (AG-150, ADR-N-064 §5): the `DelegationPolicy`
/// manifests of the organization that bind them, by Keycloak user id or by a Group, read from the
/// mirror on every request, so a changed policy narrows existing connectors at their next call.
/// Several policies give their union; none gives the organization default.
///
/// Decided (worker-t5953, owner may override): the default is the policy every person would get
/// from an empty manifest: `data-read`, reads only, 30 days, any client, any project the person
/// reads, requests above it refused; options: no connector at all (the feature stays dark until an
/// administrator writes a policy), the person's full rights (defeats the ceiling).
#[derive(Clone)]
pub struct Ceiling {
    /// The binding policies by name; empty for the organization default.
    policies: Vec<(String, DelegationPolicySpec)>,
    /// Policies that name the person but fail their kind's validation: they grant nothing, and
    /// a person only they name gets nothing rather than the default, which could reach more.
    broken: Vec<String>,
    /// Whether a scope item may tick `personalData` (AG-158). No DelegationPolicy field grants
    /// it yet, so every ceiling keeps those attributes out of connector answers.
    personal_data: bool,
    /// The date an approval let one connector live to, past every `maxExpiry`
    /// ([`Self::with_approved`]).
    approved_until: Option<i64>,
    /// The projects an approval let one connector be autonomous in.
    approved_autonomous: Vec<String>,
}

/// The organization default as a policy: no `projects` list means every project the person reads.
static DEFAULT: std::sync::LazyLock<DelegationPolicySpec> =
    std::sync::LazyLock::new(default_policy);

fn default_policy() -> DelegationPolicySpec {
    DelegationPolicySpec {
        subjects: Vec::new(),
        projects: Vec::new(),
        segments: None,
        kinds: None,
        endpoints: None,
        writes: false,
        autonomous: false,
        personal_data: false,
        max_expiry: None,
        allowed_clients: None,
        above_ceiling: AboveCeiling::Refuse,
    }
}

/// The resource kind a call works on: the operation's own, or for a `kind: "*"` operation the
/// `kind` its arguments name. `jc_activity_list`'s `kind` filters event kinds
/// (`pipeline.error`), which no policy or segment lists: its ceiling is the project (T-6281).
fn resource_kind<'a>(op: &Operation, args: &'a Value) -> Option<&'a str> {
    match op.kind {
        "*" if op.name == "jc_activity_list" => None,
        "*" => args.get("kind").and_then(Value::as_str),
        kind => Some(kind),
    }
}

/// The tools and kinds `connector` reaches under one policy: the segments both allow, and
/// `jc_change_apply` when both let it write.
fn reach_under(
    policy: &DelegationPolicySpec,
    connector: &Connector,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let (mut tools, kinds) = reach_of(|s| {
        connector.segments.iter().any(|c| c == s || c == ADMIN) && policy_segment(policy, s)
    });
    if connector.writes && policy.writes {
        tools.extend(written(&kinds));
    }
    (tools, kinds)
}

/// Whether `policy` lets a connector carry the segment named `segment`.
fn policy_segment(policy: &DelegationPolicySpec, segment: &str) -> bool {
    policy.segments().iter().any(|s| {
        matches!(s, Segment::Admin)
            || serde_json::to_value(s)
                .ok()
                .as_ref()
                .and_then(Value::as_str)
                == Some(segment)
    })
}

/// Whether `policy` lets a connector tick `verb` on `kind`: its kinds list, when it has one,
/// holds the verb, and for a change verb one of its segments reaches the kind and plans
/// changes (`run`: reaches Pipeline, with the policy's `writes`).
fn policy_offers(policy: &DelegationPolicySpec, kind: &str, verb: ConnectorVerb) -> bool {
    let reaches = |plans: bool| {
        SEGMENTS.iter().any(|(segment, tools, kinds)| {
            policy_segment(policy, segment)
                && kinds.contains(&kind)
                && (!plans || tools.contains(&PLAN))
        })
    };
    let offered = match verb {
        ConnectorVerb::Read => true,
        ConnectorVerb::Run => policy.writes && reaches(false),
        _ => reaches(true),
    };
    offered && (policy.kinds.is_none() || verbs_allow(policy.verbs_for(kind), verb))
}

/// The projects a connector over `scope` writes in: those of its `write` items, or every one
/// where no item says so.
fn written_projects(scope: &[ScopeItem]) -> std::collections::BTreeSet<&str> {
    let written = scope.iter().any(|item| item.access == ItemAccess::Write);
    scope
        .iter()
        .filter(|item| !written || item.access == ItemAccess::Write)
        .map(|item| item.project.as_str())
        .collect()
}

/// The single verbs `verb` stands for: `propose` is create, edit and delete.
fn each_verb(verb: ConnectorVerb) -> Vec<ConnectorVerb> {
    match verb {
        ConnectorVerb::Propose => vec![
            ConnectorVerb::Create,
            ConnectorVerb::Edit,
            ConnectorVerb::Delete,
        ],
        verb => vec![verb],
    }
}

/// Gives `policy` one approved value and changes no other field of it.
fn widen(policy: &mut DelegationPolicySpec, over: &Over) {
    let grant = |policy: &mut DelegationPolicySpec, kind: &str, verb: ConnectorVerb| {
        let Some(kinds) = policy.kinds.as_mut() else {
            return;
        };
        match kinds.iter_mut().find(|entry| entry.kind == kind) {
            Some(entry) if entry.verbs.contains(&verb) => {}
            Some(entry) => entry.verbs.push(verb),
            None => kinds.push(KindVerbs {
                kind: kind.to_owned(),
                verbs: vec![verb],
            }),
        }
    };
    match over {
        Over::Segment(segment) if !policy_segment(policy, segment) => {
            let Ok(parsed) = serde_json::from_value::<Segment>(json!(segment)) else {
                return;
            };
            let mut segments = policy.segments().to_vec();
            segments.push(parsed);
            policy.segments = Some(segments);
            // A segment is approved to be used, and each of its tools reads its kinds.
            for kind in reach_of(|s| s == segment || segment == ADMIN).1 {
                grant(policy, kind, ConnectorVerb::Read);
            }
        }
        Over::Project(project) if !Ceiling::allows_project(policy, project) => {
            policy.projects.push(project.clone());
        }
        Over::Verb { kind, verb } => {
            // A plan reads what it changes.
            grant(policy, kind, ConnectorVerb::Read);
            grant(policy, kind, *verb);
        }
        Over::Kinds => policy.kinds = None,
        Over::Writes => policy.writes = true,
        Over::Client(client) => {
            if let Some(clients) = policy.allowed_clients.as_mut() {
                clients.push(client.clone());
            }
        }
        Over::AnyClient => policy.allowed_clients = None,
        Over::Segment(_)
        | Over::Project(_)
        | Over::Endpoint { .. }
        | Over::Autonomous(_)
        | Over::Until(_) => {}
    }
}

impl Ceiling {
    /// The policies of the organization that bind `identity`: their `User` subject is its `sub`,
    /// or a `Group` subject names a group of its token or a `Group` manifest listing the person.
    pub fn of(mirror: &crate::store::Mirror, identity: &crate::auth::session::Identity) -> Self {
        let options = crate::store::ListOptions::default();
        let person = |user: &str| crate::permissions::names_user(identity, user);
        let mut groups: Vec<String> = identity.groups.clone();
        for env in mirror.list(ORG_NAMESPACE, "Group", &options).items {
            if serde_json::from_value::<jc_core::kinds::GroupSpec>(env.spec)
                .is_ok_and(|spec| spec.members.iter().any(|m| person(&m.user)))
            {
                groups.push(env.metadata.name);
            }
        }
        let groups: Vec<&str> = groups.iter().map(String::as_str).collect();
        let mut ceiling = Self::of_policies(Vec::new());
        for env in mirror
            .list(ORG_NAMESPACE, DelegationPolicySpec::KIND, &options)
            .items
        {
            let name = env.metadata.name;
            match serde_json::from_value::<DelegationPolicySpec>(env.spec) {
                Ok(spec) if spec.binds(&identity.subject, &groups) => {
                    if spec.validate().is_ok() {
                        ceiling.policies.push((name, spec));
                    } else {
                        ceiling.broken.push(name);
                    }
                }
                Ok(_) => {}
                // Unreadable: nobody can tell whom it binds, so it binds nobody; the reconciler
                // logs it as failing its kind's validation.
                Err(_) => {}
            }
        }
        ceiling
    }

    /// A ceiling of exactly these policies (tests and the harness build one by hand).
    pub fn of_policies(policies: Vec<(String, DelegationPolicySpec)>) -> Self {
        Self {
            policies,
            broken: Vec::new(),
            personal_data: false,
            approved_until: None,
            approved_autonomous: Vec::new(),
        }
    }

    /// The policies a check runs over: the binding ones, the default alone when none binds, and
    /// none at all when only a broken one does.
    fn each(&self) -> Vec<&DelegationPolicySpec> {
        if self.policies.is_empty() && !self.broken.is_empty() {
            Vec::new()
        } else if self.policies.is_empty() {
            vec![&*DEFAULT]
        } else {
            self.policies.iter().map(|(_, p)| p).collect()
        }
    }

    /// The policy names a refusal and a `narrowedBy` mark cite, or the default's words.
    pub fn named(&self) -> String {
        if self.policies.is_empty() && !self.broken.is_empty() {
            format!(
                "DelegationPolicy {}, which fails its kind's validation and allows nothing until an administrator fixes it,",
                self.broken.join(", ")
            )
        } else if self.policies.is_empty() {
            "the organization default (no DelegationPolicy names you)".to_owned()
        } else {
            let mut names: Vec<&str> = Vec::new();
            for (name, _) in &self.policies {
                // An approved Endpoint repeats its policy for one project.
                if !names.contains(&name.as_str()) {
                    names.push(name);
                }
            }
            format!("DelegationPolicy {}", names.join(", "))
        }
    }

    fn allows_project(policy: &DelegationPolicySpec, project: &str) -> bool {
        policy.projects.is_empty() || policy.projects.iter().any(|p| p == project)
    }

    pub fn allows_segment(&self, segment: &str) -> bool {
        self.each().iter().any(|p| policy_segment(p, segment))
    }

    pub fn segments(&self) -> Vec<String> {
        let mut all: Vec<String> = SEGMENTS
            .iter()
            .map(|(s, _, _)| (*s).to_owned())
            .chain([ADMIN.to_owned()])
            .filter(|s| self.allows_segment(s))
            .collect();
        all.dedup();
        all
    }

    pub fn allows_project_name(&self, project: &str) -> bool {
        self.each().iter().any(|p| Self::allows_project(p, project))
    }

    /// Whether some policy over `project` lets an item reach the Endpoint `endpoint` (`None` or
    /// `*`: every Endpoint of the project, which a policy listing Endpoints does not allow).
    pub fn allows_endpoint(&self, project: &str, endpoint: Option<&str>) -> bool {
        self.each().iter().any(|p| {
            Self::allows_project(p, project)
                && p.endpoints.as_ref().is_none_or(|listed| {
                    endpoint.is_some_and(|e| e != "*" && listed.iter().any(|l| l == e))
                })
        })
    }

    /// Whether every binding policy lists the verbs per kind.
    fn lists_kinds(&self) -> bool {
        let each = self.each();
        !each.is_empty() && each.iter().all(|p| p.kinds.is_some())
    }

    pub fn writes(&self) -> bool {
        self.each().iter().any(|p| p.writes)
    }

    /// Whether a connector may approve its own changes in `project`: some policy over it says
    /// `autonomous`, or an approval did for a project the ceiling still reaches (T-7541).
    pub fn allows_autonomous(&self, project: &str) -> bool {
        self.each()
            .iter()
            .any(|p| p.autonomous && p.writes && Self::allows_project(p, project))
            || (self.approved_autonomous.iter().any(|p| p == project)
                && self.allows_project_name(project))
    }

    /// Whether some policy (over `project`, when named) lets a connector `verb` on `kind`
    /// (T-6302). A policy listing no kinds leaves changes to the segments and `writes`.
    /// A policy's segments must reach the kind too, as the dialog offers it, so a policy
    /// narrowed after a connector was made takes the kinds its segments dropped (JC2-07).
    pub fn allows_verb(&self, project: Option<&str>, kind: &str, verb: ConnectorVerb) -> bool {
        self.each().iter().any(|p| {
            project.is_none_or(|project| Self::allows_project(p, project))
                && policy_offers(p, kind, verb)
        })
    }

    /// Whether one policy over `project` lets a connector `verb` the object `kind`/`name`: the
    /// verb as [`Self::allows_verb`] reads it and, for an Endpoint, the policy's Endpoint list
    /// holding `name` (JC2-07).
    pub fn allows_change(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        verb: ConnectorVerb,
    ) -> bool {
        self.each().iter().any(|p| {
            Self::allows_project(p, project)
                && policy_offers(p, kind, verb)
                && (kind != "Endpoint"
                    || p.endpoints
                        .as_ref()
                        .is_none_or(|listed| listed.iter().any(|l| l == name)))
        })
    }

    /// The verbs the ceiling lets a connector tick on each of `kinds`, for the dialog: `read`
    /// always, a change verb only where a policy's proposing segment reaches the kind, and
    /// `run` only with that policy's `writes`, so a read-only ceiling greys every change verb.
    pub fn kind_verbs(&self, kinds: &[&str]) -> Vec<KindGrant> {
        kinds
            .iter()
            .map(|kind| KindGrant {
                kind: (*kind).to_owned(),
                verbs: kind_choices(kind)
                    .into_iter()
                    .filter(|verb| self.each().iter().any(|p| policy_offers(p, kind, *verb)))
                    .collect(),
            })
            .collect()
    }

    pub fn max_days(&self) -> i64 {
        self.each()
            .iter()
            .map(|p| i64::from(p.max_expiry_days()))
            .max()
            .unwrap_or(i64::from(
                jc_core::kinds::delegation_policy::DEFAULT_EXPIRY_DAYS,
            ))
    }

    pub fn allows_client(&self, client: &str) -> bool {
        self.each().iter().any(|p| p.allows_client(client))
    }

    /// Every client id a binding policy lists, sorted, e.g. claude.ai's CIMD URL (T-6309).
    pub fn listed_clients(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .each()
            .iter()
            .flat_map(|p| p.allowed_clients.iter().flatten().cloned())
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// Whether a connector naming no client fits: no policy lists clients it must name.
    pub fn any_client(&self) -> bool {
        self.each().iter().any(|p| p.allowed_clients.is_none())
    }

    /// Whether a request above the ceiling waits for an administrator instead of a `403`.
    pub fn queues(&self) -> bool {
        self.policies
            .iter()
            .any(|(_, p)| p.above_ceiling == AboveCeiling::Queue)
    }

    /// The ceiling's half of one call (AG-144): some policy reaches the project, a segment of
    /// the connector it allows lists the tool, its verbs read the kind, and its Endpoints hold
    /// the one the call names. `Err` names the policy and what it lacks.
    pub fn admits(
        &self,
        connector: &Connector,
        op: &Operation,
        project: &str,
        args: &Value,
    ) -> Result<(), String> {
        let kind = resource_kind(op, args);
        let endpoint = match kind {
            Some("Endpoint") => object_named(op, args),
            _ => args.get("endpoint").and_then(Value::as_str),
        };
        let fits = |p: &DelegationPolicySpec| {
            let (tools, kinds) = reach_under(p, connector);
            Self::allows_project(p, project)
                && tools.contains(&op.name)
                && kind.is_none_or(|k| {
                    op.name == "jc_schema_get"
                        || (kinds.contains(&k) && p.verbs_for(k).contains(&ConnectorVerb::Read))
                })
                && p.endpoints.as_ref().is_none_or(|listed| {
                    let touches = kind == Some("Endpoint") || endpoint.is_some();
                    !touches || endpoint.is_some_and(|e| listed.iter().any(|l| l == e))
                })
                && (op.name != RUN
                    || p.kinds.is_none()
                    || verbs_allow(p.verbs_for("Pipeline"), ConnectorVerb::Run))
                && (op.name != PAUSE
                    || p.kinds.is_none()
                    || verbs_allow(p.verbs_for("SyncSource"), ConnectorVerb::Edit))
                && (op.name != BUILD || policy_builds(p, args))
        };
        if self.each().into_iter().any(fits) {
            Ok(())
        } else {
            Err(format!(
                "{} does not let this connector run {} on {}in project {project}; an administrator widens the policy",
                self.named(),
                op.name,
                kind.map(|k| format!("{k} ")).unwrap_or_default()
            ))
        }
    }

    /// Whether `tools/list` offers `op` in `project` under the ceiling: some policy reaches the
    /// project and allows a segment of the connector listing the tool.
    pub fn lists(&self, connector: &Connector, op: &Operation, project: &str) -> bool {
        self.each().into_iter().any(|p| {
            let (tools, _) = reach_under(p, connector);
            Self::allows_project(p, project) && tools.contains(&op.name)
        })
    }

    /// This ceiling widened by exactly the values `connector` was approved above it (AG-150,
    /// T-7541), while a policy binding the owner still queues requests: every check runs against
    /// what this answers. A policy turned to `refuse`, or one that no longer names the owner,
    /// takes every approval of theirs back at the next call; an approval recorded without its
    /// values covers none.
    pub fn with_approved(&self, connector: &Connector) -> Self {
        let mut ceiling = self.clone();
        if connector.approval.is_none() || !self.queues() {
            return ceiling;
        }
        for over in &connector.over {
            match over {
                Over::Until(at) if connector.expires_at.as_second() <= *at => {
                    ceiling.approved_until = Some(*at);
                }
                Over::Autonomous(project) => ceiling.approved_autonomous.push(project.clone()),
                // Judged below, once the approved projects are known.
                Over::Until(_) | Over::Endpoint { .. } => {}
                over => {
                    for (_, policy) in &mut ceiling.policies {
                        widen(policy, over);
                    }
                }
            }
        }
        let mut excepted = Vec::new();
        let projects: std::collections::BTreeSet<&str> = connector
            .over
            .iter()
            .filter_map(|over| match over {
                Over::Endpoint { project, .. } => Some(project.as_str()),
                _ => None,
            })
            .collect();
        // An Endpoint is approved in one project, so the exception is that policy over that
        // project alone: the name opens nothing in the policy's other projects.
        for project in projects {
            for (name, policy) in &ceiling.policies {
                let Some(listed) = policy.endpoints.as_ref() else {
                    continue;
                };
                if !Self::allows_project(policy, project) {
                    continue;
                }
                let mut endpoints = Some(listed.clone());
                for over in &connector.over {
                    match over {
                        Over::Endpoint {
                            project: of,
                            endpoint,
                        } if of == project => match (endpoint.as_deref(), endpoints.as_mut()) {
                            (None | Some("*"), _) => endpoints = None,
                            (Some(one), Some(list)) => list.push(one.to_owned()),
                            (Some(_), None) => {}
                        },
                        _ => {}
                    }
                }
                let exception = DelegationPolicySpec {
                    projects: vec![project.to_owned()],
                    endpoints,
                    ..policy.clone()
                };
                excepted.push((name.clone(), exception));
            }
        }
        ceiling.policies.extend(excepted);
        ceiling
    }

    /// The ceiling's half of a call before its tool is read: the connector's age and the client.
    pub fn door(
        &self,
        connector: &Connector,
        client: &str,
        now: TimestampTz,
    ) -> Result<(), String> {
        if self.approved_until.is_none()
            && now > connector.granted_at + jiff::SignedDuration::from_hours(24 * self.max_days())
        {
            return Err(format!(
                "{} lets a connector live {} days, and this one is older; create a new one on the My connectors page",
                self.named(),
                self.max_days()
            ));
        }
        if !self.allows_client(client) {
            return Err(format!(
                "{} does not allow the client {}",
                self.named(),
                if client.is_empty() {
                    "of this token (none named)"
                } else {
                    client
                }
            ));
        }
        Ok(())
    }

    /// What the ceiling takes from an existing connector today: one line per narrowed part, for
    /// the `narrowedBy` marks (empty when the connector fits).
    pub fn narrowing(&self, connector: &Connector) -> Vec<String> {
        let mut marks = Vec::new();
        for segment in &connector.segments {
            if !self.allows_segment(segment) {
                marks.push(format!("segment {segment}"));
            }
        }
        for item in &connector.scope {
            if !self.allows_project_name(&item.project) {
                marks.push(format!("project {}", item.project));
            } else if !self.allows_endpoint(&item.project, item.endpoint.as_deref()) {
                marks.push(format!(
                    "Endpoints of project {}: {}",
                    item.project,
                    item.endpoint.as_deref().unwrap_or("every one")
                ));
            }
        }
        if connector.writes && !self.writes() {
            marks.push("writes".to_owned());
        }
        if connector.autonomous {
            for project in written_projects(&connector.scope) {
                if !self.allows_autonomous(project) {
                    marks.push(format!("autonomous in project {project}"));
                }
            }
        }
        if self.approved_until.is_none()
            && connector.expires_at
                > connector.granted_at + jiff::SignedDuration::from_hours(24 * self.max_days())
        {
            marks.push(format!("expiry: at most {} days", self.max_days()));
        }
        for client in &connector.allowed_clients {
            if !self.allows_client(client) {
                marks.push(format!("client {client}"));
            }
        }
        marks
    }
}

/// What one connector may spend (T-5968): calls at once and answer size; calls a minute have no
/// limit (owner 2026-11-02, T-6484). The person's own budget (`PERSON_LIMITS`) holds across all
/// their connectors and `/api/v1/mcp` together, so a second connector adds no parallel calls.
/// Counted on the verified token's subject and the connector id from the path, never on a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConnectorLimits)]
pub struct Limits {
    /// Calls answered at once; the next answers 429 until one ends.
    pub parallel_calls: u32,
    /// The largest tool answer in bytes; a larger one is refused as a 413 tool error naming
    /// what to narrow.
    pub result_bytes: usize,
}

/// Every connector's limits (T-5968). Decided (worker-t5968, owner may override): constants in
/// code until a DelegationPolicy lowers them (T-5953); 4 at once covers a model's parallel tool
/// calls (Claude and ChatGPT issue 2-4 per turn); 1 MiB is ~250k tokens, past any model context
/// a tool answer should fill, under the route's 4 MiB.
pub const DEFAULT_LIMITS: Limits = Limits {
    parallel_calls: 4,
    result_bytes: 1024 * 1024,
};

/// One person's budget over all their connectors and `/api/v1/mcp` (AG-60, T-5968).
pub const PERSON_LIMITS: Limits = Limits {
    parallel_calls: 8,
    result_bytes: 4 * 1024 * 1024,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[schema(as = ConnectorAccess)]
pub enum ItemAccess {
    Read,
    Write,
}

/// One item of a connector's scope: a project, and at most one of a space, an Endpoint or a
/// pipeline in it (`*` for every one the owner reaches). Without one it is the whole project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ConnectorScopeItem)]
pub struct ScopeItem {
    /// A project slug, e.g. `doprava`.
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    pub access: ItemAccess,
    /// Whether answers through this item keep the attributes the model marks `personalData`
    /// (DM-122): off by default, and only where the organization's ceiling allows it (AG-158).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub personal_data: bool,
}

impl ScopeItem {
    fn narrowing(&self) -> Option<(&'static str, &str)> {
        [
            ("ContextSpace", &self.space),
            ("Endpoint", &self.endpoint),
            ("Pipeline", &self.pipeline),
        ]
        .into_iter()
        .find_map(|(kind, name)| name.as_deref().map(|name| (kind, name)))
    }

    /// Whether this item reaches the object `kind`/`name` of its project, which sits in `space`:
    /// a space item reaches the space and every object whose `contextSpaceRef` names it (T-6360).
    fn names(&self, kind: &str, name: &str, space: Option<&str>) -> bool {
        let fits = |item: &str, of: &str| item == "*" || item == of;
        match self.narrowing() {
            None => true,
            Some(("ContextSpace", item)) if kind != "ContextSpace" => {
                space.is_some_and(|space| fits(item, space))
            }
            Some((narrowed, item)) => narrowed == kind && fits(item, name),
        }
    }
}

/// The verbs a connector carries on one kind (API/14 §1, T-6302): `read`, `create` (an object
/// the project does not hold yet), `edit` (`replace`, `patch`), `delete` and, on `Pipeline`, `run`;
/// `propose` stands for create, edit and delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ConnectorKindVerbs)]
pub struct KindGrant {
    /// A kind the segments reach, e.g. `Endpoint`.
    pub kind: String,
    /// e.g. `["read", "create"]`.
    #[schema(value_type = Vec<String>, example = json!(["read", "create"]))]
    pub verbs: Vec<ConnectorVerb>,
}

/// The verbs a person may tick on `kind`: `run` only on `Pipeline`.
pub fn kind_choices(kind: &str) -> Vec<ConnectorVerb> {
    let mut verbs = vec![
        ConnectorVerb::Read,
        ConnectorVerb::Create,
        ConnectorVerb::Edit,
        ConnectorVerb::Delete,
    ];
    if kind == "Pipeline" {
        verbs.push(ConnectorVerb::Run);
    }
    verbs
}

/// What a person asks for when creating a connector (API/14 §1).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewConnector {
    /// A DNS-1123 label unique among the person's connectors, e.g. `doprava-in-claude`.
    pub name: String,
    /// Presets, e.g. `["data-read"]`.
    pub segments: Vec<String>,
    pub scope: Vec<ScopeItem>,
    /// Verbs per kind, e.g. `[{"kind": "Endpoint", "verbs": ["read", "create"]}]`; a kind left
    /// out is read only. Absent: every change the segments reach, as before T-6302.
    #[serde(default)]
    pub kinds: Option<Vec<KindGrant>>,
    #[serde(default)]
    pub writes: bool,
    /// Each change the connector applies is approved at once as its owner (T-6552), e.g. `true`;
    /// needs `writes` and approve rights in every project it writes. Access changes, public
    /// publications and deletions still wait for a click. Default `false`.
    #[serde(default)]
    pub autonomous: bool,
    /// RFC 3339, e.g. `2026-11-27T00:00:00Z`.
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: TimestampTz,
    /// OAuth client ids the connector answers, e.g. a CIMD URL `https://claude.ai/oauth/mcp-client-metadata`.
    #[serde(default)]
    pub allowed_clients: Vec<String>,
}

/// What the Portal keeps of a connector, besides its id, owner and times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantDoc {
    segments: Vec<String>,
    scope: Vec<ScopeItem>,
    /// Absent on a record kept before T-6302.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kinds: Option<Vec<KindGrant>>,
    writes: bool,
    /// Absent on a record kept before T-6552: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    autonomous: bool,
    expires_at: TimestampTz,
    allowed_clients: Vec<String>,
    /// Above the ceiling, waiting for an administrator (AG-150); a record kept before T-5953 has
    /// none of the three members below.
    #[serde(default)]
    pending: bool,
    /// What the request asked above the ceiling, one line per field, for the approver.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    above: Vec<String>,
    /// The values behind `above`, which an approval covers (T-7541); absent on a record kept
    /// before it, whose approval covers nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    asked: Vec<Over>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<Approval>,
    /// When an edit last judged the grant against the ceiling; absent: at its creation. A
    /// renewal's age counts from here (JC2-45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    granted_at: Option<TimestampTz>,
}

/// Who let a request above the ceiling through, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConnectorApproval)]
pub struct Approval {
    /// The approver's Keycloak user id.
    pub by: String,
    #[schema(value_type = String, format = DateTime)]
    pub at: TimestampTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    id: String,
    owner: String,
    name: String,
    grant: GrantDoc,
    created_at: TimestampTz,
    last_used_at: Option<TimestampTz>,
    revoked_at: Option<TimestampTz>,
}

/// A connector as its owner reads it (API/14 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConnectorGrant)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub segments: Vec<String>,
    pub scope: Vec<ScopeItem>,
    /// Verbs per kind (T-6302); absent: every change the segments reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<KindGrant>>,
    pub writes: bool,
    /// Whether the changes it applies are approved at once as its owner (T-6552).
    pub autonomous: bool,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: TimestampTz,
    pub allowed_clients: Vec<String>,
    /// Whether the allowed clients bind the connector (claude.ai, ChatGPT) or only say whom it
    /// is meant for (Claude Code, `mcp-local`): AG-153.
    pub client_is_boundary: bool,
    /// `active`, `pending` (above the ceiling, waiting for an administrator), `expired` or
    /// `revoked`.
    pub state: String,
    /// What the request asked above the owner's DelegationPolicy ceiling; set while pending and
    /// kept after an approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub above_ceiling: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    /// The parts of this connector the owner's DelegationPolicy ceiling takes away today, each
    /// one line such as `segment pipeline`; calls run without them (AG-150, "narrowed by
    /// policy"). Empty when it fits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narrowed_by: Vec<String>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: TimestampTz,
    /// When the grant was last judged against the ceiling (created or edited): its age for the
    /// ceiling's longest life counts from here (JC2-45).
    #[serde(skip)]
    pub granted_at: TimestampTz,
    /// The values asked above the ceiling, which [`Ceiling::with_approved`] lets through.
    #[serde(skip)]
    pub(crate) over: Vec<Over>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_used_at: Option<TimestampTz>,
    /// The MCP address a client adds: `https://portal.{domain}/c/{id}`.
    pub url: String,
    /// What the connector may spend; shown on My connectors.
    pub limits: Limits,
}

impl Connector {
    pub fn is_active(&self) -> bool {
        self.state == "active"
    }

    /// Whether this connector's approval still covers `new`, which asks `above`: every value
    /// above was approved and the grant is no wider than the one the approver judged, or it
    /// waits again (AG-150, T-7541).
    pub fn approval_covers(&self, new: &NewConnector, above: &Above) -> bool {
        let approved = above.values.iter().all(|asked| match asked {
            Over::Until(at) => self
                .over
                .iter()
                .any(|over| matches!(over, Over::Until(until) if at <= until)),
            asked => self.over.contains(asked),
        });
        let held = |kind: &str, verb: ConnectorVerb| {
            let Some(kinds) = self.kinds.as_ref() else {
                return true;
            };
            verb == ConnectorVerb::Read
                || kinds
                    .iter()
                    .any(|entry| entry.kind == kind && verbs_allow(&entry.verbs, verb))
        };
        let kinds_no_wider = match &new.kinds {
            None => self.kinds.is_none(),
            Some(kinds) => kinds.iter().all(|entry| {
                entry
                    .verbs
                    .iter()
                    .flat_map(|verb| each_verb(*verb))
                    .all(|verb| held(&entry.kind, verb))
            }),
        };
        let clients_no_wider = self.allowed_clients.is_empty()
            || (!new.allowed_clients.is_empty()
                && new
                    .allowed_clients
                    .iter()
                    .all(|client| self.allowed_clients.contains(client)));
        !above.values.is_empty()
            && approved
            && kinds_no_wider
            && clients_no_wider
            && new.segments.iter().all(|s| self.segments.contains(s))
            && new.scope.iter().all(|item| self.scope.contains(item))
            && (self.writes || !new.writes)
            && (self.autonomous || !new.autonomous)
            && new.expires_at.as_second() <= self.expires_at.as_second()
    }

    /// The tools and kinds the segments reach.
    pub fn reach(&self) -> (Vec<&'static str>, Vec<&'static str>) {
        let (mut tools, kinds) =
            reach_of(|segment| self.segments.iter().any(|s| s == segment || s == ADMIN));
        if self.writes {
            tools.extend(written(&kinds));
            tools.sort_unstable();
        }
        (tools, kinds)
    }

    /// The narrowing a call through this connector carries, as an agent profile's access block
    /// (AG-70): only the segments' tools, `read` on their kinds, every Endpoint the scope names.
    pub fn access(&self) -> crate::agents::access::Access {
        let (tools, kinds) = self.reach();
        // `propose` on a kind only for the tools listed that need it: sampling and trying a
        // mapping (an author's read, nothing saved) and, with `writes`, a run (T-6028).
        let proposing: Vec<&str> = tools
            .iter()
            .filter_map(|t| crate::ops::find(t))
            .filter(|op| op.verb == Some(jc_core::kinds::Verb::Propose))
            .map(|op| op.kind)
            .collect();
        let verbs = |k: &str| {
            if proposing.contains(&k) {
                json!(["read", "propose"])
            } else {
                json!(["read"])
            }
        };
        crate::agents::access::Access::from_spec(&json!({
            "access": {
                "operations": tools,
                "kinds": kinds.iter().map(|k| json!({ "kind": k, "verbs": verbs(k) })).collect::<Vec<_>>(),
            }
        }))
        .through(self.clone())
    }

    /// Whether a plan (or, with `write`, an apply) may name `kind`/`name` of `project`: a kind of
    /// the segments, a scope item that reaches the object, and for a write the `writes` switch and
    /// `write` on that item (AG-144, AG-149). `space` is where the change sits (`change_space`).
    /// `Err` says which, naming the target.
    pub fn may_change(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        space: Option<&str>,
        write: bool,
    ) -> Result<(), String> {
        let (_, kinds) = self.reach();
        if !kinds.contains(&kind) {
            return Err(format!(
                "this connector's segments ({}) change only {}, not {kind}",
                self.segments.join(", "),
                kinds.join(", ")
            ));
        }
        let mut items = self
            .scope
            .iter()
            .filter(|i| i.project == project && i.names(kind, name, space))
            .peekable();
        if items.peek().is_none() {
            return Err(format!(
                "this connector does not reach {kind} {name} of project {project}"
            ));
        }
        if !write {
            return Ok(());
        }
        if !self.writes {
            return Err(format!(
                "this connector has writes off: apply the plan in the Portal, or turn writes on \
                 for connector {} on the My connectors page",
                self.name
            ));
        }
        if items.any(|i| i.access == ItemAccess::Write) {
            return Ok(());
        }
        Err(format!(
            "this connector only reads {kind} {name} of project {project}: its scope gives no \
             write there"
        ))
    }

    /// Whether this connector's verbs let it `verb` on `kind` (T-6302): a kind its `kinds` list
    /// leaves out is read only, and a connector without the list changes what its segments
    /// reach. `Err` names the kind, the object and the missing verb.
    pub fn may_verb(&self, kind: &str, name: &str, verb: ConnectorVerb) -> Result<(), String> {
        let Some(kinds) = &self.kinds else {
            return Ok(());
        };
        let verbs = kinds
            .iter()
            .find(|k| k.kind == kind)
            .map_or(&[ConnectorVerb::Read][..], |k| k.verbs.as_slice());
        if verbs_allow(verbs, verb) {
            return Ok(());
        }
        let held: Vec<&str> = verbs.iter().map(|v| v.word()).collect();
        Err(format!(
            "this connector may not {word} {kind} {name}: its verbs on {kind} are {held}; tick \
             {word} on {kind} for connector {connector} on the My connectors page",
            word = verb.word(),
            held = if held.is_empty() {
                "none".to_owned()
            } else {
                held.join(", ")
            },
            connector = self.name,
        ))
    }

    /// The projects the scope names, in order, once each.
    pub fn projects(&self) -> Vec<&str> {
        let mut projects: Vec<&str> = Vec::new();
        for item in &self.scope {
            if !projects.contains(&item.project.as_str()) {
                projects.push(&item.project);
            }
        }
        projects
    }

    /// The Endpoints the scope names in `project` when it names nothing else there: what a
    /// data tool's `endpoint` may be (T-6413). `None` when an item reaches more.
    pub fn endpoints_named(&self, project: &str) -> Option<Vec<&str>> {
        let mut named = Vec::new();
        for item in self.scope.iter().filter(|i| i.project == project) {
            match item.narrowing() {
                Some(("Endpoint", name)) if name != "*" => named.push(name),
                _ => return None,
            }
        }
        (!named.is_empty()).then_some(named)
    }

    fn whole(&self, project: &str) -> bool {
        self.scope
            .iter()
            .any(|i| i.project == project && i.narrowing().is_none())
    }

    /// Whether `tools/list` offers `op` in `project`: in a project the scope narrows to named
    /// items, only a tool that names its object can be held to them.
    pub fn lists(&self, op: &Operation, project: &str) -> bool {
        self.scope.iter().any(|i| i.project == project)
            && (self.whole(project)
                || names_its_object(op)
                || (STEPS.contains(&op.name) && self.holds_pipeline(project)))
    }

    /// Whether an item of `project` narrows the scope to a Pipeline, or to a space that holds
    /// Pipelines (T-6360): what the authoring steps (`STEPS`) work on.
    fn holds_pipeline(&self, project: &str) -> bool {
        self.scope.iter().any(|i| {
            i.project == project && matches!(i.narrowing(), Some(("Pipeline" | "ContextSpace", _)))
        })
    }

    /// The scope's half of an authoring step (T-6363): each object the call names (the
    /// candidate `pipeline` manifest, its `draft`, the `dataSource` it samples) is an item of
    /// the scope. A sample sent as text names none: a scope holding a Pipeline may try one.
    fn admits_step(
        &self,
        project: &str,
        args: &Value,
        space_of: &dyn Fn(&str, &str) -> Option<String>,
    ) -> Result<(), String> {
        let mut named = Vec::new();
        if let Some(pipeline) = args.get("pipeline") {
            let name = pipeline
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .ok_or("this connector reaches only named objects: name the candidate in pipeline.metadata.name")?;
            let kind = pipeline
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("Pipeline");
            named.push((kind, name));
        }
        if let Some(draft) = args.get("draft") {
            let text = |key: &str| draft.get(key).and_then(Value::as_str);
            match (text("kind"), text("name")) {
                (Some(kind), Some(name)) => named.push((kind, name)),
                _ => return Err("the draft names its kind and name: give both".to_owned()),
            }
        }
        if let Some(source) = args.get("dataSource").and_then(Value::as_str) {
            named.push(("DataSource", source));
        }
        if named.is_empty() && !self.holds_pipeline(project) {
            return Err(format!(
                "this connector reaches only {} of project {project}: name one",
                self.named(project)
            ));
        }
        for (kind, name) in named {
            if !self.scope.iter().any(|i| {
                i.project == project && i.names(kind, name, space_of(kind, name).as_deref())
            }) {
                return Err(format!(
                    "this connector does not reach {kind} {name} of project {project}; it names {}",
                    self.named(project)
                ));
            }
        }
        Ok(())
    }

    /// The scope's half of one call (AG-144): the project, the kind and the object the call
    /// names, which sits in the space `space_of(kind, name)` answers (`space_now`). `Err` is the
    /// reason the refusal carries, naming the target.
    pub fn admits(
        &self,
        op: &Operation,
        project: &str,
        args: &Value,
        space_of: &dyn Fn(&str, &str) -> Option<String>,
    ) -> Result<(), String> {
        if !self.scope.iter().any(|i| i.project == project) {
            return Err(format!(
                "this connector does not reach project {project}; its scope names {}",
                self.projects().join(", ")
            ));
        }
        let kind = resource_kind(op, args);
        if matches!(op.name, "jc_resource_list" | "jc_resource_get") {
            let (_, kinds) = self.reach();
            match kind {
                Some(kind) if kinds.contains(&kind) => {}
                _ => {
                    return Err(format!(
                        "this connector's segments ({}) read only {}",
                        self.segments.join(", "),
                        kinds.join(", ")
                    ))
                }
            }
        }
        // A sample is read from a DataSource the project declares, or sent as text: never from
        // a URL a prompt carries, which the runner would fetch inside the cluster (T-6028).
        if args.pointer("/sample/url").is_some() {
            return Err(format!(
                "{} through a connector reads a DataSource of the project or a sample sent as text, never a URL; declare the source as a DataSource first",
                op.name
            ));
        }
        if let Some(admitted) = self.admits_write(op, project, args, space_of) {
            return admitted;
        }
        // A plan or an apply names its objects per change; the operation holds each one to
        // the scope with `may_change` (T-5956).
        if self.whole(project) || matches!(op.name, "jc_schema_get" | PLAN | APPLY | REVERT) {
            return Ok(());
        }
        if STEPS.contains(&op.name) {
            return self.admits_step(project, args, space_of);
        }
        let name = names_its_object(op)
            .then(|| object_named(op, args))
            .flatten();
        match (kind, name) {
            (Some(kind), Some(name))
                if self.scope.iter().any(|i| {
                    i.project == project && i.names(kind, name, space_of(kind, name).as_deref())
                }) =>
            {
                Ok(())
            }
            (Some(kind), Some(name)) => Err(format!(
                "this connector does not reach {kind} {name} of project {project}; it names {}",
                self.named(project)
            )),
            _ => Err(format!(
                "this connector reaches only {} of project {project}: name one",
                self.named(project)
            )),
        }
    }

    /// The writing tools' half of [`Self::admits`]: a run, a pause or an entity write is a
    /// write on the object it names, with the kind's verb where one applies (an entity write is
    /// a write on its Endpoint: `writes` and `write` on an item reaching it). `None` for any
    /// other operation.
    fn admits_write(
        &self,
        op: &Operation,
        project: &str,
        args: &Value,
        space_of: &dyn Fn(&str, &str) -> Option<String>,
    ) -> Option<Result<(), String>> {
        let (kind, key, noun, verb) = match op.name {
            RUN => ("Pipeline", "name", "pipeline", Some(ConnectorVerb::Run)),
            PAUSE => (
                "SyncSource",
                "name",
                "SyncSource",
                Some(ConnectorVerb::Edit),
            ),
            DATA_WRITE => ("Endpoint", "endpoint", "endpoint", None),
            BUILD => return Some(self.may_build(project, args)),
            _ => return None,
        };
        let Some(name) = args.get(key).and_then(Value::as_str) else {
            return Some(Err(format!("{} names the {noun}: name it", op.name)));
        };
        let space = space_of(kind, name);
        Some(
            verb.map_or(Ok(()), |verb| self.may_verb(kind, name, verb))
                .and_then(|()| self.may_change(project, kind, name, space.as_deref(), true)),
        )
    }

    /// A build's half of [`Self::admits`] (T-6487): `create` on the App and `write` on an item of
    /// the whole project. No scope item narrows to an App, and that item also reaches every
    /// Endpoint the run reads, so a narrowed connector starts no build.
    fn may_build(&self, project: &str, args: &Value) -> Result<(), String> {
        let name = args
            .get("appName")
            .and_then(Value::as_str)
            .ok_or("jc_run_create names the App: appName")?;
        self.may_verb("App", name, ConnectorVerb::Create)?;
        self.may_change(project, "App", name, None, true)
    }

    /// Whether a call in `project` with `args` may answer the attributes the model marks
    /// personal (AG-158): a ticked item of the project that reaches the call's Endpoint, named
    /// by `endpoint` or `name`. A ticked space or pipeline item does not reach one.
    pub fn shows_personal(&self, project: &str, args: &Value) -> bool {
        let endpoint = ["endpoint", "name"]
            .iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str));
        self.scope.iter().any(|i| {
            i.project == project
                && i.personal_data
                && match i.narrowing() {
                    None => true,
                    Some(("Endpoint", name)) => name == "*" || Some(name) == endpoint,
                    Some(_) => false,
                }
        })
    }

    fn named(&self, project: &str) -> String {
        self.scope
            .iter()
            .filter(|i| i.project == project)
            .filter_map(ScopeItem::narrowing)
            .map(|(kind, name)| format!("{kind} {name}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Where `kind`/`name` of `project` sits now: the `contextSpaceRef` of the copy the mirror holds
/// in the nearest home of the kind (T-6360).
pub(crate) fn space_now(
    mirror: &crate::store::Mirror,
    project: &str,
    kind: &str,
    name: &str,
) -> Option<String> {
    let info = crate::resource::by_kind(kind)?;
    crate::resource::homes(info, project)
        .iter()
        .find_map(|home| mirror.get(home, kind, name))
        .and_then(|envelope| crate::permissions::spec_space(&envelope.spec))
}

/// The space a change sits in (T-6360): where the object is now (`now`, its spec) and where its
/// manifest puts it (`next`). `None` when an end that exists names no space, or the change moves
/// the object between spaces: a space item reaches neither end of a move.
pub(crate) fn change_space(now: Option<&Value>, next: Option<&Value>) -> Option<String> {
    let spaces: Vec<Option<String>> = [now, next]
        .into_iter()
        .flatten()
        .map(crate::permissions::space_ref)
        .collect();
    match spaces.as_slice() {
        [Some(space)] => Some(space.clone()),
        [Some(a), Some(b)] if a == b => Some(a.clone()),
        _ => None,
    }
}

/// The writing tools a `writes` connector adds to its segments' tools, by the kinds they reach:
/// a run to one reaching Pipeline, the entity write to one reaching Endpoint, whose Policy then
/// decides each entity (T-6389), a build to one reaching App (T-6487) and the pause switch to one
/// reaching SyncSource (T-6533).
fn written(kinds: &[&str]) -> Vec<&'static str> {
    let run = kinds.contains(&"Pipeline").then_some(RUN);
    let pause = kinds.contains(&"SyncSource").then_some(PAUSE);
    let data = kinds.contains(&"Endpoint").then_some(DATA_WRITE);
    let build = kinds.contains(&"App").then_some(BUILD);
    [APPLY]
        .into_iter()
        .chain(run)
        .chain(pause)
        .chain(data)
        .chain(build)
        .collect()
}

/// The Endpoints a build call names, by `endpointName` and `endpointNames`.
fn build_endpoints(args: &Value) -> Vec<&str> {
    let one = args.get("endpointName").and_then(Value::as_str);
    let several = args
        .get("endpointNames")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    one.into_iter().chain(several).collect()
}

/// Whether `policy` lets a connector build: a kinds list holds `create` on App, and an Endpoints
/// list holds each Endpoint the run would read.
fn policy_builds(policy: &DelegationPolicySpec, args: &Value) -> bool {
    let named = build_endpoints(args);
    (policy.kinds.is_none() || verbs_allow(policy.verbs_for("App"), ConnectorVerb::Create))
        && policy.endpoints.as_ref().is_none_or(|listed| {
            !named.is_empty() && named.iter().all(|e| listed.iter().any(|l| l == e))
        })
}

/// The tools and kinds of the segments `carries` picks.
fn reach_of(carries: impl Fn(&str) -> bool) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut tools = Vec::new();
    let mut kinds = Vec::new();
    for (segment, seg_tools, seg_kinds) in SEGMENTS {
        if carries(segment) {
            tools.extend_from_slice(seg_tools);
            kinds.extend_from_slice(seg_kinds);
        }
    }
    tools.sort_unstable();
    tools.dedup();
    kinds.sort_unstable();
    kinds.dedup();
    (tools, kinds)
}

/// The kinds of the presets `carries` picks that plan changes (list `jc_change_plan`): the kinds
/// a connector may hold a change verb on (T-6302). `data-read` plans nothing.
fn proposing_reach(carries: impl Fn(&str) -> bool) -> (Vec<&'static str>, Vec<&'static str>) {
    reach_of(|segment| {
        carries(segment)
            && SEGMENTS
                .iter()
                .any(|(name, tools, _)| *name == segment && tools.contains(&PLAN))
    })
}

/// The pipeline authoring steps: their calls name the candidate by `pipeline` or `draft`, or the
/// DataSource by `dataSource`, never by `name`, so a narrowed scope holds them in `admits_step`.
const STEPS: &[&str] = &[
    "jc_pipeline_sample_source",
    "jc_pipeline_try_mapping",
    "jc_pipeline_test",
];

/// A tool whose call names one object by `name`: what a narrowed scope can hold it to.
fn names_its_object(op: &Operation) -> bool {
    let input = (op.input)();
    matches!(op.name, "jc_resource_get" | PLAN | APPLY | REVERT)
        || (matches!(op.kind, "Endpoint" | "ContextSpace" | "Pipeline")
            && input.pointer("/properties/name").is_some())
        || (op.kind == "Endpoint" && input.pointer("/properties/endpoint").is_some())
        || op.name == "jc_schema_get"
        // Its Endpoint sits in `data.query`, which the door hands in as the arguments (T-6860).
        || op.name == crate::ops::render::NAME
}

/// The object a call names: `name`, or `endpoint` (jc_data_write, T-6389). `jc_kpi_compute`'s
/// `name` is the indicator's own, so only its `endpoint` is what it reads.
fn object_named<'a>(op: &Operation, args: &'a Value) -> Option<&'a str> {
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    if op.name == "jc_kpi_compute" {
        return text("endpoint");
    }
    text("name").or_else(|| text("endpoint"))
}

/// Whether a client id binds: a published client whose redirect URIs no local program can use
/// (AG-153). Claude Code's own document and realm clients are loopback, so "intended for" only.
// ponytail: the hosts of today's two vendors; T-5954 records the exact CIMD ids.
pub fn binds(client: &str) -> bool {
    let Ok(url) = url::Url::parse(client) else {
        return false;
    };
    url.scheme() == "https"
        && matches!(url.host_str(), Some("claude.ai" | "chatgpt.com"))
        && !url.path().contains("claude-code")
}

impl Record {
    fn view(&self, base: &str, now: TimestampTz) -> Connector {
        let state = if self.revoked_at.is_some() {
            "revoked"
        } else if self.grant.pending {
            "pending"
        } else if self.grant.expires_at <= now {
            "expired"
        } else {
            "active"
        };
        Connector {
            id: self.id.clone(),
            name: self.name.clone(),
            owner: self.owner.clone(),
            segments: self.grant.segments.clone(),
            scope: self.grant.scope.clone(),
            kinds: self.grant.kinds.clone(),
            writes: self.grant.writes,
            autonomous: self.grant.autonomous,
            expires_at: self.grant.expires_at,
            client_is_boundary: !self.grant.allowed_clients.is_empty()
                && self.grant.allowed_clients.iter().all(|c| binds(c)),
            allowed_clients: self.grant.allowed_clients.clone(),
            state: state.to_owned(),
            above_ceiling: self.grant.above.clone(),
            approval: self.grant.approval.clone(),
            narrowed_by: Vec::new(),
            created_at: self.created_at,
            granted_at: self.grant.granted_at.unwrap_or(self.created_at),
            over: self.grant.asked.clone(),
            last_used_at: self.last_used_at,
            url: address(base, &self.id),
            limits: DEFAULT_LIMITS,
        }
    }
}

/// The connector's MCP address, which is also the audience its tokens carry (AG-143).
pub fn address(base: &str, id: &str) -> String {
    format!("{}/c/{id}", base.trim_end_matches('/'))
}

/// The audience of every connector token (T-5954, ADR-N-064 §3.3). Keycloak 26.8 mints no
/// audience per connector, so the realm's `connector` scope names this one (and the Portal's
/// client, which exchanges the token for Endpoint reads), and the token's one [`scope`] says
/// which connector it is for.
pub const AUDIENCE: &str = "portal-connectors";

/// The realm's parameterized scope `connector`: a token holding any `connector:` scope is a
/// connector token, taken only at `/c/{id}`.
pub const SCOPE_FAMILY: &str = "connector";

/// The OAuth scope that binds a token to connector `id`.
pub fn scope(id: &str) -> String {
    format!("{SCOPE_FAMILY}:{id}")
}

/// 26 random base32 characters (`a-z`, `2-7`): 130 bits, not guessable, not a secret either.
fn new_id() -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0u8; 26];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 31)]))
        .collect()
}

/// Whether `id` could be a connector id at all, so a probe never reaches the store.
pub fn is_id(id: &str) -> bool {
    id.len() == 26 && id.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'))
}

fn invalid(field: &str, detail: String) -> ApiError {
    ApiError::Invalid {
        detail,
        errors: vec![field.to_owned()],
    }
}

fn above_ceiling(field: &str, detail: String) -> ApiError {
    ApiError::Denied(format!("{field}: {detail} (AG-150)"))
}

/// Checks a request against its shape and the ceiling; the owner's rights over each scope item
/// are the caller's half (`api::connectors`), since they need the Portal's state. A request above
/// the ceiling is a `403` naming the field, or, when a policy of the owner queues such requests,
/// `Ok` with one line per field above it: the connector is kept `pending` (AG-150).
pub fn validate(
    new: &NewConnector,
    ceiling: &Ceiling,
    now: TimestampTz,
) -> Result<Above, ApiError> {
    if !is_dns1123(&new.name) || new.name.len() > 63 {
        return Err(invalid(
            "name",
            format!("name '{}' must be a DNS-1123 label of at most 63 characters, e.g. doprava-in-claude", new.name),
        ));
    }
    validate_segments(&new.segments)?;
    validate_scope(new)?;
    validate_kinds(new)?;
    validate_personal(&new.scope, ceiling)?;
    if new.expires_at <= now {
        return Err(invalid(
            "expiresAt",
            "expiresAt must lie in the future".into(),
        ));
    }
    let most = i64::from(jc_core::kinds::delegation_policy::MAX_EXPIRY_DAYS);
    if new.expires_at > now + jiff::SignedDuration::from_hours(24 * most) {
        return Err(invalid(
            "expiresAt",
            format!("a connector lives at most {most} days"),
        ));
    }
    validate_clients(&new.allowed_clients)?;
    let above = above(new, ceiling, now);
    match above.first() {
        None => Ok(Above::default()),
        Some(_) if ceiling.queues() => {
            let mut asked = Above::default();
            for (values, field, why) in above {
                asked.values.extend(values);
                asked.lines.push(format!("{field}: {why}"));
            }
            Ok(asked)
        }
        Some((_, field, why)) => Err(above_ceiling(field, why.clone())),
    }
}

/// What a well-formed request asks above the ceiling, field by field, in field order: the values
/// an approval of the field covers, the field and the sentence naming what was asked.
fn above(
    new: &NewConnector,
    ceiling: &Ceiling,
    now: TimestampTz,
) -> Vec<(Vec<Over>, String, String)> {
    let named = ceiling.named();
    let mut above = Vec::new();
    for (i, segment) in new.segments.iter().enumerate() {
        if !ceiling.allows_segment(segment) {
            let only = ceiling.segments().join(", ");
            above.push((
                vec![Over::Segment(segment.clone())],
                format!("segments[{i}]"),
                format!("{named} lets a connector carry only {only} (asked {segment})"),
            ));
        }
    }
    for (i, item) in new.scope.iter().enumerate() {
        let endpoint = || Over::Endpoint {
            project: item.project.clone(),
            endpoint: item.endpoint.clone(),
        };
        if !ceiling.allows_project_name(&item.project) {
            let why = format!(
                "{named} does not let a connector reach project {}",
                item.project
            );
            // No policy lists Endpoints for a project none reaches, so the item's own go with it.
            above.push((
                vec![Over::Project(item.project.clone()), endpoint()],
                format!("scope[{i}].project"),
                why,
            ));
        } else if !ceiling.allows_endpoint(&item.project, item.endpoint.as_deref()) {
            let why = format!(
                "{named} lets a connector reach only the Endpoints it lists in project {}: name one (asked {})",
                item.project,
                item.endpoint.as_deref().unwrap_or("every Endpoint")
            );
            above.push((vec![endpoint()], format!("scope[{i}].endpoint"), why));
        }
    }
    for (i, entry) in new.kinds.iter().flatten().enumerate() {
        for verb in entry.verbs.iter().flat_map(|verb| each_verb(*verb)) {
            if !ceiling.allows_verb(None, &entry.kind, verb) {
                above.push((
                    vec![Over::Verb {
                        kind: entry.kind.clone(),
                        verb,
                    }],
                    format!("kinds[{i}].verbs"),
                    format!(
                        "{named} does not let a connector {} {}",
                        verb.word(),
                        entry.kind
                    ),
                ));
            }
        }
    }
    // Without a kinds list a connector changes whatever its segments reach; where every policy
    // lists the verbs per kind, that is above it and an approval must not cover it (JC2-06).
    if new.kinds.is_none() && ceiling.lists_kinds() {
        above.push((
            vec![Over::Kinds],
            "kinds".into(),
            format!("{named} names the verbs a connector holds per kind: tick them"),
        ));
    }
    if new.writes && !ceiling.writes() {
        above.push((
            vec![Over::Writes],
            "writes".into(),
            format!("{named} lets a connector read only"),
        ));
    }
    if new.autonomous {
        for project in written_projects(&new.scope) {
            if !ceiling.allows_autonomous(project) {
                above.push((
                    vec![Over::Autonomous(project.to_owned())],
                    "autonomous".into(),
                    format!("{named} does not let a connector approve its own changes in project {project}"),
                ));
            }
        }
    }
    let days = ceiling.max_days();
    if new.expires_at > now + jiff::SignedDuration::from_hours(24 * days) {
        // The asked date is part of the line, so an approval of it covers no later one (JC2-06).
        let why = format!(
            "{named} lets a connector live at most {days} days; asked until {}",
            new.expires_at
                .to_rfc3339_opts(jc_time::SecondsFormat::Secs, true)
        );
        above.push((
            vec![Over::Until(new.expires_at.as_second())],
            "expiresAt".into(),
            why,
        ));
    }
    for (i, client) in new.allowed_clients.iter().enumerate() {
        if !ceiling.allows_client(client) {
            let why = format!("{named} does not allow the client {client}");
            above.push((
                vec![Over::Client(client.clone())],
                format!("allowedClients[{i}]"),
                why,
            ));
        }
    }
    if new.allowed_clients.is_empty() && !ceiling.any_client() {
        let why = format!("{named} lets a connector answer only the clients it lists: name one");
        above.push((vec![Over::AnyClient], "allowedClients".into(), why));
    }
    above
}

/// A connector's `kinds`: the shape jc-core checks for a policy's list, and only kinds its
/// segments reach (T-6302).
fn validate_kinds(new: &NewConnector) -> Result<(), ApiError> {
    let Some(kinds) = &new.kinds else {
        return Ok(());
    };
    let listed: Vec<KindVerbs> = kinds
        .iter()
        .map(|k| KindVerbs {
            kind: k.kind.clone(),
            verbs: k.verbs.clone(),
        })
        .collect();
    if let Err(jc_core::Error::Name {
        field,
        value,
        reason,
    }) = validate_kind_verbs("kinds", &listed)
    {
        return Err(invalid(field, format!("kind {value}: {reason}")));
    }
    let ticked = |segment: &str| new.segments.iter().any(|s| s == segment || s == ADMIN);
    let (_, reached) = reach_of(ticked);
    let (_, proposed) = proposing_reach(ticked);
    for (i, entry) in kinds.iter().enumerate() {
        if !reached.contains(&entry.kind.as_str()) {
            return Err(invalid(
                &format!("kinds[{i}].kind"),
                format!(
                    "the segments {} do not reach {}; they reach {}",
                    new.segments.join(", "),
                    entry.kind,
                    reached.join(", ")
                ),
            ));
        }
        let changes = entry.verbs.iter().any(|v| *v != ConnectorVerb::Read);
        if changes && !proposed.contains(&entry.kind.as_str()) {
            return Err(invalid(
                &format!("kinds[{i}].verbs"),
                format!(
                    "no ticked preset proposes changes of {}; tick one that does, such as context or pipeline, or keep it read",
                    entry.kind
                ),
            ));
        }
    }
    Ok(())
}

fn validate_segments(segments: &[String]) -> Result<(), ApiError> {
    if segments.is_empty() {
        return Err(invalid(
            "segments",
            "segments names at least one preset, e.g. data-read".into(),
        ));
    }
    for (i, segment) in segments.iter().enumerate() {
        if !is_segment(segment) {
            return Err(invalid(
                &format!("segments[{i}]"),
                format!("'{segment}' is no segment; one of data-read, context, pipeline, access, publish, apps, operate, admin"),
            ));
        }
    }
    Ok(())
}

fn validate_scope(new: &NewConnector) -> Result<(), ApiError> {
    if new.scope.is_empty() || new.scope.len() > MAX_SCOPE {
        return Err(invalid("scope", format!("scope names 1 to {MAX_SCOPE} items, e.g. {{\"project\": \"doprava\", \"access\": \"read\"}}")));
    }
    for (i, item) in new.scope.iter().enumerate() {
        if !is_dns1123(&item.project) {
            return Err(invalid(
                &format!("scope[{i}].project"),
                format!("'{}' is not a project name", item.project),
            ));
        }
        let named = [
            ("space", &item.space),
            ("endpoint", &item.endpoint),
            ("pipeline", &item.pipeline),
        ];
        if named.iter().filter(|(_, v)| v.is_some()).count() > 1 {
            return Err(invalid(
                &format!("scope[{i}]"),
                "a scope item names at most one of space, endpoint, pipeline".into(),
            ));
        }
        for (field, value) in named {
            if let Some(value) = value.as_deref().filter(|v| *v != "*" && !is_dns1123(v)) {
                return Err(invalid(
                    &format!("scope[{i}].{field}"),
                    format!("'{value}' is neither a name nor *"),
                ));
            }
        }
        if item.access == ItemAccess::Write && !new.writes {
            return Err(invalid(
                &format!("scope[{i}].access"),
                "a write item needs writes: true".into(),
            ));
        }
    }
    Ok(())
}

/// A `personalData` tick needs a ceiling that allows it (AG-158).
fn validate_personal(scope: &[ScopeItem], ceiling: &Ceiling) -> Result<(), ApiError> {
    match scope.iter().position(|item| item.personal_data) {
        Some(i) if !ceiling.personal_data => Err(above_ceiling(
            &format!("scope[{i}].personalData"),
            "your organization keeps the attributes a model marks personalData out of AI connector answers; untick it, or ask an organization admin for a DelegationPolicy that allows it".into(),
        )),
        _ => Ok(()),
    }
}

fn validate_clients(clients: &[String]) -> Result<(), ApiError> {
    if clients.len() > MAX_CLIENTS {
        return Err(invalid(
            "allowedClients",
            format!("at most {MAX_CLIENTS} clients"),
        ));
    }
    for (i, client) in clients.iter().enumerate() {
        let plain = client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:/".contains(&b));
        if client.is_empty() || client.len() > 512 || !plain {
            return Err(invalid(
                &format!("allowedClients[{i}]"),
                "a client is an OAuth client id: a realm client such as mcp-local or a CIMD https URL".into(),
            ));
        }
    }
    Ok(())
}

/// A connector as an organization administrator reads it (T-5975): its grant and how much it is
/// used, never what a call carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConnectorUsage)]
pub struct Overview {
    #[serde(flatten)]
    pub connector: Connector,
    /// Calls in the last 24 hours, allowed and refused.
    pub calls_per_day: i64,
    /// Refused calls in the last 24 hours.
    pub denials_per_day: i64,
    /// [`DENIALS_FLAG`] refused calls or more in the last 24 hours.
    pub flagged: bool,
}

impl Overview {
    fn of(connector: Connector, calls_per_day: i64, denials_per_day: i64) -> Self {
        Self {
            connector,
            calls_per_day,
            denials_per_day,
            flagged: denials_per_day >= DENIALS_FLAG,
        }
    }
}

fn refused(verdict: &str) -> bool {
    verdict.starts_with("refused")
}

/// One row of a connector's audit (AG-147).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConnectorCall)]
pub struct Call {
    #[schema(value_type = String, format = DateTime)]
    pub at: TimestampTz,
    /// The Keycloak subject of the token, the owner's unless the call was refused for it.
    pub subject: String,
    pub client: Option<String>,
    /// The tool, or the MCP method of a call that names none.
    pub tool: String,
    /// `project/kind/name`, as far as the call names them.
    pub target: Option<String>,
    /// `allowed`, `refused: <why>`, `error`, or `started` where the outcome was not recorded.
    pub verdict: String,
    pub duration_ms: i32,
}

/// Where one call's audit rows belong.
#[derive(Debug, Clone)]
pub struct Trail {
    /// The connector it came through; none for the Portal MCP.
    connector: Option<String>,
    /// The Organization an agent run's call belongs to; none: the request's.
    organization: Option<String>,
    /// What the `started` row and the outcome share.
    id: String,
}

impl Trail {
    pub fn new(connector: Option<String>, organization: Option<String>) -> Self {
        Self {
            connector,
            organization,
            id: new_id(),
        }
    }
}

/// The memory arm: the connectors, and each audit row beside where it belongs.
type Held = (Vec<Record>, Vec<(Trail, Call)>);
type CallRow = (
    Odt,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    i32,
);

enum Inner {
    Postgres(Db),
    Memory(Mutex<Held>),
}

/// The database, the mirror and the configured installation Organization that place a person's
/// rows under their Organization's reserved key (T-7282, [`crate::project_ids::person_tx`]).
struct Db {
    pool: sqlx::PgPool,
    mirror: std::sync::Arc<crate::store::Mirror>,
    installation: Option<String>,
}

type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

impl Db {
    /// The request's Organization's transaction; `None` on a host in no active Organization,
    /// where a person holds no connector.
    async fn person(&self) -> Result<Option<Tx>, ApiError> {
        crate::project_ids::person_tx(&self.pool, &self.mirror, self.installation.as_deref())
            .await
            .map_err(db_error)
    }

    /// The transaction an audit row is written in: the named Organization's, else the request's.
    /// None is an error, so no call runs unrecorded (AUD-02).
    async fn audit(&self, organization: Option<&str>) -> Result<Tx, ApiError> {
        let tx = match organization {
            Some(organization) => {
                match crate::project_ids::reserved(&self.pool, organization)
                    .await
                    .map_err(db_error)?
                {
                    Some(key) => Some(
                        crate::db::tenant_tx(&self.pool, key)
                            .await
                            .map_err(db_error)?,
                    ),
                    None => None,
                }
            }
            None => self.person().await?,
        };
        tx.ok_or_else(|| crate::project_ids::person_unplaced("audit of MCP calls"))
    }
}

pub struct ConnectorStore {
    inner: Inner,
    trimmed_at: Mutex<Option<TimestampTz>>,
}

type Odt = time::OffsetDateTime;
type OverviewRow = (
    String,
    String,
    String,
    sqlx::types::Json<GrantDoc>,
    Odt,
    Option<Odt>,
    Option<Odt>,
    i64,
    i64,
);
type Row = (
    String,
    String,
    String,
    sqlx::types::Json<GrantDoc>,
    Odt,
    Option<Odt>,
    Option<Odt>,
);
macro_rules! columns {
    () => {
        "id, owner, name, grant_doc, created_at, last_used_at, revoked_at"
    };
}

fn record((id, owner, name, grant, created_at, last_used_at, revoked_at): Row) -> Record {
    Record {
        id,
        owner,
        name,
        grant: grant.0,
        created_at: odt_to_ts(created_at),
        last_used_at: last_used_at.map(odt_to_ts),
        revoked_at: revoked_at.map(odt_to_ts),
    }
}

fn db_error(err: sqlx::Error) -> ApiError {
    tracing::error!(error = %err, "the connector store failed");
    ApiError::from_db(&err, "connector")
}

fn full(owner_has: usize) -> Result<(), ApiError> {
    if owner_has >= MAX_PER_PERSON {
        return Err(ApiError::Conflict(format!(
            "you hold {MAX_PER_PERSON} live connectors already; revoke one first"
        )));
    }
    Ok(())
}

fn taken(name: &str) -> ApiError {
    ApiError::Conflict(format!("you hold a live connector named {name} already"))
}

impl ConnectorStore {
    /// Backed by the database when there is one: each statement runs in the reserved key of the
    /// request's Organization, else of the installation's (`installation`, the configured name,
    /// when the mirror names none). By a map when there is no database.
    pub fn new(
        db: Option<(sqlx::PgPool, std::sync::Arc<crate::store::Mirror>)>,
        installation: Option<String>,
    ) -> Self {
        Self {
            trimmed_at: Mutex::default(),
            inner: match db {
                Some((pool, mirror)) => Inner::Postgres(Db {
                    pool,
                    mirror,
                    installation,
                }),
                None => Inner::Memory(Mutex::default()),
            },
        }
    }

    fn memory(rows: &Mutex<Held>) -> std::sync::MutexGuard<'_, Held> {
        rows.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Keeps a validated connector for `owner`; at most [`MAX_PER_PERSON`] live ones, one name each.
    /// `above` is what the request asks above the ceiling: not empty, it is kept `pending`.
    pub async fn create(
        &self,
        owner: &str,
        new: NewConnector,
        above: Above,
        base: &str,
    ) -> Result<Connector, ApiError> {
        let now = TimestampTz::now();
        let record = Record {
            id: new_id(),
            owner: owner.to_owned(),
            name: new.name,
            grant: GrantDoc {
                segments: new.segments,
                scope: new.scope,
                kinds: new.kinds,
                writes: new.writes,
                autonomous: new.autonomous,
                expires_at: new.expires_at,
                allowed_clients: new.allowed_clients,
                pending: !above.is_empty(),
                above: above.lines,
                asked: above.values,
                approval: None,
                granted_at: None,
            },
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        };
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Err(crate::project_ids::person_unplaced("connector"));
                };
                // One person's creates in a row, so two at once cannot both pass the count.
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext('connectors/' || $1))")
                    .bind(owner)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                let (live,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM connectors WHERE owner = $1 AND revoked_at IS NULL",
                )
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_error)?;
                full(usize::try_from(live).unwrap_or(usize::MAX))?;
                let inserted = sqlx::query(
                    "INSERT INTO connectors (id, owner, name, grant_doc, created_at) VALUES ($1, $2, $3, $4, $5) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(&record.id)
                .bind(owner)
                .bind(&record.name)
                .bind(sqlx::types::Json(&record.grant))
                .bind(ts_to_odt(now))
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                if inserted.rows_affected() == 0 {
                    return Err(taken(&record.name));
                }
                tx.commit().await.map_err(db_error)?;
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                let live: Vec<&Record> = rows
                    .0
                    .iter()
                    .filter(|r| r.owner == owner && r.revoked_at.is_none())
                    .collect();
                full(live.len())?;
                if live.iter().any(|r| r.name == record.name) {
                    return Err(taken(&record.name));
                }
                rows.0.push(record.clone());
            }
        }
        Ok(record.view(base, now))
    }

    /// Replaces the grant of the owner's connector `id` that is not revoked (an expired one
    /// lives again under a new `expiresAt`); `false` when there is none. The name stays.
    /// Replaces a live connector's grant: `above` names what the edit asks above the ceiling
    /// (pending unless `approval` already covers it, AG-150).
    pub async fn update(
        &self,
        id: &str,
        owner: &str,
        new: NewConnector,
        above: Above,
        approval: Option<Approval>,
    ) -> Result<bool, ApiError> {
        let grant = GrantDoc {
            segments: new.segments,
            scope: new.scope,
            kinds: new.kinds,
            writes: new.writes,
            autonomous: new.autonomous,
            expires_at: new.expires_at,
            allowed_clients: new.allowed_clients,
            pending: !above.is_empty() && approval.is_none(),
            above: above.lines,
            asked: above.values,
            approval,
            granted_at: Some(TimestampTz::now()),
        };
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(false);
                };
                let done = sqlx::query(
                    "UPDATE connectors SET grant_doc = $3 WHERE id = $1 AND owner = $2 AND revoked_at IS NULL",
                )
                .bind(id)
                .bind(owner)
                .bind(sqlx::types::Json(&grant))
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                Ok(done.rows_affected() == 1)
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                let found = rows
                    .0
                    .iter_mut()
                    .find(|r| r.id == id && r.owner == owner && r.revoked_at.is_none());
                Ok(found.map(|r| r.grant = grant).is_some())
            }
        }
    }

    /// The connector `id`, whoever owns it: the caller compares the owner.
    pub async fn get(&self, id: &str, base: &str) -> Result<Option<Connector>, ApiError> {
        let now = TimestampTz::now();
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(None);
                };
                let row: Option<Row> = sqlx::query_as(concat!(
                    "SELECT ",
                    columns!(),
                    " FROM connectors WHERE id = $1"
                ))
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
                Ok(row.map(|row| record(row).view(base, now)))
            }
            Inner::Memory(rows) => Ok(Self::memory(rows)
                .0
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.view(base, now))),
        }
    }

    /// The owner's connectors, newest first, revoked ones included.
    pub async fn of_owner(&self, owner: &str, base: &str) -> Result<Vec<Connector>, ApiError> {
        let now = TimestampTz::now();
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(Vec::new());
                };
                let rows: Vec<Row> = sqlx::query_as(concat!(
                    "SELECT ",
                    columns!(),
                    " FROM connectors WHERE owner = $1 ORDER BY created_at DESC LIMIT 200"
                ))
                .bind(owner)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
                Ok(rows
                    .into_iter()
                    .map(|row| record(row).view(base, now))
                    .collect())
            }
            Inner::Memory(rows) => Ok(Self::memory(rows)
                .0
                .iter()
                .rev()
                .filter(|r| r.owner == owner)
                .map(|r| r.view(base, now))
                .collect()),
        }
    }

    /// Every live connector waiting for an administrator, oldest first (AG-150).
    pub async fn pending(&self, base: &str) -> Result<Vec<Connector>, ApiError> {
        let now = TimestampTz::now();
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(Vec::new());
                };
                let rows: Vec<Row> = sqlx::query_as(concat!(
                    "SELECT ",
                    columns!(),
                    " FROM connectors WHERE revoked_at IS NULL AND (grant_doc->>'pending')::boolean \
                     ORDER BY created_at LIMIT 200"
                ))
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
                Ok(rows
                    .into_iter()
                    .map(|row| record(row).view(base, now))
                    .collect())
            }
            Inner::Memory(rows) => Ok(Self::memory(rows)
                .0
                .iter()
                .filter(|r| r.revoked_at.is_none() && r.grant.pending)
                .map(|r| r.view(base, now))
                .collect()),
        }
    }

    /// Lets the pending connector `id` through, naming its approver; `false` when it is not
    /// pending (any more), so two approvals at once record one, or when an edit moved it since
    /// `judged`, the `granted_at` the approver's rights were judged on (T-7541).
    pub async fn approve(&self, id: &str, by: &str, judged: TimestampTz) -> Result<bool, ApiError> {
        let approval = Approval {
            by: by.to_owned(),
            at: TimestampTz::now(),
        };
        // Only an edit sets `granted_at`: none is the grant as it was created.
        let unmoved = |grant: &GrantDoc| grant.granted_at.is_none_or(|at| at == judged);
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(false);
                };
                let held: Option<(sqlx::types::Json<GrantDoc>,)> = sqlx::query_as(
                    "SELECT grant_doc FROM connectors WHERE id = $1 AND revoked_at IS NULL FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
                if !held.is_some_and(|(grant,)| unmoved(&grant.0)) {
                    return Ok(false);
                }
                let done = sqlx::query(
                    "UPDATE connectors SET grant_doc = grant_doc || jsonb_build_object('pending', false, 'approval', $2::jsonb) \
                     WHERE id = $1 AND revoked_at IS NULL AND (grant_doc->>'pending')::boolean",
                )
                .bind(id)
                .bind(sqlx::types::Json(&approval))
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                Ok(done.rows_affected() == 1)
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                let found = rows.0.iter_mut().find(|r| {
                    r.id == id && r.revoked_at.is_none() && r.grant.pending && unmoved(&r.grant)
                });
                Ok(found
                    .map(|r| {
                        r.grant.pending = false;
                        r.grant.approval = Some(approval);
                    })
                    .is_some())
            }
        }
    }

    /// Revokes the owner's live connector `id`; `false` when there is none to revoke.
    pub async fn revoke(&self, id: &str, owner: &str) -> Result<bool, ApiError> {
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(false);
                };
                let done = sqlx::query(
                    "UPDATE connectors SET revoked_at = now() WHERE id = $1 AND owner = $2 AND revoked_at IS NULL",
                )
                .bind(id)
                .bind(owner)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                Ok(done.rows_affected() == 1)
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                let found = rows
                    .0
                    .iter_mut()
                    .find(|r| r.id == id && r.owner == owner && r.revoked_at.is_none());
                Ok(found
                    .map(|r| r.revoked_at = Some(TimestampTz::now()))
                    .is_some())
            }
        }
    }

    /// Every connector of the organization with its last day's calls and denials, live ones first
    /// and then newest first, at most `OVERVIEW_PAGE` (T-5975).
    pub async fn overview(&self, base: &str) -> Result<Vec<Overview>, ApiError> {
        let now = TimestampTz::now();
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(Vec::new());
                };
                let rows: Vec<OverviewRow> = sqlx::query_as(concat!(
                    "SELECT ",
                    columns!(),
                    ", (SELECT count(*) FROM connector_calls k WHERE k.connector = connectors.id \
                       AND k.at > now() - interval '1 day' AND k.verdict <> 'started'), \
                     (SELECT count(*) FROM connector_calls k WHERE k.connector = connectors.id \
                       AND k.at > now() - interval '1 day' AND k.verdict LIKE 'refused%') \
                     FROM connectors ORDER BY revoked_at IS NULL DESC, created_at DESC LIMIT $1"
                ))
                .bind(OVERVIEW_PAGE)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(id, owner, name, grant, created, used, revoked, calls, denials)| {
                            let row = (id, owner, name, grant, created, used, revoked);
                            Overview::of(record(row).view(base, now), calls, denials)
                        },
                    )
                    .collect())
            }
            Inner::Memory(rows) => {
                let rows = Self::memory(rows);
                let since = now - jiff::SignedDuration::from_hours(24);
                let mut records: Vec<&Record> = rows.0.iter().rev().collect();
                records.sort_by_key(|r| r.revoked_at.is_some());
                Ok(records
                    .into_iter()
                    .take(usize::try_from(OVERVIEW_PAGE).unwrap_or(500))
                    .map(|r| {
                        let day = || {
                            rows.1.iter().filter(|(trail, c)| {
                                trail.connector.as_deref() == Some(r.id.as_str())
                                    && c.verdict != STARTED
                                    && c.at > since
                            })
                        };
                        let calls = i64::try_from(day().count()).unwrap_or(i64::MAX);
                        let denials = day().filter(|(_, c)| refused(&c.verdict)).count();
                        Overview::of(
                            r.view(base, now),
                            calls,
                            i64::try_from(denials).unwrap_or(i64::MAX),
                        )
                    })
                    .collect())
            }
        }
    }

    /// Revokes every live connector of `owner`, as offboarding does (T-5975): how many it revoked.
    pub async fn revoke_all(&self, owner: &str) -> Result<u64, ApiError> {
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(0);
                };
                let done = sqlx::query(
                    "UPDATE connectors SET revoked_at = now() WHERE owner = $1 AND revoked_at IS NULL",
                )
                .bind(owner)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?
                .rows_affected();
                tx.commit().await.map_err(db_error)?;
                Ok(done)
            }
            Inner::Memory(rows) => {
                let now = TimestampTz::now();
                let mut rows = Self::memory(rows);
                let mut done = 0;
                for r in rows
                    .0
                    .iter_mut()
                    .filter(|r| r.owner == owner && r.revoked_at.is_none())
                {
                    r.revoked_at = Some(now);
                    done += 1;
                }
                Ok(done)
            }
        }
    }

    /// Keeps the one audit row of a connector's event that is no call to wait for (a paired tab).
    pub async fn record(&self, connector: &str, call: Call) -> Result<(), ApiError> {
        self.keep(&Trail::new(Some(connector.to_owned()), None), call)
            .await
    }

    /// Appends one audit row of `trail`; an allowed call also marks its connector used, in the
    /// same transaction. An `Err` is the caller's to answer with: the call is not on record.
    pub async fn keep(&self, trail: &Trail, call: Call) -> Result<(), ApiError> {
        let used = trail
            .connector
            .as_ref()
            .filter(|_| call.verdict == "allowed");
        match &self.inner {
            Inner::Postgres(db) => {
                let mut tx = db.audit(trail.organization.as_deref()).await?;
                sqlx::query(
                    "INSERT INTO connector_calls (connector, call_id, at, subject, client, tool, target, verdict, duration_ms) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                )
                .bind(&trail.connector)
                .bind(&trail.id)
                .bind(ts_to_odt(call.at))
                .bind(&call.subject)
                .bind(&call.client)
                .bind(&call.tool)
                .bind(&call.target)
                .bind(&call.verdict)
                .bind(call.duration_ms)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                if let Some(connector) = used {
                    sqlx::query("UPDATE connectors SET last_used_at = $2 WHERE id = $1")
                        .bind(connector)
                        .bind(ts_to_odt(call.at))
                        .execute(&mut *tx)
                        .await
                        .map_err(db_error)?;
                }
                tx.commit().await.map_err(db_error)?;
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                if let Some(connector) = used {
                    if let Some(r) = rows.0.iter_mut().find(|r| r.id == *connector) {
                        r.last_used_at = Some(call.at);
                    }
                }
                let over = (rows.1.len() + 1).saturating_sub(CALLS_IN_MEMORY);
                rows.1.drain(..over);
                rows.1.push((trail.clone(), call));
            }
        }
        Ok(())
    }

    /// Deletes the audit rows older than `days`, Organization by Organization in its own key,
    /// batch by batch (AUD-04, STO-07). The leader's hourly job.
    pub async fn trim_audit(&self, now: TimestampTz, days: u32) -> Result<u64, ApiError> {
        let floor = now - jiff::SignedDuration::from_hours(24 * i64::from(days));
        match &self.inner {
            Inner::Postgres(db) => {
                let mut done = 0;
                for organization in crate::host_registry::organizations(&db.pool)
                    .await
                    .map_err(db_error)?
                {
                    let Some(key) = crate::project_ids::reserved(&db.pool, &organization.name)
                        .await
                        .map_err(db_error)?
                    else {
                        continue;
                    };
                    loop {
                        let mut tx = crate::db::tenant_tx(&db.pool, key)
                            .await
                            .map_err(db_error)?;
                        let deleted = sqlx::query(
                            "DELETE FROM connector_calls WHERE id IN \
                             (SELECT id FROM connector_calls WHERE at < $1 LIMIT $2)",
                        )
                        .bind(ts_to_odt(floor))
                        .bind(crate::activity::TRIM_BATCH)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_error)?
                        .rows_affected();
                        tx.commit().await.map_err(db_error)?;
                        done += deleted;
                        if deleted < crate::activity::TRIM_BATCH as u64 {
                            break;
                        }
                    }
                }
                Ok(done)
            }
            Inner::Memory(rows) => {
                let mut rows = Self::memory(rows);
                let before = rows.1.len();
                rows.1.retain(|(_, call)| call.at >= floor);
                Ok((before - rows.1.len()) as u64)
            }
        }
    }

    /// [`Self::trim_audit`] when an hour has passed since this process's last one, else `None`.
    pub async fn trim_audit_if_due(
        &self,
        now: TimestampTz,
        days: u32,
    ) -> Option<Result<u64, ApiError>> {
        {
            let mut last = self.trimmed_at.lock().unwrap_or_else(|p| p.into_inner());
            let every = jiff::SignedDuration::from_mins(crate::activity::TRIM_EVERY_MINUTES);
            if last.is_some_and(|at| now - at < every) {
                return None;
            }
            *last = Some(now);
        }
        Some(self.trim_audit(now, days).await)
    }

    /// The connector's newest audit rows first, at most [`CALLS_PAGE`].
    pub async fn calls(&self, connector: &str) -> Result<Vec<Call>, ApiError> {
        match &self.inner {
            Inner::Postgres(db) => {
                let Some(mut tx) = db.person().await? else {
                    return Ok(Vec::new());
                };
                let rows: Vec<CallRow> =
                    sqlx::query_as(
                        "SELECT at, subject, client, tool, target, verdict, duration_ms FROM connector_calls c \
                         WHERE connector = $1 AND NOT (verdict = 'started' AND call_id IS NOT NULL \
                           AND EXISTS (SELECT 1 FROM connector_calls o \
                                       WHERE o.call_id = c.call_id AND o.verdict <> 'started')) \
                         ORDER BY at DESC, id DESC LIMIT $2",
                    )
                    .bind(connector)
                    .bind(CALLS_PAGE)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(db_error)?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(at, subject, client, tool, target, verdict, duration_ms)| Call {
                            at: odt_to_ts(at),
                            subject,
                            client,
                            tool,
                            target,
                            verdict,
                            duration_ms,
                        },
                    )
                    .collect())
            }
            Inner::Memory(rows) => {
                let rows = Self::memory(rows);
                let ended: std::collections::HashSet<&str> = rows
                    .1
                    .iter()
                    .filter(|(_, call)| call.verdict != STARTED)
                    .map(|(trail, _)| trail.id.as_str())
                    .collect();
                Ok(rows
                    .1
                    .iter()
                    .rev()
                    .filter(|(trail, call)| {
                        trail.connector.as_deref() == Some(connector)
                            && (call.verdict != STARTED || !ended.contains(trail.id.as_str()))
                    })
                    .take(usize::try_from(CALLS_PAGE).unwrap_or(200))
                    .map(|(_, call)| call.clone())
                    .collect())
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A live connector with no scope, for another module's tests to name.
    pub(crate) fn sample() -> Connector {
        connector(&["context"], vec![])
    }

    fn op(name: &str) -> &'static Operation {
        crate::ops::find(name).expect("registered")
    }

    /// The space of no object: where a call's object sits in no Context Space.
    fn nowhere(_kind: &str, _name: &str) -> Option<String> {
        None
    }

    /// The fixture places nothing, whatever it is asked.
    #[test]
    fn nowhere_places_nothing() {
        assert_eq!(nowhere("Endpoint", "air"), None);
    }

    fn item(project: &str, endpoint: Option<&str>) -> ScopeItem {
        ScopeItem {
            project: project.into(),
            space: None,
            endpoint: endpoint.map(str::to_owned),
            pipeline: None,
            access: ItemAccess::Read,
            personal_data: false,
        }
    }

    /// T-6413: a scope of named Endpoints bounds a data tool's `endpoint`; one reaching more,
    /// or another project, does not.
    #[test]
    fn a_scope_of_named_endpoints_names_them() {
        let c = connector(
            &["data-read"],
            vec![
                item("helsinki", Some("air")),
                item("helsinki", Some("noise")),
                item("espoo", None),
            ],
        );
        assert_eq!(c.endpoints_named("helsinki"), Some(vec!["air", "noise"]));
        assert_eq!(c.endpoints_named("espoo"), None);
        assert_eq!(c.endpoints_named("vantaa"), None);
        let every = connector(&["data-read"], vec![item("helsinki", Some("*"))]);
        assert_eq!(every.endpoints_named("helsinki"), None);
    }

    fn connector(segments: &[&str], scope: Vec<ScopeItem>) -> Connector {
        Record {
            id: new_id(),
            owner: "sub-jana".into(),
            name: "c".into(),
            grant: GrantDoc {
                segments: segments.iter().map(|s| (*s).to_owned()).collect(),
                scope,
                kinds: None,
                writes: false,
                autonomous: false,
                expires_at: TimestampTz::now() + jiff::SignedDuration::from_hours(24),
                allowed_clients: vec![],
                pending: false,
                above: vec![],
                asked: vec![],
                approval: None,
                granted_at: None,
            },
            created_at: TimestampTz::now(),
            last_used_at: None,
            revoked_at: None,
        }
        .view("https://portal.test", TimestampTz::now())
    }

    /// T-6533: the operate segment pauses a SyncSource with `writes` on, a write item reaching it
    /// and `edit` on SyncSource; writes off, another project or read alone refuse it.
    #[test]
    fn the_operate_segment_pauses_a_syncsource_with_writes_alone() {
        let args = json!({ "name": "regional", "paused": true });
        let mut c = connector(&["operate"], vec![item("doprava", None)]);
        assert!(!c.access().names(op(PAUSE)), "writes off: no pause");
        assert!(c
            .admits(op(PAUSE), "doprava", &args, &|_, _| None)
            .expect_err("writes off")
            .contains("writes off"));
        c.writes = true;
        c.scope[0].access = ItemAccess::Write;
        assert!(c.access().names(op(PAUSE)));
        assert_eq!(c.admits(op(PAUSE), "doprava", &args, &|_, _| None), Ok(()));
        assert!(c.admits(op(PAUSE), "other", &args, &|_, _| None).is_err());
        let read = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(
            !read.access().names(op(PAUSE)),
            "no segment reaching SyncSource"
        );
        c.kinds = Some(vec![KindGrant {
            kind: "SyncSource".into(),
            verbs: vec![ConnectorVerb::Read],
        }]);
        assert!(c
            .admits(op(PAUSE), "doprava", &args, &|_, _| None)
            .expect_err("read alone")
            .contains("may not edit"));
    }

    /// T-6028: the pipeline segment samples, tries a mapping and reads runs; a run comes with
    /// `writes` alone and a write on the pipeline's item; a connector without the segment gets
    /// none of them, one narrowed to a pipeline reaches that pipeline alone, and no sample is
    /// read from a URL a call carries.
    #[test]
    fn the_pipeline_tools_are_the_segments_and_held_to_the_scope() {
        const TOOLS: [&str; 4] = [
            "jc_pipeline_sample_source",
            "jc_pipeline_try_mapping",
            "jc_pipeline_logs",
            RUN,
        ];
        let read = connector(&["data-read"], vec![item("doprava", None)]);
        for tool in TOOLS {
            assert!(!read.access().names(op(tool)), "{tool} without the segment");
        }
        let mut c = connector(&["pipeline"], vec![item("doprava", None)]);
        for tool in &TOOLS[..3] {
            assert!(c.access().names(op(tool)), "{tool}");
            assert_eq!(
                c.admits(op(tool), "doprava", &json!({ "name": "p" }), &|_, _| None),
                Ok(())
            );
            assert!(c
                .admits(op(tool), "other", &json!({ "name": "p" }), &|_, _| None)
                .is_err());
        }
        assert!(!c.access().names(op(RUN)), "writes off: no run");
        assert!(c
            .admits(op(RUN), "doprava", &json!({ "name": "p" }), &|_, _| None)
            .expect_err("writes off")
            .contains("writes off"));
        // T-6040: the test a Pipeline plan asks for is the segment's too, held to the same rule.
        for tool in [
            "jc_pipeline_sample_source",
            "jc_pipeline_try_mapping",
            "jc_pipeline_test",
        ] {
            assert!(c.access().names(op(tool)), "{tool}");
            let url = json!({ "sample": { "url": "http://10.0.0.5/a.csv", "format": "csv" } });
            assert!(c
                .admits(op(tool), "doprava", &url, &|_, _| None)
                .expect_err("a prompt's URL")
                .contains("never a URL"));
        }
        c.writes = true;
        c.scope[0].access = ItemAccess::Write;
        assert!(c.access().names(op(RUN)));
        assert_eq!(
            c.admits(op(RUN), "doprava", &json!({ "name": "p" }), &|_, _| None),
            Ok(())
        );
        let mut one = item("doprava", None);
        one.pipeline = Some("counters".into());
        one.access = ItemAccess::Write;
        let mut narrow = connector(&["pipeline"], vec![one]);
        narrow.writes = true;
        for tool in ["jc_pipeline_logs", RUN] {
            assert!(narrow.lists(op(tool), "doprava"), "{tool}");
            assert_eq!(
                narrow.admits(
                    op(tool),
                    "doprava",
                    &json!({ "name": "counters" }),
                    &|_, _| None
                ),
                Ok(())
            );
            assert!(narrow
                .admits(op(tool), "doprava", &json!({ "name": "other" }), &|_, _| {
                    None
                })
                .expect_err("outside the scope")
                .contains("other"));
        }
        // T-6363: sampling, trying and testing name their pipeline by `pipeline` or `draft`, so a
        // scope narrowed to one Pipeline offers them and holds each call to that Pipeline.
        let named = |name: &str| json!({ "kind": "Pipeline", "metadata": { "name": name } });
        let text = json!({ "text": "id\nc1\n", "format": "csv" });
        for tool in ["jc_pipeline_try_mapping", "jc_pipeline_test"] {
            assert!(narrow.lists(op(tool), "doprava"), "{tool}");
            for args in [
                json!({ "pipeline": named("counters"), "sample": text }),
                json!({ "draft": { "kind": "Pipeline", "name": "counters" }, "sample": text }),
            ] {
                assert_eq!(
                    narrow.admits(op(tool), "doprava", &args, &|_, _| None),
                    Ok(()),
                    "{tool} {args}"
                );
            }
            for args in [
                json!({ "pipeline": named("other"), "sample": text }),
                json!({ "draft": { "kind": "Pipeline", "name": "other" }, "sample": text }),
                json!({ "pipeline": named("counters"), "draft": { "kind": "Pipeline", "name": "other" }, "sample": text }),
                json!({ "draft": { "kind": "DataModel", "name": "counters" }, "sample": text }),
            ] {
                assert!(
                    narrow
                        .admits(op(tool), "doprava", &args, &|_, _| None)
                        .expect_err("outside the scope")
                        .contains("does not reach"),
                    "{tool} {args}"
                );
            }
            let unnamed = json!({ "pipeline": { "kind": "Pipeline" }, "sample": text });
            assert!(
                narrow
                    .admits(op(tool), "doprava", &unnamed, &|_, _| None)
                    .is_err(),
                "{tool}"
            );
            assert!(narrow
                .admits(
                    op(tool),
                    "other",
                    &json!({ "pipeline": named("counters") }),
                    &|_, _| None
                )
                .is_err());
        }
        let sample = op("jc_pipeline_sample_source");
        assert!(narrow.lists(sample, "doprava"));
        assert_eq!(
            narrow.admits(sample, "doprava", &json!({ "sample": text }), &|_, _| None),
            Ok(())
        );
        assert!(narrow
            .admits(
                sample,
                "doprava",
                &json!({ "dataSource": "counters-csv" }),
                &|_, _| None
            )
            .expect_err("a DataSource the scope does not name")
            .contains("DataSource counters-csv"));
        // A space item (T-6360) holds the Pipelines that sit in its space, and no other.
        let mut room = item("doprava", None);
        room.space = Some("counting".into());
        let spaced = connector(&["pipeline"], vec![room]);
        let in_space = |_: &str, name: &str| (name == "counters").then(|| "counting".to_owned());
        assert!(spaced.lists(op("jc_pipeline_test"), "doprava"));
        assert_eq!(
            spaced.admits(
                op("jc_pipeline_test"),
                "doprava",
                &json!({ "pipeline": named("counters") }),
                &in_space
            ),
            Ok(())
        );
        assert!(spaced
            .admits(
                op("jc_pipeline_test"),
                "doprava",
                &json!({ "pipeline": named("other") }),
                &in_space
            )
            .is_err());
        // A scope narrowed to an Endpoint offers no authoring step and admits none.
        let other = connector(
            &["pipeline"],
            vec![item("doprava", Some("doprava-counters"))],
        );
        for tool in STEPS {
            assert!(!other.lists(op(tool), "doprava"), "{tool}");
            assert!(other
                .admits(op(tool), "doprava", &json!({ "sample": text }), &nowhere)
                .is_err());
            assert!(other
                .admits(
                    op(tool),
                    "doprava",
                    &json!({ "pipeline": named("counters") }),
                    &|_, _| None
                )
                .is_err());
        }
    }

    /// T-6290: the pipeline segment reaches the project's MQTT brokers and reads a capture; a
    /// connector without the segment, or narrowed to one pipeline, reads none.
    #[test]
    fn the_pipeline_segment_reaches_mqtt_brokers_and_their_capture() {
        let captured = op(crate::ops::mqtt::CAPTURED);
        let list = json!({ "kind": "MqttBroker" });
        let c = connector(&["pipeline"], vec![item("doprava", None)]);
        assert!(c.access().names(captured));
        assert_eq!(
            c.admits(
                captured,
                "doprava",
                &json!({ "name": "sensors" }),
                &|_, _| None
            ),
            Ok(())
        );
        assert_eq!(
            c.admits(op("jc_resource_list"), "doprava", &list, &|_, _| None),
            Ok(())
        );
        let read = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(!read.access().names(captured));
        assert!(read
            .admits(op("jc_resource_list"), "doprava", &list, &|_, _| None)
            .is_err());
        let mut one = item("doprava", None);
        one.pipeline = Some("counters".into());
        let narrow = connector(&["pipeline"], vec![one]);
        assert!(narrow
            .admits(
                captured,
                "doprava",
                &json!({ "name": "sensors" }),
                &|_, _| None
            )
            .is_err());
    }

    /// JC2-28..34, T-6271, T-6389: each segment reaches the page reads its jobs need, and the
    /// entity write comes with `writes` alone, to a connector reaching Endpoint, held to the
    /// Endpoints its scope names.
    #[test]
    fn the_parity_reads_reach_their_segments_and_the_entity_write_needs_writes() {
        let reached = |segments: &[&str], writes: bool| {
            let mut c = connector(segments, vec![item("doprava", None)]);
            c.writes = writes;
            c.reach().0
        };
        for (segment, tools) in [
            ("data-read", &["jc_attribute_explain"][..]),
            (
                "context",
                &[
                    "jc_model_sdm_search",
                    "jc_model_sdm_import",
                    "jc_model_generate",
                    "jc_endpoint_preview",
                    "jc_space_governance",
                ][..],
            ),
            (
                "pipeline",
                &[
                    "jc_pipeline_inspect",
                    "jc_mqtt_test_publish",
                    "jc_space_governance",
                ][..],
            ),
            ("access", &["jc_access_simulate", "jc_access_explore"][..]),
            (
                "publish",
                &["jc_catalogue_draft", "jc_endpoint_preview"][..],
            ),
            ("operate", &["jc_change_history", "jc_pipeline_inspect"][..]),
        ] {
            let listed = reached(&[segment], false);
            for tool in tools {
                assert!(listed.contains(tool), "{segment} lists {tool}");
                assert!(crate::ops::find(tool).is_some(), "{tool} is an operation");
            }
        }
        assert!(!reached(&["data-read"], false).contains(&DATA_WRITE));
        assert!(reached(&["data-read"], true).contains(&DATA_WRITE));
        assert!(
            !reached(&["operate"], true).contains(&DATA_WRITE),
            "operate reaches no Endpoint"
        );

        let write = op(DATA_WRITE);
        let entity = json!({ "endpoint": "bikes", "action": "delete", "ids": ["urn:ngsi-ld:T:1"] });
        let mut open = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(
            open.admits(write, "doprava", &entity, &|_, _| None)
                .is_err(),
            "no writes switch"
        );
        open.writes = true;
        assert!(open
            .admits(write, "doprava", &entity, &|_, _| None)
            .expect_err("a read item")
            .contains("only reads"),);
        open.scope[0].access = ItemAccess::Write;
        assert_eq!(open.admits(write, "doprava", &entity, &|_, _| None), Ok(()));
        let mut one = item("doprava", Some("counters"));
        one.access = ItemAccess::Write;
        let mut narrow = connector(&["data-read"], vec![one]);
        narrow.writes = true;
        assert!(
            narrow
                .admits(write, "doprava", &entity, &|_, _| None)
                .is_err(),
            "another Endpoint"
        );
    }

    /// T-6360: a space item reaches every object whose `contextSpaceRef` names the space, for
    /// reads and plans, and no object of another space, of no space or moved across spaces.
    #[test]
    fn a_space_item_reaches_the_objects_of_its_space_and_no_other() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let mut one = item("doprava", None);
        one.space = Some("traffic".into());
        let c = connector(&["data-read", "context"], vec![one]);
        let mirror = crate::store::Mirror::new();
        for (kind, name, space) in [
            ("DataModel", "bikes", Some("traffic")),
            ("Endpoint", "counters", Some("traffic")),
            ("Policy", "readers", Some("traffic")),
            ("DataModel", "parks", Some("green")),
            ("Endpoint", "trees", Some("green")),
            ("Policy", "gardeners", Some("green")),
            ("Policy", "loose", None),
        ] {
            mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta {
                    name: name.to_owned(),
                    namespace: Some("doprava".to_owned()),
                    ..Default::default()
                },
                spec: space.map_or(json!({}), |s| json!({ "contextSpaceRef": { "name": s } })),
                status: None,
            });
        }
        let space_of = |kind: &str, name: &str| space_now(&mirror, "doprava", kind, name);
        let get = |kind: &str, name: &str| json!({ "kind": kind, "name": name });
        for (kind, inside, outside) in [
            ("DataModel", "bikes", "parks"),
            ("Endpoint", "counters", "trees"),
            ("Policy", "readers", "gardeners"),
        ] {
            assert_eq!(
                c.admits(
                    op("jc_resource_get"),
                    "doprava",
                    &get(kind, inside),
                    &space_of
                ),
                Ok(()),
                "{kind} {inside}"
            );
            let refused = c
                .admits(
                    op("jc_resource_get"),
                    "doprava",
                    &get(kind, outside),
                    &space_of,
                )
                .expect_err("another space");
            assert!(refused.contains(outside), "{refused}");
            assert_eq!(
                c.may_change(
                    "doprava",
                    kind,
                    inside,
                    space_of(kind, inside).as_deref(),
                    false
                ),
                Ok(())
            );
            assert!(c
                .may_change(
                    "doprava",
                    kind,
                    outside,
                    space_of(kind, outside).as_deref(),
                    false
                )
                .is_err());
        }
        assert!(c
            .admits(
                op("jc_resource_get"),
                "doprava",
                &get("Policy", "loose"),
                &space_of
            )
            .is_err());
        assert!(c
            .admits(
                op("jc_resource_get"),
                "doprava",
                &get("DataModel", "gone"),
                &space_of
            )
            .is_err());
        // A data query is held to its Endpoint's space.
        let query = |endpoint: &str| json!({ "endpoint": endpoint, "type": "Counter" });
        assert_eq!(
            c.admits(
                op("jc_data_query"),
                "doprava",
                &query("counters"),
                &space_of
            ),
            Ok(())
        );
        assert!(c
            .admits(op("jc_data_query"), "doprava", &query("trees"), &space_of)
            .is_err());
        // The space itself, and no other space.
        assert_eq!(
            c.admits(
                op("jc_resource_get"),
                "doprava",
                &get("ContextSpace", "traffic"),
                &space_of
            ),
            Ok(())
        );
        assert!(c
            .admits(
                op("jc_resource_get"),
                "doprava",
                &get("ContextSpace", "green"),
                &space_of
            )
            .is_err());
    }

    /// T-6360: a change sits in a space when every end that exists names that one space.
    #[test]
    fn a_change_sits_in_a_space_only_when_both_ends_name_it() {
        let at = |space: &str| json!({ "spec": { "contextSpaceRef": space } });
        let bare = json!({ "spec": {} });
        let traffic = Some("traffic".to_owned());
        assert_eq!(change_space(None, Some(&at("traffic"))), traffic, "create");
        assert_eq!(change_space(Some(&at("traffic")), None), traffic, "delete");
        assert_eq!(
            change_space(Some(&at("traffic")), Some(&at("traffic"))),
            traffic
        );
        assert_eq!(
            change_space(Some(&at("green")), Some(&at("traffic"))),
            None,
            "move in"
        );
        assert_eq!(
            change_space(Some(&at("traffic")), Some(&at("green"))),
            None,
            "move out"
        );
        assert_eq!(change_space(Some(&bare), Some(&at("traffic"))), None);
        assert_eq!(change_space(Some(&at("traffic")), Some(&bare)), None);
        assert_eq!(change_space(None, None), None);
    }

    /// T-6487: a build comes with `writes` on a segment reaching App, and needs `write` on an
    /// item of the whole project and `create` on App.
    #[test]
    fn a_build_is_held_to_writes_the_verbs_and_the_scope() {
        let call = json!({ "appName": "todays-counters", "endpointName": "counters" });
        let space_of = |_: &str, _: &str| None;
        let mut c = connector(&["apps"], vec![item("doprava", None)]);
        assert!(!c.access().names(op(BUILD)), "writes off: no build");
        assert!(c
            .admits(op(BUILD), "doprava", &call, &space_of)
            .expect_err("writes off")
            .contains("writes off"));
        c.writes = true;
        assert!(c.access().names(op(BUILD)));
        assert!(c
            .admits(op(BUILD), "doprava", &call, &space_of)
            .expect_err("read item")
            .contains("only reads"));
        c.scope[0].access = ItemAccess::Write;
        assert_eq!(c.admits(op(BUILD), "doprava", &call, &space_of), Ok(()));
        assert!(c
            .admits(op(BUILD), "doprava", &json!({}), &space_of)
            .expect_err("no App named")
            .contains("appName"));
        c.kinds = Some(vec![KindGrant {
            kind: "App".into(),
            verbs: vec![ConnectorVerb::Read],
        }]);
        assert!(c
            .admits(op(BUILD), "doprava", &call, &space_of)
            .expect_err("read only on App")
            .contains("App todays-counters"));
        c.kinds = None;
        // A connector narrowed to one Endpoint starts no run: a run would read past the item.
        let mut narrowed = item("doprava", Some("counters"));
        narrowed.access = ItemAccess::Write;
        c.scope = vec![narrowed];
        assert!(c
            .admits(op(BUILD), "doprava", &call, &space_of)
            .expect_err("narrowed")
            .contains("does not reach App"));
        // A segment without App offers no build, whatever the switch.
        let mut data = connector(&["data-read"], vec![item("doprava", None)]);
        data.writes = true;
        assert!(!data.access().names(op(BUILD)));
    }

    /// T-6487: a policy listing Endpoints or kinds holds a build the same way: a run names its
    /// Endpoints by other members than `endpoint`, which the ceiling read alone.
    #[test]
    fn a_policy_holds_a_builds_endpoints_and_its_verb() {
        let policy = |spec: Value| -> DelegationPolicySpec {
            serde_json::from_value(spec).expect("a policy")
        };
        let mut base = json!({
            "subjects": [{ "kind": "Group", "name": "g" }],
            "projects": ["doprava"],
            "segments": ["apps"],
            "writes": true,
            "maxExpiry": "P90D",
        });
        let open = policy(base.clone());
        let call = json!({ "appName": "a", "endpointName": "counters" });
        assert!(policy_builds(&open, &call));
        base["endpoints"] = json!(["counters"]);
        let listed = policy(base.clone());
        assert!(policy_builds(&listed, &call));
        assert!(!policy_builds(
            &listed,
            &json!({ "appName": "a", "endpointName": "permits" })
        ));
        assert!(
            !policy_builds(&listed, &json!({ "appName": "a" })),
            "no Endpoint named"
        );
        base.as_object_mut().expect("object").remove("endpoints");
        base["kinds"] = json!([{ "kind": "App", "verbs": ["read"] }]);
        assert!(!policy_builds(&policy(base), &call));
    }

    /// T-5956, AG-149: a segment that proposes plans; only `writes` brings the apply, and an
    /// apply needs `write` on the item; a plan stays within the segments' kinds and the scope.
    #[test]
    fn a_plan_is_held_to_the_scope_and_an_apply_to_the_writes_switch() {
        let mut c = connector(&["context"], vec![item("doprava", None)]);
        let access = c.access();
        assert!(access.names(op(PLAN)));
        assert!(!access.names(op(APPLY)), "writes off: no apply");
        assert_eq!(
            c.may_change("doprava", "DataModel", "bikes", None, false),
            Ok(())
        );
        assert!(c
            .may_change("doprava", "DataModel", "bikes", None, true)
            .expect_err("writes off")
            .contains("writes off"));
        assert!(c
            .may_change("doprava", "Pipeline", "p", None, false)
            .expect_err("not a context kind")
            .contains("not Pipeline"));
        assert!(c
            .may_change("other", "DataModel", "bikes", None, false)
            .is_err());
        c.writes = true;
        assert!(c.access().names(op(APPLY)));
        assert!(c
            .may_change("doprava", "DataModel", "bikes", None, true)
            .expect_err("read item")
            .contains("only reads"));
        c.scope[0].access = ItemAccess::Write;
        assert_eq!(
            c.may_change("doprava", "DataModel", "bikes", None, true),
            Ok(())
        );
        // A data-read connector plans nothing.
        let read = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(!read.access().names(op(PLAN)));
        // Narrowed to one Endpoint: that Endpoint alone.
        let narrow = connector(&["context"], vec![item("doprava", Some("counters"))]);
        assert!(narrow.lists(op(PLAN), "doprava"));
        assert_eq!(
            narrow.may_change("doprava", "Endpoint", "counters", None, false),
            Ok(())
        );
        assert!(narrow
            .may_change("doprava", "Endpoint", "other", None, false)
            .is_err());
    }

    fn new(segments: &[&str], writes: bool, days: i64) -> NewConnector {
        NewConnector {
            name: "doprava-in-claude".into(),
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
            scope: vec![item("doprava", None)],
            kinds: None,
            writes,
            autonomous: false,
            expires_at: TimestampTz::now() + jiff::SignedDuration::from_hours(24 * days),
            allowed_clients: vec![],
        }
    }

    #[test]
    fn every_preset_lists_registered_read_only_tools_snapshot() {
        let listed = |segment: &str| -> Vec<&'static str> {
            let c = connector(&[segment], vec![item("doprava", None)]);
            let access = c.access();
            crate::ops::registry()
                .iter()
                .filter(|op| access.names(op))
                .map(|op| op.name)
                .collect()
        };
        for (segment, tools, _) in SEGMENTS {
            for tool in *tools {
                let op = op(tool);
                // A plan, a revert plan included, saves nothing but is offered only to who may
                // propose (T-5956, T-5974).
                assert!(
                    op.annotations.read_only_hint || op.name == PLAN || op.name == REVERT,
                    "{segment}: {tool} is not read-only"
                );
            }
            let mut want: Vec<&str> = tools.to_vec();
            want.sort_unstable();
            let mut got = listed(segment);
            got.sort_unstable();
            assert_eq!(got, want, "{segment}");
        }
        // The snapshot of the default preset, so a change to it is a reviewed diff.
        let mut data_read = listed("data-read");
        data_read.sort_unstable();
        assert_eq!(
            data_read,
            [
                "jc_attribute_explain",
                "jc_catalog_search",
                "jc_data_query",
                "jc_kpi_compute",
                "jc_render",
                "jc_resource_get",
                "jc_resource_list",
                "jc_schema_get"
            ]
        );
        // admin is every segment, and still no write, approval or cross-project tool.
        let admin = listed(ADMIN);
        for absent in [
            "jc_resource_propose",
            "jc_change_approve",
            "jc_person_list",
            "jc_endpoint_list_all",
        ] {
            assert!(!admin.contains(&absent), "admin lists {absent}");
        }
    }

    #[test]
    fn a_call_is_held_to_the_project_the_kind_and_the_named_object() {
        let whole = connector(&["data-read"], vec![item("doprava", None)]);
        let get = op("jc_resource_get");
        let list = op("jc_resource_list");
        assert_eq!(
            whole.admits(list, "doprava", &json!({ "kind": "Endpoint" }), &nowhere),
            Ok(())
        );
        let err = whole
            .admits(list, "air", &json!({ "kind": "Endpoint" }), &|_, _| None)
            .expect_err("other project");
        assert!(err.contains("does not reach project air"), "{err}");
        // data-read reads no Role, whatever the person may read.
        let err = whole
            .admits(
                get,
                "doprava",
                &json!({ "kind": "Role", "name": "x" }),
                &|_, _| None,
            )
            .expect_err("kind");
        assert!(err.contains("read only"), "{err}");

        let narrow = connector(&["data-read"], vec![item("doprava", Some("counters"))]);
        let named = json!({ "kind": "Endpoint", "name": "counters" });
        assert_eq!(narrow.admits(get, "doprava", &named, &|_, _| None), Ok(()));
        let err = narrow
            .admits(
                get,
                "doprava",
                &json!({ "kind": "Endpoint", "name": "parking" }),
                &|_, _| None,
            )
            .expect_err("unticked Endpoint");
        assert!(err.contains("does not reach Endpoint parking"), "{err}");
        // A list cannot be held to the named Endpoints, so a narrowed project refuses it and
        // tools/list does not offer it.
        assert!(narrow
            .admits(list, "doprava", &json!({ "kind": "Endpoint" }), &nowhere)
            .is_err());
        assert!(!narrow.lists(list, "doprava"));
        assert!(narrow.lists(get, "doprava"));
        assert!(narrow.lists(op("jc_kpi_compute"), "doprava"));
        // A catalogue search matches every object of the project: a whole project lists and
        // runs it, another project and a narrowed item do neither (T-6194).
        let search = op("jc_catalog_search");
        let q = json!({ "q": "schools" });
        assert!(whole.lists(search, "doprava"));
        assert_eq!(whole.admits(search, "doprava", &q, &|_, _| None), Ok(()));
        assert!(!whole.lists(search, "air"));
        assert!(whole.admits(search, "air", &q, &|_, _| None).is_err());
        assert!(!narrow.lists(search, "doprava"));
        assert!(narrow.admits(search, "doprava", &q, &|_, _| None).is_err());
        // query_data names its Endpoint by `endpoint`: the ticked one reads, another is refused.
        let query = op("jc_data_query");
        assert!(narrow.lists(query, "doprava"));
        assert_eq!(
            narrow.admits(
                query,
                "doprava",
                &json!({ "endpoint": "counters" }),
                &|_, _| None
            ),
            Ok(())
        );
        let err = narrow
            .admits(
                query,
                "doprava",
                &json!({ "endpoint": "parking" }),
                &|_, _| None,
            )
            .expect_err("unticked Endpoint");
        assert!(err.contains("does not reach Endpoint parking"), "{err}");
        assert!(narrow
            .admits(query, "doprava", &json!({}), &|_, _| None)
            .is_err());
        // An indicator's `name` is its own: only `endpoint` is what it reads (T-6484).
        let kpi = op("jc_kpi_compute");
        assert_eq!(
            narrow.admits(
                kpi,
                "doprava",
                &json!({ "name": "daily", "endpoint": "counters" }),
                &|_, _| None
            ),
            Ok(())
        );
        for args in [
            json!({ "name": "counters", "endpoint": "parking" }),
            json!({ "name": "counters", "rows": [] }),
        ] {
            assert!(narrow.admits(kpi, "doprava", &args, &|_, _| None).is_err());
        }
        assert!(!narrow.lists(get, "air"));
        assert_eq!(
            narrow.admits(
                op("jc_schema_get"),
                "doprava",
                &json!({ "kind": "Endpoint" }),
                &|_, _| None
            ),
            Ok(())
        );
        // A DataModel is no object a narrowed item names.
        assert!(narrow
            .admits(
                get,
                "doprava",
                &json!({ "kind": "DataModel", "name": "counters" }),
                &|_, _| None
            )
            .is_err());
        let star = connector(&["data-read"], vec![item("doprava", Some("*"))]);
        assert_eq!(
            star.admits(
                get,
                "doprava",
                &json!({ "kind": "Endpoint", "name": "any" }),
                &|_, _| None
            ),
            Ok(())
        );
    }

    #[test]
    fn the_default_ceiling_refuses_more_than_data_read_for_thirty_days() {
        let now = TimestampTz::now();
        assert!(validate(
            &new(&["data-read"], false, 29),
            &Ceiling::of_policies(vec![]),
            now
        )
        .is_ok());
        let denied = |n: NewConnector, field: &str| {
            let err = validate(&n, &Ceiling::of_policies(vec![]), now).expect_err(field);
            assert!(
                matches!(&err, ApiError::Denied(why) if why.starts_with(field)),
                "{field}: {err:?}"
            );
        };
        denied(new(&["pipeline"], false, 1), "segments[0]");
        denied(new(&["data-read"], true, 1), "writes");
        denied(new(&["data-read"], false, 31), "expiresAt");
        let invalid = |n: NewConnector, field: &str| {
            let err = validate(&n, &Ceiling::of_policies(vec![]), now).expect_err(field);
            assert!(
                matches!(&err, ApiError::Invalid { errors, .. } if *errors == vec![field.to_owned()]),
                "{field}: {err:?}"
            );
        };
        invalid(new(&["everything"], false, 1), "segments[0]");
        invalid(new(&[], false, 1), "segments");
        invalid(new(&["data-read"], false, -1), "expiresAt");
        let mut bad = new(&["data-read"], false, 1);
        bad.name = "Doprava In Claude".into();
        invalid(bad, "name");
        let mut bad = new(&["data-read"], false, 1);
        bad.scope = vec![];
        invalid(bad, "scope");
        let mut bad = new(&["data-read"], false, 1);
        bad.scope[0].endpoint = Some("a".into());
        bad.scope[0].space = Some("b".into());
        invalid(bad, "scope[0]");
        let mut bad = new(&["data-read"], false, 1);
        bad.scope[0].access = ItemAccess::Write;
        invalid(bad, "scope[0].access");
        let mut bad = new(&["data-read"], false, 1);
        bad.allowed_clients = vec!["evil\nclient".into()];
        invalid(bad, "allowedClients[0]");
    }

    #[test]
    fn a_personal_data_tick_needs_a_ceiling_that_allows_it() {
        let now = TimestampTz::now();
        let mut ticked = new(&["data-read"], false, 1);
        ticked.scope[0].personal_data = true;
        let refused = validate(&ticked, &Ceiling::of_policies(Vec::new()), now)
            .expect_err("the default ceiling keeps personal data out");
        assert!(
            matches!(&refused, ApiError::Denied(why) if why.starts_with("scope[0].personalData")),
            "{refused:?}"
        );
        let allows = Ceiling {
            personal_data: true,
            ..Ceiling::of_policies(Vec::new())
        };
        assert!(validate(&ticked, &allows, now).is_ok());
    }

    #[test]
    fn personal_attributes_show_only_through_a_ticked_item_that_reaches_the_endpoint() {
        let ticked = |endpoint: Option<&str>| ScopeItem {
            personal_data: true,
            ..item("doprava", endpoint)
        };
        let args = |endpoint: &str| json!({ "project": "doprava", "endpoint": endpoint });
        let plain = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(!plain.shows_personal("doprava", &args("hlasenia")));
        let whole = connector(&["data-read"], vec![ticked(None)]);
        assert!(whole.shows_personal("doprava", &args("hlasenia")));
        assert!(!whole.shows_personal("skolstvo", &args("hlasenia")));
        let one = connector(
            &["data-read"],
            vec![ticked(Some("hlasenia")), item("doprava", Some("scitace"))],
        );
        assert!(one.shows_personal("doprava", &args("hlasenia")));
        assert!(!one.shows_personal("doprava", &args("scitace")));
        assert!(!one.shows_personal("doprava", &json!({ "project": "doprava" })));
        let space = connector(
            &["data-read"],
            vec![ScopeItem {
                space: Some("doprava".into()),
                ..ticked(None)
            }],
        );
        assert!(!space.shows_personal("doprava", &args("hlasenia")));
    }

    #[test]
    fn only_published_vendor_clients_bind() {
        assert!(binds("https://claude.ai/oauth/mcp-client-metadata"));
        assert!(binds("https://chatgpt.com/connector/oauth/client.json"));
        assert!(!binds(
            "https://claude.ai/oauth/claude-code-client-metadata"
        ));
        assert!(!binds("mcp-local"));
        assert!(!binds("http://claude.ai/x"));
        assert!(!binds("https://claude.ai.evil.example/x"));
    }

    #[test]
    fn ids_are_26_base32_characters() {
        let id = new_id();
        assert!(is_id(&id), "{id}");
        assert_ne!(id, new_id());
        assert!(!is_id("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
        assert!(!is_id("a"));
        assert!(!is_id("abcdefghijklmnopqrstuvwxy1"));
    }

    #[test]
    fn an_expired_or_revoked_connector_is_no_longer_active() {
        let mut record = Record {
            id: new_id(),
            owner: "o".into(),
            name: "n".into(),
            grant: GrantDoc {
                segments: vec!["data-read".into()],
                scope: vec![item("doprava", None)],
                kinds: None,
                writes: false,
                autonomous: false,
                expires_at: TimestampTz::now() - jiff::SignedDuration::from_secs(1),
                allowed_clients: vec![],
                pending: false,
                above: vec![],
                asked: vec![],
                approval: None,
                granted_at: None,
            },
            created_at: TimestampTz::now(),
            last_used_at: None,
            revoked_at: None,
        };
        let view = record.view("https://portal.test/", TimestampTz::now());
        assert_eq!((view.state.as_str(), view.is_active()), ("expired", false));
        assert_eq!(view.url, format!("https://portal.test/c/{}", record.id));
        record.revoked_at = Some(TimestampTz::now());
        assert_eq!(
            record.view("https://portal.test", TimestampTz::now()).state,
            "revoked"
        );
    }

    #[tokio::test]
    async fn the_memory_store_keeps_twenty_live_connectors_one_name_each_and_their_calls() {
        let store = ConnectorStore::new(None, None);
        let base = "https://portal.test";
        let mut first = None;
        for n in 0..MAX_PER_PERSON {
            let mut ask = new(&["data-read"], false, 1);
            ask.name = format!("c{n}");
            let made = store
                .create("jana", ask, Default::default(), base)
                .await
                .expect("created");
            first.get_or_insert(made.id);
        }
        let mut ask = new(&["data-read"], false, 1);
        ask.name = "one-more".into();
        assert!(matches!(
            store.create("jana", ask, Default::default(), base).await,
            Err(ApiError::Conflict(_))
        ));
        // Another person has their own twenty; a revoke frees a place and keeps the record.
        let mut ask = new(&["data-read"], false, 1);
        ask.name = "c0".into();
        assert!(store
            .create("petra", ask, Default::default(), base)
            .await
            .is_ok());
        let first = first.expect("one made");
        assert!(
            !store.revoke(&first, "petra").await.expect("revoke"),
            "not petra's"
        );
        assert!(store.revoke(&first, "jana").await.expect("revoke"));
        assert!(!store.revoke(&first, "jana").await.expect("revoke"), "once");
        let mut ask = new(&["data-read"], false, 1);
        ask.name = "c0".into();
        assert!(
            store
                .create("jana", ask, Default::default(), base)
                .await
                .is_ok(),
            "the name of a revoked one is free"
        );
        assert_eq!(
            store.of_owner("jana", base).await.expect("list").len(),
            MAX_PER_PERSON + 1
        );

        let call = |verdict: &str| Call {
            at: TimestampTz::now(),
            subject: "jana".into(),
            client: Some("mcp-local".into()),
            tool: "tools/list".into(),
            target: None,
            verdict: verdict.into(),
            duration_ms: 3,
        };
        store.record(&first, call("allowed")).await.expect("kept");
        store
            .record(&first, call("refused: revoked"))
            .await
            .expect("kept");
        let calls = store.calls(&first).await.expect("calls");
        assert_eq!(
            calls.iter().map(|c| c.verdict.as_str()).collect::<Vec<_>>(),
            ["refused: revoked", "allowed"]
        );
        let used = store.get(&first, base).await.expect("get").expect("there");
        assert!(used.last_used_at.is_some());
    }

    /// A ceiling and the mirror it is read from: Group `analysts` (jana is a member) bound to
    /// project doprava, data-read and pipeline, Endpoint counters, 90 days, claude.ai, queued.
    fn analysts(queue: &str) -> (crate::store::Mirror, crate::auth::session::Identity) {
        let mirror = crate::store::Mirror::new();
        let envelope = |kind: &str, name: &str, spec: Value| crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        };
        mirror.upsert(envelope(
            "Group",
            "analysts",
            json!({ "members": [{ "user": "jana@hel.fi" }] }),
        ));
        mirror.upsert(envelope(
            "DelegationPolicy",
            "analysts-doprava",
            json!({
                "subjects": [{ "kind": "Group", "name": "analysts" }],
                "projects": ["doprava"],
                "segments": ["data-read", "pipeline"],
                "endpoints": ["counters"],
                "maxExpiry": "P90D",
                "allowedClients": [CLAUDE],
                "aboveCeiling": queue,
            }),
        ));
        // A manifest its kind refuses (`*` in projects) binds nobody, and never widens.
        mirror.upsert(envelope(
            "DelegationPolicy",
            "broken",
            json!({ "subjects": [{ "kind": "User", "id": "sub-jana" }], "projects": ["*"],
                    "segments": ["admin"], "writes": true }),
        ));
        let identity = crate::auth::session::Identity {
            client: None,
            subject: "sub-jana".into(),
            username: "jana".into(),
            email: Some("jana@hel.fi".into()),
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        };
        (mirror, identity)
    }

    const CLAUDE: &str = "https://claude.ai/oauth/mcp-client-metadata";

    fn ask(segments: &[&str], endpoint: Option<&str>, days: i64) -> NewConnector {
        let mut ask = new(segments, false, days);
        ask.scope = vec![item("doprava", endpoint)];
        ask.allowed_clients = vec![CLAUDE.into()];
        ask
    }

    /// F-R2POR3-005: a Group member written as a Keycloak id (ADR-N-061) is the person whose
    /// `sub` it is, so the group's narrower policy binds them, not the wider default.
    #[test]
    fn a_group_member_written_as_an_id_binds_its_person() {
        const ID: &str = "0b6f0e4c-1d2e-4f00-9a7b-3c4d5e6f7a8b";
        let (mirror, mut jana) = analysts("refuse");
        mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Group".into(),
            metadata: crate::resource::ObjectMeta::new("analysts", ORG_NAMESPACE),
            spec: json!({ "members": [{ "user": ID }] }),
            status: None,
        });
        jana.subject = ID.into();
        let ceiling = Ceiling::of(&mirror, &jana);
        assert!(
            ceiling.named().contains("analysts-doprava"),
            "{}",
            ceiling.named()
        );
    }

    #[test]
    fn a_delegation_policy_binds_by_group_and_bounds_each_field() {
        let now = TimestampTz::now();
        let (mirror, jana) = analysts("refuse");
        let ceiling = Ceiling::of(&mirror, &jana);
        assert!(
            ceiling.named().contains("analysts-doprava"),
            "{}",
            ceiling.named()
        );
        assert!(!ceiling.named().contains("broken"));
        assert_eq!(ceiling.segments(), vec!["data-read", "pipeline"]);
        assert!(
            validate(&ask(&["pipeline"], Some("counters"), 89), &ceiling, now)
                .is_ok_and(|above| above.is_empty())
        );
        let refused = |n: NewConnector, field: &str| {
            let err = validate(&n, &ceiling, now).expect_err(field);
            assert!(
                matches!(&err, ApiError::Denied(why)
                    if why.starts_with(field) && why.contains("analysts-doprava")),
                "{field}: {err:?}"
            );
        };
        refused(ask(&["access"], Some("counters"), 1), "segments[0]");
        refused(ask(&["data-read"], None, 1), "scope[0].endpoint");
        refused(ask(&["data-read"], Some("other"), 1), "scope[0].endpoint");
        refused(ask(&["data-read"], Some("counters"), 91), "expiresAt");
        let mut other = ask(&["data-read"], Some("counters"), 1);
        other.scope[0].project = "skolstvo".into();
        refused(other, "scope[0].project");
        let mut writes = ask(&["data-read"], Some("counters"), 1);
        writes.writes = true;
        refused(writes, "writes");
        let mut client = ask(&["data-read"], Some("counters"), 1);
        client.allowed_clients = vec!["mcp-local".into()];
        refused(client, "allowedClients[0]");
        let mut none = ask(&["data-read"], Some("counters"), 1);
        none.allowed_clients = vec![];
        refused(none, "allowedClients");
        // 365 days is the hard limit, whatever a policy says.
        assert!(matches!(
            validate(&ask(&["data-read"], Some("counters"), 366), &ceiling, now),
            Err(ApiError::Invalid { .. })
        ));
        // Someone no policy names gets the organization default.
        let mut petra = jana.clone();
        petra.subject = "sub-petra".into();
        petra.email = Some("petra@hel.fi".into());
        petra.username = "petra".into();
        let default = Ceiling::of(&mirror, &petra);
        assert_eq!(default.segments(), vec!["data-read"]);
        assert!(default.named().contains("organization default"));
        assert!(default.allows_project_name("skolstvo") && !default.writes() && !default.queues());
        assert_eq!(default.max_days(), 30);
        // Someone only a broken policy names gets nothing, not the wider default.
        let mut eva = petra.clone();
        eva.subject = "sub-eva".into();
        mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "DelegationPolicy".into(),
            metadata: crate::resource::ObjectMeta::new("eva-broken", ORG_NAMESPACE),
            spec: json!({ "subjects": [{ "kind": "User", "id": "sub-eva" }], "projects": [] }),
            status: None,
        });
        let nothing = Ceiling::of(&mirror, &eva);
        assert!(nothing.segments().is_empty() && !nothing.allows_project_name("doprava"));
        assert!(
            nothing.named().contains("eva-broken"),
            "{}",
            nothing.named()
        );
        let refused = validate(&new(&["data-read"], false, 1), &nothing, now)
            .expect_err("a broken policy allows nothing");
        assert!(
            matches!(&refused, ApiError::Denied(why) if why.starts_with("segments[0]")),
            "{refused:?}"
        );
    }

    #[test]
    fn a_queued_policy_keeps_the_request_with_what_it_asks_above() {
        let (mirror, jana) = analysts("queue");
        let ceiling = Ceiling::of(&mirror, &jana);
        let mut writes = ask(&["data-read"], Some("counters"), 1);
        writes.writes = true;
        let above = validate(&writes, &ceiling, TimestampTz::now()).expect("queued");
        assert_eq!(above.lines.len(), 1);
        assert!(above.lines[0].starts_with("writes: "), "{above:?}");
        // A field out of shape is still refused, never queued.
        let mut bad = ask(&["data-read"], Some("counters"), 1);
        bad.name = "Bad Name".into();
        assert!(matches!(
            validate(&bad, &ceiling, TimestampTz::now()),
            Err(ApiError::Invalid { .. })
        ));
    }

    /// T-6281: an event-kind filter on the activity list is no resource kind, so the operate
    /// segment reads `pipeline.error` events; a resource kind outside the segments stays refused.
    #[test]
    fn an_activity_kind_filter_is_held_to_the_project_alone() {
        let ceiling = Ceiling::of_policies(vec![(
            "operators-doprava".into(),
            serde_json::from_value(json!({
                "subjects": [{ "kind": "Group", "name": "operators" }],
                "projects": ["doprava"],
                "segments": ["operate", "data-read"],
            }))
            .expect("policy"),
        )]);
        let live = connector(&["operate", "data-read"], vec![item("doprava", None)]);
        let events = json!({ "kind": "pipeline.error,pipeline.run", "severity": "error" });
        let activity = op("jc_activity_list");
        assert_eq!(ceiling.admits(&live, activity, "doprava", &events), Ok(()));
        assert_eq!(
            live.admits(activity, "doprava", &events, &|_, _| None),
            Ok(())
        );
        assert!(ceiling
            .admits(&live, activity, "skolstvo", &events)
            .is_err());
        let secret = json!({ "kind": "Secret" });
        let list = op("jc_resource_list");
        let why = ceiling
            .admits(&live, list, "doprava", &secret)
            .expect_err("Secret is in no segment");
        assert!(why.contains("on Secret"), "{why}");
        assert!(live.admits(list, "doprava", &secret, &|_, _| None).is_err());
    }

    /// T-6288: a steward's Subscription and ModelProjection manifests are project configuration
    /// the context segment lists and plans (pipeline too for Subscription); data-read still reads
    /// neither, and a policy whose kinds leave them out refuses them.
    #[test]
    fn context_reaches_subscriptions_and_model_projections_within_the_ceiling() {
        let list = op("jc_resource_list");
        let plan = op(PLAN);
        let context = connector(&["context"], vec![item("doprava", None)]);
        let pipeline = connector(&["pipeline"], vec![item("doprava", None)]);
        let reader = connector(&["data-read"], vec![item("doprava", None)]);
        let open = Ceiling::of_policies(vec![(
            "stewards-doprava".into(),
            serde_json::from_value(json!({
                "subjects": [{ "kind": "Group", "name": "stewards" }],
                "projects": ["doprava"],
                "segments": ["context", "pipeline", "data-read"],
            }))
            .expect("policy"),
        )]);
        for kind in ["Subscription", "ModelProjection"] {
            let args = json!({ "kind": kind });
            assert_eq!(
                context.admits(list, "doprava", &args, &|_, _| None),
                Ok(()),
                "{kind}"
            );
            assert_eq!(
                context.admits(plan, "doprava", &args, &|_, _| None),
                Ok(()),
                "{kind}"
            );
            assert_eq!(
                open.admits(&context, list, "doprava", &args),
                Ok(()),
                "{kind}"
            );
            assert_eq!(
                open.admits(&context, plan, "doprava", &args),
                Ok(()),
                "{kind}"
            );
            let why = reader
                .admits(list, "doprava", &args, &|_, _| None)
                .expect_err("data-read reads no manifest of the kind");
            assert!(why.contains("read only"), "{why}");
        }
        let subscription = json!({ "kind": "Subscription" });
        assert_eq!(
            pipeline.admits(plan, "doprava", &subscription, &|_, _| None),
            Ok(())
        );
        assert!(pipeline
            .admits(
                list,
                "doprava",
                &json!({ "kind": "ModelProjection" }),
                &|_, _| None
            )
            .is_err());
        // The ceiling still decides per kind: a policy naming only Endpoint refuses both.
        let endpoints_only = Ceiling::of_policies(vec![(
            "stewards-endpoints".into(),
            serde_json::from_value(json!({
                "subjects": [{ "kind": "Group", "name": "stewards" }],
                "projects": ["doprava"],
                "segments": ["context"],
                "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }],
            }))
            .expect("policy"),
        )]);
        for kind in ["Subscription", "ModelProjection"] {
            let why = endpoints_only
                .admits(&context, list, "doprava", &json!({ "kind": kind }))
                .expect_err("kind outside the policy");
            assert!(why.contains(&format!("on {kind}")), "{why}");
        }
    }

    #[test]
    fn the_ceiling_narrows_a_live_connector_at_its_next_call() {
        let (mirror, jana) = analysts("refuse");
        let ceiling = Ceiling::of(&mirror, &jana);
        let mut live = connector(&["pipeline"], vec![item("doprava", Some("counters"))]);
        live.allowed_clients = vec![CLAUDE.into()];
        let metrics = op("jc_pipeline_metrics");
        let get = op("jc_resource_get");
        let counters = json!({ "kind": "Endpoint", "name": "counters" });
        assert_eq!(ceiling.admits(&live, get, "doprava", &counters), Ok(()));
        assert!(ceiling.lists(&live, metrics, "doprava"));
        assert!(ceiling.narrowing(&live).is_empty());
        assert_eq!(ceiling.door(&live, CLAUDE, TimestampTz::now()), Ok(()));
        // The administrator drops `pipeline` and the Endpoint from the policy: the same
        // connector loses them at the next call, and its marks say so.
        let narrowed = Ceiling::of_policies(vec![(
            "analysts-doprava".into(),
            serde_json::from_value(json!({
                "subjects": [{ "kind": "Group", "name": "analysts" }],
                "projects": ["doprava"], "endpoints": ["bikes"],
            }))
            .expect("policy"),
        )]);
        assert!(!narrowed.lists(&live, metrics, "doprava"));
        let why = narrowed
            .admits(&live, get, "doprava", &counters)
            .expect_err("narrowed");
        assert!(
            why.contains("analysts-doprava") && why.contains("jc_resource_get"),
            "{why}"
        );
        assert_eq!(
            narrowed.narrowing(&live),
            vec![
                "segment pipeline".to_owned(),
                "Endpoints of project doprava: counters".to_owned()
            ]
        );
        // A project the policy does not list is refused at the call.
        assert!(ceiling.admits(&live, get, "skolstvo", &counters).is_err());
        // The door: a client the policy does not list, and a connector older than its maxExpiry.
        assert!(ceiling
            .door(&live, "mcp-local", TimestampTz::now())
            .is_err());
        let mut old = live.clone();
        old.created_at = TimestampTz::now() - jiff::SignedDuration::from_hours(24 * 91);
        old.granted_at = old.created_at;
        let why = ceiling
            .door(&old, CLAUDE, TimestampTz::now())
            .expect_err("too old");
        assert!(why.contains("90 days"), "{why}");
        // JC2-45: an edit judged against the ceiling today renews it; its age counts from there.
        let mut renewed = old.clone();
        renewed.granted_at = TimestampTz::now();
        renewed.expires_at = renewed.granted_at + jiff::SignedDuration::from_hours(24 * 89);
        assert!(ceiling.door(&renewed, CLAUDE, TimestampTz::now()).is_ok());
        assert!(ceiling.narrowing(&renewed).is_empty());
    }

    /// T-7541 (S1-14): `autonomous` is the policy's to allow, as `writes` is.
    #[test]
    fn autonomous_is_above_a_policy_that_does_not_allow_it() {
        let ceiling = Ceiling::of_policies(vec![(
            "stewards".into(),
            serde_json::from_value(json!({
                "subjects": [{ "kind": "Group", "name": "stewards" }],
                "projects": ["doprava"], "segments": ["context"], "writes": true,
            }))
            .expect("a policy"),
        )]);
        let mut ask = new(&["context"], true, 1);
        assert!(validate(&ask, &ceiling, TimestampTz::now()).is_ok());
        ask.autonomous = true;
        let refused = validate(&ask, &ceiling, TimestampTz::now())
            .expect_err("autonomous passed the ceiling");
        assert!(
            matches!(&refused, ApiError::Denied(why) if why.starts_with("autonomous:")),
            "not a 403 on autonomous: {refused:?}"
        );
        let allowing = |projects: &[&str]| {
            Ceiling::of_policies(vec![(
                "stewards".into(),
                serde_json::from_value(json!({
                    "subjects": [{ "kind": "Group", "name": "stewards" }], "projects": projects,
                    "segments": ["context"], "writes": true, "autonomous": true,
                }))
                .expect("a policy"),
            )])
        };
        let allowed = allowing(&["doprava"]);
        assert!(
            matches!(validate(&ask, &allowed, TimestampTz::now()), Ok(above) if above.is_empty())
        );
        assert!(allowed.allows_autonomous("doprava") && !allowed.allows_autonomous("zdravie"));
        let mut held = connector(&["context"], vec![item("doprava", None)]);
        held.writes = true;
        held.autonomous = true;
        assert!(allowed.narrowing(&held).is_empty());
        assert!(ceiling
            .narrowing(&held)
            .contains(&"autonomous in project doprava".to_owned()));
    }

    /// An approval of `autonomous` holds in the project it named, while that is reached.
    #[test]
    fn an_approved_autonomous_is_its_projects_alone() {
        let mut policy = default_policy();
        policy.projects = vec!["doprava".into(), "skoly".into()];
        policy.writes = true;
        policy.above_ceiling = AboveCeiling::Queue;
        let mut approved = connector(&["data-read"], vec![item("doprava", None)]);
        approved.approval = Some(Approval {
            by: "sub-admin".into(),
            at: TimestampTz::now(),
        });
        approved.over = vec![Over::Autonomous("doprava".into())];
        let ceiling = Ceiling::of_policies(vec![("p".into(), policy.clone())]);
        let widened = ceiling.with_approved(&approved);
        assert!(widened.allows_autonomous("doprava") && !widened.allows_autonomous("skoly"));
        policy.projects = vec!["skoly".into()];
        let dropped = Ceiling::of_policies(vec![("p".into(), policy)]).with_approved(&approved);
        assert!(!dropped.allows_autonomous("doprava"));
    }

    #[test]
    fn an_approval_stands_only_while_a_policy_still_queues() {
        let (mirror, jana) = analysts("queue");
        let mut approved = connector(&["access"], vec![item("doprava", None)]);
        approved.approval = Some(Approval {
            by: "sub-admin".into(),
            at: TimestampTz::now(),
        });
        approved.over = vec![Over::Segment("access".into()), Over::AnyClient];
        let queued = Ceiling::of(&mirror, &jana).with_approved(&approved);
        assert!(queued.allows_segment("access"));
        assert_eq!(queued.door(&approved, "", TimestampTz::now()), Ok(()));
        let (mirror, jana) = analysts("refuse");
        let refused = Ceiling::of(&mirror, &jana).with_approved(&approved);
        assert!(!refused.allows_segment("access"));
        assert!(refused.door(&approved, "", TimestampTz::now()).is_err());
    }

    /// T-7541 (S1-13): each approved value opens its own field and no other one.
    #[test]
    fn each_approved_value_widens_its_field_alone() {
        let mut policy = default_policy();
        policy.projects = vec!["doprava".into()];
        policy.segments = Some(vec![Segment::Context]);
        policy.endpoints = Some(vec!["counters".into()]);
        policy.allowed_clients = Some(vec!["claude".into()]);
        policy.above_ceiling = AboveCeiling::Queue;
        policy.kinds = Some(vec![KindVerbs {
            kind: "Endpoint".into(),
            verbs: vec![ConnectorVerb::Read],
        }]);
        let ceiling = Ceiling::of_policies(vec![("listed".into(), policy)]);
        let open = |c: &Ceiling| {
            [
                ("segment", c.allows_segment("access")),
                ("project", c.allows_project_name("zdravie")),
                ("endpoint", c.allows_endpoint("doprava", Some("parking"))),
                ("every endpoint", c.allows_endpoint("doprava", None)),
                ("verb", c.allows_verb(None, "Endpoint", ConnectorVerb::Edit)),
                ("kinds", !c.lists_kinds()),
                ("writes", c.writes()),
                ("client", c.allows_client("chatgpt")),
                ("any client", c.any_client()),
                ("age", c.approved_until.is_some()),
            ]
            .into_iter()
            .filter_map(|(field, opened)| opened.then_some(field))
            .collect::<Vec<_>>()
        };
        assert!(open(&ceiling).is_empty(), "{:?}", open(&ceiling));
        let endpoint = |name: Option<&str>| Over::Endpoint {
            project: "doprava".into(),
            endpoint: name.map(str::to_owned),
        };
        let mut approved = connector(&["context"], vec![item("doprava", None)]);
        approved.approval = Some(Approval {
            by: "sub-admin".into(),
            at: TimestampTz::now(),
        });
        let until = approved.expires_at.as_second();
        for (over, opens) in [
            (Over::Segment("access".into()), vec!["segment"]),
            (Over::Project("zdravie".into()), vec!["project"]),
            (endpoint(Some("parking")), vec!["endpoint"]),
            (endpoint(None), vec!["endpoint", "every endpoint"]),
            (
                Over::Verb {
                    kind: "Endpoint".into(),
                    verb: ConnectorVerb::Edit,
                },
                vec!["verb"],
            ),
            // No list: the verbs are the segments', so a change verb opens with it.
            (Over::Kinds, vec!["verb", "kinds"]),
            (Over::Writes, vec!["writes"]),
            (Over::Client("chatgpt".into()), vec!["client"]),
            // Any client is this client too.
            (Over::AnyClient, vec!["client", "any client"]),
            (Over::Until(until), vec!["age"]),
            (Over::Until(until - 1), vec![]),
        ] {
            approved.over = vec![over];
            let widened = ceiling.with_approved(&approved);
            assert_eq!(open(&widened), opens, "{:?}", approved.over);
            // An Endpoint's exception is its project's alone.
            assert!(!widened.allows_endpoint("skoly", Some("parking")));
        }
        approved.over.clear();
        assert!(open(&ceiling.with_approved(&approved)).is_empty());
    }

    /// T-7541 (S1-13): an approved project opens that project's tools and lists, and no other
    /// project's; every other part of the policy still binds the approved connector.
    #[test]
    fn an_approved_project_opens_that_project_at_the_door_alone() {
        let mut policy = default_policy();
        policy.projects = vec!["doprava".into()];
        policy.segments = Some(vec![Segment::Context]);
        policy.above_ceiling = AboveCeiling::Queue;
        let scope = ["doprava", "zdravie", "skoly"].map(|project| item(project, None));
        let mut approved = connector(&["context", "access"], scope.to_vec());
        approved.approval = Some(Approval {
            by: "sub-admin".into(),
            at: TimestampTz::now(),
        });
        approved.over = vec![Over::Project("zdravie".into())];
        let ceiling =
            Ceiling::of_policies(vec![("listed".into(), policy)]).with_approved(&approved);
        let (get, args) = (
            op("jc_resource_get"),
            json!({ "kind": "DataModel", "name": "air" }),
        );
        assert_eq!(ceiling.admits(&approved, get, "zdravie", &args), Ok(()));
        assert!(ceiling.lists(&approved, get, "zdravie"));
        assert!(ceiling.admits(&approved, get, "skoly", &args).is_err());
        assert!(!ceiling.lists(&approved, get, "skoly"));
        let binding = json!({ "kind": "RoleBinding", "name": "x" });
        assert!(ceiling.admits(&approved, get, "zdravie", &binding).is_err());
    }

    /// An edit keeps an approval inside what was approved; another value waits again.
    #[test]
    fn an_approval_covers_no_swapped_value() {
        let (mirror, jana) = analysts("queue");
        let ceiling = Ceiling::of(&mirror, &jana);
        let now = TimestampTz::now();
        let mut ask = new(&["access"], false, 1);
        ask.allowed_clients = vec![];
        let first = validate(&ask, &ceiling, now).expect("queued");
        let mut held = connector(&["access"], ask.scope.clone());
        held.kinds = ask.kinds.clone();
        held.allowed_clients = ask.allowed_clients.clone();
        held.expires_at = ask.expires_at;
        held.over = first.values.clone();
        assert!(held.approval_covers(&ask, &first));
        ask.segments = vec!["context".into()];
        let swapped = validate(&ask, &ceiling, now).expect("queued");
        assert!(!held.approval_covers(&ask, &swapped));
    }

    #[tokio::test]
    async fn a_pending_connector_waits_for_one_approval() {
        let store = ConnectorStore::new(None, None);
        let base = "https://portal.test";
        let made = store
            .create(
                "sub-jana",
                new(&["data-read"], false, 1),
                Above {
                    values: vec![Over::Writes],
                    lines: vec!["writes: above".into()],
                },
                base,
            )
            .await
            .expect("kept");
        assert_eq!(made.state, "pending");
        assert_eq!(made.above_ceiling, vec!["writes: above".to_owned()]);
        let waiting = store.pending(base).await.expect("pending");
        assert_eq!(
            waiting.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec![made.id.as_str()]
        );
        let judged = made.granted_at;
        assert!(store
            .approve(&made.id, "sub-admin", judged)
            .await
            .expect("approve"));
        assert!(
            !store
                .approve(&made.id, "sub-other", judged)
                .await
                .expect("twice"),
            "approved once"
        );
        let now = store
            .get(&made.id, base)
            .await
            .expect("get")
            .expect("there");
        assert_eq!(now.state, "active");
        assert_eq!(
            now.approval.as_ref().map(|a| a.by.as_str()),
            Some("sub-admin")
        );
        assert!(store.pending(base).await.expect("pending").is_empty());
    }

    /// T-5975: offboarding revokes every live connector of one person and nobody else's; the
    /// overview lists live ones first, then the revoked.
    #[tokio::test]
    async fn revoke_all_ends_one_persons_connectors_and_the_overview_lists_live_first() {
        let store = ConnectorStore::new(None, None);
        for (owner, name) in [("jana", "a"), ("jana", "b"), ("petra", "c")] {
            let mut ask = new(&["data-read"], false, 1);
            ask.name = name.into();
            store
                .create(owner, ask, Default::default(), "")
                .await
                .expect("created");
        }
        assert_eq!(store.revoke_all("jana").await.expect("revoked"), 2);
        assert_eq!(store.revoke_all("jana").await.expect("again"), 0);
        let listed = store.overview("").await.expect("overview");
        let rows: Vec<(&str, &str)> = listed
            .iter()
            .map(|o| (o.connector.name.as_str(), o.connector.state.as_str()))
            .collect();
        assert_eq!(rows, [("c", "active"), ("b", "revoked"), ("a", "revoked")]);
    }

    fn grant(kind: &str, verbs: &[ConnectorVerb]) -> KindGrant {
        KindGrant {
            kind: kind.into(),
            verbs: verbs.to_vec(),
        }
    }

    /// T-6302: each change is held to its verb on its kind; a kind the list leaves out is read
    /// only, `propose` stands for the three, and a connector without a list changes as before.
    #[test]
    fn a_kinds_list_holds_each_change_to_its_verb() {
        use ConnectorVerb::{Create, Delete, Edit, Propose, Read, Run};
        let mut c = connector(&["context", "apps"], vec![item("banskabystrica", None)]);
        c.kinds = Some(vec![
            grant("Endpoint", &[Read, Create]),
            grant("App", &[Read, Edit]),
        ]);
        assert_eq!(c.may_verb("Endpoint", "fresh", Create), Ok(()));
        let edit = c.may_verb("Endpoint", "counters", Edit).unwrap_err();
        assert!(edit.contains("may not edit Endpoint counters"), "{edit}");
        assert!(
            edit.contains("its verbs on Endpoint are read, create"),
            "{edit}"
        );
        assert!(c.may_verb("Endpoint", "counters", Delete).is_err());
        assert_eq!(c.may_verb("App", "board", Edit), Ok(()));
        assert!(c.may_verb("App", "board", Create).is_err());
        let left_out = c.may_verb("DataModel", "bikes", Create).unwrap_err();
        assert!(left_out.contains("are read;"), "{left_out}");
        c.kinds = Some(vec![grant("Endpoint", &[Propose])]);
        for verb in [Create, Edit, Delete] {
            assert_eq!(c.may_verb("Endpoint", "counters", verb), Ok(()));
        }
        c.kinds = Some(vec![grant("Pipeline", &[Read])]);
        assert!(c.may_verb("Pipeline", "p", Run).is_err());
        c.kinds = None;
        assert_eq!(c.may_verb("Endpoint", "counters", Delete), Ok(()));
    }

    /// T-6302: a connector's list has the policy's shape, names only kinds its presets reach,
    /// and asks no verb the ceiling withholds; a policy without `kinds` withholds none.
    #[test]
    fn a_kinds_list_is_checked_and_held_to_the_ceiling() {
        use ConnectorVerb::{Create, Delete, Edit, Read, Run};
        let now = TimestampTz::now();
        let invalid = |n: NewConnector, field: &str| {
            let err = validate(&n, &Ceiling::of_policies(vec![]), now).expect_err(field);
            assert!(
                matches!(&err, ApiError::Invalid { errors, .. } if *errors == vec![field.to_owned()]),
                "{field}: {err:?}"
            );
        };
        let mut bad = new(&["data-read"], false, 1);
        bad.kinds = Some(vec![grant("Pipeline", &[Read])]);
        invalid(bad, "kinds[0].kind");
        let mut bad = new(&["data-read"], false, 1);
        bad.kinds = Some(vec![grant("Endpoint", &[Run])]);
        invalid(bad, "kinds[].verbs");
        // data-read plans nothing: a change verb under it alone would be a dead tick.
        let mut bad = new(&["data-read"], false, 1);
        bad.kinds = Some(vec![grant("Endpoint", &[Read, Create])]);
        invalid(bad, "kinds[0].verbs");
        let mut ok = new(&["data-read"], false, 1);
        ok.kinds = Some(vec![grant("Endpoint", &[Read])]);
        assert!(validate(&ok, &Ceiling::of_policies(vec![]), now).is_ok());

        let mut policy = default_policy();
        policy.projects = vec!["doprava".into()];
        policy.segments = Some(vec![Segment::Context]);
        policy.kinds = Some(vec![KindVerbs {
            kind: "Endpoint".into(),
            verbs: vec![Read, Create],
        }]);
        let ceiling = Ceiling::of_policies(vec![("creators".into(), policy.clone())]);
        let mut ask = new(&["context"], false, 1);
        ask.kinds = Some(vec![grant("Endpoint", &[Read, Create])]);
        assert!(matches!(validate(&ask, &ceiling, now), Ok(above) if above.is_empty()));
        ask.kinds = Some(vec![grant("Endpoint", &[Read, Edit])]);
        let refused = validate(&ask, &ceiling, now).expect_err("a ceiling refusal");
        assert!(
            matches!(&refused, ApiError::Denied(why)
                if why.starts_with("kinds[0].verbs") && why.contains("edit Endpoint")),
            "{refused:?}"
        );
        assert!(ceiling.allows_verb(Some("doprava"), "Endpoint", Create));
        assert!(!ceiling.allows_verb(Some("doprava"), "Endpoint", Edit));
        assert!(!ceiling.allows_verb(Some("other"), "Endpoint", Create));
        assert_eq!(
            ceiling.kind_verbs(&["Endpoint", "Pipeline"]),
            vec![grant("Endpoint", &[Read, Create]), grant("Pipeline", &[])]
        );
        policy.kinds = None;
        let open = Ceiling::of_policies(vec![("open".into(), policy.clone())]);
        assert!(open.allows_verb(Some("doprava"), "Endpoint", Edit));
        // The dialog offers a change verb only where a proposing segment reaches the kind.
        assert_eq!(
            open.kind_verbs(&["Endpoint", "Pipeline"]),
            vec![
                grant("Endpoint", &[Read, Create, Edit, Delete]),
                grant("Pipeline", &[Read])
            ]
        );
        policy.segments = Some(vec![Segment::Operate]);
        // JC2-07: a policy whose segments reach no Endpoint lets no connector change one, kinds
        // list or not; nor an Endpoint its own list leaves out.
        let pipelines_only = Ceiling::of_policies(vec![("ops".into(), policy.clone())]);
        assert!(!pipelines_only.allows_verb(Some("doprava"), "Endpoint", Create));
        assert!(!pipelines_only.allows_change("doprava", "Endpoint", "counters", Create));
        assert!(pipelines_only.allows_change("doprava", "Pipeline", "counters", Edit));
        let mut listed = policy.clone();
        listed.segments = Some(vec![Segment::Context]);
        listed.endpoints = Some(vec!["counters".into()]);
        let listed = Ceiling::of_policies(vec![("listed".into(), listed)]);
        assert!(listed.allows_change("doprava", "Endpoint", "counters", Edit));
        assert!(!listed.allows_change("doprava", "Endpoint", "parking", Edit));
        let operate = |writes| {
            let mut p = policy.clone();
            p.writes = writes;
            Ceiling::of_policies(vec![("ops".into(), p)]).kind_verbs(&["Pipeline"])
        };
        assert_eq!(
            operate(false),
            vec![grant("Pipeline", &[Read, Create, Edit, Delete])]
        );
        assert_eq!(
            operate(true),
            vec![grant("Pipeline", &kind_choices("Pipeline"))]
        );
        // The default ceiling (data-read, writes off) offers read alone.
        assert_eq!(
            Ceiling::of_policies(vec![]).kind_verbs(&["Endpoint"]),
            vec![grant("Endpoint", &[Read])]
        );
    }

    /// JC2-06: what an approval covers is the exact request; a later expiry, another Endpoint
    /// or a connector without a kinds list under policies that list kinds reads as a new line,
    /// so an edit to it waits for a new approval.
    #[test]
    fn an_above_ceiling_line_names_what_was_asked() {
        let now = TimestampTz::now();
        let mut policy = default_policy();
        policy.projects = vec!["doprava".into()];
        policy.segments = Some(vec![Segment::Context]);
        policy.endpoints = Some(vec!["counters".into()]);
        policy.kinds = Some(vec![KindVerbs {
            kind: "Endpoint".into(),
            verbs: vec![ConnectorVerb::Read],
        }]);
        let ceiling = Ceiling::of_policies(vec![("listed".into(), policy)]);
        let mut ask = new(&["context"], false, 100);
        ask.scope = vec![item("doprava", Some("parking"))];
        let first = above(&ask, &ceiling, now);
        assert!(first.iter().any(|(_, f, _)| f == "kinds"), "{first:?}");
        ask.expires_at = now + jiff::SignedDuration::from_hours(24 * 300);
        ask.scope = vec![item("doprava", Some("bikes"))];
        let later = above(&ask, &ceiling, now);
        for field in ["expiresAt", "scope[0].endpoint"] {
            let line = |lines: &[(Vec<Over>, String, String)]| {
                lines
                    .iter()
                    .find(|(_, f, _)| f == field)
                    .cloned()
                    .expect(field)
            };
            assert_ne!(line(&first), line(&later), "{field}");
        }
        ask.kinds = Some(vec![grant("Endpoint", &[ConnectorVerb::Read])]);
        assert!(!above(&ask, &ceiling, now)
            .iter()
            .any(|(_, f, _)| f == "kinds"));
    }

    // --- coverage: the branches the scenarios above leave out ---------------------------------

    fn listed_policy() -> DelegationPolicySpec {
        let mut policy = default_policy();
        policy.projects = vec!["doprava".into()];
        policy.segments = Some(vec![Segment::Context]);
        policy.above_ceiling = AboveCeiling::Queue;
        policy
    }

    fn approved_with(over: Vec<Over>) -> Connector {
        let mut approved = connector(&["context"], vec![item("doprava", None)]);
        approved.approval = Some(Approval {
            by: "sub-admin".into(),
            at: TimestampTz::now(),
        });
        approved.over = over;
        approved
    }

    #[test]
    fn propose_stands_for_create_edit_and_delete_and_any_other_verb_for_itself() {
        use ConnectorVerb::{Create, Delete, Edit, Propose, Read, Run};
        assert_eq!(each_verb(Propose), vec![Create, Edit, Delete]);
        for verb in [Read, Create, Edit, Delete, Run] {
            assert_eq!(each_verb(verb), vec![verb]);
        }
    }

    #[test]
    fn an_approved_value_that_changes_nothing_leaves_the_policy_as_it_was() {
        let policy = listed_policy();
        for over in [
            Over::Segment("bogus".into()),
            Over::Segment("context".into()),
            Over::Project("doprava".into()),
            Over::Client("claude".into()),
            Over::Endpoint {
                project: "doprava".into(),
                endpoint: None,
            },
            Over::Autonomous("doprava".into()),
            Over::Until(1),
            Over::Verb {
                kind: "Endpoint".into(),
                verb: ConnectorVerb::Edit,
            },
        ] {
            let mut widened = policy.clone();
            widen(&mut widened, &over);
            assert_eq!(widened, policy, "{over:?}");
        }
    }

    #[test]
    fn an_approved_client_joins_a_list_and_any_client_drops_it() {
        let mut policy = listed_policy();
        policy.allowed_clients = Some(vec!["claude".into()]);
        widen(&mut policy, &Over::Client("chatgpt".into()));
        assert_eq!(
            policy.allowed_clients,
            Some(vec!["claude".to_owned(), "chatgpt".to_owned()])
        );
        widen(&mut policy, &Over::AnyClient);
        assert_eq!(policy.allowed_clients, None);
    }

    #[test]
    fn a_policy_that_does_not_parse_binds_nobody() {
        let (mirror, jana) = analysts("refuse");
        mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "DelegationPolicy".into(),
            metadata: crate::resource::ObjectMeta::new("garbled", ORG_NAMESPACE),
            spec: json!({ "subjects": "jana", "projects": 7, "nonsense": true }),
            status: None,
        });
        let named = Ceiling::of(&mirror, &jana).named();
        assert!(named.contains("analysts-doprava"), "{named}");
        assert!(!named.contains("garbled"), "{named}");
    }

    #[test]
    fn a_policy_repeated_for_an_approved_endpoint_is_cited_once() {
        let policy = listed_policy();
        let twice = Ceiling::of_policies(vec![
            ("listed".into(), policy.clone()),
            ("listed".into(), policy.clone()),
            ("other".into(), policy),
        ]);
        assert_eq!(twice.named(), "DelegationPolicy listed, other");
    }

    #[test]
    fn what_the_ceiling_takes_from_a_connector_is_told_mark_by_mark() {
        let mut policy = default_policy();
        policy.allowed_clients = Some(vec!["claude".into()]);
        let ceiling = Ceiling::of_policies(vec![("tight".into(), policy)]);
        let mut wide = connector(&["data-read"], vec![item("doprava", None)]);
        assert!(ceiling.narrowing(&wide).is_empty());
        wide.writes = true;
        wide.expires_at = wide.granted_at + jiff::SignedDuration::from_hours(24 * 400);
        wide.allowed_clients = vec!["claude".into(), "other".into()];
        assert_eq!(
            ceiling.narrowing(&wide),
            vec![
                "writes".to_owned(),
                "expiry: at most 30 days".to_owned(),
                "client other".to_owned()
            ]
        );
    }

    #[test]
    fn an_approval_stands_for_a_request_no_wider_than_the_one_judged() {
        use ConnectorVerb::{Create, Edit, Propose};
        let mut ask = new(&["data-read"], false, 1);
        let mut held = approved_with(vec![Over::Writes]);
        held.segments = ask.segments.clone();
        held.expires_at = ask.expires_at + jiff::SignedDuration::from_hours(1);
        let above = Above {
            values: vec![Over::Writes],
            lines: vec!["writes".into()],
        };
        assert!(held.approval_covers(&ask, &above));
        // No kinds list held: every change is held, so any list asked is no wider.
        ask.kinds = Some(vec![grant("Endpoint", &[Propose])]);
        assert!(held.approval_covers(&ask, &above));
        // A list held: a verb it leaves out, also spelled as `propose`, is wider.
        held.kinds = Some(vec![grant("Endpoint", &[ConnectorVerb::Read, Create])]);
        ask.kinds = Some(vec![grant("Endpoint", &[Create])]);
        assert!(held.approval_covers(&ask, &above));
        ask.kinds = Some(vec![grant("Endpoint", &[Edit])]);
        assert!(!held.approval_covers(&ask, &above));
        ask.kinds = Some(vec![grant("Endpoint", &[Propose])]);
        assert!(!held.approval_covers(&ask, &above));
        ask.kinds = None;
        assert!(
            !held.approval_covers(&ask, &above),
            "no list asks every change"
        );
        // Clients: a held list covers the clients it names and not an open list.
        held.kinds = None;
        held.allowed_clients = vec!["claude".into()];
        ask.allowed_clients = vec!["claude".into()];
        assert!(held.approval_covers(&ask, &above));
        ask.allowed_clients = vec!["claude".into(), "other".into()];
        assert!(!held.approval_covers(&ask, &above));
        ask.allowed_clients = vec![];
        assert!(!held.approval_covers(&ask, &above));
        // Nothing asked above the ceiling is nothing to cover.
        ask.allowed_clients = vec!["claude".into()];
        assert!(!held.approval_covers(&ask, &Above::default()));
    }

    #[test]
    fn a_kind_with_no_verb_ticked_names_none() {
        let mut c = connector(&["context"], vec![item("doprava", None)]);
        c.kinds = Some(vec![grant("Endpoint", &[])]);
        let err = c
            .may_verb("Endpoint", "counters", ConnectorVerb::Edit)
            .expect_err("no verb");
        assert!(err.contains("its verbs on Endpoint are none"), "{err}");
    }

    #[test]
    fn a_project_named_by_two_items_is_listed_once() {
        let c = connector(
            &["context"],
            vec![
                item("doprava", Some("a")),
                item("doprava", Some("b")),
                item("skoly", None),
            ],
        );
        assert_eq!(c.projects(), vec!["doprava", "skoly"]);
        assert_eq!(c.endpoints_named("doprava"), Some(vec!["a", "b"]));
        assert_eq!(c.endpoints_named("skoly"), None);
        assert_eq!(c.endpoints_named("none"), None);
    }

    #[test]
    fn an_authoring_step_whose_draft_lacks_its_kind_or_name_is_refused() {
        let c = connector(&["pipeline"], vec![item("doprava", Some("counters"))]);
        for draft in [
            json!({ "kind": "Pipeline" }),
            json!({ "name": "p" }),
            json!({}),
            json!({ "kind": 3, "name": "p" }),
        ] {
            let err = c
                .admits(
                    op(STEPS[0]),
                    "doprava",
                    &json!({ "draft": draft }),
                    &nowhere,
                )
                .expect_err("a draft without both");
            assert!(err.contains("give both"), "{draft}: {err}");
        }
    }

    #[test]
    fn a_write_that_names_no_object_is_asked_for_its_name() {
        let mut c = connector(&["operate", "context"], vec![item("doprava", None)]);
        c.writes = true;
        c.scope[0].access = ItemAccess::Write;
        for (tool, noun) in [
            (RUN, "pipeline"),
            (PAUSE, "SyncSource"),
            (DATA_WRITE, "endpoint"),
        ] {
            for args in [json!({}), json!({ "name": 7, "endpoint": null })] {
                let err = c
                    .admits(op(tool), "doprava", &args, &nowhere)
                    .expect_err("no name");
                assert_eq!(err, format!("{tool} names the {noun}: name it"), "{args}");
            }
        }
        let err = c
            .admits(op(BUILD), "doprava", &json!({}), &nowhere)
            .expect_err("no App named");
        assert!(err.contains("appName"), "{err}");
    }

    #[test]
    fn a_run_and_a_pause_need_their_verb_where_a_policy_lists_kinds() {
        use ConnectorVerb::{Edit, Read, Run};
        let mut c = connector(&["operate"], vec![item("doprava", None)]);
        c.writes = true;
        let run_args = json!({ "name": "counters-in" });
        let pause_args = json!({ "name": "regional", "paused": true });
        let under = |pipeline: &[ConnectorVerb], source: &[ConnectorVerb]| {
            let mut policy = default_policy();
            policy.projects = vec!["doprava".into()];
            policy.segments = Some(vec![Segment::Operate]);
            policy.writes = true;
            policy.kinds = Some(vec![
                KindVerbs {
                    kind: "Pipeline".into(),
                    verbs: pipeline.to_vec(),
                },
                KindVerbs {
                    kind: "SyncSource".into(),
                    verbs: source.to_vec(),
                },
            ]);
            Ceiling::of_policies(vec![("ops".into(), policy)])
        };
        let both = under(&[Read, Run], &[Read, Edit]);
        assert_eq!(both.admits(&c, op(RUN), "doprava", &run_args), Ok(()));
        assert_eq!(both.admits(&c, op(PAUSE), "doprava", &pause_args), Ok(()));
        let neither = under(&[Read], &[Read]);
        assert!(neither.admits(&c, op(RUN), "doprava", &run_args).is_err());
        assert!(neither
            .admits(&c, op(PAUSE), "doprava", &pause_args)
            .is_err());
        // A policy with no kinds list withholds neither.
        let mut open = default_policy();
        open.projects = vec!["doprava".into()];
        open.segments = Some(vec![Segment::Operate]);
        open.writes = true;
        let open = Ceiling::of_policies(vec![("open".into(), open)]);
        assert_eq!(open.admits(&c, op(RUN), "doprava", &run_args), Ok(()));
        assert_eq!(open.admits(&c, op(PAUSE), "doprava", &pause_args), Ok(()));
    }

    #[test]
    fn an_approved_endpoint_of_a_policy_without_an_endpoint_list_changes_nothing() {
        let ceiling = Ceiling::of_policies(vec![("open".into(), listed_policy())]);
        let approved = approved_with(vec![Over::Endpoint {
            project: "doprava".into(),
            endpoint: Some("parking".into()),
        }]);
        let widened = ceiling.with_approved(&approved);
        assert_eq!(widened.policies.len(), 1);
        assert!(widened.allows_endpoint("doprava", Some("parking")));
    }

    #[test]
    fn approved_endpoints_of_one_project_combine_and_other_approvals_pass_by() {
        let mut policy = listed_policy();
        policy.endpoints = Some(vec!["counters".into()]);
        let ceiling = Ceiling::of_policies(vec![("listed".into(), policy)]);
        let endpoint = |project: &str, name: Option<&str>| Over::Endpoint {
            project: project.into(),
            endpoint: name.map(str::to_owned),
        };
        // `*` after a named one, a named one after `*`, an approval of another field between,
        // and an Endpoint of a project the policy does not reach.
        let approved = approved_with(vec![
            endpoint("doprava", Some("parking")),
            Over::Writes,
            endpoint("doprava", Some("*")),
            endpoint("doprava", Some("bikes")),
            endpoint("skoly", Some("canteen")),
        ]);
        let widened = ceiling.with_approved(&approved);
        assert!(widened.allows_endpoint("doprava", Some("anything")));
        assert!(widened.allows_endpoint("doprava", None));
        assert!(!widened.allows_endpoint("skoly", Some("canteen")));
        assert!(widened.writes());
    }

    #[test]
    fn a_name_that_is_no_project_or_no_name_is_refused_in_the_scope() {
        let now = TimestampTz::now();
        let invalid = |n: NewConnector, field: &str| {
            let err = validate(&n, &Ceiling::of_policies(vec![]), now).expect_err(field);
            assert!(
                matches!(&err, ApiError::Invalid { errors, .. } if *errors == vec![field.to_owned()]),
                "{field}: {err:?}"
            );
        };
        let mut bad = new(&["data-read"], false, 1);
        bad.scope[0].project = "Not A Project".into();
        invalid(bad, "scope[0].project");
        for (field, set) in [
            ("scope[0].space", 0),
            ("scope[0].endpoint", 1),
            ("scope[0].pipeline", 2),
        ] {
            let mut bad = new(&["data-read"], false, 1);
            let name = Some("Bad Name".to_owned());
            match set {
                0 => bad.scope[0].space = name,
                1 => bad.scope[0].endpoint = name,
                _ => bad.scope[0].pipeline = name,
            }
            invalid(bad, field);
        }
        let mut fine = new(&["data-read"], false, 1);
        fine.scope[0].endpoint = Some("*".into());
        assert!(validate(&fine, &Ceiling::of_policies(vec![]), now).is_ok());
    }

    #[test]
    fn more_than_ten_clients_are_refused_and_ten_are_not() {
        let now = TimestampTz::now();
        let mut ask = new(&["data-read"], false, 1);
        ask.allowed_clients = (0..MAX_CLIENTS).map(|n| format!("client-{n}")).collect();
        assert!(validate(&ask, &Ceiling::of_policies(vec![]), now).is_ok());
        ask.allowed_clients.push("client-last".into());
        let err = validate(&ask, &Ceiling::of_policies(vec![]), now).expect_err("too many");
        assert!(
            matches!(&err, ApiError::Invalid { errors, .. } if *errors == vec!["allowedClients".to_owned()]),
            "{err:?}"
        );
    }

    // The memory arm.

    fn call_at(at: TimestampTz, verdict: &str) -> Call {
        Call {
            at,
            subject: "jana".into(),
            client: None,
            tool: "tools/list".into(),
            target: None,
            verdict: verdict.into(),
            duration_ms: 1,
        }
    }

    #[tokio::test]
    async fn the_memory_store_refuses_a_second_live_connector_of_the_same_name() {
        let store = ConnectorStore::new(None, None);
        let base = "https://portal.test";
        let mut ask = new(&["data-read"], false, 1);
        ask.name = "bikes".into();
        store
            .create("jana", ask, Default::default(), base)
            .await
            .expect("created");
        let mut again = new(&["data-read"], false, 1);
        again.name = "bikes".into();
        let refused = store.create("jana", again, Default::default(), base).await;
        assert!(
            matches!(&refused, Err(ApiError::Conflict(m)) if m.contains("named bikes already")),
            "{refused:?}"
        );
        assert_eq!(store.of_owner("jana", base).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn an_allowed_call_of_a_connector_nobody_holds_is_kept_and_marks_none() {
        let store = ConnectorStore::new(None, None);
        let trail = Trail::new(Some("ghost".into()), None);
        store
            .keep(&trail, call_at(TimestampTz::now(), "allowed"))
            .await
            .expect("kept");
        assert_eq!(store.calls("ghost").await.expect("calls").len(), 1);
        assert!(store
            .get("ghost", "https://portal.test")
            .await
            .expect("get")
            .is_none());
    }

    #[tokio::test]
    async fn the_memory_audit_drops_what_is_older_than_the_retention() {
        let store = ConnectorStore::new(None, None);
        let now = TimestampTz::now();
        let old = now - jiff::SignedDuration::from_hours(24 * 500);
        store
            .record("c", call_at(old, "allowed"))
            .await
            .expect("kept");
        store
            .record("c", call_at(now, "allowed"))
            .await
            .expect("kept");
        assert_eq!(store.trim_audit(now, 400).await.expect("trim"), 1);
        assert_eq!(store.trim_audit(now, 400).await.expect("trim"), 0);
        let left = store.calls("c").await.expect("calls");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].at, now);
        let after = now + jiff::SignedDuration::from_secs(1);
        assert_eq!(store.trim_audit(after, 0).await.expect("trim all"), 1);
    }

    #[tokio::test]
    async fn the_audit_is_trimmed_once_an_hour_by_each_process() {
        let store = ConnectorStore::new(None, None);
        let now = TimestampTz::now();
        assert_eq!(
            store.trim_audit_if_due(now, 400).await.expect("due").ok(),
            Some(0)
        );
        assert!(store.trim_audit_if_due(now, 400).await.is_none());
        let later = now + jiff::SignedDuration::from_mins(59);
        assert!(store.trim_audit_if_due(later, 400).await.is_none());
        let next = now + jiff::SignedDuration::from_mins(crate::activity::TRIM_EVERY_MINUTES);
        assert!(store.trim_audit_if_due(next, 400).await.is_some());
    }

    #[tokio::test]
    async fn a_poisoned_trim_clock_still_lets_the_audit_trim() {
        let store = std::sync::Arc::new(ConnectorStore::new(None, None));
        let holder = std::sync::Arc::clone(&store);
        let poisoned = std::thread::spawn(move || {
            let _held = holder.trimmed_at.lock().expect("the clock");
            std::panic::panic_any("poisoning the clock on purpose");
        })
        .join();
        assert!(poisoned.is_err());
        assert!(store.trimmed_at.is_poisoned());
        assert!(store
            .trim_audit_if_due(TimestampTz::now(), 400)
            .await
            .is_some());
    }

    #[test]
    fn a_poisoned_memory_store_still_answers() {
        let rows = std::sync::Arc::new(Mutex::<Held>::default());
        let holder = std::sync::Arc::clone(&rows);
        let poisoned = std::thread::spawn(move || {
            let _held = holder.lock().expect("the rows");
            std::panic::panic_any("poisoning the rows on purpose");
        })
        .join();
        assert!(poisoned.is_err());
        let held = ConnectorStore::memory(&rows);
        assert!(held.0.is_empty() && held.1.is_empty());
    }
}
