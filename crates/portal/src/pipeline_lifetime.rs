//! `spec.lifetime` at the door (PL-80…PL-83, T-4839): every entry judged against the model of
//! each space it lands in before a Change exists, so a name that is no slot, a `from` that is no
//! date-time slot, a type the space's model does not hold or an `at` already past is refused with
//! the type's own slots named. jc-core has already refused what needs no model (modes, durations,
//! names, a type no typed output writes, an author step that assigns `expiresAt`).
//!
//! An entry lands where an output that writes its type writes: an output naming the type, or one
//! naming no type at all. The Endpoint writes into its space, and the space takes only its model's
//! classes, so a type outside that model is refused here as a type the Endpoint does not write.

use crate::error::ApiError;
use crate::pipeline_validation::ModelSchemas;
use crate::state::AppState;
use crate::store::Mirror;
use chrono::{DateTime, Utc};
use jc_core::kinds::PipelineSpec;
use jcctl::model_schema::ModelSchema;
use serde_json::Value;

/// Refuses a Pipeline whose lifetime does not fit the spaces it writes, with every reason at once
/// (CC-24). Anything but a Pipeline, or one with no lifetime, passes untouched.
pub fn check(state: &AppState, project: &str, kind: &str, spec: &Value) -> Result<(), ApiError> {
    if kind != "Pipeline" {
        return Ok(());
    }
    // jc-core has refused a malformed spec before this door.
    let Ok(pipeline) = serde_json::from_value::<PipelineSpec>(spec.clone()) else {
        return Ok(());
    };
    let errors = problems(
        &state.model_schemas,
        &state.mirror,
        project,
        &pipeline,
        Utc::now(),
    );
    if errors.is_empty() {
        return Ok(());
    }
    Err(ApiError::Invalid {
        detail: "the pipeline's lifetime does not fit the spaces it writes (PL-81)".to_owned(),
        errors,
    })
}

/// One line per entry and space that does not fit, each naming the field to fix.
pub fn problems(
    schemas: &ModelSchemas,
    mirror: &Mirror,
    project: &str,
    pipeline: &PipelineSpec,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, entry) in pipeline.lifetime.iter().enumerate() {
        let mut spaces: Vec<String> = pipeline
            .outputs()
            .into_iter()
            .filter(|output| {
                output
                    .entity_type
                    .as_deref()
                    .is_none_or(|t| t == entry.entity_type)
            })
            .filter_map(|output| {
                crate::pipeline_expiry::space_of_endpoint(
                    mirror,
                    project,
                    output.target_endpoint.local_id(),
                )
            })
            .collect();
        spaces.sort();
        spaces.dedup();
        // The entry alone, so the checker's field path is this entry's.
        let alone = PipelineSpec {
            lifetime: vec![entry.clone()],
            ..pipeline.clone()
        };
        for space in spaces {
            // A space with no compiled model is not narrowed here; the runner's stage holds it.
            let Some(model) = schemas.get(project, &space) else {
                continue;
            };
            if let Err(error) = jcctl::lifetime::check(&alone, &model, now) {
                problems.push(explained(error, index, &entry.entity_type, &space, &model));
            }
        }
    }
    problems
}

