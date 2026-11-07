//! How a pipeline's writes become history rows, per entity type and attribute (T-4844, DM-104..
//! DM-109): the record mode the space's retention rule gives the attribute, where its
//! `observedAt` comes from, its `unitCode` and `datasetId`, the type's history, the pipeline's
//! lifetime, and a verdict. The deployment's `scripts/temporal-audit.py` applies the same rules to
//! the seeds in CI.
//!
//! The mapping is read as text, the way a person reads it: every attribute object (`"type":
//! "Property"` and the other NGSI-LD kinds) with the key in front of it, under the last entity
//! type literal before it.
//! ponytail: an attribute built inside a named Bloblang map is not seen; running the pipeline's
//! output samples through the same rules is the upgrade path.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use jc_core::kinds::RetentionRule;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

/// The `history` of a type that records no rows (DM-97).
const HISTORY_OFF: &str = "off";
/// The history the broker keeps for a type no rule names: none, since the platform runs it with
/// `ANTARES_TEMPORAL_RECORD=ruled` (history opt-in, DM-132, T-5056).
pub const PLATFORM_HISTORY: &str = HISTORY_OFF;
const ANY_TYPE: &str = "*";

const KINDS: &[&str] = &[
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "ListProperty",
    "VocabProperty",
    "JsonProperty",
];
const GEOJSON: &[&str] = &[
    "Point",
    "LineString",
    "Polygon",
    "MultiPoint",
    "MultiLineString",
    "MultiPolygon",
    "GeometryCollection",
    "Feature",
    "FeatureCollection",
];
/// Members of an attribute object, never an attribute's own name.
const MEMBERS: &[&str] = &[
    "type",
    "value",
    "object",
    "observedAt",
    "unitCode",
    "datasetId",
    "languageMap",
    "vocab",
    "json",
    "valueList",
    "coordinates",
];
/// How far before an attribute object its key is looked for.
const KEY_WINDOW: usize = 600;

/// Where a row's `observedAt` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ObservedAtSource)]
pub enum ObservedAt {
    /// An expression of the record: the time the source measured it.
    Mapped,
    /// One literal time: every write is the same measurement (DM-104).
    Constant,
    /// None, or `now()`: the broker stamps the time it received the write (DM-107).
    Received,
}

/// What the rows of one attribute come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
#[schema(as = TemporalVerdict)]
pub enum Verdict {
    /// The rows match the measurements.
    Ok,
    /// A constant `observedAt` makes every write one measurement, so each replaces the last (DM-105).
    LosesHistory,
    /// `history: forever` keeps every row.
    UnboundedGrowth,
    /// Without `observedAt` under `record: every`, an unchanged value is a new row on every write.
    DuplicatesRows,
    /// A measurement (an attribute with a unit) without `observedAt`: its rows carry the receive time.
    StampedOnReceive,
}

/// One attribute a mapping writes, as the scan saw it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Seen {
    pub observed_at: BTreeSet<ObservedAt>,
    pub unit_code: BTreeSet<String>,
    pub dataset_id: BTreeSet<String>,
    /// One write of it carries a `unitCode` and no `observedAt`.
    pub received_with_unit: bool,
}

/// One row of the audit (T-4844). Names and sources only: no value of the data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TemporalAuditRow)]
pub struct AuditRow {
    pub pipeline: String,
    pub entity_type: String,
    pub attribute: String,
    /// `every`, `onChange`, or `none` under `history: off` (DM-108).
    pub record: String,
    pub observed_at: Vec<ObservedAt>,
    /// The codes the mapping writes, or the model's unit as `model <code>` when it writes none.
    pub unit_code: Vec<String>,
    /// The literal `datasetId`s, or `mapped` for one the record decides.
    pub dataset_id: Vec<String>,
    /// How long the type's rows are kept: the rule's, the space's `*` rule's, or the platform's.
    pub history: String,
    /// What ends the entities: the pipeline's `lifetime` and `expiry`, or an `expiresAt` it maps.
    pub lifetime: Vec<String>,
    pub verdict: Verdict,
}

