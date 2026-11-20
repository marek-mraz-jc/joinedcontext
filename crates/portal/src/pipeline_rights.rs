//! A pipeline reaches only what its people reach (PL-94, T-4893): the person who proposes or
//! approves a Pipeline reads every source it reads and may write every space it writes.
//!
//! The pipeline runs as `pl-{pipeline}` with its Endpoints' grants (PL-19), so without this check
//! a person who may propose Pipelines could route a DataSource or a space they cannot read into
//! one they can, with the platform's account as the deputy. Every door that writes a Pipeline
//! asks [`check`]: the resource route and the operations behind it, an import, a blueprint and
//! both approval paths.
//!
//! What "reads" means is what the Portal already shows the person: `read` on the DataSource
//! manifest (its constraints included), `read` on `Entity` in a source Endpoint's space (DM-74),
//! and for a target the door's own verb on `Pipeline` in the target's space (T-4593) with `read`
//! on `Entity` there. A source Endpoint whose audience is public reads what anybody reads. An
//! Endpoint the mirror does not hold yet says nothing here: the reference check refuses a name
//! that never appears.
//!
//! A source Endpoint of another project is that project's to admit at the gateway, and to copy
//! only where its Policy permits `store` to the pipeline's account ([`unstored`], SHR-03): that
//! refusal binds every door's caller, a bootstrap admin too, and the reconciler at each render.

use crate::error::ApiError;
use crate::permissions::{effective, Effective};
use crate::state::AppState;
use crate::store::Mirror;
use jc_core::kinds::{PipelineSpec, PlatformAction, PolicySpec, PrincipalKind, Verb};
use jc_time::TimestampTz;
use serde_json::Value;

/// `Ok` unless `manifest` is a Pipeline of `project` that reads or writes something `identity`
/// could not with `verb` (`Propose` at the propose doors, `Approve` at approval). The 403 names
/// only references the manifest itself holds, never the space behind them.
pub fn check(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    manifest: &Value,
    verb: Verb,
) -> Result<(), ApiError> {
    // The provider's agreement binds every proposer, a bootstrap admin included (SHR-03).
    let unstored = unstored(
        &state.mirror,
        project,
        manifest,
        &account(state.config.pipeline_identity, manifest),
    );
    if !unstored.is_empty() {
        return Err(ApiError::Unprocessable {
            detail: "a pipeline copies another project's Endpoint only where that project's \
                     Policy permits store (SHR-03)"
                .to_owned(),
            errors: unstored,
        });
    }
    let missing = missing(
        &state.mirror,
        &state.config.bootstrap_admins,
        identity,
        project,
        manifest,
        verb,
    );
    if missing.is_empty() {
        return Ok(());
    }
    let who = if verb == Verb::Approve {
        "approve"
    } else {
        "propose"
    };
    Err(ApiError::Denied(format!(
        "No access to {}: a pipeline reads and writes only what the person who may {who} it reads \
         and writes, ask for read on the source or write on the target first (PL-94)",
        missing.join(", ")
    )))
}

/// The references of the Pipeline `manifest` that `identity` cannot reach with `verb` today,
/// each once, as the manifest names them: what [`check`] refuses at a door, and what the
/// reconciler re-asks for a Pipeline's recorded authors at each render (PL-95).
pub fn missing(
    mirror: &Mirror,
    bootstrap_group: &str,
    identity: &crate::auth::session::Identity,
    project: &str,
    manifest: &Value,
    verb: Verb,
) -> Vec<String> {
    if manifest.get("kind").and_then(Value::as_str) != Some("Pipeline") {
        return Vec::new();
    }
    let effective = effective(
        mirror,
        bootstrap_group,
        identity,
        project,
        TimestampTz::now(),
    );
    if effective.bootstrap {
        return Vec::new();
    }
    // A spec jc-core cannot read is refused by the kind's own validation at every door.
    let Some(spec) = manifest
        .get("spec")
        .and_then(|spec| serde_json::from_value::<PipelineSpec>(spec.clone()).ok())
    else {
        return Vec::new();
    };
    unreachable(mirror, &effective, project, &spec, verb)
}

