//! Stale-entity expiry: a pipeline whose source sends the whole picture deletes what the source
//! stopped sending (PL-64).
//!
//! Two halves. The door refuses an expiry that could delete what is not the pipeline's: another
//! Pipeline of the project writes the same space, a type is not a class of the space's model, or
//! no Policy grants the pipelines account the delete. The reconciler then renders the sweep, a
//! second stream `{pipeline}.expiry` on the project's runner, which once an hour pages through the
//! listed types on the output Endpoint and deletes the ids whose `modifiedAt` (or `createdAt`) is
//! older than the window, through the same Endpoint and as the same account, so the gateway judges
//! and audits every delete like a write.

use crate::error::ApiError;
use crate::state::AppState;
use crate::store::Mirror;
use jc_core::kinds::{Expiry, Operation, OperationRef, PipelineSpec, PolicySpec, PrincipalKind};
use serde_json::{json, Value};

/// The stream name of a pipeline's sweep. A pipeline name is a DNS label and has no dot, so the
/// sweep never shares a name with a pipeline.
pub fn sweep_name(pipeline: &str) -> String {
    format!("{pipeline}.expiry")
}

/// How often the sweep runs: an entity goes within an hour after its window passes.
const SWEEP_INTERVAL: &str = "1h";

/// Entities read per request; a broker that caps lower answers fewer and the sweep pages on.
const PAGE: u32 = 1000;

/// The most pages one sweep reads, so a sweep that cannot advance stops instead of looping.
/// ponytail: a million entities per type set per hour; a space past that needs a server-side
/// `modifiedAt` filter on the query API.
const MAX_PAGES: u32 = 1000;

