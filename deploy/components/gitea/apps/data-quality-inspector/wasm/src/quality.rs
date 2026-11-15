//! Type-level and dataset-level quality inspection: computes attribute completeness,
//! entity validity against schemas, rule violation findings, and freshness metrics (T-3336).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::validate::{
    days_from_civil, days_in_month, validate_value, CompiledPropertySchema, Finding,
};

const FRESHNESS_FIELDS: &[&str] = &[
    "dateObserved",
    "dateModified",
    "datePublished",
    "dateIssued",
    "validFrom",
    "startDate",
];

/// Parses an RFC 3339 date-time or YYYY-MM-DD date string into UTC epoch seconds.
pub fn parse_timestamp_seconds(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let parse_2 = |slice: &[u8]| -> Option<u32> {
        if slice.len() == 2 && slice[0].is_ascii_digit() && slice[1].is_ascii_digit() {
            Some(((slice[0] - b'0') as u32) * 10 + ((slice[1] - b'0') as u32))
        } else {
            None
        }
    };
    let parse_4 = |slice: &[u8]| -> Option<i64> {
        if slice.len() == 4 && slice.iter().all(|c| c.is_ascii_digit()) {
            Some(
                ((slice[0] - b'0') as i64) * 1000
                    + ((slice[1] - b'0') as i64) * 100
                    + ((slice[2] - b'0') as i64) * 10
                    + ((slice[3] - b'0') as i64),
            )
        } else {
            None
        }
    };

    let year = parse_4(&b[0..4])?;
    let month = parse_2(&b[5..7])?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let max_day = days_in_month(year, month);
    let day = parse_2(&b[8..10])?;
    if day < 1 || day > max_day {
        return None;
    }
    let days = days_from_civil(year, month, day);

    if b.len() == 10 {
        return Some((days * 86_400) as f64);
    }

    if b[10] != b'T' && b[10] != b't' && b[10] != b' ' {
        return None;
    }
    if b.len() < 19 || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let hour = parse_2(&b[11..13])?;
    if hour > 23 {
        return None;
    }
    let minute = parse_2(&b[14..16])?;
    if minute > 59 {
        return None;
    }
    let second = parse_2(&b[17..19])?;
    if second > 60 {
        return None;
    }

    let mut idx = 19;
    let mut frac_seconds = 0.0;
    if idx < b.len() && b[idx] == b'.' {
        idx += 1;
        let frac_start = idx;
        let mut div = 10.0;
        while idx < b.len() && b[idx].is_ascii_digit() {
            frac_seconds += ((b[idx] - b'0') as f64) / div;
            div *= 10.0;
            idx += 1;
        }
        if idx == frac_start {
            return None;
        }
    }

    let mut tz_offset_seconds = 0i64;
    if idx < b.len() {
        if b[idx] == b'Z' || b[idx] == b'z' {
            tz_offset_seconds = 0;
        } else if b[idx] == b'+' || b[idx] == b'-' {
            let sign = if b[idx] == b'+' { 1 } else { -1 };
            let tz_part = &b[idx + 1..];
            if tz_part.len() >= 2 {
                let tz_h = parse_2(&tz_part[0..2])?;
                let tz_m = if tz_part.len() >= 5 && tz_part[2] == b':' {
                    parse_2(&tz_part[3..5])?
                } else if tz_part.len() >= 4 {
                    parse_2(&tz_part[2..4])?
                } else {
                    0
                };
                tz_offset_seconds = sign * ((tz_h as i64) * 3600 + (tz_m as i64) * 60);
            }
        }
    }

    let total_secs = days * 86_400 + (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64)
        - tz_offset_seconds;
    Some((total_secs as f64) + frac_seconds)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TypeInput {
    #[serde(rename = "type")]
    pub entity_type: String,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub rows: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Input {
    pub now: f64,
    #[serde(default)]
    pub types: Vec<TypeInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttributeOutput {
    pub name: String,
    pub completeness: f64,
    pub required: bool,
    pub not_checked: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FreshnessOutput {
    pub field: String,
    pub median_seconds: f64,
    pub max_seconds: f64,
    pub older_than_day: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TypeOutput {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub entities: usize,
    pub completeness: f64,
    pub valid: Option<f64>,
    pub attributes: Vec<AttributeOutput>,
    pub findings: Vec<Finding>,
    pub findings_total: usize,
    pub freshness: Option<FreshnessOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Output {
    pub types: Vec<TypeOutput>,
}

/// Findings the browser's answer lists per type; `findings_total` counts them all.
pub const FINDINGS_SHOWN: usize = 200;

/// Runs data quality inspection on one entity type, listing the first [`FINDINGS_SHOWN`] findings.
pub fn inspect_type(now: f64, type_input: &TypeInput) -> TypeOutput {
    inspect_type_with(now, type_input, FINDINGS_SHOWN)
}

/// [`inspect_type`] listing the first `keep` findings: the server keeps them all (T-3354).
pub fn inspect_type_with(now: f64, type_input: &TypeInput, keep: usize) -> TypeOutput {
    let total_entities = type_input.rows.len();

    let has_schema = type_input
        .schema
        .as_ref()
        .and_then(|s| s.get("properties"))
        .is_some();

    if !has_schema {
        let mut attr_set = BTreeSet::new();
        for row in &type_input.rows {
            if let Some(obj) = row.as_object() {
                for k in obj.keys() {
                    if k != "id" && k != "type" && k != "@context" {
                        attr_set.insert(k.clone());
                    }
                }
            }
        }

        let attr_names: Vec<String> = attr_set.into_iter().collect();
        let mut sum_comp = 0.0;
        let mut attributes = Vec::with_capacity(attr_names.len());

        for attr_name in &attr_names {
            let non_null = type_input
                .rows
                .iter()
                .filter(|r| r.get(attr_name).is_some_and(|v| !v.is_null()))
                .count();
            let completeness = if total_entities > 0 {
                non_null as f64 / total_entities as f64
            } else {
                1.0
            };
            sum_comp += completeness;
            attributes.push(AttributeOutput {
                name: attr_name.clone(),
                completeness,
                required: false,
                not_checked: 0,
            });
        }

        let completeness = if !attr_names.is_empty() {
            sum_comp / attr_names.len() as f64
        } else {
            1.0
        };

        return TypeOutput {
            entity_type: type_input.entity_type.clone(),
            entities: total_entities,
            completeness,
            valid: None,
            attributes,
            findings: Vec::new(),
            findings_total: 0,
            freshness: None,
        };
    }

    let schema_val = type_input.schema.as_ref().unwrap();
    let props_obj = schema_val
        .get("properties")
        .and_then(Value::as_object)
        .unwrap();

    let required_set: BTreeSet<String> = schema_val
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let additional_props = schema_val
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut compiled_props = HashMap::new();
    for (k, v) in props_obj {
        compiled_props.insert(k.clone(), CompiledPropertySchema::compile(v));
    }

    let mut attr_names: Vec<String> = props_obj
        .keys()
        .filter(|&k| k != "id" && k != "type" && k != "@context")
        .cloned()
        .collect();
    attr_names.sort();

    let mut attr_non_null_counts: HashMap<String, usize> = HashMap::new();
    let mut attr_not_checked_counts: HashMap<String, usize> = HashMap::new();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut invalid_entities: BTreeSet<String> = BTreeSet::new();

    for row in &type_input.rows {
        // An entity is its whole id: two with the same local id under different prefixes are two
        // entities, and the page opens a finding's entity by it (SDK-40).
        let full_id = row.get("id").and_then(Value::as_str).unwrap_or("");
        let Some(row_obj) = row.as_object() else {
            continue;
        };

        for req in &required_set {
            if req == "id" || req == "type" || req == "@context" {
                continue;
            }
            if row_obj.get(req).is_none_or(Value::is_null) {
                all_findings.push(Finding {
                    entity: full_id.to_string(),
                    attribute: req.clone(),
                    rule: "required".to_string(),
                    detail: "required".to_string(),
                });
                invalid_entities.insert(full_id.to_string());
            }
        }

        if !additional_props {
            for k in row_obj.keys() {
                if k == "id" || k == "type" || k == "@context" {
                    continue;
                }
                if !props_obj.contains_key(k) {
                    all_findings.push(Finding {
                        entity: full_id.to_string(),
                        attribute: k.clone(),
                        rule: "unknown".to_string(),
                        detail: "additionalProperties: false".to_string(),
                    });
                    invalid_entities.insert(full_id.to_string());
                }
            }
        }

        for attr_name in &attr_names {
            if let Some(val) = row_obj.get(attr_name) {
                if !val.is_null() {
                    *attr_non_null_counts.entry(attr_name.clone()).or_default() += 1;
                }
                if let Some(comp_schema) = compiled_props.get(attr_name) {
                    let (findings, not_checked) =
                        validate_value(full_id, attr_name, val, comp_schema);
                    if !findings.is_empty() {
                        invalid_entities.insert(full_id.to_string());
                        all_findings.extend(findings);
                    }
                    *attr_not_checked_counts
                        .entry(attr_name.clone())
                        .or_default() += not_checked;
                }
            }
        }
    }

    let mut sum_comp = 0.0;
    let mut attributes = Vec::with_capacity(attr_names.len());
    for name in &attr_names {
        let non_null = attr_non_null_counts.get(name).copied().unwrap_or(0);
        let not_checked = attr_not_checked_counts.get(name).copied().unwrap_or(0);
        let completeness = if total_entities > 0 {
            non_null as f64 / total_entities as f64
        } else {
            1.0
        };
        sum_comp += completeness;
        attributes.push(AttributeOutput {
            name: name.clone(),
            completeness,
            required: required_set.contains(name),
            not_checked,
        });
    }

    let completeness = if !attr_names.is_empty() {
        sum_comp / attr_names.len() as f64
    } else {
        1.0
    };

    let valid_count = total_entities.saturating_sub(invalid_entities.len());
    let valid = if total_entities > 0 {
        Some(valid_count as f64 / total_entities as f64)
    } else {
        Some(1.0)
    };

    let freshness_field = FRESHNESS_FIELDS
        .iter()
        .find(|&&f| props_obj.contains_key(f))
        .copied();

    let freshness = freshness_field.and_then(|field| {
        let mut ages = Vec::new();
        for row in &type_input.rows {
            if let Some(val_str) = row.get(field).and_then(Value::as_str) {
                if let Some(ts_sec) = parse_timestamp_seconds(val_str) {
                    let age = (now - ts_sec).max(0.0);
                    ages.push(age);
                }
            }
        }
        if ages.is_empty() {
            return None;
        }
        ages.sort_by(|a, b| a.total_cmp(b));
        let n = ages.len();
        let max_seconds = ages[n - 1];
        let median_seconds = if n % 2 == 1 {
            ages[n / 2]
        } else {
            (ages[n / 2 - 1] + ages[n / 2]) / 2.0
        };
        let older_count = ages.iter().filter(|&&a| a > 86_400.0).count();
        let older_than_day = older_count as f64 / n as f64;
        Some(FreshnessOutput {
            field: field.to_string(),
            median_seconds,
            max_seconds,
            older_than_day,
        })
    });

    let findings_total = all_findings.len();
    let findings: Vec<Finding> = all_findings.into_iter().take(keep).collect();

    TypeOutput {
        entity_type: type_input.entity_type.clone(),
        entities: total_entities,
        completeness,
        valid,
        attributes,
        findings,
        findings_total,
        freshness,
    }
}

/// Runs the complete data quality inspection across all entity types,
/// sorted with the worst validity percentage first.
pub fn run(input: &Input) -> Output {
    let mut types: Vec<TypeOutput> = input
        .types
        .iter()
        .map(|t| inspect_type(input.now, t))
        .collect();

    types.sort_by(|a, b| match (a.valid, b.valid) {
        (Some(va), Some(vb)) => va
            .total_cmp(&vb)
            .then_with(|| a.entity_type.cmp(&b.entity_type)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.entity_type.cmp(&b.entity_type),
    });

    Output { types }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_timestamp_reads_its_fraction_and_its_offset_and_a_broken_one_is_none() {
        let z = parse_timestamp_seconds("2026-10-08T12:00:00Z").expect("a time");
        assert_eq!(
            parse_timestamp_seconds("2026-10-08T12:00:00.5Z"),
            Some(z + 0.5)
        );
        assert_eq!(
            parse_timestamp_seconds("2026-10-08T15:00:00+03:00"),
            Some(z)
        );
        assert_eq!(parse_timestamp_seconds("2026-10-08T15:00:00+0300"), Some(z));
        assert_eq!(parse_timestamp_seconds("2026-10-08T15:00:00+03"), Some(z));
        assert_eq!(
            parse_timestamp_seconds("2026-10-08T11:30:00-00:30"),
            Some(z)
        );
        // A date alone is its midnight.
        assert_eq!(
            parse_timestamp_seconds("2026-10-08"),
            Some(z - 12.0 * 3600.0)
        );
        for bad in [
            "2026-10",
            "2026/10/08",
            "2026-1x-08T12:00:00Z",
            "2026-10-08T12:00:00.Z",
            "20x6-10-08T12:00:00Z",
            "2026-10-0xT12:00:00Z",
            "2026-10-08T1x:00:00Z",
        ] {
            assert_eq!(parse_timestamp_seconds(bad), None, "{bad}");
        }
    }

    #[test]
    fn two_entities_sharing_a_local_id_are_two_entities_named_by_their_whole_id() {
        let input = TypeInput {
            entity_type: "PointOfInterest".to_string(),
            schema: Some(json!({
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            })),
            rows: vec![
                json!({ "id": "urn:ngsi-ld:POI:hel.fi:a:1" }),
                json!({ "id": "urn:ngsi-ld:POI:hel.fi:b:1" }),
                json!({ "id": "urn:ngsi-ld:POI:hel.fi:b:2", "name": "Esplanadi" }),
            ],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        let named: Vec<&str> = out.findings.iter().map(|f| f.entity.as_str()).collect();
        assert_eq!(
            named,
            ["urn:ngsi-ld:POI:hel.fi:a:1", "urn:ngsi-ld:POI:hel.fi:b:1"]
        );
        // Two of three invalid, not one: the local id `1` is two entities.
        assert_eq!(out.valid, Some(1.0 / 3.0));
    }

    #[test]
    fn required_missing_produces_finding() {
        let input = TypeInput {
            entity_type: "PointOfInterest".to_string(),
            schema: Some(json!({
                "properties": {
                    "name": { "type": "string" },
                    "category": { "type": "string" }
                },
                "required": ["name"]
            })),
            rows: vec![
                json!({ "id": "urn:ngsi-ld:POI:1", "category": "park" }),
                json!({ "id": "urn:ngsi-ld:POI:2", "name": "Esplanadi", "category": "park" }),
            ],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.entities, 2);
        assert_eq!(out.findings_total, 1);
        assert_eq!(out.findings[0].attribute, "name");
        assert_eq!(out.findings[0].rule, "required");
        assert_eq!(out.valid, Some(0.5));
    }

    #[test]
    fn unknown_attribute_with_additional_properties_false() {
        let input = TypeInput {
            entity_type: "Alert".to_string(),
            schema: Some(json!({
                "properties": {
                    "validFrom": { "type": "string" }
                },
                "additionalProperties": false
            })),
            rows: vec![
                json!({ "id": "urn:ngsi-ld:Alert:1", "validFrom": "2030-10-08T00:00:00Z", "extraField": 123 }),
            ],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.findings_total, 1);
        assert_eq!(out.findings[0].attribute, "extraField");
        assert_eq!(out.findings[0].rule, "unknown");
    }

    #[test]
    fn completeness_with_nulls() {
        let input = TypeInput {
            entity_type: "Event".to_string(),
            schema: Some(json!({
                "properties": {
                    "name": { "type": "string" }
                }
            })),
            rows: vec![
                json!({ "id": "e1", "name": "E1" }),
                json!({ "id": "e2", "name": "E2" }),
                json!({ "id": "e3", "name": "E3" }),
                json!({ "id": "e4", "name": null }),
            ],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.attributes[0].completeness, 0.75);
        assert_eq!(out.completeness, 0.75);
    }

    #[test]
    fn freshness_picks_field_and_computes_median_even_count() {
        // Schema has both dateModified and startDate; dateModified has higher priority
        let input = TypeInput {
            entity_type: "WeatherObserved".to_string(),
            schema: Some(json!({
                "properties": {
                    "dateModified": { "type": "string" },
                    "startDate": { "type": "string" }
                }
            })),
            rows: vec![
                json!({ "id": "w1", "dateModified": "2026-10-08T12:00:00Z" }), // age: 100s
                json!({ "id": "w2", "dateModified": "2026-10-08T11:58:20Z" }), // age: 200s
                json!({ "id": "w3", "dateModified": "2026-10-08T11:56:40Z" }), // age: 300s
                json!({ "id": "w4", "dateModified": "2026-10-07T00:00:00Z" }), // age: 129700 s (36 h 1 min 40 s, > 86400)
            ],
        };
        let now = 1791460900.0; // 2026-10-08T12:01:40Z
        let out = inspect_type(now, &input);
        let fresh = out.freshness.expect("freshness computed");
        assert_eq!(fresh.field, "dateModified");
        // Ages: 100, 200, 300, 129700 -> median of 200 and 300 is 250.0
        assert_eq!(fresh.median_seconds, 250.0);
        assert_eq!(fresh.max_seconds, 129700.0);
        assert_eq!(fresh.older_than_day, 0.25);
    }

    #[test]
    fn no_schema_gives_valid_null_and_row_attributes() {
        let input = TypeInput {
            entity_type: "CustomSensor".to_string(),
            schema: None,
            rows: vec![
                json!({ "id": "s1", "temp": 12.5, "pressure": 1013 }),
                json!({ "id": "s2", "temp": 14.0 }),
            ],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.valid, None);
        assert_eq!(out.findings_total, 0);
        assert_eq!(out.attributes.len(), 2);
        assert_eq!(out.freshness, None);
    }

    #[test]
    fn empty_rows_handled_safely() {
        let input = TypeInput {
            entity_type: "Empty".to_string(),
            schema: Some(json!({
                "properties": { "name": { "type": "string" } }
            })),
            rows: vec![],
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.entities, 0);
        assert_eq!(out.completeness, 1.0);
        assert_eq!(out.valid, Some(1.0));
        assert!(out.freshness.is_none());
    }

    #[test]
    fn findings_cap_at_200_with_total_kept() {
        let mut rows = Vec::new();
        for i in 0..250 {
            rows.push(json!({ "id": format!("e{i}"), "val": "not-an-integer" }));
        }
        let input = TypeInput {
            entity_type: "ManyErrors".to_string(),
            schema: Some(json!({
                "properties": { "val": { "type": "integer" } }
            })),
            rows,
        };
        let out = inspect_type(1_700_000_000.0, &input);
        assert_eq!(out.findings.len(), 200);
        assert_eq!(out.findings_total, 250);
        // The server's full report lists every one (T-3354).
        let all = inspect_type_with(1_700_000_000.0, &input, usize::MAX);
        assert_eq!(all.findings.len(), 250);
        assert_eq!(all.findings_total, 250);
    }

    #[test]
    fn run_sorts_worst_valid_first() {
        let inp = Input {
            now: 1_700_000_000.0,
            types: vec![
                TypeInput {
                    entity_type: "Perfect".to_string(),
                    schema: Some(json!({ "properties": { "n": { "type": "integer" } } })),
                    rows: vec![json!({ "id": "p1", "n": 10 })],
                },
                TypeInput {
                    entity_type: "Bad".to_string(),
                    schema: Some(json!({ "properties": { "n": { "type": "integer" } } })),
                    rows: vec![json!({ "id": "b1", "n": "wrong" })],
                },
                TypeInput {
                    entity_type: "NoSchema".to_string(),
                    schema: None,
                    rows: vec![json!({ "id": "ns1", "x": 1 })],
                },
            ],
        };
        let out = run(&inp);
        assert_eq!(out.types[0].entity_type, "Bad");
        assert_eq!(out.types[1].entity_type, "Perfect");
        assert_eq!(out.types[2].entity_type, "NoSchema");
    }
}
