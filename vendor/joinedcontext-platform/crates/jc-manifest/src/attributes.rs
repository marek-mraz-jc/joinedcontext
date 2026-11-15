//! `spec.attributes` (G-15) checked against the space's model and compiled into one Bloblang step
//! that runs after the author's steps and before the history and lifetime steps (PL-107 order).
//!
//! The text is generated from checked parts only: types match the entity-type pattern, names the
//! slot pattern, paths the record-path pattern, and every string enters the text as a JSON
//! literal, never as code.

use jc_core::error::{Error, Result};
use jc_core::kinds::pipeline_attributes::{
    dataset_template, is_record_path, DatasetPart, SourceDates, SystemMembers, DATE_CREATED,
    DATE_MODIFIED, FROM_MODEL, NOW, PIPELINE_DATASET, STRIPPED_MEMBERS,
};
use jc_core::kinds::pipeline_history::RECEIVED;
use jc_core::kinds::pipeline_lifetime::is_slot_name;
use jc_core::kinds::PipelineSpec;

use crate::model_schema::ModelSchema;

/// The model checks of G-15 at proposal time: each type a class of the space's model, each name
/// one of its slots, each `observedAt` slot a date-time slot, a `unitCode` the slot's own unit
/// (`model` needs the slot to declare one; a fixed code must be that one, or the gateway refuses
/// the write), and `dateCreated` / `dateModified` slots of the class when `sourceDates` fills them.
pub fn check(spec: &PipelineSpec, model: &ModelSchema) -> Result<()> {
    let refused = |field: String, reason: String| Error::Invalid { field, reason };
    for (index, entry) in spec.attributes.iter().enumerate() {
        let path = format!("spec.attributes[{index}]");
        let Some(class) = model.attributes(&entry.entity_type) else {
            return Err(refused(
                format!("{path}.type"),
                format!(
                    "the space's model declares no class `{}` (G-15)",
                    entry.entity_type
                ),
            ));
        };
        let slot = |name: &str| class.attributes.iter().find(|a| a.name == name);
        if let Some(dates) = &entry.source_dates {
            for (field, target) in [
                ("created", dates.created.as_ref().map(|_| DATE_CREATED)),
                ("modified", dates.modified.as_ref().map(|_| DATE_MODIFIED)),
            ] {
                if let Some(target) = target.filter(|target| slot(target).is_none()) {
                    return Err(refused(
                        format!("{path}.sourceDates.{field}"),
                        format!(
                            "`{}` has no slot `{target}` in the space's model {} {}; the source \
                             date has nowhere to go (G-15)",
                            entry.entity_type, class.model, class.version
                        ),
                    ));
                }
            }
        }
        for (at, rule) in entry.rules.iter().enumerate() {
            let at = format!("{path}.rules[{at}]");
            for name in &rule.names {
                let Some(attribute) = slot(name) else {
                    return Err(refused(
                        format!("{at}.names"),
                        format!(
                            "`{name}` is not a slot of `{}` in the space's model {} {} (G-15)",
                            entry.entity_type, class.model, class.version
                        ),
                    ));
                };
                let unit = attribute.unit.as_ref().and_then(|u| u.code.as_deref());
                match (rule.unit_code.as_deref(), unit) {
                    (None, _) => {}
                    (Some(FROM_MODEL), None) => {
                        return Err(refused(
                            format!("{at}.unitCode"),
                            format!(
                                "the model gives `{name}` no unit; write a UN/CEFACT code or no \
                                 unitCode (G-15)"
                            ),
                        ))
                    }
                    (Some(FROM_MODEL), Some(_)) => {}
                    (Some(code), Some(model_code)) if code != model_code => {
                        return Err(refused(
                            format!("{at}.unitCode"),
                            format!(
                                "the model writes `{name}` in `{model_code}`, not `{code}`; the \
                                 gateway would refuse the write, so write `model` (G-15, DM-06)"
                            ),
                        ))
                    }
                    (Some(_), _) => {}
                }
            }
            if let Some(field) = rule
                .observed_at
                .as_deref()
                .filter(|from| ![NOW, RECEIVED].contains(from))
            {
                if slot(field).and_then(|a| a.format.as_deref()) != Some("date-time") {
                    return Err(refused(
                        format!("{at}.observedAt"),
                        format!(
                            "`{field}` is not a date-time slot of `{}`; name one, `now` or \
                             `received` (G-15)",
                            entry.entity_type
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Each `unitCode: model` of `spec` replaced by the slot's code in `model`, for the render: the
/// step writes literal codes only. A slot without a unit keeps `model`, which [`step`] skips
/// (admission refuses that rule, [`check`]).
pub fn resolve_units(spec: &mut PipelineSpec, model: &ModelSchema) {
    for entry in &mut spec.attributes {
        let Some(class) = model.attributes(&entry.entity_type) else {
            continue;
        };
        for rule in &mut entry.rules {
            if rule.unit_code.as_deref() != Some(FROM_MODEL) {
                continue;
            }
            let codes: Vec<&str> = rule
                .names
                .iter()
                .filter_map(|name| class.attributes.iter().find(|a| &a.name == name))
                .filter_map(|a| a.unit.as_ref()?.code.as_deref())
                .collect();
            // One rule names one unit for all its names: a rule over slots of different units
            // stays unresolved rather than writing one slot's code onto another.
            if codes.len() == rule.names.len() && codes.windows(2).all(|w| w[0] == w[1]) {
                if let Some(code) = codes.first() {
                    rule.unit_code = Some((*code).to_owned());
                }
            }
        }
    }
}

/// The step that applies `spec.attributes` (G-15) to each entity of a planned type, `None` when
/// the pipeline has none. It takes a message holding one entity or an array of entities; a type
/// without an entry passes unchanged. `pipeline` names the `datasetId` `urn:jc:pipeline:<name>`.
/// The output is the same for the same manifest: entries by type, names sorted.
pub fn step(spec: &PipelineSpec, pipeline: &str) -> Result<Option<String>> {
    if spec.attributes.is_empty() {
        return Ok(None);
    }
    let invalid = |reason: String| Error::Invalid {
        field: "spec.attributes".to_owned(),
        reason,
    };
    let mut entries: Vec<(&str, String)> = Vec::new();
    for entry in &spec.attributes {
        let mut body = String::new();
        if let Some(dates) = &entry.source_dates {
            body.push_str(&source_dates(dates).map_err(invalid)?);
        }
        if entry.system_members == Some(SystemMembers::Strip) {
            for member in STRIPPED_MEMBERS {
                body.push_str(&format!("    root.{member} = deleted()\n"));
            }
        }
        let mut planned: Vec<(&str, String)> = Vec::new();
        for rule in &entry.rules {
            let members = members(rule, pipeline).map_err(invalid)?;
            for name in &rule.names {
                if !is_slot_name(name) {
                    return Err(invalid(format!(
                        "`{}` is not a slot name (G-15)",
                        name.escape_debug()
                    )));
                }
                if let Some(members) = &members {
                    planned.push((name, members.clone()));
                }
            }
        }
        planned.sort();
        for (name, members) in planned {
            body.push_str(&block(name, &members));
        }
        if !body.is_empty() {
            entries.push((&entry.entity_type, body));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort();
    let mut text = String::from(
        "# spec.attributes (G-15), generated by jcctl: observedAt, unitCode, datasetId, system \
         members and source dates, after the author's steps.\n\
         map jc_attributes {\n  root = this\n",
    );
    for (entity_type, body) in entries {
        text.push_str(&format!("  if this.type == {} {{\n", literal(entity_type)));
        text.push_str(&body);
        text.push_str("  }\n");
    }
    text.push_str(
        "}\n\
         root = if this.type() == \"array\" { this.map_each(e -> if e.type() == \"object\" { \
         e.apply(\"jc_attributes\") } else { e }) } else if this.type() == \"object\" { \
         this.apply(\"jc_attributes\") } else { this }\n",
    );
    Ok(Some(text))
}

/// A record path read as a value: a Property's `value`, else the member itself; null when absent.
fn read(path: &str) -> String {
    format!("this.{path}.(v -> if v.type() == \"object\" {{ v.value }} else {{ v }}).catch(null)")
}

fn source_dates(dates: &SourceDates) -> std::result::Result<String, String> {
    let mut text = String::new();
    for (from, target) in [
        (&dates.created, DATE_CREATED),
        (&dates.modified, DATE_MODIFIED),
    ] {
        let Some(from) = from else { continue };
        if !is_record_path(from) {
            return Err(format!(
                "`{}` is no record path (G-15)",
                from.escape_debug()
            ));
        }
        text.push_str(&format!(
            "    let {target} = {}\n    if ${target} != null {{\n      root.{target} = \
             {{\"type\": \"Property\", \"value\": ${target}}}\n    }}\n",
            read(from)
        ));
    }
    Ok(text)
}

/// The Bloblang object of members a rule sets on each instance, the `observedAt` of `received`
/// as `null` (removed by [`block`]); `None` when the rule sets nothing the step writes.
fn members(
    rule: &jc_core::kinds::AttributeRule,
    pipeline: &str,
) -> std::result::Result<Option<String>, String> {
    let mut parts: Vec<String> = Vec::new();
    match rule.observed_at.as_deref() {
        None => {}
        // `received` drops the instance's `observedAt` ([`block`]).
        Some(RECEIVED) => parts.push("\"observedAt\": null".to_owned()),
        Some(NOW) => parts.push(format!(
            "\"observedAt\": now().ts_parse({TS}).ts_format({TS}, \"UTC\")"
        )),
        Some(slot) if is_slot_name(slot) => parts.push(format!(
            "\"observedAt\": {}.ts_parse({TS}).ts_format({TS}, \"UTC\").catch(null)",
            read(slot)
        )),
        Some(other) => {
            return Err(format!(
                "`{}` is not a slot name (G-15)",
                other.escape_debug()
            ))
        }
    }
    match rule.unit_code.as_deref() {
        None | Some(FROM_MODEL) => {}
        Some(code) => parts.push(format!("\"unitCode\": {}", literal(code))),
    }
    match rule.dataset_id.as_deref() {
        None => {}
        Some(PIPELINE_DATASET) => parts.push(format!(
            "\"datasetId\": {}",
            literal(&format!("urn:jc:pipeline:{pipeline}"))
        )),
        Some(template) => {
            let pieces: Vec<String> = dataset_template(template)?
                .into_iter()
                .map(|part| match part {
                    DatasetPart::Text(text) => literal(&text),
                    DatasetPart::Field(path) => format!(
                        "this.{path}.(v -> if v.type() == \"object\" {{ v.value }} else {{ v \
                         }}).not_null().string().escape_url_query().replace_all(\"+\", \"%20\")"
                    ),
                })
                .collect();
            // A field the record lacks leaves the instance without a datasetId: a write never
            // carries a datasetId with a hole in it.
            parts.push(format!(
                "\"datasetId\": ({}).catch(null)",
                pieces.join(" + ")
            ));
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("{{{}}}", parts.join(", "))))
}

/// The time format the steps read and write: RFC 3339 with up to nine fraction digits.
const TS: &str = "\"2006-01-02T15:04:05.999999999Z07:00\"";

/// Sets `members` on every instance of `name`. A member whose value is null is not written: a
/// datasetId with a missing field stays off, and a `received` rule's null `observedAt` removes the
/// instance's own.
fn block(name: &str, members: &str) -> String {
    let drop = if members.contains("\"observedAt\": null") {
        ".without(\"observedAt\")"
    } else {
        ""
    };
    format!(
        "    if this.exists({key}) {{\n      let keep = {members}.filter(kv -> kv.value != null)\n      \
         root.{name} = this.{name}.(a -> if a.type() == \"array\" {{ a.map_each(i -> if i.type() \
         == \"object\" {{ i{drop}.assign($keep) }} else {{ i }}) }} else if a.type() == \"object\" \
         {{ a{drop}.assign($keep) }} else {{ a }})\n    }}\n",
        key = literal(name),
    )
}

/// A Bloblang string literal: JSON's quoting is Bloblang's.
fn literal(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(attributes: &str) -> PipelineSpec {
        serde_norway::from_str(&format!(
            "class: resident\nsources: [{{ dataSourceRef: {{ kind: DataSource, name: s }} }}]\n\
             outputs: [{{ targetEndpoint: \"urn:ngsi-ld:Endpoint:example.org:air:air\", type: \
             WeatherObserved }}]\nattributes: {attributes}\n"
        ))
        .expect("a pipeline spec")
    }

    #[test]
    fn no_rules_render_no_step_and_the_step_is_stable() {
        assert_eq!(step(&spec("[]"), "p").expect("renders"), None);
        let rules = "[{ type: WeatherObserved, systemMembers: strip, sourceDates: { created: \
                     createdAt }, rules: [{ names: [temperature, humidity], unitCode: CEL, \
                     datasetId: pipeline }] }]";
        let text = step(&spec(rules), "air-ingest")
            .expect("renders")
            .expect("a step");
        assert_eq!(
            step(&spec(rules), "air-ingest").expect("renders"),
            Some(text.clone())
        );
        assert!(text.contains("root.createdAt = deleted()"), "{text}");
        assert!(
            text.contains("\"datasetId\": \"urn:jc:pipeline:air-ingest\""),
            "{text}"
        );
        assert!(text.contains("\"unitCode\": \"CEL\""), "{text}");
        // humidity sorts before temperature.
        assert!(
            text.find("\"humidity\"") < text.find("\"temperature\""),
            "{text}"
        );
        // The source date is read before the system members go.
        assert!(text.find("let dateCreated") < text.find("root.createdAt = deleted()"));
    }

    #[test]
    fn a_rule_left_on_model_writes_no_unit_code() {
        let text = step(
            &spec("[{ type: WeatherObserved, rules: [{ names: [t], unitCode: model, observedAt: now }] }]"),
            "p",
        )
        .expect("renders")
        .expect("a step");
        assert!(!text.contains("\"unitCode\":"), "{text}");
        assert!(text.contains("now()"), "{text}");
    }
}