/// The space an Endpoint writes into, by its `contextSpaceRef` as a name or a reference.
pub fn space_of_endpoint(mirror: &Mirror, project: &str, endpoint: &str) -> Option<String> {
    let envelope = mirror.get(project, "Endpoint", endpoint)?;
    let reference = envelope.spec.get("contextSpaceRef")?;
    reference
        .as_str()
        .or_else(|| reference.get("name").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The spaces a pipeline writes, one per output whose Endpoint the mirror holds.
fn spaces_written(mirror: &Mirror, project: &str, spec: &PipelineSpec) -> Vec<String> {
    spec.outputs()
        .iter()
        .filter_map(|output| space_of_endpoint(mirror, project, output.target_endpoint.local_id()))
        .collect()
}

/// Refuses a pipeline whose entities a sweep could delete when they are not the sweeping
/// pipeline's, with every reason at once (PL-64, CC-24): an expiry while another pipeline writes
/// the space, a type the model does not have, a delete no Policy grants, and a second writer into
/// a space another pipeline sweeps. Anything but a Pipeline passes untouched.
/// ponytail: an Endpoint moved to another space is not re-judged here; the next edit of either
/// pipeline is.
pub fn check(
    state: &AppState,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    if kind != "Pipeline" {
        return Ok(());
    }
    // jc-core has refused a malformed spec before this door (one output, the window, the types).
    let Ok(pipeline) = serde_json::from_value::<PipelineSpec>(spec.clone()) else {
        return Ok(());
    };
    let problems = match pipeline.expiry.as_ref() {
        Some(expiry) => problems(state, project, name, &pipeline, expiry),
        None => swept_by_another(&state.mirror, project, name, &pipeline),
    };
    let ungranted = source_delete_grants(state, project, name, &pipeline);
    if problems.is_empty() && !ungranted.is_empty() {
        return Err(ApiError::Invalid {
            detail: "spec.sourceDeletes deletes through the pipeline's Endpoint, and no Policy \
                     lets it (PL-99)"
                .to_owned(),
            errors: ungranted,
        });
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(ApiError::Invalid {
        detail: "the pipeline's entities are not safe from a stale-entity sweep (PL-64)".to_owned(),
        errors: problems,
    })
}

/// A pipeline writing into a space another pipeline sweeps would lose its entities to that sweep.
fn swept_by_another(
    mirror: &Mirror,
    project: &str,
    name: &str,
    pipeline: &PipelineSpec,
) -> Vec<String> {
    let written = spaces_written(mirror, project, pipeline);
    if written.is_empty() {
        return Vec::new();
    }
    let mut problems = Vec::new();
    for (other, other_spec) in pipelines(mirror, project, name) {
        let Some(expiry) = other_spec.expiry.as_ref() else {
            continue;
        };
        for space in spaces_written(mirror, project, &other_spec) {
            if written.contains(&space) {
                problems.push(format!(
                    "pipeline {other} removes {} entities not updated for {} in space {space}, \
                     and would remove this pipeline's too: write into another space, or turn \
                     expiry off on {other}",
                    expiry.types.join(", "),
                    expiry.after
                ));
            }
        }
    }
    problems
}

/// Every other Pipeline of the project that parses, by name.
fn pipelines(mirror: &Mirror, project: &str, name: &str) -> Vec<(String, PipelineSpec)> {
    mirror
        .list(project, "Pipeline", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .filter(|other| other.metadata.name != name)
        .filter_map(|other| {
            serde_json::from_value(other.spec)
                .ok()
                .map(|spec| (other.metadata.name, spec))
        })
        .collect()
}

fn problems(
    state: &AppState,
    project: &str,
    name: &str,
    pipeline: &PipelineSpec,
    expiry: &Expiry,
) -> Vec<String> {
    let mirror = &state.mirror;
    let Some(output) = pipeline.outputs().into_iter().next() else {
        return vec!["expiry needs the one output the pipeline writes".to_owned()];
    };
    let endpoint = output.target_endpoint.local_id().to_owned();
    let Some(space) = space_of_endpoint(mirror, project, &endpoint) else {
        // The reference check names a missing Endpoint; nothing here can be judged without it.
        return Vec::new();
    };
    let mut problems = Vec::new();

    // Nothing on an entity says which pipeline wrote it, so the space is the scope.
    for (other, other_spec) in pipelines(mirror, project, name) {
        if spaces_written(mirror, project, &other_spec).contains(&space) {
            problems.push(format!(
                "pipeline {other} also writes into space {space}, so expiry would delete its \
                 entities too: give this pipeline a space of its own, or leave expiry off"
            ));
        }
    }

    if let Some(model) = state.model_schemas.get(project, &space) {
        for entity_type in &expiry.types {
            if !model.classes.contains_key(entity_type) {
                problems.push(format!(
                    "{entity_type} is not a class of model {}, the model of space {space}",
                    model.model
                ));
            }
        }
    }

    let (account, policies) = grants_of(state, project, name, pipeline);
    for (operation, wire) in [
        (Operation::QueryBatch, "queryBatch"),
        (Operation::DeleteBatch, "deleteBatch"),
    ] {
        let missing: Vec<&str> = expiry
            .types
            .iter()
            .filter(|t| !granted(&policies, &space, &account, operation, Some(t)))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let extend = pipeline_policy(&policies, &space, &account)
                .map_or_else(|| "a Policy".to_owned(), |name| format!("Policy {name}"));
            problems.push(format!(
                "no Policy grants the pipelines account {wire} on {} in space {space}: add it \
                 to {extend} in the same change, knowing that it lets the pipeline delete",
                missing.join(", ")
            ));
        }
    }
    problems
}

fn policies(mirror: &Mirror, project: &str) -> Vec<(String, PolicySpec)> {
    mirror
        .list(project, "Policy", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .filter_map(|p| {
            serde_json::from_value::<PolicySpec>(p.spec.clone())
                .ok()
                .map(|spec| (p.metadata.name, spec))
        })
        // One grant per rule, `{policy}#{n}` (ADR-N-057); an invalid shape grants nothing.
        .flat_map(|(name, spec)| spec.expand(&name).unwrap_or_default())
        .collect()
}

/// A `snapshot` or `events` pipeline deletes through each output Endpoint: every output's space
/// must grant the pipelines account `queryBatch` and `deleteBatch` on the output's type, or on
/// every type for an untyped output (PL-99). The Portal never widens a grant for it.
/// What a source delete stream does on the Endpoint: read the chunk, then delete it.
pub(crate) const SOURCE_DELETE_OPERATIONS: [(Operation, &str); 2] = [
    (Operation::QueryBatch, "queryBatch"),
    (Operation::DeleteBatch, "deleteBatch"),
];

fn source_delete_grants(
    state: &AppState,
    project: &str,
    name: &str,
    pipeline: &PipelineSpec,
) -> Vec<String> {
    use jc_core::kinds::SourceDeleteMode;
    let deletes = pipeline.source_deletes.as_ref().is_some_and(|rule| {
        matches!(
            rule.mode,
            SourceDeleteMode::Snapshot | SourceDeleteMode::Events
        )
    });
    if !deletes {
        return Vec::new();
    }
    let mirror = &state.mirror;
    let (account, policies) = grants_of(state, project, name, pipeline);
    let mut problems = Vec::new();
    for output in pipeline.outputs() {
        let Some(space) = space_of_endpoint(mirror, project, output.target_endpoint.local_id())
        else {
            continue;
        };
        let entity_type = output.entity_type.as_deref();
        for (operation, wire) in SOURCE_DELETE_OPERATIONS {
            if granted(&policies, &space, &account, operation, entity_type) {
                continue;
            }
            let extend = pipeline_policy(&policies, &space, &account)
                .map_or_else(|| "a Policy".to_owned(), |name| format!("Policy {name}"));
            problems.push(format!(
                "no Policy grants the pipelines account {wire} on {} in space {space}: add it \
                 to {extend} in the same change, knowing that it lets the pipeline delete what \
                 its source drops",
                entity_type.unwrap_or("every type")
            ));
        }
    }
    problems
}

/// Whether a permission Policy on `space` for the pipelines account grants `operation` on every
/// entity of `entity_type`: named directly or through its group, over no `information` (every
/// type) or over the type with no id narrowing it. `None` asks for every type.
fn granted(
    policies: &[(String, PolicySpec)],
    space: &str,
    account: &str,
    operation: Operation,
    entity_type: Option<&str>,
) -> bool {
    policies.iter().any(|(_, policy)| {
        !policy.effect.is_prohibition()
            && for_pipelines(policy, space, account)
            && policy.operations.iter().any(|op| match op {
                OperationRef::Single(single) => *single == operation,
                OperationRef::Group(group) => group.operations().contains(&operation),
            })
            && (policy.information.is_empty()
                || policy.information.iter().any(|info| {
                    info.entities.iter().any(|selector| {
                        Some(selector.entity_type.as_str()) == entity_type
                            && selector.id.is_none()
                            && selector.id_pattern.is_none()
                    })
                }))
            && policy.q.is_none()
            && policy.scope_q.is_none()
            && policy.geo_q.is_none()
            && policy.temporal_q.is_none()
    })
}

fn for_pipelines(policy: &PolicySpec, space: &str, account: &str) -> bool {
    policy.context_space_ref.name() == space
        && policy.assignee.kind == PrincipalKind::ServiceAccount
        && policy.assignee.id == account
}

/// The account the pipeline deletes as and the Policies that bind it: under per-pipeline
/// identity its own derived ones, as the stream writes (F-POR3-018); else the shared
/// `pipelines` account and the project's Policies.
fn grants_of(
    state: &AppState,
    project: &str,
    name: &str,
    pipeline: &PipelineSpec,
) -> (String, Vec<(String, PolicySpec)>) {
    match crate::record_check::identity(state, project, name, pipeline) {
        (account, Some(derived)) => (
            account,
            derived
                .into_iter()
                .flat_map(|(name, spec)| spec.expand(&name).unwrap_or_default())
                .collect(),
        ),
        (account, None) => (account, policies(&state.mirror, project)),
    }
}

/// The Policy the author extends: the pipelines account's write grant on the space.
fn pipeline_policy<'a>(
    policies: &'a [(String, PolicySpec)],
    space: &str,
    account: &str,
) -> Option<&'a str> {
    policies
        .iter()
        .find(|(_, policy)| {
            !policy.effect.is_prohibition() && for_pipelines(policy, space, account)
        })
        .map(|(name, _)| PolicySpec::policy_of(name))
}

/// The sweep of one pipeline: once an hour, every page of the listed types on the output
/// Endpoint, the ids older than the window deleted through the same Endpoint (PL-64).
///
/// The offset advances by what a page kept: the entities it read minus the ones the gateway
/// confirmed deleted, so a failed delete is stepped over rather than read forever. A failed read
/// or delete ends that hour's sweep and is logged; the next hour starts again from the top.
///
/// `None` when the window or the types are not what PL-64 allows: the reconciler reads a
/// Pipeline with serde, not jc-core's validation, and a window read as zero would delete every
/// entity of the types, a query naming no type every entity of the space.
pub fn render_sweep(project: &str, slug: &str, expiry: &Expiry) -> Option<Value> {
    use jc_core::kinds::pipeline::{EXPIRY_MAX_HOURS, EXPIRY_MAX_TYPES};
    let hours = expiry
        .hours()
        .filter(|hours| (1..=EXPIRY_MAX_HOURS).contains(hours))?;
    let named = &expiry.types;
    let types_valid = (1..=EXPIRY_MAX_TYPES).contains(&named.len())
        && named.iter().enumerate().all(|(at, entity_type)| {
            jc_core::names::validate_entity_type(entity_type).is_ok()
                && !named[..at].contains(entity_type)
        });
    if !types_valid {
        return None;
    }
    let seconds = hours * 3600;
    let types = json!(expiry
        .types
        .iter()
        .map(|t| json!({ "type": t }))
        .collect::<Vec<_>>());
    let base = format!("${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations");
    let http = |url: String| {
        json!({
            "url": url,
            "verb": "POST",
            "headers": { "Content-Type": "application/json", "Accept": "application/json" },
            "oauth2": crate::reconciler::streams::pipeline_oauth2(project),
            "timeout": "30s",
            "rate_limit": crate::reconciler::streams::egress_label(project)
        })
    };
    Some(json!({
        "input": {
            "generate": {
                "interval": SWEEP_INTERVAL,
                "mapping": "root = {\"offset\": 0, \"more\": true}"
            }
        },
        "pipeline": {
            "processors": [{
                "while": {
                    "check": "this.more",
                    "max_loops": MAX_PAGES,
                    "processors": [
                        { "try": [
                            { "branch": {
                                "request_map": format!(
                                    "meta expiry_offset = this.offset.string()\nroot = {{\"type\": \"Query\", \"entities\": {types}}}"
                                ),
                                "processors": [{ "http": http(format!(
                                    "{base}/query?options=sysAttrs&limit={PAGE}&offset=${{! @expiry_offset }}"
                                )) }],
                                "result_map": "root.page = this"
                            } },
                            { "mapping": format!(
                                "let cutoff = now().ts_unix() - {seconds}\n\
                                 let stale = this.page.map_each(e -> {{\"id\": e.id, \"at\": (e.modifiedAt | e.createdAt | \"\").ts_unix().catch(null)}}).filter(x -> x.at != null && x.at < $cutoff).map_each(x -> x.id)\n\
                                 root = this\n\
                                 root.read = this.page.length()\n\
                                 root.stale = $stale\n\
                                 root.page = deleted()\n\
                                 meta expiry_stale = $stale.length().string()"
                            ) },
                            { "branch": {
                                "request_map": "root = if this.stale.length() == 0 { deleted() } else { this.stale }",
                                "processors": [{ "http": http(format!("{base}/delete")) }],
                                "result_map": "root.deleted = if content().length() == 0 { @expiry_stale.number() } else { this.success.or([]).length() }"
                            } },
                            { "mapping": "root = this\nroot.offset = this.offset + this.read - this.deleted.or(0)\nroot.more = this.read > 0\nroot.stale = deleted()\nroot.read = deleted()\nroot.deleted = deleted()" }
                        ] },
                        { "catch": [
                            { "log": { "level": "ERROR", "message": "expiry sweep stopped for this hour: ${! error() }" } },
                            { "mapping": "root = this\nroot.more = false" }
                        ] }
                    ]
                }
            }]
        },
        "output": { "drop": {} }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::pipeline_validation::ModelSchema;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, "helsinki"),
            spec,
            status: None,
        }
    }

    fn endpoint(name: &str, space: &str) -> ResourceEnvelope {
        manifest(
            "Endpoint",
            name,
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space }, "slug": name }),
        )
    }

    fn pipeline_spec(endpoint: &str, expiry: Option<Value>) -> Value {
        let mut spec = json!({
            "class": "resident",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
            "outputs": [{ "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:helsinki:{endpoint}") }],
        });
        if let Some(expiry) = expiry {
            spec["expiry"] = expiry;
        }
        spec
    }

    fn grant(name: &str, space: &str, operations: Value, information: Value) -> ResourceEnvelope {
        manifest(
            "Policy",
            name,
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                "assigner": "did:web:hel.fi",
                "assignee": { "kind": "serviceAccount", "id": "pipelines" },
                "operations": operations,
                "information": information,
            }),
        )
    }

    /// Helsinki with the vehicles space, its write endpoint, a model of Vehicle and Bus, and the
    /// pipelines account allowed to query and delete there.
    fn world(extra: Vec<ResourceEnvelope>) -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(endpoint("ep-vehicles", "vehicles"));
        state.mirror.upsert(endpoint("ep-stops", "stops"));
        state.mirror.upsert(grant(
            "vehicles-pipelines-write",
            "vehicles",
            json!(["upsertBatch", "createBatch", "queryBatch", "deleteBatch"]),
            json!([]),
        ));
        for envelope in extra {
            state.mirror.upsert(envelope);
        }
        let model = ModelSchema::compile(
            "transport",
            "1.0.0",
            &json!({}),
            &["Vehicle".to_owned(), "Bus".to_owned()],
            false,
        );
        state.model_schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "vehicles".to_owned()),
            Arc::new(model),
        )]));
        state
    }

    fn expiry(types: &[&str]) -> Option<Value> {
        Some(json!({ "after": "14d", "types": types }))
    }

    fn refusals(state: &AppState, name: &str, spec: Value) -> Vec<String> {
        match check(state, "helsinki", "Pipeline", name, &spec) {
            Ok(()) => Vec::new(),
            Err(ApiError::Invalid { errors, .. }) => errors,
            Err(other) => panic!("not a list of reasons: {other:?}"),
        }
    }

    #[test]
    fn the_only_writer_of_a_space_with_the_grant_may_expire_its_model_types() {
        let state = world(Vec::new());
        let spec = pipeline_spec("ep-vehicles", expiry(&["Vehicle", "Bus"]));
        assert_eq!(refusals(&state, "vehicles", spec), Vec::<String>::new());
        // Proposing it again, with itself in the mirror, is not a second writer.
        let state = world(vec![manifest(
            "Pipeline",
            "vehicles",
            pipeline_spec("ep-vehicles", expiry(&["Vehicle"])),
        )]);
        let spec = pipeline_spec("ep-vehicles", expiry(&["Vehicle"]));
        assert_eq!(refusals(&state, "vehicles", spec), Vec::<String>::new());
    }

    #[test]
    fn another_writer_into_the_space_and_a_type_outside_the_model_are_named() {
        let state = world(vec![
            manifest("Pipeline", "buses", pipeline_spec("ep-vehicles", None)),
            manifest("Pipeline", "stops", pipeline_spec("ep-stops", None)),
        ]);
        let found = refusals(
            &state,
            "vehicles",
            pipeline_spec("ep-vehicles", expiry(&["Vehicle", "Tram"])),
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("pipeline buses also writes into space vehicles"));
        assert!(found[1].contains("Tram is not a class of model transport"));
        assert!(!found.join(" ").contains("stops"), "{found:?}");
    }

    #[test]
    fn a_delete_nobody_granted_is_refused_naming_the_policy_to_extend() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(endpoint("ep-vehicles", "vehicles"));
        state.mirror.upsert(grant(
            "vehicles-pipelines-write",
            "vehicles",
            json!(["upsertBatch", "createBatch", "queryBatch"]),
            json!([]),
        ));
        let spec = pipeline_spec("ep-vehicles", expiry(&["Vehicle"]));
        let found = refusals(&state, "vehicles", spec.clone());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("deleteBatch on Vehicle in space vehicles"));
        assert!(found[0].contains("Policy vehicles-pipelines-write"));

        // Grants that do not reach every entity of the type, or are not the pipelines
        // account's, or take access away, do not count.
        let narrow = |name: &str, information: Value| {
            grant(name, "vehicles", json!(["deleteBatch"]), information)
        };
        state.mirror.upsert(narrow(
            "by-pattern",
            json!([{ "entities": [{ "type": "Vehicle", "idPattern": "^urn:x:.*$" }] }]),
        ));
        state.mirror.upsert(narrow(
            "other-type",
            json!([{ "entities": [{ "type": "Bus" }] }]),
        ));
        let mut elsewhere = grant("elsewhere", "stops", json!(["deleteBatch"]), json!([]));
        state.mirror.upsert(elsewhere.clone());
        elsewhere.metadata.name = "someone-else".to_owned();
        elsewhere.spec["contextSpaceRef"]["name"] = json!("vehicles");
        elsewhere.spec["assignee"] = json!({ "kind": "serviceAccount", "id": "agent" });
        state.mirror.upsert(elsewhere);
        let mut denied = narrow("denied", json!([]));
        denied.spec["effect"] = json!("prohibition");
        state.mirror.upsert(denied);
        assert_eq!(refusals(&state, "vehicles", spec.clone()).len(), 1);

        // The type named with no id narrowing it is the grant.
        state.mirror.upsert(narrow(
            "vehicles-delete",
            json!([{ "entities": [{ "type": "Vehicle" }] }]),
        ));
        assert_eq!(refusals(&state, "vehicles", spec), Vec::<String>::new());
    }

    /// PL-99: a pipeline that deletes what its source drops needs the pipelines account's
    /// queryBatch and deleteBatch in every space it writes, on its type or on every type.
    #[test]
    fn source_deletes_need_the_delete_grant_and_never_and_lease_do_not() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(endpoint("ep-stops", "stops"));
        state.mirror.upsert(grant(
            "stops-pipelines-write",
            "stops",
            json!(["upsertBatch", "queryBatch"]),
            json!([]),
        ));
        let with = |deletes: Value, typed: bool| {
            let mut spec = pipeline_spec("ep-stops", None);
            spec["sourceDeletes"] = deletes;
            if typed {
                spec["outputs"][0]["type"] = json!("Stop");
            }
            spec
        };
        let snapshot = json!({ "mode": "snapshot" });
        let found = refusals(&state, "stops", with(snapshot.clone(), false));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("deleteBatch on every type in space stops"));
        assert!(found[0].contains("Policy stops-pipelines-write"));
        let events = json!({ "mode": "events", "deletedWhen": "this.status == \"withdrawn\"" });
        assert_eq!(refusals(&state, "stops", with(events, true)).len(), 1);
        for passes in [json!({ "mode": "never" }), json!({ "mode": "lease" })] {
            assert!(refusals(&state, "stops", with(passes, false)).is_empty());
        }
        // The type named, or every type, is the grant.
        state.mirror.upsert(grant(
            "stops-delete",
            "stops",
            json!(["deleteBatch"]),
            json!([{ "entities": [{ "type": "Stop" }] }]),
        ));
        assert!(refusals(&state, "stops", with(snapshot.clone(), true)).is_empty());
        assert_eq!(
            refusals(&state, "stops", with(snapshot.clone(), false)).len(),
            1
        );
        state.mirror.upsert(grant(
            "stops-delete-all",
            "stops",
            json!(["deleteBatch"]),
            json!([]),
        ));
        assert!(refusals(&state, "stops", with(snapshot, false)).is_empty());
    }

    /// F-POR3-018: under per-pipeline identity the stream deletes as `pl-{pipeline}` with its
    /// derived Policies, so those are what the door judges. Since PL-96 the derivation grants
    /// deleteBatch to the write Policy of a snapshot or events rule, and the delete always
    /// lands in the pipeline's own write target, so the expiry and both source-delete modes
    /// pass with no shared grant; no delete is left that the derived Policies do not hold.
    #[test]
    fn under_pipeline_identity_the_derived_policies_admit_expiry_and_source_deletes() {
        let state = AppState::new(
            Config {
                pipeline_identity: true,
                ..Config::for_tests()
            },
            None,
        );
        state.mirror.upsert(endpoint("ep-vehicles", "vehicles"));
        let swept = pipeline_spec("ep-vehicles", expiry(&["Vehicle"]));
        assert_eq!(refusals(&state, "vehicles", swept), Vec::<String>::new());

        for deletes in [
            json!({ "mode": "snapshot" }),
            json!({ "mode": "events", "deletedWhen": "this.status == \"withdrawn\"" }),
        ] {
            let mut spec = pipeline_spec("ep-vehicles", None);
            spec["sourceDeletes"] = deletes;
            spec["outputs"][0]["type"] = json!("Vehicle");
            assert_eq!(refusals(&state, "vehicles", spec), Vec::<String>::new());
        }
    }

    #[test]
    fn a_new_writer_into_a_swept_space_is_refused_and_elsewhere_passes() {
        let state = world(vec![manifest(
            "Pipeline",
            "vehicles",
            pipeline_spec("ep-vehicles", expiry(&["Vehicle"])),
        )]);
        let found = refusals(&state, "buses", pipeline_spec("ep-vehicles", None));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("pipeline vehicles removes Vehicle entities not updated for 14d"));
        assert_eq!(
            refusals(&state, "stops", pipeline_spec("ep-stops", None)),
            Vec::<String>::new()
        );
        // Any other kind passes this door untouched.
        assert!(check(&state, "helsinki", "Endpoint", "x", &json!({})).is_ok());
    }

    /// The reconciler reads a Pipeline with serde, not jc-core's validate(), so a window or a
    /// type list the door would refuse can still reach the render. It renders no sweep: an
    /// unparsed window read as zero was a cutoff of now, every entity of the types deleted, and
    /// a query naming no type reads them all.
    #[test]
    fn a_window_or_types_that_do_not_validate_render_no_sweep() {
        let sweep = |after: &str, types: Vec<&str>| {
            let expiry: Expiry =
                serde_json::from_value(json!({ "after": after, "types": types })).expect("expiry");
            render_sweep("helsinki", "ep-slug", &expiry)
        };
        for after in [
            "30m", "0h", "0d", "366d", "8761h", "", "d", "14", "-1d", "1.5d",
        ] {
            assert!(
                sweep(after, vec!["Vehicle"]).is_none(),
                "{after:?} renders a sweep"
            );
        }
        assert!(sweep("14d", vec![]).is_none(), "no type names every type");
        assert!(
            sweep("14d", vec!["Vehicle"; 2]).is_none(),
            "a type named twice"
        );
        let many: Vec<String> = (0..=jc_core::kinds::pipeline::EXPIRY_MAX_TYPES)
            .map(|at| format!("Type{at}"))
            .collect();
        assert!(sweep("14d", many.iter().map(String::as_str).collect()).is_none());
        assert!(sweep("14d", vec!["not a type"]).is_none());
        assert!(sweep("1h", vec!["Vehicle"]).is_some());
        assert!(sweep("365d", vec!["Vehicle"]).is_some());
    }

    #[test]
    fn the_sweep_reads_and_deletes_through_the_output_endpoint_as_the_pipelines_account() {
        let expiry: Expiry =
            serde_json::from_value(json!({ "after": "2d", "types": ["Vehicle", "Bus"] }))
                .expect("expiry");
        let sweep = render_sweep("helsinki", "ep-slug", &expiry).expect("a valid expiry");
        assert_eq!(sweep["input"]["generate"]["interval"], "1h");
        assert_eq!(sweep["output"], json!({ "drop": {} }));
        let body = &sweep["pipeline"]["processors"][0]["while"];
        assert_eq!(body["max_loops"], MAX_PAGES);
        let steps = &body["processors"][0]["try"];
        let read = &steps[0]["branch"];
        assert!(read["request_map"]
            .as_str()
            .is_some_and(|m| m.contains(r#"[{"type":"Vehicle"},{"type":"Bus"}]"#)));
        let query = &read["processors"][0]["http"];
        assert_eq!(
            query["url"],
            "${JC_GATEWAY_URL}/api/endpoint/ep-slug/ngsi-ld/v1/entityOperations/query\
             ?options=sysAttrs&limit=1000&offset=${! @expiry_offset }"
        );
        assert!(steps[1]["mapping"]
            .as_str()
            .is_some_and(|m| m.contains("now().ts_unix() - 172800")));
        let delete = &steps[2]["branch"]["processors"][0]["http"];
        assert_eq!(
            delete["url"],
            "${JC_GATEWAY_URL}/api/endpoint/ep-slug/ngsi-ld/v1/entityOperations/delete"
        );
        for http in [query, delete] {
            assert_eq!(http["verb"], "POST");
            assert_eq!(http["oauth2"]["client_key"], "helsinki-pipelines");
            assert_eq!(
                http["oauth2"]["client_secret"],
                "${JC_CLIENT_SECRET_HELSINKI}"
            );
        }
        assert_eq!(sweep_name("vehicles"), "vehicles.expiry");
    }
}
