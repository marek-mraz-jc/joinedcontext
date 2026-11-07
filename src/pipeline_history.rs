//! `spec.history` of a Pipeline on the Portal's side (PL-106…PL-110): the model check at
//! admission, the fold of each plan into its space's retention document (PL-108), and the rules
//! the record check refuses a plan by when the space or the broker cannot keep it (PL-109).
//! The kind and the rendered `observedAt` step are `jc_core::kinds::pipeline_history` and
//! `jcctl::history`.

use std::collections::BTreeMap;

use jc_core::kinds::pipeline_history::RECEIVED;
use jc_core::kinds::{AttributeRecording, Keep, PipelineSpec, RetentionRule};
use serde_json::Value;

use crate::error::ApiError;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};
use crate::temporal_audit::PLATFORM_HISTORY;

const HISTORY_OFF: &str = "off";
const ANY_TYPE: &str = "*";

/// One attribute's plan in one pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub pipeline: String,
    pub entity_type: String,
    pub attribute: String,
    pub keep: Keep,
    pub heartbeat: Option<String>,
    pub observed_at: Option<String>,
}

impl Plan {
    /// The recording this plan folds into, `None` for `latest` (PL-108).
    fn recording(&self) -> Option<AttributeRecording> {
        Some(AttributeRecording {
            record: Some(self.keep.record()?.to_owned()),
            heartbeat: self.heartbeat.clone(),
            ..AttributeRecording::default()
        })
    }
}

/// Every attribute `spec` plans, under the pipeline's `name`.
pub fn plans(name: &str, spec: &PipelineSpec) -> Vec<Plan> {
    spec.history
        .iter()
        .flat_map(|entry| {
            entry.attributes.iter().flat_map(move |attribute| {
                attribute.names.iter().map(move |name_of| Plan {
                    pipeline: name.to_owned(),
                    entity_type: entry.entity_type.clone(),
                    attribute: name_of.clone(),
                    keep: attribute.keep,
                    heartbeat: attribute.heartbeat.clone(),
                    observed_at: attribute.observed_at.clone(),
                })
            })
        })
        .collect()
}

/// The history the space keeps of `entity_type`: its rule's, the `*` rule's, or none (DM-132).
pub fn type_history(rules: &[RetentionRule], entity_type: &str) -> String {
    let rule = |t: &str| rules.iter().find(|r| r.entity_type == t);
    rule(entity_type)
        .and_then(|r| r.history.clone())
        .or_else(|| rule(ANY_TYPE).and_then(|r| r.history.clone()))
        .unwrap_or_else(|| PLATFORM_HISTORY.to_owned())
}

/// The space's own recording of one attribute, when its rule names one.
fn space_recording<'a>(
    rules: &'a [RetentionRule],
    entity_type: &str,
    attribute: &str,
) -> Option<&'a AttributeRecording> {
    rules
        .iter()
        .find(|r| r.entity_type == entity_type)?
        .attributes
        .get(attribute)
}

/// The space's rules with every plan the broker can keep written in (PL-108): `every` and
/// `changes` become the attribute's `record` where the space names no recording of it, the type
/// keeps history, and every pipeline that plans the attribute plans the same.
pub fn fold(rules: &[RetentionRule], plans: &[Plan]) -> Vec<RetentionRule> {
    let mut agreed: BTreeMap<(&str, &str), Option<AttributeRecording>> = BTreeMap::new();
    for plan in plans {
        let key = (plan.entity_type.as_str(), plan.attribute.as_str());
        let recording = plan.recording();
        agreed
            .entry(key)
            .and_modify(|seen| {
                if *seen != recording {
                    *seen = None;
                }
            })
            .or_insert(recording);
    }
    let mut out = rules.to_vec();
    for ((entity_type, attribute), recording) in agreed {
        let Some(recording) = recording else { continue };
        if type_history(rules, entity_type) == HISTORY_OFF
            || space_recording(rules, entity_type, attribute).is_some()
        {
            continue;
        }
        let at = match out.iter().position(|r| r.entity_type == entity_type) {
            Some(at) => at,
            None => {
                out.push(RetentionRule {
                    entity_type: entity_type.to_owned(),
                    ..RetentionRule::default()
                });
                out.len() - 1
            }
        };
        out[at].attributes.insert(attribute.to_owned(), recording);
    }
    out
}