/// The checker's refusal at this entry's path, with the slots or classes a person picks from.
fn explained(
    error: jc_core::error::Error,
    index: usize,
    entity_type: &str,
    space: &str,
    model: &ModelSchema,
) -> String {
    let jc_core::error::Error::Invalid { field, reason } = error else {
        return format!("spec.lifetime[{index}]: {error} (space {space})");
    };
    let field = field.replacen("spec.lifetime[0]", &format!("spec.lifetime[{index}]"), 1);
    let choices = match model.attributes(entity_type) {
        None => {
            let classes: Vec<&str> = model.classes.keys().map(String::as_str).collect();
            format!("its classes are {}", classes.join(", "))
        }
        Some(class) if field.ends_with(".from") => {
            let times: Vec<&str> = class
                .attributes
                .iter()
                .filter(|a| a.format.as_deref() == Some("date-time"))
                .map(|a| a.name.as_str())
                .collect();
            if times.is_empty() {
                format!("`{entity_type}` has no date-time slot; use `observedAt` or `rolling`")
            } else {
                format!("its date-time slots are {}", times.join(", "))
            }
        }
        Some(class) => {
            let slots: Vec<&str> = class.attributes.iter().map(|a| a.name.as_str()).collect();
            format!("the slots of `{entity_type}` are {}", slots.join(", "))
        }
    };
    format!("{field}: {reason} in space {space}; {choices}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use chrono::TimeZone;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn endpoint(name: &str, space: &str) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new(name, "helsinki"),
            spec: json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space }, "slug": name }),
            status: None,
        }
    }

    /// Helsinki's vehicles space, whose model's Bus has a date-time `observedOn`, a `speed` and a
    /// `position`, and a stops space with no model.
    fn world() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(endpoint("ep-vehicles", "vehicles"));
        state.mirror.upsert(endpoint("ep-stops", "stops"));
        let schema = json!({ "definitions": { "Bus": { "type": "object", "properties": {
            "observedOn": { "type": "string", "format": "date-time", "x-ngsi-ld-kind": "Property" },
            "speed": { "type": "number", "x-ngsi-ld-kind": "Property" },
            "position": { "type": "object", "x-ngsi-ld-kind": "GeoProperty" }
        }}}});
        let model = crate::pipeline_validation::ModelSchema::compile(
            "transport",
            "1.0.0",
            &schema,
            &["Bus".to_owned(), "Stop".to_owned()],
            false,
        );
        state.model_schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "vehicles".to_owned()),
            Arc::new(model),
        )]));
        state
    }

    fn spec(outputs: Value, lifetime: Value) -> Value {
        json!({
            "class": "resident",
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
            "outputs": outputs,
            "lifetime": lifetime,
        })
    }

    fn to(endpoint: &str) -> Value {
        json!({ "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:helsinki:{endpoint}") })
    }

    fn refusals(state: &AppState, spec: &Value) -> Vec<String> {
        let pipeline: PipelineSpec = serde_json::from_value(spec.clone()).expect("a PipelineSpec");
        let now = Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap();
        problems(
            &state.model_schemas,
            &state.mirror,
            "helsinki",
            &pipeline,
            now,
        )
    }

    #[test]
    fn every_mode_on_slots_of_the_model_passes() {
        let state = world();
        let lifetime = json!([{
            "type": "Bus",
            "entity": { "from": "observedOn", "plus": "PT10M" },
            "attributes": [{ "names": ["position"], "rolling": "PT5M" }, { "names": ["speed"], "at": "2030-01-01T00:00:00Z" }]
        }]);
        assert_eq!(
            refusals(&state, &spec(json!([to("ep-vehicles")]), lifetime)),
            Vec::<String>::new()
        );
        // A space with no compiled model is not narrowed at the door.
        let elsewhere = json!([{ "type": "Tram", "entity": { "rolling": "PT1H" } }]);
        assert_eq!(
            refusals(&state, &spec(json!([to("ep-stops")]), elsewhere)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_name_that_is_no_slot_is_refused_with_the_types_slots() {
        let state = world();
        let lifetime = json!([{ "type": "Bus", "attributes": [{ "names": ["position", "colour"], "rolling": "PT5M" }] }]);
        let found = refusals(&state, &spec(json!([to("ep-vehicles")]), lifetime));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with(
                "spec.lifetime[0].attributes[0].names: `colour` is not a slot of `Bus`"
            ),
            "{found:?}"
        );
        assert!(found[0].contains("in space vehicles"), "{found:?}");
        assert!(
            found[0].contains("the slots of `Bus` are observedOn, position, speed"),
            "{found:?}"
        );
    }

    #[test]
    fn a_from_that_is_no_date_time_slot_names_the_date_time_slots() {
        let state = world();
        let lifetime = json!([{ "type": "Bus", "entity": { "from": "speed", "plus": "PT10M" } }]);
        let found = refusals(&state, &spec(json!([to("ep-vehicles")]), lifetime));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("spec.lifetime[0].entity.from:"),
            "{found:?}"
        );
        assert!(
            found[0].ends_with("its date-time slots are observedOn"),
            "{found:?}"
        );
    }

    #[test]
    fn a_type_the_endpoint_does_not_write_is_refused_with_the_spaces_classes() {
        let state = world();
        // The output names no type, so the space's model decides what the Endpoint writes.
        let lifetime = json!([
            { "type": "Bus", "entity": { "rolling": "PT10M" } },
            { "type": "Tram", "entity": { "rolling": "PT10M" } }
        ]);
        let found = refusals(&state, &spec(json!([to("ep-vehicles")]), lifetime));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("spec.lifetime[1].type:"), "{found:?}");
        assert!(
            found[0].contains("no class `Tram`") && found[0].ends_with("its classes are Bus, Stop"),
            "{found:?}"
        );
    }

    #[test]
    fn an_at_already_past_is_refused_and_the_door_answers_400_with_every_reason() {
        let state = world();
        let lifetime = json!([{ "type": "Bus", "entity": { "at": "2020-01-01T00:00:00Z" } }]);
        let spec = spec(json!([to("ep-vehicles")]), lifetime);
        let found = refusals(&state, &spec);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("spec.lifetime[0].entity.at:")
                && found[0].contains("already passed"),
            "{found:?}"
        );
        match check(&state, "helsinki", "Pipeline", &spec) {
            Err(ApiError::Invalid { detail, errors }) => {
                assert!(detail.contains("lifetime"), "{detail}");
                assert_eq!(errors, found);
            }
            other => panic!("expected a 400 with reasons, got {other:?}"),
        }
        // Another kind, or a pipeline with no lifetime, passes the door untouched.
        assert!(check(&state, "helsinki", "Endpoint", &spec).is_ok());
        let plain = json!({ "class": "resident", "outputs": [to("ep-vehicles")] });
        assert!(check(&state, "helsinki", "Pipeline", &plain).is_ok());
    }
}
