//! What a plan says before a Pipeline output turns to `replace` (PL-79, S-23, T-6001).
//!
//! A replace write deletes every Attribute it omits, so the broker judges it against every
//! Attribute the stored Entity holds (Antares `WriteGuard::pre_whole`, threat-model D6): while one
//! stored Entity of the output's type holds an Attribute the target Endpoint's write view does not
//! admit, each replace of it is refused whole. An older mapping that wrote a since-renamed
//! Attribute leaves exactly that. The plan reads the stored names from the broker (names only,
//! with the admin token), judges each through the Endpoint as [`writable`] does, and names the
//! refused ones in a warning. A broker that does not answer is a warning that the check could not
//! run, never a silent pass.

use jc_core::kinds::{OutputMode, PipelineSpec};
use jc_time::TimestampTz;
use serde_json::{json, Value};

use crate::record_check::writable;
use crate::state::AppState;

/// The warnings for each output of `manifest` (a Pipeline planned in `project`) whose mode turns
/// to `replace` against the Pipeline as `home` holds it now.
pub async fn warnings(state: &AppState, project: &str, home: &str, manifest: &Value) -> Vec<Value> {
    let Ok(spec) = serde_json::from_value::<PipelineSpec>(manifest["spec"].clone()) else {
        return Vec::new();
    };
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let before: Vec<bool> = state
        .mirror
        .get(home, "Pipeline", name)
        .and_then(|env| serde_json::from_value::<PipelineSpec>(env.spec).ok())
        .map(|b| b.outputs().iter().map(replaces).collect())
        .unwrap_or_default();
    let lane = spec.lane_outputs().len();
    let mut out = Vec::new();
    for (at, output) in spec.outputs().iter().enumerate() {
        if !replaces(output) || before.get(at).copied().unwrap_or(false) {
            continue;
        }
        let path = match (at < lane, manifest["spec"]["outputs"].is_array()) {
            (true, true) => format!("spec.outputs.{at}.mode"),
            (true, false) => "spec.output.mode".to_owned(),
            (false, _) => "spec.routes".to_owned(),
        };
        let endpoint = output.target_endpoint.local_id().to_owned();
        let message = match output.entity_type.as_deref() {
            None => unchecked(&endpoint, "the output names no entity type"),
            Some(entity_type) => refused(state, project, manifest, &endpoint, entity_type).await,
        };
        if let Some(message) = message {
            out.push(json!({ "level": "warning", "path": path, "message": message }));
        }
    }
    out
}

fn replaces(output: &jc_core::kinds::PipelineOutput) -> bool {
    output.mode == Some(OutputMode::Replace)
}

fn unchecked(endpoint: &str, why: &str) -> Option<String> {
    Some(format!(
        "the stored attributes could not be checked before this output replaces entities ({why}): \
         the broker refuses a replace while a stored entity holds an attribute the Endpoint \
         '{endpoint}' does not admit (WriteGuard::pre_whole)"
    ))
}

/// The warning naming the stored Attributes of `entity_type` the Endpoint does not admit; `None`
/// when every stored one is admitted.
async fn refused(
    state: &AppState,
    project: &str,
    manifest: &Value,
    endpoint: &str,
    entity_type: &str,
) -> Option<String> {
    let naming = context_gateway::store::TenantNames::from_env();
    refused_as(state, project, manifest, (endpoint, entity_type), naming).await
}

