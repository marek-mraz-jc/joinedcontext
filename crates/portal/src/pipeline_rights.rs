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

use crate::error::ApiError;
use crate::permissions::{effective, Effective};
use crate::state::AppState;
use crate::store::Mirror;
use chrono::Utc;
use jc_core::kinds::{PipelineSpec, Verb};
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
    let effective = effective(mirror, bootstrap_group, identity, project, Utc::now());
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
