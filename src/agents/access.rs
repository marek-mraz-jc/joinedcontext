//! What an agent may call (AG-70): the profile's `spec.access` narrows the operation registry,
//! and the person who started the run narrows it again at every call, so a profile never widens
//! anyone. Read from the untyped spec like the rest of the profile (see `profile.rs`). An agent
//! therefore has no rights of its own (AG-03): what it may touch is the person's `Policy` and
//! bindings, and every write it makes is a proposal on the configuration repository under them.

use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::Verb;
use serde_json::Value;

use crate::auth::session::Identity;
use crate::ops::{self, Operation};
use crate::state::AppState;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// `None` when the profile has no access block: operations annotated read-only, nothing else.
    declared: Option<Declared>,
    /// The connector this call comes through (AG-143): `jc_change_plan` and `jc_change_apply`
    /// hold each change to its scope and its `writes` switch (AG-149, T-5956).
    connector: Option<Box<crate::connectors::Connector>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Declared {
    operations: BTreeSet<String>,
    /// Kind → the verbs granted on it (`read`, `propose`).
    kinds: BTreeMap<String, BTreeSet<String>>,
    /// Endpoint name → the verbs granted on it (`read`, `write`); empty grants every endpoint
    /// the person may use.
    endpoints: BTreeMap<String, BTreeSet<String>>,
}