/// The references of `spec` the caller lacks, each once, as the manifest names them.
fn unreachable(
    mirror: &Mirror,
    effective: &Effective,
    project: &str,
    spec: &PipelineSpec,
    verb: Verb,
) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let mut lacks = |item: String| {
        if !missing.contains(&item) {
            missing.push(item);
        }
    };
    for source in spec.sources() {
        if let Some(reference) = &source.data_source_ref {
            let name = reference.name();
            let readable = match mirror.get(project, "DataSource", name) {
                // `check` asks the verb itself; `propose` reads too (jc-core `Rule::grants`).
                Some(envelope) => serde_json::to_value(&envelope).is_ok_and(|manifest| {
                    [Verb::Read, Verb::Propose]
                        .into_iter()
                        .any(|held| effective.check("DataSource", held, Some(&manifest)).is_ok())
                }),
                None => effective.may_read("DataSource"),
            };
            if !readable {
                lacks(format!("DataSource {name}"));
            }
        }
        if let Some(reference) = &source.endpoint_ref {
            let name = reference.name();
            // Another project's Endpoint (PL-90) is that project's to admit, when the run
            // renders: a same-named Endpoint here says nothing about it (T-5702).
            if reference.namespace().is_some_and(|owner| owner != project) {
                continue;
            }
            // An Endpoint this project does not hold is no read the person holds either.
            let Some(endpoint) = mirror.get(project, "Endpoint", name) else {
                lacks(format!("source Endpoint {name}"));
                continue;
            };
            let public = endpoint
                .spec
                .get("audience")
                .and_then(Value::as_str)
                .is_none_or(|audience| audience == "public");
            let readable = public
                || crate::pipeline_expiry::space_of_endpoint(mirror, project, name)
                    .is_some_and(|space| effective.may_read_in("Entity", Some(&space)));
            if !readable {
                lacks(format!("source Endpoint {name}"));
            }
        }
    }
    for output in spec.outputs() {
        let name = output.target_endpoint.local_id();
        let Some(space) = crate::pipeline_expiry::space_of_endpoint(mirror, project, name) else {
            continue;
        };
        if !(effective.may_in("Pipeline", verb, Some(&space))
            && effective.may_read_in("Entity", Some(&space)))
        {
            lacks(format!("target Endpoint {name}"));
        }
    }
    missing
}

/// The account the Pipeline `manifest` runs as: its own `pl-{pipeline}` under per-pipeline
/// identity (PL-19), else its project's shared `pipelines` account.
pub fn account(pipeline_identity: bool, manifest: &Value) -> String {
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match pipeline_identity {
        true => jc_core::kinds::pipeline_identity::account_name(name),
        false => crate::reconciler::streams::PIPELINE_ACCOUNT.to_owned(),
    }
}

/// One entry per source Endpoint of another project that the Pipeline `manifest` of `project`
/// would copy without the provider's consent (SHR-03): no permission Policy of that Endpoint's
/// space, in force now, names the pipeline's `account` as `serviceAccount {project}/{account}`
/// with the action `store`, or a prohibition takes `store` from it. Each entry is
/// `<field>: <reason>` and names the Policy to change. The doors refuse on it and the
/// reconciler holds the pipeline out on it, so a revoked `store` stops the next run. `store`
/// grants no read: the gateway still judges every read by the Policy's own operations.
pub fn unstored(mirror: &Mirror, project: &str, manifest: &Value, account: &str) -> Vec<String> {
    if manifest.get("kind").and_then(Value::as_str) != Some("Pipeline") {
        return Vec::new();
    }
    let Some(spec) = manifest
        .get("spec")
        .and_then(|spec| serde_json::from_value::<PipelineSpec>(spec.clone()).ok())
    else {
        return Vec::new();
    };
    let principal = format!("{project}/{account}");
    let now = TimestampTz::now();
    let mut refused = Vec::new();
    for (index, source) in spec.sources().iter().enumerate() {
        let Some(reference) = &source.endpoint_ref else {
            continue;
        };
        let Some(owner) = reference.namespace().filter(|owner| *owner != project) else {
            continue;
        };
        let name = reference.name();
        // An Endpoint the mirror does not hold is the reference check's refusal, not this one.
        let Some(space) = crate::pipeline_expiry::space_of_endpoint(mirror, owner, name) else {
            continue;
        };
        let field = match spec.is_second_shape() {
            true => format!("spec.sources[{index}].endpointRef"),
            false => "spec.source.endpointRef".to_owned(),
        };
        let policies: Vec<_> = crate::pipeline_expiry::policies(mirror, owner)
            .into_iter()
            .filter(|(_, policy)| {
                policy.context_space_ref.name() == space
                    && policy.assignee.kind == PrincipalKind::ServiceAccount
                    && policy.assignee.id == principal
                    && in_force(policy, now)
            })
            .collect();
        let stores = |prohibition: bool| {
            policies.iter().any(|(_, policy)| {
                policy.effect.is_prohibition() == prohibition
                    && policy.actions.contains(&PlatformAction::Store)
            })
        };
        if stores(false) && !stores(true) {
            continue;
        }
        let grant = policies
            .iter()
            .find(|(_, policy)| !policy.effect.is_prohibition())
            .map_or_else(
                || format!("a Policy of space {space} for serviceAccount {principal}"),
                |(rule, _)| format!("Policy {}", PolicySpec::policy_of(rule)),
            );
        refused.push(format!(
            "{field}: Endpoint {name} of project {owner} permits no copy to this pipeline's \
             account {principal}; project {owner} adds `actions: [store]` to {grant} (SHR-03)"
        ));
    }
    refused
}

