//! `spec.attributes` of a Pipeline (G-15): per written type, the rules the rendered stream
//! applies to every entity of it after the author's steps and before the history and lifetime
//! steps (PL-107 order): where each chosen attribute's `observedAt` comes from, the `unitCode`
//! and `datasetId` it is written with, the NGSI-LD system members a source carries dropped, and a
//! source's own created and modified dates kept as the model's `dateCreated` and `dateModified`.
//!
//! The checks here need neither the space's model nor a clock; the ones that do (a name that is
//! not a slot, an `observedAt` slot that is no date-time, a unit the model does not declare) are
//! `jc_manifest::attributes::check`. A rule widens no grant: it only sets members of attributes
//! the pipeline writes anyway (PL-84 reasoning).

use crate::error::{Error, Result};
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::LazyLock;

use super::pipeline_history::{History, RECEIVED};
use super::pipeline_lifetime::{is_slot_name, LIFETIME_MAX_ATTRIBUTES, LIFETIME_MAX_TYPES};

/// The `observedAt` of the write's own time: the moment the generated step runs.
pub const NOW: &str = "now";
/// The `unitCode` the space's model declares for the slot (its `unece:` mapping).
pub const FROM_MODEL: &str = "model";
/// The `datasetId` `urn:jc:pipeline:<name>` of the pipeline itself (PL-100).
pub const PIPELINE_DATASET: &str = "pipeline";
/// The model slot a source's creation date is kept in.
pub const DATE_CREATED: &str = "dateCreated";
/// The model slot a source's last change date is kept in.
pub const DATE_MODIFIED: &str = "dateModified";
/// The system members `systemMembers: strip` drops; `id`, `type`, `scope`, `@context` and
/// `expiresAt` are the writer's own (DM-130, PL-80).
pub const STRIPPED_MEMBERS: [&str; 3] = ["createdAt", "modifiedAt", "deletedAt"];

/// A record path: dot-separated identifiers, nothing a Bloblang query could read as an operator.
static RECORD_PATH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,63}(\.[A-Za-z_][A-Za-z0-9_]{0,63}){0,7}$")
        .expect("a valid regex")
});
/// A UN/CEFACT Recommendation 20 code: two or three capitals or digits.
static UNIT_CODE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Z0-9]{2,3}$").expect("a valid regex"));
/// The literal text of a `datasetId` template: RFC 8141 characters without `$`, `{` or `}`.
static TEMPLATE_TEXT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9._~!&'()*+,;=:@/%-]*$").expect("a valid regex")
});

/// The attribute rules of one written type (G-15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AttributeRules {
    /// The entity type, one an output of the pipeline writes.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// Up to 64 rules, each attribute in one of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<AttributeRule>,
    /// `strip` drops the `createdAt`, `modifiedAt` and `deletedAt` the source carries, which the
    /// broker sets itself; absent keeps the record as the steps leave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_members: Option<SystemMembers>,
    /// The record members holding the source's own created and modified dates, kept as the
    /// Properties `dateCreated` and `dateModified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dates: Option<SourceDates>,
}

/// What happens to the system members a source carries (DM-130).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SystemMembers {
    /// Drop `createdAt`, `modifiedAt` and `deletedAt`.
    Strip,
}

/// Where a source's own dates are read (record paths such as `createdAt` or `meta.updated`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceDates {
    /// Written as `dateCreated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Written as `dateModified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

/// One rule over chosen attributes of a type (G-15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AttributeRule {
    /// One to 64 slots of the type.
    pub names: Vec<String>,
    /// A date-time slot of the type whose value is each instance's `observedAt`, `now` (the
    /// write's time) or `received` (none: the broker's receive time, DM-107).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// `model` (the slot's unit in the space's model) or a UN/CEFACT code such as `CEL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_code: Option<String>,
    /// `pipeline` (`urn:jc:pipeline:<name>`, PL-100) or a template such as
    /// `urn:ngsi-ld:Dataset:{station.code}` whose `{path}` reads the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_id: Option<String>,
}

/// One part of a `datasetId` template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetPart {
    /// Text written as it is.
    Text(String),
    /// A record path whose value is written.
    Field(String),
}