impl Access {
    pub fn from_spec(spec: &Value) -> Self {
        let Some(access) = spec.get("access").filter(|access| access.is_object()) else {
            return Self::default();
        };
        let strings = |value: Option<&Value>| -> BTreeSet<String> {
            value
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let grants = |list: &str, key: &str| -> BTreeMap<String, BTreeSet<String>> {
            access
                .get(list)
                .and_then(Value::as_array)
                .map(|grants| {
                    grants
                        .iter()
                        .filter_map(|grant| {
                            let name = grant.get(key)?.as_str()?.to_owned();
                            Some((name, strings(grant.get("verbs"))))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            declared: Some(Declared {
                operations: strings(access.get("operations")),
                kinds: grants("kinds", "kind"),
                endpoints: grants("endpoints", "name"),
            }),
            connector: None,
        }
    }

    /// This narrowing, for calls through `connector`.
    pub fn through(mut self, connector: crate::connectors::Connector) -> Self {
        self.connector = Some(Box::new(connector));
        self
    }

    /// The connector a call comes through, if it comes through one.
    pub fn connector(&self) -> Option<&crate::connectors::Connector> {
        self.connector.as_deref()
    }

    /// The profile's half. Without an access block: an operation annotated read-only. With one:
    /// an operation it names whose kind verb it grants, `read` for a verbless operation of a
    /// kind and `propose` for a proposal; approving and deleting are never an agent's.
    pub fn names(&self, op: &Operation) -> bool {
        let Some(declared) = &self.declared else {
            return op.annotations.read_only_hint;
        };
        if !declared.operations.contains(op.name) {
            return false;
        }
        if op.kind == "*" {
            return true;
        }
        let verb = match op.verb {
            None | Some(Verb::Read) => "read",
            Some(Verb::Propose) => "propose",
            Some(_) => return false,
        };
        declared
            .kinds
            .get(op.kind)
            .is_some_and(|verbs| verbs.contains(verb))
    }

    /// The profile's half of one call of a kind-`*` operation (T-6366): the first kind its input
    /// names that the profile refuses. Each named kind needs `read` for a read-only operation and
    /// `propose` for any other, as [`Access::names`] asks it of an operation with a fixed kind.
    /// Every kind is checked (`kind`, `manifest.kind`, `draft.kind`), so a granted one beside it
    /// hides nothing. An input that names no manifest kind is left to [`Access::names`]; without
    /// an access block the read-only default stands.
    pub fn refused_kind<'a>(&self, op: &Operation, input: &'a Value) -> Option<&'a str> {
        let declared = self.declared.as_ref().filter(|_| op.kind == "*")?;
        let verb = if op.annotations.read_only_hint {
            "read"
        } else {
            "propose"
        };
        ops::named_kinds(input).find(|kind| {
            !declared
                .kinds
                .get(*kind)
                .is_some_and(|verbs| verbs.contains(verb))
        })
    }

    /// The profile's half of reading a kind's manifests outside a tool (MCP `resources/*`):
    /// without an access block, every kind the person reads; with one, a kind it grants `read`.
    pub fn reads(&self, kind: &str) -> bool {
        self.declared.as_ref().is_none_or(|declared| {
            declared
                .kinds
                .get(kind)
                .is_some_and(|verbs| verbs.contains("read"))
        })
    }

    /// The profile's half of a tool that names its kind only when it is called (`change_resource`):
    /// the operation named and `propose` granted on that kind. Without an access block, nothing.
    pub fn proposes(&self, operation: &str, kind: &str) -> bool {
        self.declared.as_ref().is_some_and(|declared| {
            declared.operations.contains(operation)
                && declared
                    .kinds
                    .get(kind)
                    .is_some_and(|verbs| verbs.contains("propose"))
        })
    }

    /// Whether a run may build on this endpoint: `read`, and `write` when the run writes. A
    /// profile that lists no endpoints leaves them to the person and the Endpoint's Policy.
    pub fn grants_endpoint(&self, name: &str, write: bool) -> bool {
        let Some(declared) = self.declared.as_ref().filter(|d| !d.endpoints.is_empty()) else {
            return true;
        };
        declared
            .endpoints
            .get(name)
            .is_some_and(|verbs| verbs.contains("read") && (!write || verbs.contains("write")))
    }

    /// Both halves at the moment of the call: the profile names the operation and the person who
    /// started the run may call it. The error is the reason the refused `tool` event carries.
    pub fn check(
        &self,
        name: &str,
        identity: &Identity,
        state: &AppState,
        project: &str,
    ) -> Result<(), String> {
        let op = ops::find(name).ok_or_else(|| format!("operation '{name}' is not registered"))?;
        if !self.names(op) {
            return Err(refusal(name));
        }
        ops::permitted(op, identity, state, project).map_err(|err| err.to_string())
    }
}

/// Why the profile's half refuses an operation.
pub fn refusal(operation: &str) -> String {
    format!("the agent profile does not grant {operation} (AG-70)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(name: &str) -> &'static Operation {
        ops::find(name).expect("registered")
    }

    #[test]
    fn no_access_block_allows_read_only_operations_only() {
        let access = Access::from_spec(&json!({ "role": "builder" }));
        assert!(access.names(op("jc_catalog_search")));
        assert!(access.names(op("jc_kpi_compute")));
        assert!(!access.names(op("jc_space_complete")));
        // It renders from parameters and opens a change from a manifest or a draft, so it is
        // not read-only and a profile that declares nothing does not get it (T-0917).
        assert!(!access.names(op("jc_endpoint_propose")));
        assert!(!access.names(op("jc_change_approve")));
    }

    /// JC1-04: an operation that declares `read` (jc_pipeline_validate) is offered to a profile
    /// that names it and grants `read` on its kind, as a verbless read is; without `read` it is not.
    #[test]
    fn a_profile_granting_read_is_offered_an_operation_declaring_read() {
        let reads = Access::from_spec(&json!({ "access": {
            "operations": ["jc_pipeline_validate"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read"] }]
        }}));
        assert!(reads.names(op("jc_pipeline_validate")));
        let proposes = Access::from_spec(&json!({ "access": {
            "operations": ["jc_pipeline_validate"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["propose"] }]
        }}));
        assert!(!proposes.names(op("jc_pipeline_validate")));
    }

    /// JC1-02: a profile reads over MCP `resources/*` only the kinds it grants `read`; without an
    /// access block, the person's own reach decides.
    #[test]
    fn a_profile_reads_resources_of_the_kinds_it_grants_read() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_resource_get"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read"] }, { "kind": "Policy", "verbs": ["propose"] }]
        }}));
        assert!(access.reads("Pipeline"));
        assert!(!access.reads("Policy"));
        assert!(!access.reads("RoleBinding"));
        assert!(Access::from_spec(&json!({})).reads("RoleBinding"));
    }

    /// A workspace is no manifest kind, so a profile names its operations and nothing more
    /// (T-1475). Bringing one back stays refused for an agent wherever it is named (AG-82).
    #[test]
    fn a_profile_that_names_the_workspace_operations_is_offered_them() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_workspace_open", "jc_workspace_compare", "jc_workspace_propose"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }]
        }}));
        assert!(access.names(op("jc_workspace_open")));
        assert!(access.names(op("jc_workspace_compare")));
        assert!(
            !access.names(op("jc_workspace_discard")),
            "not named, not offered"
        );
        let agent = ops::Caller {
            bearer: None,
            identity: crate::auth::session::Identity {
                client: None,
                subject: "s".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via: ops::Via::Agent,
            access: None,
        };
        assert!(agent.may_run(op("jc_workspace_propose")).is_err());
    }

    /// Every verbless operation must be reachable by a profile that names it (T-1475, AG-70).
    ///
    /// `names` reads `read` off `access.kinds[op.kind]`, and jc-core refuses a kind in that block
    /// that is not a manifest kind of the catalogue (MF-40). So a verbless operation whose kind is
    /// neither `*` nor a manifest kind can be listed in a profile and never offered — which is how
    /// the assistant was left saying "jc_change_list is unavailable" on dev while the profile named
    /// it. A verbless operation's kind plays no part in `permitted`, so `*` is the honest value.
    #[test]
    fn every_verbless_operation_names_a_kind_a_profile_can_grant() {
        let unreachable: Vec<&str> = ops::registry()
            .iter()
            .filter(|op| op.verb.is_none())
            .filter(|op| op.kind != "*" && crate::resource::by_kind(op.kind).is_none())
            .map(|op| op.name)
            .collect();
        assert!(
            unreachable.is_empty(),
            "no profile could ever be offered these: {unreachable:?}"
        );
    }

    /// The reads of the approval queue, which is the question T-1475 was filed for.
    #[test]
    fn a_profile_that_names_the_change_reads_is_offered_them() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_change_list", "jc_change_get", "jc_change_approve"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }]
        }}));
        assert!(
            access.names(op("jc_change_list")),
            "the approval queue is readable without naming a kind no profile may name"
        );
        assert!(access.names(op("jc_change_get")));
        assert!(
            !access.names(op("jc_change_approve")),
            "deciding stays the person's, named or not (AG-11)"
        );
    }

    #[test]
    fn a_kind_named_at_the_call_needs_the_operation_and_propose_on_that_kind() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_resource_propose"],
            "kinds": [
                { "kind": "Pipeline", "verbs": ["read", "propose"] },
                { "kind": "ContextSpace", "verbs": ["read"] }
            ]
        }}));
        assert!(access.proposes("jc_resource_propose", "Pipeline"));
        assert!(!access.proposes("jc_resource_propose", "ContextSpace"));
        assert!(!access.proposes("jc_resource_propose", "Endpoint"));
        assert!(!access.proposes("jc_endpoint_propose", "Pipeline"));
        assert!(!Access::default().proposes("jc_resource_propose", "Pipeline"));
    }

    #[test]
    fn a_declared_operation_needs_its_kind_verb() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_catalog_search", "jc_endpoint_propose", "jc_kpi_compute", "jc_change_approve"],
            "kinds": [{ "kind": "Endpoint", "verbs": ["read"] }, { "kind": "Change", "verbs": ["read", "propose"] }],
        }}));
        assert!(access.names(op("jc_catalog_search")));
        assert!(access.names(op("jc_kpi_compute")));
        assert!(
            !access.names(op("jc_endpoint_propose")),
            "read does not grant propose"
        );
        assert!(
            !access.names(op("jc_change_approve")),
            "approve is never an agent's"
        );
        assert!(!access.names(op("jc_space_complete")), "not named");
        assert!(
            !access.names(op("jc_manifest_dry_run")),
            "read-only but not named"
        );
    }

    /// Whether the profile admits this call of a kind-`*` operation.
    fn admits(access: &Access, op: &Operation, input: &Value) -> bool {
        access.refused_kind(op, input).is_none()
    }

    /// A kind-`*` operation reaches only the kinds the profile grants, with the verb the call
    /// needs: read for a read-only one, propose otherwise (T-6366, AG-70). Naming the operation
    /// used to be enough, so a run that may read Pipelines read and proposed Policies too.
    #[test]
    fn a_kind_star_operation_needs_the_named_kinds_verb() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": [
                "jc_resource_list", "jc_resource_get", "jc_resource_propose", "jc_resource_delete",
                "jc_draft_put", "jc_draft_get", "jc_draft_drop", "jc_manifest_dry_run"
            ],
            "kinds": [
                { "kind": "Pipeline", "verbs": ["read", "propose"] },
                { "kind": "Endpoint", "verbs": ["read"] }
            ]
        }}));
        let named = |kind: &str| json!({ "kind": kind, "name": "x" });
        assert!(admits(&access, op("jc_resource_get"), &named("Pipeline")));
        assert!(admits(&access, op("jc_resource_list"), &named("Endpoint")));
        assert!(!admits(&access, op("jc_resource_get"), &named("Policy")));
        assert!(!admits(&access, op("jc_resource_list"), &named("Policy")));
        assert!(admits(&access, op("jc_draft_put"), &named("Pipeline")));
        assert!(
            !admits(&access, op("jc_draft_put"), &named("Endpoint")),
            "read is no propose"
        );
        assert!(!admits(&access, op("jc_draft_get"), &named("Policy")));
        assert!(!admits(&access, op("jc_draft_drop"), &named("Endpoint")));
        assert!(!admits(&access, op("jc_resource_delete"), &named("Policy")));
        let manifest = json!({ "manifest": { "kind": "Policy", "metadata": { "name": "x" } } });
        assert!(!admits(&access, op("jc_resource_propose"), &manifest));
        assert!(!admits(&access, op("jc_manifest_dry_run"), &manifest));
        let draft = json!({ "draft": { "kind": "Policy", "name": "x" } });
        assert!(!admits(&access, op("jc_resource_propose"), &draft));
        assert!(admits(
            &access,
            op("jc_resource_propose"),
            &json!({ "draft": { "kind": "Pipeline", "name": "x" } })
        ));
        // Every kind the input names counts: a granted one beside it hides nothing.
        let mixed = json!({ "kind": "Pipeline", "manifest": { "kind": "Policy", "metadata": { "name": "x" } } });
        assert!(!admits(&access, op("jc_resource_propose"), &mixed));
        let mixed = json!({ "kind": "Pipeline", "draft": { "kind": "Policy", "name": "x" } });
        assert!(!admits(&access, op("jc_draft_put"), &mixed));
        // A kind of no manifest is the input's 422, which the call answers.
        assert!(admits(&access, op("jc_resource_get"), &named("Nothing")));
        // A profile without an access block keeps its read-only default, which `names` holds.
        assert!(admits(
            &Access::default(),
            op("jc_resource_get"),
            &named("Policy")
        ));
        assert!(access.reads("Endpoint"));
        assert!(!access.reads("Policy"));
        assert!(Access::default().reads("Policy"));
    }

    /// The door itself refuses, before the person is asked or the input is parsed: one refusal
    /// per operation family through `ops::call` (T-6366).
    #[tokio::test]
    async fn the_door_refuses_a_kind_the_profile_does_not_grant() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_resource_get", "jc_resource_propose", "jc_draft_get", "jc_draft_put"],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read"] }]
        }}));
        let caller = ops::Caller::for_run(
            Identity {
                client: None,
                subject: "s".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            access,
        );
        let manifest = json!({ "kind": "Policy", "metadata": { "name": "x" } });
        for (name, input) in [
            ("jc_resource_get", json!({ "kind": "Policy", "name": "x" })),
            ("jc_resource_propose", json!({ "manifest": manifest })),
            ("jc_draft_get", json!({ "kind": "Policy", "name": "x" })),
            (
                "jc_draft_put",
                json!({ "kind": "Pipeline", "name": "x", "manifest": {} }),
            ),
        ] {
            let refused = ops::call(op(name), &caller, &state, "p", input).await;
            assert!(
                matches!(
                    refused,
                    Err(ops::OpError::Api(crate::error::ApiError::Denied(_)))
                ),
                "{name}: {refused:?}"
            );
        }
    }

    #[test]
    fn listed_endpoints_grant_read_and_write_separately() {
        let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
            { "name": "helsinki-bikes", "verbs": ["read"] },
            { "name": "helsinki-kpi", "verbs": ["read", "write"] },
        ]}}));
        assert!(access.grants_endpoint("helsinki-bikes", false));
        assert!(!access.grants_endpoint("helsinki-bikes", true));
        assert!(access.grants_endpoint("helsinki-kpi", true));
        assert!(!access.grants_endpoint("helsinki-air", false));
        let unlisted = Access::from_spec(&json!({ "access": { "operations": [] } }));
        assert!(unlisted.grants_endpoint("helsinki-air", true));
        assert!(Access::default().grants_endpoint("helsinki-air", true));
    }
}