/// Whether `policy`'s validity window holds `now` (no window holds always).
fn in_force(policy: &PolicySpec, now: TimestampTz) -> bool {
    policy.validity.as_ref().is_none_or(|validity| {
        validity.from.is_none_or(|from| from <= now) && validity.to.is_none_or(|to| now < to)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::Identity;
    use crate::config::Config;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    fn person(email: &str) -> Identity {
        Identity {
            client: None,
            subject: email.to_owned(),
            username: email.to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        }
    }

    /// Helsinki with the spaces `a` and `b`, one internal Endpoint into each, a public one into
    /// `b`, the DataSources `feed` and `secret-feed`, and `dev@hel.fi` holding the pipeline
    /// editor's propose on the project, `read` on Entity in space `a` only, and `read` on the
    /// DataSource `feed` only.
    fn world() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        let mirror = &state.mirror;
        for (name, space, audience) in [
            ("ep-a", "a", "internal"),
            ("ep-b", "b", "internal"),
            ("ep-b-public", "b", "public"),
        ] {
            mirror.upsert(manifest(
                "Endpoint",
                name,
                "helsinki",
                json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                        "slug": name, "audience": audience }),
            ));
        }
        for name in ["feed", "secret-feed", "proposed-feed"] {
            mirror.upsert(manifest(
                "DataSource",
                name,
                "helsinki",
                json!({ "http": { "url": "https://example.org/feed.json" } }),
            ));
        }
        mirror.upsert(manifest(
            "Role",
            "pipeline-editor",
            "org",
            json!({ "rules": [
                { "kinds": ["Pipeline"], "verbs": ["propose"] },
                { "kinds": ["DataSource"], "verbs": ["propose"],
                  "constraints": [{ "field": "metadata.name", "equals": "proposed-feed" }] }
            ] }),
        ));
        mirror.upsert(manifest(
            "Role",
            "feed-reader",
            "org",
            json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["read"],
                    "constraints": [{ "field": "metadata.name", "equals": "feed" }] }] }),
        ));
        mirror.upsert(manifest(
            "Role",
            "space-reader",
            "org",
            json!({ "rules": [{ "kinds": ["Entity", "ContextSpace"], "verbs": ["read"] }] }),
        ));
        for (name, role, scope) in [
            (
                "dev-pipelines",
                "pipeline-editor",
                json!({ "project": "helsinki" }),
            ),
            ("dev-feed", "feed-reader", json!({ "project": "helsinki" })),
            (
                "dev-space-a",
                "space-reader",
                json!({ "contextSpace": "a" }),
            ),
        ] {
            mirror.upsert(manifest(
                "RoleBinding",
                name,
                "org",
                json!({ "role": role, "subjects": [{ "user": "dev@hel.fi" }], "scope": scope }),
            ));
        }
        mirror.upsert(manifest(
            "ContextSpace",
            "a",
            "helsinki",
            json!({ "projectRef": "helsinki" }),
        ));
        mirror.upsert(manifest(
            "ContextSpace",
            "b",
            "helsinki",
            json!({ "projectRef": "helsinki" }),
        ));
        state
    }

    fn pipeline(source: Value, target: &str) -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha2",
            "kind": "Pipeline",
            "metadata": { "name": "route", "namespace": "helsinki" },
            "spec": {
                "class": "resident",
                "sources": [source],
                "outputs": [{ "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:a:{target}") }],
            }
        })
    }

    fn data_source(name: &str) -> Value {
        json!({ "dataSourceRef": { "kind": "DataSource", "name": name } })
    }

    fn endpoint_source(name: &str) -> Value {
        json!({ "endpointRef": { "kind": "Endpoint", "name": name },
                "query": { "type": "Vehicle" } })
    }

    fn refusal(state: &AppState, manifest: &Value, verb: Verb) -> Option<String> {
        match check(state, &person("dev@hel.fi"), "helsinki", manifest, verb) {
            Ok(()) => None,
            Err(ApiError::Denied(message)) => Some(message),
            Err(other) => panic!("expected a 403, got {other:?}"),
        }
    }

    #[test]
    fn a_pipeline_within_the_authors_reach_passes() {
        let state = world();
        assert_eq!(
            refusal(
                &state,
                &pipeline(data_source("feed"), "ep-a"),
                Verb::Propose
            ),
            None
        );
        assert_eq!(
            refusal(
                &state,
                &pipeline(endpoint_source("ep-a"), "ep-a"),
                Verb::Propose
            ),
            None
        );
    }

    /// `propose` on a DataSource reads it, under the same constraint (jc-core `Rule::grants`).
    #[test]
    fn a_data_source_the_author_may_propose_is_one_they_read() {
        let state = world();
        assert_eq!(
            refusal(
                &state,
                &pipeline(data_source("proposed-feed"), "ep-a"),
                Verb::Propose
            ),
            None
        );
    }

    /// The exploit of T-4893: read on space `a` only, a pipeline from space `b` into `a`.
    #[test]
    fn a_source_space_the_author_cannot_read_is_refused() {
        let state = world();
        let message = refusal(
            &state,
            &pipeline(endpoint_source("ep-b"), "ep-a"),
            Verb::Propose,
        )
        .expect("refused");
        assert!(
            message.contains("No access to source Endpoint ep-b"),
            "{message}"
        );
        assert!(
            !message.contains(" b "),
            "the space behind the Endpoint stays unnamed"
        );
    }

    #[test]
    fn a_data_source_the_author_cannot_read_is_refused() {
        let state = world();
        let message = refusal(
            &state,
            &pipeline(data_source("secret-feed"), "ep-a"),
            Verb::Propose,
        )
        .expect("refused");
        assert!(
            message.contains("No access to DataSource secret-feed"),
            "{message}"
        );
    }

    #[test]
    fn a_target_space_the_author_cannot_reach_is_refused() {
        let state = world();
        let message = refusal(
            &state,
            &pipeline(data_source("feed"), "ep-b"),
            Verb::Propose,
        )
        .expect("refused");
        assert!(
            message.contains("No access to target Endpoint ep-b"),
            "{message}"
        );
    }

    /// T-5702 (F-POR3-005): the reference's project was ignored, so another project's
    /// Endpoint was judged by a same-named one here, and an Endpoint nobody holds was skipped.
    #[test]
    fn a_source_endpoint_is_judged_in_its_own_project_or_counted_missing() {
        let state = world();
        let foreign = json!({ "endpointRef": { "kind": "Endpoint", "name": "ep-b", "namespace": "praha" },
                              "query": { "type": "Vehicle" } });
        assert_eq!(
            refusal(&state, &pipeline(foreign, "ep-a"), Verb::Propose),
            None
        );
        let message = refusal(
            &state,
            &pipeline(endpoint_source("ep-gone"), "ep-a"),
            Verb::Propose,
        )
        .expect("refused");
        assert!(message.contains("source Endpoint ep-gone"), "{message}");
    }

    /// Praha's Endpoint `shared` into its space `s`, and one Policy per `grants` entry naming
    /// `{account}` with its effect, its actions and its validity end.
    fn shared_by_praha(state: &AppState, grants: &[(&str, &str, Value, Option<&str>)]) {
        state.mirror.upsert(manifest(
            "Endpoint",
            "shared",
            "praha",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "s" },
                    "slug": "shared", "audience": "project-list", "allowedProjects": ["helsinki"] }),
        ));
        for (n, (assignee, effect, actions, until)) in grants.iter().enumerate() {
            let mut spec = json!({ "contextSpaceRef": "s", "assigner": "did:web:praha.eu",
                                   "assignee": { "kind": "serviceAccount", "id": assignee },
                                   "effect": effect, "operations": ["queryBatch"], "actions": actions });
            if let Some(until) = until {
                spec["validity"] = json!({ "to": until });
            }
            state
                .mirror
                .upsert(manifest("Policy", &format!("share-{n}"), "praha", spec));
        }
    }

    fn copy_of_praha() -> Value {
        pipeline(
            json!({ "endpointRef": { "kind": "Endpoint", "name": "shared", "namespace": "praha" },
                    "query": { "type": "Vehicle" } }),
            "ep-a",
        )
    }

    /// SHR-03: a copy of another project's Endpoint passes only with a `store` that project's
    /// Policy grants this pipeline's own account, in force and not taken back.
    #[test]
    fn a_copy_of_another_projects_endpoint_needs_its_store_for_the_pipelines_account() {
        let unstored_with = |grants: &[(&str, &str, Value, Option<&str>)]| {
            let state = world();
            shared_by_praha(&state, grants);
            unstored(&state.mirror, "helsinki", &copy_of_praha(), "pl-route")
        };
        let own = "helsinki/pl-route";
        assert!(unstored_with(&[(own, "permission", json!(["store"]), None)]).is_empty());

        let read_only = unstored_with(&[(own, "permission", json!([]), None)]);
        assert_eq!(read_only.len(), 1, "{read_only:?}");
        assert!(
            read_only[0]
                .starts_with("spec.sources[0].endpointRef: Endpoint shared of project praha"),
            "{read_only:?}"
        );
        assert!(
            read_only[0].contains("adds `actions: [store]` to Policy share-0"),
            "{read_only:?}"
        );
        let nothing = unstored_with(&[]);
        assert!(
            nothing[0].contains("to a Policy of space s for serviceAccount helsinki/pl-route"),
            "{nothing:?}"
        );
        // Another account's store, an ended one, or one a prohibition takes back copies nothing.
        for grants in [
            vec![("helsinki/pl-other", "permission", json!(["store"]), None)],
            vec![("pl-route", "permission", json!(["store"]), None)],
            vec![(
                own,
                "permission",
                json!(["store"]),
                Some("2020-01-01T00:00:00Z"),
            )],
            vec![
                (own, "permission", json!(["store"]), None),
                (own, "prohibition", json!(["store"]), None),
            ],
        ] {
            assert_eq!(unstored_with(&grants).len(), 1, "{grants:?}");
        }
    }

    /// SHR-03 at the door: the refusal is a 422 naming the field, a bootstrap admin included.
    #[test]
    fn the_door_refuses_a_copy_without_store_with_the_field() {
        let state = world();
        shared_by_praha(
            &state,
            &[("helsinki/pipelines", "permission", json!([]), None)],
        );
        let mut admin = person("root@hel.fi");
        admin.groups = vec![state.config.bootstrap_admins.clone()];
        match check(&state, &admin, "helsinki", &copy_of_praha(), Verb::Propose) {
            Err(ApiError::Unprocessable { errors, .. }) => {
                assert!(errors[0].contains("helsinki/pipelines"), "{errors:?}")
            }
            other => panic!("expected a 422, got {other:?}"),
        }
        let state = world();
        shared_by_praha(
            &state,
            &[("helsinki/pipelines", "permission", json!(["store"]), None)],
        );
        assert!(check(&state, &admin, "helsinki", &copy_of_praha(), Verb::Propose).is_ok());
    }

    #[test]
    fn a_public_source_endpoint_reads_what_anybody_reads() {
        let state = world();
        assert_eq!(
            refusal(
                &state,
                &pipeline(endpoint_source("ep-b-public"), "ep-a"),
                Verb::Propose
            ),
            None
        );
    }

    /// The approver is held to the same reach with their own verb: `dev` may propose but not
    /// approve Pipelines, so approving even a pipeline inside their reads is refused for the target.
    #[test]
    fn the_approver_needs_approve_on_the_target_space() {
        let state = world();
        let message = refusal(
            &state,
            &pipeline(data_source("feed"), "ep-a"),
            Verb::Approve,
        )
        .expect("refused");
        assert!(message.contains("target Endpoint ep-a"), "{message}");
        assert!(message.contains("may approve"), "{message}");
    }

    #[test]
    fn every_missing_reference_is_named_once() {
        let state = world();
        let mut manifest = pipeline(endpoint_source("ep-b"), "ep-b");
        manifest["spec"]["sources"]
            .as_array_mut()
            .expect("sources")
            .extend([data_source("secret-feed"), endpoint_source("ep-b")]);
        let message = refusal(&state, &manifest, Verb::Propose).expect("refused");
        assert_eq!(
            message.matches("source Endpoint ep-b").count(),
            1,
            "{message}"
        );
        assert!(message.contains("DataSource secret-feed"), "{message}");
        assert!(message.contains("target Endpoint ep-b"), "{message}");
    }

    #[test]
    fn other_kinds_and_the_bootstrap_group_pass() {
        let state = world();
        let mut endpoint = pipeline(endpoint_source("ep-b"), "ep-b");
        endpoint["kind"] = json!("Endpoint");
        assert_eq!(refusal(&state, &endpoint, Verb::Propose), None);
        let mut admin = person("root@hel.fi");
        admin.groups = vec![state.config.bootstrap_admins.clone()];
        assert!(check(
            &state,
            &admin,
            "helsinki",
            &pipeline(endpoint_source("ep-b"), "ep-b"),
            Verb::Propose
        )
        .is_ok());
    }
}