/// Words a person reads for each choice, as the editor names them.
fn said(keep: Keep) -> &'static str {
    match keep {
        Keep::Every => "keep every reading",
        Keep::Changes => "keep only changes",
        Keep::Latest => "keep the latest value only",
    }
}

/// Why the space or the broker cannot keep `plan` (PL-109), as `(rule, reason)`; `others` are the
/// plans of the space's other pipelines.
pub fn refusal(
    rules: &[RetentionRule],
    space: &str,
    plan: &Plan,
    others: &[Plan],
) -> Option<(&'static str, String)> {
    let (t, a) = (plan.entity_type.as_str(), plan.attribute.as_str());
    let history = type_history(rules, t);
    if plan.keep == Keep::Latest && history != HISTORY_OFF {
        return Some((
            "latest",
            format!(
                "{} needs {t}'s history off, and space {space} keeps {history} of it; the broker \
                 keeps history per type (DM-97): choose keep only changes, or ask a steward to \
                 turn {t}'s history off in the space's Retention tab",
                said(plan.keep)
            ),
        ));
    }
    if plan.keep != Keep::Latest && history == HISTORY_OFF {
        return Some((
            "historyOff",
            format!(
                "space {space} keeps no history of {t} (its history is off, or no rule names it), so \
                 no row is kept; choose keep the latest value only, or ask a steward to set {t}'s \
                 history in the space's Retention tab"
            ),
        ));
    }
    if plan.keep == Keep::Every && plan.observed_at.as_deref() == Some(RECEIVED) {
        return Some((
            "received",
            "keep every reading with the receive time makes each write a new row (DM-104): \
             take the time from a field of the record, or keep only changes"
                .to_owned(),
        ));
    }
    if let (Some(own), Some(planned)) = (space_recording(rules, t, a), plan.recording()) {
        if own.record != planned.record || own.heartbeat != planned.heartbeat {
            return Some((
                "spaceRule",
                format!(
                    "space {space} records {a} of {t} as {}{} in its own rule, which wins \
                     (PL-108): choose that, or ask a steward to change the space's Retention tab",
                    own.record.as_deref().unwrap_or("every"),
                    own.heartbeat
                        .as_deref()
                        .map(|h| format!(" with heartbeat {h}"))
                        .unwrap_or_default()
                ),
            ));
        }
    }
    let other = others.iter().find(|o| {
        o.pipeline != plan.pipeline
            && o.entity_type == t
            && o.attribute == a
            && (o.keep != plan.keep || o.heartbeat != plan.heartbeat)
    })?;
    Some((
        "otherPipeline",
        format!(
            "pipeline {} plans {a} of {t} as {}; two plans for one attribute leave it to the \
             space's rule (PL-108): choose the same",
            other.pipeline,
            said(other.keep)
        ),
    ))
}

/// What a record breaks of its own plans (PL-109): `every` with no `observedAt` on the attribute
/// once the step ran, and an `observedAt` slot that gives no date-time.
pub fn record_refusal(plan: &Plan, record: &Value) -> Option<(&'static str, String)> {
    let attribute = record.get(&plan.attribute)?;
    let instances: Vec<&Value> = match attribute {
        Value::Array(items) => items.iter().collect(),
        one => vec![one],
    };
    let timed = instances
        .iter()
        .all(|i| i.get("observedAt").and_then(Value::as_str).is_some());
    if let Some(slot) = plan.observed_at.as_deref().filter(|s| *s != RECEIVED) {
        if !timed {
            return Some((
                "observedAt",
                format!(
                    "{slot} gives no date-time in this record, so {} takes the receive time",
                    plan.attribute
                ),
            ));
        }
    }
    if plan.keep == Keep::Every && !timed {
        return Some((
            "observedAt",
            format!(
                "keep every reading needs a time of its own on {}, and this record carries none: \
                 each write would be a new row (DM-104); pick the time from a field",
                plan.attribute
            ),
        ));
    }
    None
}