/// The parts of a `datasetId` template: it starts `urn:`, holds RFC 8141 text and `{path}`
/// fields, and at least one field (a fixed id would be one dataset for every entity).
pub fn dataset_template(template: &str) -> std::result::Result<Vec<DatasetPart>, String> {
    if !template.starts_with("urn:") {
        return Err("a datasetId template starts with `urn:`".to_owned());
    }
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (text, after) = rest.split_at(open);
        let close = after
            .find('}')
            .ok_or("a `{` is closed by a `}`".to_owned())?;
        let field = &after[1..close];
        if !text.is_empty() {
            parts.push(DatasetPart::Text(text.to_owned()));
        }
        if !RECORD_PATH.is_match(field) {
            return Err(format!(
                "`{}` is no record path such as `station.code`",
                field.escape_debug()
            ));
        }
        parts.push(DatasetPart::Field(field.to_owned()));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(DatasetPart::Text(rest.to_owned()));
    }
    if parts
        .iter()
        .any(|part| matches!(part, DatasetPart::Text(text) if !TEMPLATE_TEXT.is_match(text)))
    {
        return Err("the text of a template holds RFC 8141 characters only".to_owned());
    }
    if !parts
        .iter()
        .any(|part| matches!(part, DatasetPart::Field(_)))
    {
        return Err(format!(
            "a template names at least one `{{field}}`; for one dataset of the pipeline write \
             `{PIPELINE_DATASET}`"
        ));
    }
    Ok(parts)
}

/// Whether `path` is a record path a rule may read.
pub fn is_record_path(path: &str) -> bool {
    RECORD_PATH.is_match(path)
}

/// What [`validate`] reads of the rest of the Pipeline.
pub(super) struct Context<'a> {
    /// The types the outputs write; `None` when an output names none.
    pub written: Option<&'a BTreeSet<&'a str>>,
    /// `spec.history`, whose `observedAt` plan a rule must not repeat.
    pub history: &'a [History],
    /// Every Bloblang text of the author's steps and routes' steps.
    pub author_texts: &'a [String],
}

