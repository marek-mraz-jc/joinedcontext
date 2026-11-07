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

use chrono::Utc;
use jc_core::kinds::{OutputMode, PipelineSpec};
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
    let tenant = crate::spaces::segment(&state.mirror, project, &space);
    let (names, partial) = match state
        .space_usage
        .stored_attributes(&broker, &tenant, entity_type)
        .await
    {
        Ok(read) => read,
        Err(why) => return unchecked(endpoint, &why),
    };
    let judged = match writable(state, project, manifest, entity_type, &names, Utc::now()) {
        Ok(judged) => judged,
        Err(err) => return unchecked(endpoint, &err.to_string()),
    };
    let not: Vec<String> = judged
        .into_iter()
        .filter(|w| !w.writable)
        .map(|w| match w.reason {
            Some(reason) => format!("{} ({reason})", w.name),
            None => w.name,
        })
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
}