/// [`refused`] under the broker's tenant `naming`.
async fn refused_as(
    state: &AppState,
    project: &str,
    manifest: &Value,
    (endpoint, entity_type): (&str, &str),
    naming: context_gateway::store::TenantNames,
) -> Option<String> {
    if state.views.is_none() {
        // Without the broker views no write is judged: every name would pass unseen.
        return unchecked(
            endpoint,
            "this Portal writes no broker views (JC_PORTAL_BROKER_URL)",
        );
    }
    let Some(space) = crate::pipeline_expiry::space_of_endpoint(&state.mirror, project, endpoint)
    else {
        return unchecked(
            endpoint,
            "the Endpoint names no Context Space this Portal holds",
        );
    };
    let broker = crate::spaces::broker(&state.mirror, project, &space);
    let (names, partial) =
        match match crate::spaces::tenant_as(&state.mirror, project, &space, naming) {
            Ok(tenant) => {
                state
                    .space_usage
                    .stored_attributes(&broker, &tenant, entity_type)
                    .await
            }
            Err(reason) => Err(reason),
        } {
            Ok(read) => read,
            Err(why) => return unchecked(endpoint, &why),
        };
    let judged = match writable(
        state,
        project,
        manifest,
        entity_type,
        &names,
        TimestampTz::now(),
    ) {
        Ok(judged) => judged,
        Err(err) => return unchecked(endpoint, &err.to_string()),
    };
    let not: Vec<String> = judged
        .into_iter()
        .filter(|w| !w.writable)
        // A name is not writable only for a finding, whose reason it carries (writable_reason).
        .map(|w| format!("{} ({})", w.name, w.reason.unwrap_or_default()))
        .collect();
    let more = if partial {
        "; the broker read only part of the stored entities, so more may exist"
    } else {
        ""
    };
    match (not.is_empty(), partial) {
        (true, false) => None,
        (true, true) => unchecked(
            endpoint,
            "the broker read only part of the stored entities and found none it would refuse",
        ),
        (false, _) => Some(format!(
            "this output now replaces whole {entity_type} entities, and the broker refuses each \
             replace while a stored entity holds an attribute the Endpoint '{endpoint}' does not \
             admit (WriteGuard::pre_whole). Stored and not admitted: {}{more}. Admit them in the \
             Endpoint's write Policy, or remove them from the stored entities, before applying",
            not.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn pipeline(output: Value) -> Value {
        json!({ "kind": "Pipeline", "metadata": { "name": "counters", "namespace": "city" },
                "spec": { "class": "scheduled", "schedule": "0 * * * *",
                          "source": { "dataSourceRef": { "kind": "DataSource", "name": "csv" } },
                          "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-doprava:intake",
                          "output": output,
                          "quotas": { "maxMemoryMb": 128, "cpuMillicores": 100 } } })
    }

    #[tokio::test]
    async fn only_an_output_turning_to_replace_is_checked_and_an_unchecked_one_says_why() {
        let state = AppState::new(Config::for_tests(), None);
        let upsert = pipeline(json!({ "type": "BikeCounter", "mode": "upsert" }));
        assert_eq!(
            warnings(&state, "city", "city", &upsert).await,
            Vec::<Value>::new()
        );
        // This Portal writes no broker views: the check could not run, and says so.
        let replace = pipeline(json!({ "type": "BikeCounter", "mode": "replace" }));
        let got = warnings(&state, "city", "city", &replace).await;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["level"], "warning");
        assert_eq!(got[0]["path"], "spec.output.mode");
        let text = got[0]["message"].as_str().unwrap_or_default();
        assert!(text.contains("could not be checked"), "{text}");
        assert!(text.contains("writes no broker views"), "{text}");
        assert!(text.contains("WriteGuard::pre_whole"), "{text}");
    }

    use super::super::testkit::envelope;
    use std::sync::Arc;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The broker token file, written once: a rewrite truncates it first, and a test reading it in
    /// parallel then found it empty ("the broker's admin token is not readable").
    fn token_file() -> std::path::PathBuf {
        static FILE: std::sync::LazyLock<std::path::PathBuf> = std::sync::LazyLock::new(|| {
            let file = std::env::temp_dir().join(format!("replace-check-{}", std::process::id()));
            std::fs::write(&file, "t").expect("write the token");
            file
        });
        FILE.clone()
    }

    /// A Portal that writes broker views, counts on `broker` (none: no address), and holds the
    /// Endpoint `intake` writing into the space `doprava` of project `city`.
    fn writing(broker: Option<&MockServer>) -> AppState {
        let mut state = AppState::new(Config::for_tests(), None);
        let uri = broker.map_or_else(|| "http://127.0.0.1:9".to_owned(), MockServer::uri);
        state.views = Some(Arc::new(crate::reconciler::views::ViewSync::new(
            uri.clone(),
            token_file(),
        )));
        state.space_usage = Arc::new(crate::space_usage::Counter::new(
            broker.map(|_| uri),
            Some(token_file()),
        ));
        state.mirror.upsert(envelope(
            "Endpoint",
            "intake",
            "city",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "doprava" }, "slug": "intake" }),
        ));
        state
            .mirror
            .upsert(envelope("ContextSpace", "doprava", "city", json!({})));
        state
    }

    /// The broker's answer for the stored attribute names of BikeCounter.
    async fn stored(answer: ResponseTemplate) -> MockServer {
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(
                "^/q/tenants/[^/]+/types/BikeCounter/attributes$",
            ))
            .respond_with(answer)
            .mount(&broker)
            .await;
        broker
    }

    fn replace() -> Value {
        pipeline(json!({ "type": "BikeCounter", "mode": "replace" }))
    }

    async fn one_message(state: &AppState, manifest: &Value) -> String {
        let got = warnings(state, "city", "city", manifest).await;
        assert_eq!(got.len(), 1, "{got:?}");
        got[0]["message"].as_str().unwrap_or_default().to_owned()
    }

    /// Each stored attribute the Endpoint does not admit is named with why; the broker's partial
    /// read says more may exist.
    #[tokio::test]
    async fn the_stored_attributes_the_endpoint_does_not_admit_are_named() {
        let broker = stored(
            ResponseTemplate::new(200).set_body_json(json!({ "attributes": ["count", 7, "lane"] })),
        )
        .await;
        let text = one_message(&writing(Some(&broker)), &replace()).await;
        assert!(
            text.contains("replaces whole BikeCounter entities"),
            "{text}"
        );
        assert!(text.contains("Stored and not admitted: count ("), "{text}");
        assert!(text.contains("lane ("), "{text}");
        assert!(!text.contains("more may exist"), "{text}");
        assert!(text.contains("Endpoint 'intake'"), "{text}");

        let partial = stored(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "attributes": ["count"], "partial": true })),
        )
        .await;
        let text = one_message(&writing(Some(&partial)), &replace()).await;
        assert!(text.contains("Stored and not admitted: count ("), "{text}");
        assert!(text.ends_with("before applying"), "{text}");
        assert!(
            text.contains("read only part of the stored entities, so more may exist"),
            "{text}"
        );
    }

    /// Nothing stored is no warning; nothing found in a partial read is still a warning that the
    /// check did not see everything.
    #[tokio::test]
    async fn nothing_stored_passes_and_a_partial_read_that_finds_nothing_says_so() {
        let empty =
            stored(ResponseTemplate::new(200).set_body_json(json!({ "attributes": [] }))).await;
        assert_eq!(
            warnings(&writing(Some(&empty)), "city", "city", &replace()).await,
            Vec::<Value>::new()
        );
        let partial = stored(
            ResponseTemplate::new(200).set_body_json(json!({ "attributes": [], "partial": true })),
        )
        .await;
        let text = one_message(&writing(Some(&partial)), &replace()).await;
        assert!(text.contains("could not be checked"), "{text}");
        assert!(text.contains("found none it would refuse"), "{text}");
    }

    /// A broker that refuses, answers garbage or answers no list, and no broker address at all:
    /// each is a warning that names why the check could not run, never a silent pass.
    #[tokio::test]
    async fn a_broker_that_cannot_answer_is_a_warning_naming_why() {
        for (answer, why) in [
            (
                ResponseTemplate::new(503),
                "the broker answered 503 Service Unavailable",
            ),
            (
                ResponseTemplate::new(200).set_body_string("<html>"),
                "not JSON",
            ),
            (
                ResponseTemplate::new(200).set_body_json(json!({ "names": [] })),
                "names no attributes",
            ),
        ] {
            let broker = stored(answer).await;
            let text = one_message(&writing(Some(&broker)), &replace()).await;
            assert!(text.contains("could not be checked"), "{text}");
            assert!(text.contains(why), "{why}: {text}");
        }
        let text = one_message(&writing(None), &replace()).await;
        assert!(text.contains("JC_PORTAL_BROKER_URL"), "{text}");
    }

    /// An Endpoint the Portal does not hold, or one naming no space, cannot be judged.
    #[tokio::test]
    async fn an_endpoint_without_a_space_cannot_be_judged() {
        let state = writing(None);
        state.mirror.upsert(envelope(
            "Endpoint",
            "intake",
            "city",
            json!({ "slug": "intake" }),
        ));
        let text = one_message(&state, &replace()).await;
        assert!(
            text.contains("names no Context Space this Portal holds"),
            "{text}"
        );
    }

    /// An output that names no entity type cannot be checked; an output already replacing in the
    /// Pipeline the project holds is not warned again; a spec that does not parse warns nothing.
    #[tokio::test]
    async fn only_a_new_replace_of_a_named_type_is_judged() {
        let state = AppState::new(Config::for_tests(), None);
        // Only an output of the outputs list may leave its type out.
        let mut untyped = replace();
        let spec = untyped["spec"].as_object_mut().expect("a spec");
        spec.remove("output");
        spec.remove("targetEndpoint");
        spec.insert("outputs".into(), json!([{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-doprava:intake", "mode": "replace" }]));
        let text = one_message(&state, &untyped).await;
        assert!(text.contains("the output names no entity type"), "{text}");

        let held = replace();
        state.mirror.upsert(envelope(
            "Pipeline",
            "counters",
            "city",
            held["spec"].clone(),
        ));
        assert_eq!(
            warnings(&state, "city", "city", &held).await,
            Vec::<Value>::new()
        );
        // Held in another project: still a new replace there.
        assert_eq!(warnings(&state, "city", "elsewhere", &held).await.len(), 1);

        let mut broken = replace();
        broken["spec"]["class"] = json!(7);
        assert_eq!(
            warnings(&state, "city", "city", &broken).await,
            Vec::<Value>::new()
        );
    }

    /// The warning's path names the field to change: one output, the outputs list by index.
    #[tokio::test]
    async fn the_path_names_the_output_that_turns_to_replace() {
        let state = AppState::new(Config::for_tests(), None);
        let mut listed = replace();
        let spec = listed["spec"].as_object_mut().expect("a spec");
        spec.remove("output");
        spec.remove("targetEndpoint");
        spec.insert("outputs".into(), json!([
            { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-doprava:intake", "type": "BikeCounter", "mode": "upsert" },
            { "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-doprava:intake", "type": "BikeCounter", "mode": "replace" }
        ]));
        let got = warnings(&state, "city", "city", &listed).await;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["path"], "spec.outputs.1.mode");

        // A route's output is named by the routes it sits in; the lane's own stays upsert.
        let mut routed = pipeline(json!({ "type": "BikeCounter", "mode": "upsert" }));
        routed["spec"]["routes"] = json!([{ "name": "lanes", "when": "this.lane != null",
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-doprava:intake",
                          "type": "BikeCounter", "mode": "replace" }] }]);
        let got = warnings(&state, "city", "city", &routed).await;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["path"], "spec.routes");
    }

    /// A space with no broker tenant yet (the project has no registry id under project-key
    /// naming) cannot be judged, and the warning says why instead of passing.
    #[tokio::test]
    async fn a_space_without_a_tenant_is_a_warning_naming_why() {
        let state = writing(None);
        let text = refused_as(
            &state,
            "city",
            &replace(),
            ("intake", "BikeCounter"),
            context_gateway::store::TenantNames::ProjectKey,
        )
        .await
        .expect("a warning");
        assert!(text.contains("could not be checked"), "{text}");
        assert!(text.contains("has no broker tenant yet"), "{text}");
        assert!(text.contains("Endpoint 'intake'"), "{text}");
    }
}