/// The admission checks of G-15 that need no model (PL-81, PL-83 style).
pub(super) fn validate(entries: &[AttributeRules], context: &Context<'_>) -> Result<()> {
    let refused = |field: String, reason: String| Error::Invalid { field, reason };
    if entries.len() > LIFETIME_MAX_TYPES {
        return Err(refused(
            "spec.attributes".to_owned(),
            format!("name one to {LIFETIME_MAX_TYPES} types (G-15)"),
        ));
    }
    let assigned = observed_at_assignments(context.author_texts);
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("spec.attributes[{index}]");
        names::validate_entity_type(&entry.entity_type)?;
        if entries[..index]
            .iter()
            .any(|other| other.entity_type == entry.entity_type)
        {
            return Err(refused(
                format!("{path}.type"),
                format!(
                    "`{}` has attribute rules already; write one entry per type (G-15)",
                    entry.entity_type
                ),
            ));
        }
        if let Some(written) = context.written {
            if !written.contains(entry.entity_type.as_str()) {
                return Err(refused(
                    format!("{path}.type"),
                    format!(
                        "no output of this pipeline writes `{}`; the outputs write {} (G-15)",
                        entry.entity_type,
                        written.iter().copied().collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
        if entry.rules.is_empty() && entry.system_members.is_none() && entry.source_dates.is_none()
        {
            return Err(refused(
                path,
                "give `rules`, `systemMembers` or `sourceDates`; a type the steps write as it \
                 should be needs no entry (G-15)"
                    .to_owned(),
            ));
        }
        if let Some(dates) = &entry.source_dates {
            for (field, value) in [("created", &dates.created), ("modified", &dates.modified)] {
                if let Some(value) = value.as_deref().filter(|v| !is_record_path(v)) {
                    return Err(refused(
                        format!("{path}.sourceDates.{field}"),
                        format!(
                            "`{}` is no record path such as `meta.updated` (G-15)",
                            value.escape_debug()
                        ),
                    ));
                }
            }
            if dates.created.is_none() && dates.modified.is_none() {
                return Err(refused(
                    format!("{path}.sourceDates"),
                    "name `created`, `modified` or both (G-15)".to_owned(),
                ));
            }
        }
        let planned: BTreeSet<&str> = context
            .history
            .iter()
            .filter(|plan| plan.entity_type == entry.entity_type)
            .flat_map(|plan| &plan.attributes)
            .filter(|attribute| attribute.observed_at.is_some())
            .flat_map(|attribute| attribute.names.iter().map(String::as_str))
            .collect();
        validate_rules(&path, entry, &planned, &assigned)?;
    }
    Ok(())
}

fn validate_rules(
    path: &str,
    entry: &AttributeRules,
    planned: &BTreeSet<&str>,
    assigned: &BTreeSet<String>,
) -> Result<()> {
    let refused = |field: String, reason: String| Error::Invalid { field, reason };
    if entry.rules.len() > LIFETIME_MAX_ATTRIBUTES {
        return Err(refused(
            format!("{path}.rules"),
            format!("write at most {LIFETIME_MAX_ATTRIBUTES} rules (G-15)"),
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, rule) in entry.rules.iter().enumerate() {
        let at = format!("{path}.rules[{index}]");
        if rule.names.is_empty() || rule.names.len() > LIFETIME_MAX_ATTRIBUTES {
            return Err(refused(
                format!("{at}.names"),
                format!("name one to {LIFETIME_MAX_ATTRIBUTES} attributes (G-15)"),
            ));
        }
        if rule.observed_at.is_none() && rule.unit_code.is_none() && rule.dataset_id.is_none() {
            return Err(refused(
                at,
                "give `observedAt`, `unitCode` or `datasetId` (G-15)".to_owned(),
            ));
        }
        for name in &rule.names {
            if !is_slot_name(name) {
                return Err(refused(
                    format!("{at}.names"),
                    format!(
                        "`{}` is not an attribute: a slot name, never a system member such as \
                         createdAt (DM-130, G-15)",
                        name.escape_debug()
                    ),
                ));
            }
            if !seen.insert(name.as_str()) {
                return Err(refused(
                    format!("{at}.names"),
                    format!(
                        "`{name}` is in two rules of `{}`; keep one (G-15)",
                        entry.entity_type
                    ),
                ));
            }
            if rule.observed_at.is_some() && planned.contains(name.as_str()) {
                return Err(refused(
                    format!("{at}.observedAt"),
                    format!(
                        "spec.history plans the observedAt of `{name}` already; set it in one \
                         place (G-15, PL-106)"
                    ),
                ));
            }
            if rule.observed_at.is_some() && assigned.contains(name) {
                return Err(refused(
                    format!("{at}.observedAt"),
                    format!(
                        "a step of this pipeline assigns `{name}.observedAt` itself; remove that \
                         assignment from the mapping, the rule sets it (G-15, PL-83)"
                    ),
                ));
            }
        }
        match rule.observed_at.as_deref() {
            None | Some(NOW) | Some(RECEIVED) => {}
            Some(slot) if is_slot_name(slot) => {}
            Some(other) => {
                return Err(refused(
                    format!("{at}.observedAt"),
                    format!(
                        "`{}` is neither a slot name, `now` nor `received` (G-15)",
                        other.escape_debug()
                    ),
                ))
            }
        }
        match rule.unit_code.as_deref() {
            None | Some(FROM_MODEL) => {}
            Some(code) if UNIT_CODE.is_match(code) => {}
            Some(other) => {
                return Err(refused(
                    format!("{at}.unitCode"),
                    format!(
                        "`{}` is neither `model` nor a UN/CEFACT code such as `CEL` (G-15)",
                        other.escape_debug()
                    ),
                ))
            }
        }
        if let Some(template) = rule
            .dataset_id
            .as_deref()
            .filter(|id| *id != PIPELINE_DATASET)
        {
            dataset_template(template).map_err(|reason| {
                refused(
                    format!("{at}.datasetId"),
                    format!("{reason} (G-15, PL-100)"),
                )
            })?;
        }
    }
    Ok(())
}

/// The attributes whose `observedAt` an author's step assigns: `root.<name>.observedAt = …` or a
/// bare `<name>.observedAt = …` line. A scan cannot follow every expression; it does not have to
/// be complete for safety, because the generated step runs after the author's.
fn observed_at_assignments(texts: &[String]) -> BTreeSet<String> {
    static ASSIGNED: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"(?m)(?:^[ \t]*(?:root[ \t]*\.[ \t]*)?|\broot[ \t]*\.[ \t]*)"?([A-Za-z][A-Za-z0-9_]*)"?[ \t]*\.[ \t]*"?observedAt"?[ \t]*=(?:[^=]|$)"#,
        )
        .expect("valid regex")
    });
    texts
        .iter()
        .flat_map(|text| {
            ASSIGNED
                .captures_iter(text)
                .map(|found| found[1].to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(yaml: &str) -> Vec<AttributeRules> {
        serde_norway::from_str(yaml).expect("attribute rules")
    }

    fn check(yaml: &str, author: &[&str]) -> Result<()> {
        let written: BTreeSet<&str> = ["WeatherObserved"].into();
        let history: Vec<History> = serde_norway::from_str(
            "[{ type: WeatherObserved, attributes: [{ names: [pressure], keep: every, observedAt: dateObserved }] }]",
        )
        .expect("history");
        let author: Vec<String> = author.iter().map(|s| (*s).to_owned()).collect();
        validate(
            &entries(yaml),
            &Context {
                written: Some(&written),
                history: &history,
                author_texts: &author,
            },
        )
    }

    fn refused_at(result: Result<()>) -> String {
        match result {
            Err(Error::Invalid { field, reason }) => format!("{field}: {reason}"),
            other => panic!("refused, got {other:?}"),
        }
    }

    const GOOD: &str = r#"
- type: WeatherObserved
  systemMembers: strip
  sourceDates: { created: createdAt, modified: meta.updated }
  rules:
    - names: [temperature]
      observedAt: dateObserved
      unitCode: model
    - names: [humidity]
      observedAt: now
      unitCode: P1
      datasetId: "urn:ngsi-ld:Dataset:{station.code}"
    - names: [windSpeed]
      datasetId: pipeline
"#;

    #[test]
    fn a_full_entry_is_admitted_and_reads_back_the_same() {
        check(GOOD, &[]).expect("admitted");
        let parsed = entries(GOOD);
        let again: Vec<AttributeRules> =
            serde_json::from_value(serde_json::to_value(&parsed).expect("json")).expect("parses");
        assert_eq!(parsed, again);
        assert_eq!(parsed[0].system_members, Some(SystemMembers::Strip));
    }

    /// G-15 acceptance: a rule naming a system member or a non-slot is refused with its path
    /// (as PL-81); a rule setting an observedAt the mapping assigns is refused (as PL-83), and so
    /// is one spec.history already plans.
    #[test]
    fn each_bad_rule_is_refused_with_its_path() {
        for (yaml, author, at) in [
            ("[{ type: WeatherObserved, rules: [{ names: [createdAt], unitCode: CEL }] }]", "", "spec.attributes[0].rules[0].names"),
            ("[{ type: WeatherObserved, rules: [{ names: [\"a-b\"], unitCode: CEL }] }]", "", "spec.attributes[0].rules[0].names"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], observedAt: now }] }]", "root.temperature.observedAt = this.ts", "spec.attributes[0].rules[0].observedAt"),
            ("[{ type: WeatherObserved, rules: [{ names: [pressure], observedAt: now }] }]", "", "spec.attributes[0].rules[0].observedAt"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], observedAt: \"this.x\" }] }]", "", "spec.attributes[0].rules[0].observedAt"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], unitCode: celsius }] }]", "", "spec.attributes[0].rules[0].unitCode"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], datasetId: \"urn:ngsi-ld:Dataset:fixed\" }] }]", "", "spec.attributes[0].rules[0].datasetId"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], datasetId: \"urn:x:{a b}\" }] }]", "", "spec.attributes[0].rules[0].datasetId"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature], datasetId: \"http://x/{a}\" }] }]", "", "spec.attributes[0].rules[0].datasetId"),
            ("[{ type: WeatherObserved, rules: [{ names: [temperature] }] }]", "", "spec.attributes[0].rules[0]"),
            ("[{ type: WeatherObserved, rules: [{ names: [t], unitCode: CEL }, { names: [t], unitCode: CEL }] }]", "", "spec.attributes[0].rules[1].names"),
            ("[{ type: Vehicle, systemMembers: strip }]", "", "spec.attributes[0].type"),
            ("[{ type: WeatherObserved }]", "", "spec.attributes[0]"),
            ("[{ type: WeatherObserved, sourceDates: { created: \"a..b\" } }]", "", "spec.attributes[0].sourceDates.created"),
            ("[{ type: WeatherObserved, sourceDates: {} }]", "", "spec.attributes[0].sourceDates"),
            ("[{ type: WeatherObserved, systemMembers: strip }, { type: WeatherObserved, systemMembers: strip }]", "", "spec.attributes[1].type"),
        ] {
            let text = refused_at(check(yaml, &[author]));
            assert!(text.starts_with(&format!("{at}: ")), "{yaml}: {text}");
        }
    }

    #[test]
    fn a_dataset_template_is_split_into_text_and_fields() {
        assert_eq!(
            dataset_template("urn:ngsi-ld:Dataset:{station.code}:{kind}"),
            Ok(vec![
                DatasetPart::Text("urn:ngsi-ld:Dataset:".into()),
                DatasetPart::Field("station.code".into()),
                DatasetPart::Text(":".into()),
                DatasetPart::Field("kind".into()),
            ])
        );
        assert!(dataset_template("urn:x:$(env)").is_err());
        assert!(dataset_template("urn:x:{a").is_err());
    }
}