/// The space's retention rules, none when it has no readable ones.
pub fn space_rules(mirror: &Mirror, project: &str, space: &str) -> Vec<RetentionRule> {
    mirror
        .get(project, "ContextSpace", space)
        .and_then(|s| s.spec.get("retention").cloned())
        .and_then(|r| serde_json::from_value(r).ok())
        .unwrap_or_default()
}

/// The spaces a pipeline writes `entity_type` into, by its outputs' Endpoints.
pub fn spaces_of(
    mirror: &Mirror,
    project: &str,
    spec: &PipelineSpec,
    entity_type: &str,
) -> Vec<String> {
    let mut spaces: Vec<String> = spec
        .outputs()
        .into_iter()
        .filter(|o| o.entity_type.as_deref().is_none_or(|t| t == entity_type))
        .filter_map(|o| {
            crate::pipeline_expiry::space_of_endpoint(mirror, project, o.target_endpoint.local_id())
        })
        .collect();
    spaces.sort();
    spaces.dedup();
    spaces
}

/// The plans of every other pipeline of the project that writes `space`.
pub fn other_plans(mirror: &Mirror, project: &str, space: &str, except: &str) -> Vec<Plan> {
    mirror
        .list(project, "Pipeline", &ListOptions::default())
        .items
        .into_iter()
        .filter(|p| p.metadata.name != except)
        .filter_map(|p| {
            let spec: PipelineSpec = serde_json::from_value(p.spec.clone()).ok()?;
            Some(
                plans(&p.metadata.name, &spec)
                    .into_iter()
                    .filter(|plan| {
                        spaces_of(mirror, project, &spec, &plan.entity_type)
                            .iter()
                            .any(|s| s == space)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
}

/// The model check of PL-106 at admission: each type a class of every space it is written to,
/// each name a slot, each `observedAt` a date-time slot or `received`.
pub fn check(state: &AppState, project: &str, kind: &str, spec: &Value) -> Result<(), ApiError> {
    if kind != "Pipeline" {
        return Ok(());
    }
    let Ok(pipeline) = serde_json::from_value::<PipelineSpec>(spec.clone()) else {
        return Ok(());
    };
    let mut errors = Vec::new();
    for (index, entry) in pipeline.history.iter().enumerate() {
        let alone = PipelineSpec {
            history: vec![entry.clone()],
            ..pipeline.clone()
        };
        for space in spaces_of(&state.mirror, project, &pipeline, &entry.entity_type) {
            let Some(model) = state.model_schemas.get(project, &space) else {
                continue;
            };
            if let Err(jc_core::error::Error::Invalid { field, reason }) =
                jcctl::history::check(&alone, &model)
            {
                let field = field.replacen("spec.history[0]", &format!("spec.history[{index}]"), 1);
                errors.push(format!("{field}: {reason} in space {space}"));
            }
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(ApiError::Invalid {
        detail: "the pipeline's history plan does not fit the spaces it writes (PL-106)".to_owned(),
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(value: Value) -> Vec<RetentionRule> {
        serde_json::from_value(value).expect("rules parse")
    }

    fn plan(
        pipeline: &str,
        attribute: &str,
        keep: Keep,
        heartbeat: Option<&str>,
        observed_at: Option<&str>,
    ) -> Plan {
        Plan {
            pipeline: pipeline.to_owned(),
            entity_type: "Vehicle".to_owned(),
            attribute: attribute.to_owned(),
            keep,
            heartbeat: heartbeat.map(str::to_owned),
            observed_at: observed_at.map(str::to_owned),
        }
    }

    #[test]
    fn plans_read_one_row_per_name() {
        let spec: PipelineSpec = serde_json::from_value(json!({
            "class": "resident",
            "history": [{ "type": "Vehicle", "attributes": [
                { "names": ["location", "speed"], "keep": "changes", "heartbeat": "PT15M", "observedAt": "observedOn" },
                { "names": ["delay"], "keep": "every" }
            ]}]
        }))
        .expect("spec parses");
        let read = plans("hfp", &spec);
        assert_eq!(read.len(), 3);
        assert_eq!(
            read[1],
            plan(
                "hfp",
                "speed",
                Keep::Changes,
                Some("PT15M"),
                Some("observedOn")
            )
        );
    }

    #[test]
    fn the_fold_writes_what_the_broker_can_keep_and_leaves_the_rest_to_the_space() {
        let space = rules(json!([
            { "entityType": "Vehicle", "history": "P30D", "attributes": { "delay": { "record": "every" } } },
            { "entityType": "Stop", "history": "off" },
            { "entityType": "*", "history": "P7D" }
        ]));
        let bus = Plan {
            entity_type: "Bus".to_owned(),
            ..plan("bus", "speed", Keep::Every, None, None)
        };
        let folded = fold(
            &space,
            &[
                plan("hfp", "location", Keep::Changes, Some("PT15M"), None),
                plan("hfp", "speed", Keep::Latest, None, None),
                plan("hfp", "delay", Keep::Changes, Some("PT1H"), None),
                plan("hfp", "heading", Keep::Every, None, None),
                plan("other", "heading", Keep::Changes, Some("PT1H"), None),
                Plan {
                    entity_type: "Stop".to_owned(),
                    ..plan("stops", "name", Keep::Every, None, None)
                },
                bus.clone(),
            ],
        );
        let vehicle = &folded[0];
        let location = &vehicle.attributes["location"];
        assert_eq!(
            (location.record.as_deref(), location.heartbeat.as_deref()),
            (Some("onChange"), Some("PT15M"))
        );
        assert!(
            !vehicle.attributes.contains_key("speed"),
            "latest writes nothing"
        );
        assert_eq!(
            vehicle.attributes["delay"].record.as_deref(),
            Some("every"),
            "the space's own wins"
        );
        assert!(
            !vehicle.attributes.contains_key("heading"),
            "two plans that disagree fold none"
        );
        assert!(folded[1].attributes.is_empty(), "nothing under history off");
        assert_eq!(folded[3].entity_type, "Bus");
        assert_eq!(
            folded[3].history, None,
            "a new rule keeps the space's * history"
        );
        assert_eq!(
            folded[3].attributes["speed"].record.as_deref(),
            Some("every")
        );
        assert_eq!(fold(&space, &[]), space);
        // T-5058 (DM-132): without a * rule a type no rule names keeps no history, so nothing folds.
        let unruled = &space[..2];
        assert_eq!(fold(unruled, &[bus]), unruled.to_vec());
    }

    #[test]
    fn each_refusal_names_its_rule() {
        let space = rules(json!([
            { "entityType": "Vehicle", "history": "P30D", "attributes": { "delay": { "record": "every" } } },
            { "entityType": "*", "history": "off" }
        ]));
        let others = [plan("other", "heading", Keep::Every, None, None)];
        let rule = |p: &Plan| refusal(&space, "helsinki", p, &others).map(|(rule, _)| rule);
        assert_eq!(
            rule(&plan("hfp", "speed", Keep::Latest, None, None)),
            Some("latest")
        );
        let (_, reason) = refusal(
            &space,
            "helsinki",
            &plan("hfp", "speed", Keep::Latest, None, None),
            &others,
        )
        .expect("refused");
        assert!(reason.contains("space helsinki keeps P30D"), "{reason}");
        let stop = Plan {
            entity_type: "Stop".to_owned(),
            ..plan("hfp", "name", Keep::Every, None, None)
        };
        assert_eq!(
            rule(&stop),
            Some("historyOff"),
            "the * rule turns Stop's history off"
        );
        let latest_stop = Plan {
            keep: Keep::Latest,
            ..stop
        };
        assert_eq!(rule(&latest_stop), None);
        assert_eq!(
            rule(&plan("hfp", "speed", Keep::Every, None, Some("received"))),
            Some("received")
        );
        assert_eq!(
            rule(&plan("hfp", "delay", Keep::Changes, Some("PT1H"), None)),
            Some("spaceRule")
        );
        assert_eq!(
            rule(&plan("hfp", "delay", Keep::Every, None, None)),
            None,
            "the space's own rule agrees"
        );
        assert_eq!(
            rule(&plan("hfp", "heading", Keep::Changes, Some("PT1H"), None)),
            Some("otherPipeline")
        );
        assert_eq!(
            rule(&plan("hfp", "heading", Keep::Every, None, None)),
            None,
            "the same plan as the other"
        );
        assert_eq!(
            rule(&plan(
                "hfp",
                "location",
                Keep::Changes,
                Some("PT15M"),
                Some("observedOn")
            )),
            None
        );
    }

    #[test]
    fn a_record_without_its_time_is_refused_under_every_and_by_its_slot() {
        let timed = json!({ "speed": { "type": "Property", "value": 1, "observedAt": "2026-10-20T10:00:00Z" } });
        let untimed = json!({ "speed": [{ "type": "Property", "value": 1, "observedAt": "2026-10-20T10:00:00Z" }, { "type": "Property", "value": 2 }] });
        let every = plan("hfp", "speed", Keep::Every, None, None);
        assert_eq!(record_refusal(&every, &timed), None);
        assert_eq!(
            record_refusal(&every, &untimed).map(|(r, _)| r),
            Some("observedAt")
        );
        let from_slot = plan(
            "hfp",
            "speed",
            Keep::Changes,
            Some("PT1H"),
            Some("observedOn"),
        );
        let (_, reason) = record_refusal(&from_slot, &untimed).expect("refused");
        assert!(reason.contains("observedOn gives no date-time"), "{reason}");
        assert!(!reason.contains('2'), "no value of the record: {reason}");
        let received = plan(
            "hfp",
            "speed",
            Keep::Changes,
            Some("PT1H"),
            Some("received"),
        );
        assert_eq!(record_refusal(&received, &untimed), None);
        assert_eq!(
            record_refusal(&every, &json!({})),
            None,
            "an absent attribute is not judged"
        );
    }

    #[test]
    fn admission_names_each_history_entry_that_does_not_fit_a_space_it_writes() {
        use crate::config::Config;
        use crate::pipeline_validation::ModelSchema;
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        use std::collections::HashMap;
        use std::sync::Arc;

        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new("ep-vehicles", "helsinki"),
            spec: json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "vehicles" } }),
            status: None,
        });
        let model = ModelSchema::compile(
            "transport",
            "1.0.0",
            &json!({ "definitions": { "Vehicle": { "properties": {
                "id": { "type": "string" },
                "speed": { "type": ["number", "null"], "x-ngsi-ld-kind": "Property" }
            } } } }),
            &["Vehicle".to_owned()],
            false,
        );
        state.model_schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "vehicles".to_owned()),
            Arc::new(model),
        )]));
        let spec = |history: Value| {
            json!({
                "class": "resident",
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:ep-vehicles" }],
                "history": history,
            })
        };
        let fits =
            json!([{ "type": "Vehicle", "attributes": [{ "names": ["speed"], "keep": "every" }] }]);
        assert!(check(&state, "helsinki", "Pipeline", &spec(fits.clone())).is_ok());
        let misfit = json!([
            fits[0].clone(),
            { "type": "Vehicle", "attributes": [{ "names": ["colour"], "keep": "every" }] }
        ]);
        match check(&state, "helsinki", "Pipeline", &spec(misfit.clone())) {
            Err(ApiError::Invalid { errors, .. }) => {
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert!(errors[0].starts_with("spec.history[1]"), "{errors:?}");
                assert!(errors[0].ends_with("in space vehicles"), "{errors:?}");
            }
            other => panic!("not refused by entry: {other:?}"),
        }
        // Another kind, or a pipeline that writes no modelled space, is not judged here.
        assert!(check(&state, "helsinki", "DataSource", &spec(misfit.clone())).is_ok());
        assert!(check(&state, "oulu", "Pipeline", &spec(misfit)).is_ok());
    }
}