/// The text with every string literal's inside and every `#` comment blanked, so their braces and
/// quotes do not count.
fn blanked(text: &[u8]) -> Vec<u8> {
    let mut out = text.to_vec();
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'"' => {
                let mut j = i + 1;
                while j < text.len() && text[j] != b'"' {
                    j += if text[j] == b'\\' { 2 } else { 1 };
                }
                let end = j.min(text.len());
                out[i + 1..end].fill(b' ');
                i = end + 1;
            }
            b'#' => {
                let end = text[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(text.len(), |n| i + n);
                out[i..end].fill(b' ');
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

/// The `{` before `at` still open there, and the index after its matching `}`.
fn enclosing(blank: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut start = None;
    for i in (0..at).rev() {
        match blank[i] {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                start = Some(i);
                break;
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    let start = start?;
    let mut depth = 0usize;
    for (i, &c) in blank.iter().enumerate().skip(start) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The expression of member `name` at the object's own level.
fn member<'a>(text: &'a str, blank: &[u8], obj: (usize, usize), name: &str) -> Option<&'a str> {
    let quoted = format!("\"{name}\"");
    let inside = &text[obj.0..obj.1];
    for (at, _) in inside.match_indices(&quoted) {
        if enclosing(blank, obj.0 + at) != Some(obj) {
            continue;
        }
        // `"name"`, blanks, `:`, blanks: the member's key, as a JSON or Bloblang object writes it
        let after = &inside[at + quoted.len()..];
        let Some(value) = after.trim_start().strip_prefix(':') else {
            continue;
        };
        let from = obj.1 - value.trim_start().len();
        let mut depth = 0usize;
        let mut to = from;
        while to < obj.1 {
            match blank[to] {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' if depth == 0 => break,
                b'}' | b']' | b')' => depth -= 1,
                b',' | b'\n' if depth == 0 => break,
                _ => {}
            }
            to += 1;
        }
        return text.get(from..to).map(str::trim);
    }
    None
}

fn is_literal(expr: &str) -> bool {
    expr.len() >= 2
        && expr.starts_with('"')
        && expr.ends_with('"')
        && !expr[1..expr.len() - 1].contains('"')
}

fn observed_source(expr: Option<&str>) -> ObservedAt {
    match expr {
        None => ObservedAt::Received,
        Some(e) if e.contains("now()") => ObservedAt::Received,
        Some(e) if is_literal(e) => ObservedAt::Constant,
        Some(_) => ObservedAt::Mapped,
    }
}

/// The entity type literal and the member keys of a mapping, built once (T-4965). The patterns
/// are constants: a failure to build one is a bug a test catches, never input.
static PATTERNS: LazyLock<Option<(regex::Regex, regex::Regex)>> = LazyLock::new(|| {
    Some((
        regex::Regex::new(r#""type"\s*:\s*"([A-Za-z_][\w-]*)""#).ok()?,
        regex::Regex::new(
            r#""([A-Za-z_@][\w@]*)"\s*:|root\.([A-Za-z_]\w*)\s*=|(\$[A-Za-z_]\w*)\s*:"#,
        )
        .ok()?,
    ))
});

/// Each (entity type, attribute) a mapping writes. `default_type` is the type of an attribute
/// written before any entity type literal, the pipeline's one output type.
pub fn scan(text: &str, default_type: Option<&str>) -> BTreeMap<(String, String), Seen> {
    let Some((type_literal, key)) = PATTERNS.as_ref() else {
        return BTreeMap::new();
    };
    let blank = blanked(text.as_bytes());
    let mut found: BTreeMap<(String, String), Seen> = BTreeMap::new();
    let mut entity_type = default_type.map(str::to_owned);
    for m in type_literal.captures_iter(text) {
        let (Some(whole), Some(name)) = (m.get(0), m.get(1)) else {
            continue;
        };
        // Inside another string literal or a comment.
        if blank.get(whole.start()) != Some(&b'"') {
            continue;
        }
        let name = name.as_str();
        if GEOJSON.contains(&name) {
            continue;
        }
        if !KINDS.contains(&name) {
            entity_type = Some(name.to_owned());
            continue;
        }
        let (Some(obj), Some(of)) = (enclosing(&blank, whole.start()), entity_type.as_ref()) else {
            continue;
        };
        let mut from = obj.0.saturating_sub(KEY_WINDOW);
        while !text.is_char_boundary(from) {
            from -= 1;
        }
        let attribute = key
            .captures_iter(&text[from..obj.0])
            .filter_map(|k| k.get(1).or(k.get(2)).or(k.get(3)).map(|k| k.as_str()))
            .filter(|k| !MEMBERS.contains(k) && !k.starts_with('_'))
            .last();
        let Some(attribute) = attribute else {
            continue;
        };
        let seen = found.entry((of.clone(), attribute.to_owned())).or_default();
        let source = observed_source(member(text, &blank, obj, "observedAt"));
        seen.observed_at.insert(source);
        let unit = member(text, &blank, obj, "unitCode");
        seen.received_with_unit |= source == ObservedAt::Received && unit.is_some();
        for (field, into) in [
            (unit, &mut seen.unit_code),
            (member(text, &blank, obj, "datasetId"), &mut seen.dataset_id),
        ] {
            if let Some(v) = field.filter(|v| !v.is_empty()) {
                into.insert(match is_literal(v) {
                    true => v.trim_matches('"').to_owned(),
                    false => "mapped".to_owned(),
                });
            }
        }
    }
    found
}

/// The first rule that holds, in the order [`Verdict`] lists them.
pub fn verdict(
    history: &str,
    record: &str,
    observed: &BTreeSet<ObservedAt>,
    measured: bool,
) -> Verdict {
    if history == HISTORY_OFF {
        return Verdict::Ok;
    }
    if observed.contains(&ObservedAt::Constant) {
        return Verdict::LosesHistory;
    }
    if history == "forever" {
        return Verdict::UnboundedGrowth;
    }
    let received = observed.contains(&ObservedAt::Received);
    // `observed` stamped on receive is newer on every write: a row per write, as `every` (DM-132).
    if received
        && [
            jc_core::kinds::RECORD_EVERY,
            jc_core::kinds::space::RECORD_OBSERVED,
        ]
        .contains(&record)
    {
        return Verdict::DuplicatesRows;
    }
    if received && measured {
        return Verdict::StampedOnReceive;
    }
    Verdict::Ok
}

/// Every Bloblang a pipeline spec carries, in its steps, compute and routes.
fn bloblang_of(spec: &Value, into: &mut String) {
    match spec {
        Value::Object(map) => {
            for (k, v) in map {
                match (k.as_str(), v) {
                    ("bloblang", Value::String(text)) => {
                        into.push_str(text);
                        into.push('\n');
                    }
                    _ => bloblang_of(v, into),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| bloblang_of(v, into)),
        _ => {}
    }
}

/// What ends a pipeline's entities: each `lifetime` entry's duration, the `expiry` window, and an
/// `expiresAt` the mapping writes.
fn lifetime_of(spec: &Value, mapping: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for entry in spec
        .get("lifetime")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let said = ["after", "until", "from"]
            .iter()
            .find_map(|k| entry.get(*k).and_then(Value::as_str));
        out.insert(said.unwrap_or("lifetime").to_owned());
    }
    if let Some(after) = spec.pointer("/expiry/after").and_then(Value::as_str) {
        out.insert(format!("expiry {after}"));
    }
    let mut list: Vec<String> = out.into_iter().collect();
    if mapping.contains("\"expiresAt\"") {
        list.push("expiresAt in mapping".to_owned());
    }
    list
}

/// One pipeline writing one space: the spec, the `bento.yaml` beside it, the space's retention and
/// the model's unit of each (type, attribute).
pub struct Input<'a> {
    pub pipeline: &'a str,
    pub spec: &'a Value,
    pub bento: Option<&'a str>,
    pub default_type: Option<&'a str>,
    pub retention: &'a [RetentionRule],
    pub unit: &'a dyn Fn(&str, &str) -> Option<String>,
}

pub fn rows(input: &Input) -> Vec<AuditRow> {
    let mut mapping = String::new();
    bloblang_of(input.spec, &mut mapping);
    if let Some(bento) = input.bento {
        mapping.push_str(bento);
    }
    let lifetime = lifetime_of(input.spec, &mapping);
    let rule = |t: &str| input.retention.iter().find(|r| r.entity_type == t);
    let any = rule(ANY_TYPE).and_then(|r| r.history.clone());
    // A history plan's `observedAt` is the time the rendered step writes (PL-107), over the scan's.
    let planned: Vec<crate::pipeline_history::Plan> =
        serde_json::from_value::<jc_core::kinds::PipelineSpec>(input.spec.clone())
            .map(|spec| crate::pipeline_history::plans(input.pipeline, &spec))
            .unwrap_or_default();
    scan(&mapping, input.default_type)
        .into_iter()
        .map(|((entity_type, attribute), mut seen)| {
            let plan = planned
                .iter()
                .find(|p| p.entity_type == entity_type && p.attribute == attribute);
            match plan.and_then(|p| p.observed_at.as_deref()) {
                Some(jc_core::kinds::pipeline_history::RECEIVED) => {
                    seen.observed_at = [ObservedAt::Received].into();
                }
                Some(_) => seen.observed_at = [ObservedAt::Mapped].into(),
                None => {}
            }
            let own = rule(&entity_type);
            let history = own
                .and_then(|r| r.history.clone())
                .or_else(|| any.clone())
                .unwrap_or_else(|| PLATFORM_HISTORY.to_owned());
            let record = match history == HISTORY_OFF {
                true => "none".to_owned(),
                false => own
                    .and_then(|r| {
                        r.attributes
                            .get(&attribute)
                            .and_then(|a| a.record.clone())
                            .or(r.record.clone())
                    })
                    .unwrap_or_else(|| jc_core::kinds::space::RECORD_OBSERVED.to_owned()),
            };
            let model_unit = (input.unit)(&entity_type, &attribute);
            let measured = seen.received_with_unit || model_unit.is_some();
            let unit_code = match (seen.unit_code.is_empty(), model_unit) {
                (true, Some(u)) => vec![format!("model {u}")],
                _ => seen.unit_code.iter().cloned().collect(),
            };
            AuditRow {
                pipeline: input.pipeline.to_owned(),
                verdict: verdict(&history, &record, &seen.observed_at, measured),
                entity_type,
                attribute,
                record,
                observed_at: seen.observed_at.into_iter().collect(),
                unit_code,
                dataset_id: seen.dataset_id.into_iter().collect(),
                history,
                lifetime: lifetime.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VEHICLE: &str = r#"
root = {
  "id": $id,
  "type": "Vehicle",
  "expiresAt": meta("expires"),
  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [1, 2] }, "observedAt": this.tst },
  "speed": if this.spd.type() != "number" { deleted() } else { {
    "type": "Property", "value": this.spd, "unitCode": "MTS", "observedAt": this.tst
  } },
  # "ghost": { "type": "Property" } in a comment
  "mode": { "type": "Property", "value": "bus" },
  "fixed": { "type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z" },
  "stamped": { "type": "Property", "value": 1, "observedAt": now() },
  $attribute: { "type": "Property", "value": this.v, "unitCode": "GQ", "observedAt": this.at },
  "level": { "type": "Property", "value": this.l, "unitCode": "CMT", "datasetId": "urn:ngsi-ld:Dataset:a" },
  "_helper": { "type": "Property", "value": "{ not a brace }" }
}
"#;

    fn set(items: &[ObservedAt]) -> BTreeSet<ObservedAt> {
        items.iter().copied().collect()
    }

    #[test]
    fn the_scan_reads_each_attribute_its_observed_at_unit_and_dataset() {
        let found = scan(VEHICLE, None);
        let names: Vec<&str> = found.keys().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            names,
            [
                "$attribute",
                "fixed",
                "level",
                "location",
                "mode",
                "speed",
                "stamped"
            ]
        );
        assert!(found.keys().all(|(t, _)| t == "Vehicle"));
        let at = |a: &str| found[&("Vehicle".to_owned(), a.to_owned())].clone();
        assert_eq!(at("location").observed_at, set(&[ObservedAt::Mapped]));
        assert_eq!(at("speed").unit_code, ["MTS".to_owned()].into());
        assert_eq!(at("mode").observed_at, set(&[ObservedAt::Received]));
        assert_eq!(at("fixed").observed_at, set(&[ObservedAt::Constant]));
        assert_eq!(at("stamped").observed_at, set(&[ObservedAt::Received]));
        assert_eq!(
            at("level").dataset_id,
            ["urn:ngsi-ld:Dataset:a".to_owned()].into()
        );
        assert!(at("level").received_with_unit && !at("speed").received_with_unit);
    }

    #[test]
    fn a_member_key_is_its_quoted_name_then_a_colon_across_blanks() {
        let found = scan(
            "root = { \"type\": \"Tag\", \"tag\": { \"type\": \"Property\", \"value\": \"observedAt\", \"observedAt\"\n    :  this.t } }",
            None,
        );
        let tag = &found[&("Tag".to_owned(), "tag".to_owned())];
        assert_eq!(tag.observed_at, set(&[ObservedAt::Mapped]));
    }

    #[test]
    fn a_default_type_names_attributes_before_any_type_literal() {
        let found = scan(
            r#"root.name = { "type": "Property", "value": this.n }"#,
            Some("Bridge"),
        );
        assert_eq!(
            found.keys().collect::<Vec<_>>(),
            [&("Bridge".to_owned(), "name".to_owned())]
        );
        assert!(scan(r#"root.name = { "type": "Property" }"#, None).is_empty());
    }

    #[test]
    fn each_verdict_rule() {
        let mapped = set(&[ObservedAt::Mapped]);
        let received = set(&[ObservedAt::Received]);
        assert_eq!(verdict("P30D", "every", &mapped, false), Verdict::Ok);
        assert_eq!(
            verdict("off", "none", &set(&[ObservedAt::Constant]), false),
            Verdict::Ok
        );
        assert_eq!(
            verdict("P30D", "every", &set(&[ObservedAt::Constant]), false),
            Verdict::LosesHistory
        );
        assert_eq!(
            verdict("forever", "every", &mapped, false),
            Verdict::UnboundedGrowth
        );
        assert_eq!(
            verdict("P30D", "every", &received, false),
            Verdict::DuplicatesRows
        );
        assert_eq!(
            verdict(
                "P30D",
                "every",
                &set(&[ObservedAt::Mapped, ObservedAt::Received]),
                false
            ),
            Verdict::DuplicatesRows
        );
        assert_eq!(verdict("P30D", "onChange", &received, false), Verdict::Ok);
        assert_eq!(
            verdict("P30D", "onChange", &received, true),
            Verdict::StampedOnReceive
        );
    }

    #[test]
    fn rows_take_the_rule_of_the_attribute_then_the_type_then_the_space_then_the_platform() {
        let retention: Vec<RetentionRule> = serde_json::from_value(serde_json::json!([
            { "entityType": "*", "history": "P7D" },
            { "entityType": "Vehicle", "history": "PT1H",
              "attributes": { "mode": { "record": "onChange", "heartbeat": "PT15M" } } },
        ]))
        .unwrap();
        let spec = serde_json::json!({ "lifetime": [{ "after": "PT5M" }], "steps": [{ "kind": "bloblang", "bloblang":
            r#"root = { "type": "Vehicle", "mode": { "type": "Property", "value": 1 }, "speed": { "type": "Property", "value": 2, "observedAt": this.t } }
               root = { "type": "Stop", "name": { "type": "Property", "value": 3 } }"# }] });
        let unit = |t: &str, a: &str| (t == "Vehicle" && a == "speed").then(|| "MTS".to_owned());
        let input = Input {
            pipeline: "p",
            spec: &spec,
            bento: None,
            default_type: None,
            retention: &retention,
            unit: &unit,
        };
        let got: Vec<(String, String, String, String, Vec<String>, Verdict)> = rows(&input)
            .into_iter()
            .map(|r| {
                (
                    r.entity_type,
                    r.attribute,
                    r.record,
                    r.history,
                    r.unit_code,
                    r.verdict,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "Stop".into(),
                    "name".into(),
                    "observed".into(),
                    "P7D".into(),
                    vec![],
                    Verdict::DuplicatesRows
                ),
                (
                    "Vehicle".into(),
                    "mode".into(),
                    "onChange".into(),
                    "PT1H".into(),
                    vec![],
                    Verdict::Ok
                ),
                (
                    "Vehicle".into(),
                    "speed".into(),
                    "observed".into(),
                    "PT1H".into(),
                    vec!["model MTS".into()],
                    Verdict::Ok
                ),
            ]
        );
        let none: Vec<RetentionRule> = vec![];
        let input = Input {
            retention: &none,
            ..input
        };
        assert!(rows(&input).iter().all(|r| r.history == PLATFORM_HISTORY
            && r.record == "none"
            && r.verdict == Verdict::Ok
            && r.lifetime == ["PT5M"]));
    }
}
