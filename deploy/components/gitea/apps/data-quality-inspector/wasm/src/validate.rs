//! Property-level schema validation: checks one value against a property schema using the exact
//! keywords the platform's JSON Schema models define (T-3336).

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Checks whether a year is a leap year in the Gregorian calendar.
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Returns the number of days in a month for a given year.
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 0,
    }
}

fn parse_2_digits(slice: &[u8]) -> Option<u32> {
    if slice.len() == 2 && slice[0].is_ascii_digit() && slice[1].is_ascii_digit() {
        Some(((slice[0] - b'0') as u32) * 10 + ((slice[1] - b'0') as u32))
    } else {
        None
    }
}

fn parse_4_digits(slice: &[u8]) -> Option<i64> {
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
}

/// Validates an RFC 3339 date-time without external date crates.
/// Seconds are mandatory; an offset or 'Z' is mandatory.
pub fn is_rfc3339_date_time(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 {
        return false;
    }
    if b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }

    let year = match parse_4_digits(&b[0..4]) {
        Some(y) => y,
        None => return false,
    };
    let month = match parse_2_digits(&b[5..7]) {
        Some(m) if (1..=12).contains(&m) => m,
        _ => return false,
    };
    let max_day = days_in_month(year, month);
    match parse_2_digits(&b[8..10]) {
        Some(d) if d >= 1 && d <= max_day => d,
        _ => return false,
    };
    match parse_2_digits(&b[11..13]) {
        Some(h) if h <= 23 => h,
        _ => return false,
    };
    match parse_2_digits(&b[14..16]) {
        Some(m) if m <= 59 => m,
        _ => return false,
    };
    match parse_2_digits(&b[17..19]) {
        Some(s) if s <= 60 => s,
        _ => return false,
    };

    let mut idx = 19;
    if idx < b.len() && b[idx] == b'.' {
        idx += 1;
        let frac_start = idx;
        while idx < b.len() && b[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == frac_start {
            return false;
        }
    }

    if idx >= b.len() {
        return false;
    }

    if b[idx] == b'Z' || b[idx] == b'z' {
        return idx + 1 == b.len();
    }

    if b[idx] == b'+' || b[idx] == b'-' {
        let tz_part = &b[idx + 1..];
        if tz_part.len() != 5 || tz_part[2] != b':' {
            return false;
        }
        match parse_2_digits(&tz_part[0..2]) {
            Some(h) if h <= 23 => h,
            _ => return false,
        };
        match parse_2_digits(&tz_part[3..5]) {
            Some(m) if m <= 59 => m,
            _ => return false,
        };
        return true;
    }

    false
}

/// Validates a real calendar date in YYYY-MM-DD format (exactly 10 characters).
pub fn is_rfc3339_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let year = match parse_4_digits(&b[0..4]) {
        Some(y) => y,
        None => return false,
    };
    let month = match parse_2_digits(&b[5..7]) {
        Some(m) if (1..=12).contains(&m) => m,
        _ => return false,
    };
    let max_day = days_in_month(year, month);
    matches!(parse_2_digits(&b[8..10]), Some(d) if (1..=max_day).contains(&d))
}

/// Validates a URI: starts with an ASCII scheme, ':', then no whitespace.
pub fn is_uri(s: &str) -> bool {
    if s.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    if scheme.is_empty() {
        return false;
    }
    let sb = scheme.as_bytes();
    if !sb[0].is_ascii_alphabetic() {
        return false;
    }
    sb.iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
}

/// One validation finding for an entity attribute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub entity: String,
    pub attribute: String,
    pub rule: String,
    pub detail: String,
}

/// Compiled schema for a single property, prepared once per model definition.
#[derive(Debug, Clone)]
pub struct CompiledPropertySchema {
    pub types: Option<Vec<String>>,
    pub enum_values: Option<Vec<Value>>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub pattern: Option<Result<Regex, String>>,
    pub format: Option<String>,
    pub has_ref: bool,
    pub is_language_property: bool,
    pub object_required: Option<Vec<String>>,
    pub object_properties: Option<HashMap<String, CompiledPropertySchema>>,
}

impl CompiledPropertySchema {
    pub fn compile(schema: &Value) -> Self {
        let Some(obj) = schema.as_object() else {
            return Self {
                types: None,
                enum_values: None,
                minimum: None,
                maximum: None,
                pattern: None,
                format: None,
                has_ref: false,
                is_language_property: false,
                object_required: None,
                object_properties: None,
            };
        };

        let types = match obj.get("type") {
            Some(Value::String(s)) => Some(vec![s.clone()]),
            Some(Value::Array(arr)) => Some(
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
            ),
            _ => None,
        };

        let enum_values = obj.get("enum").and_then(Value::as_array).cloned();

        let minimum = obj.get("minimum").and_then(Value::as_f64);
        let maximum = obj.get("maximum").and_then(Value::as_f64);

        let pattern = obj
            .get("pattern")
            .and_then(Value::as_str)
            .map(|p| Regex::new(p).map_err(|_| p.to_string()));

        let format = obj.get("format").and_then(Value::as_str).map(String::from);
        let has_ref = obj.contains_key("$ref");
        let is_language_property =
            obj.get("x-ngsi-ld-kind").and_then(Value::as_str) == Some("LanguageProperty");

        let object_required = obj.get("required").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let object_properties = obj
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| {
                props
                    .iter()
                    .map(|(k, v)| (k.clone(), CompiledPropertySchema::compile(v)))
                    .collect()
            });

        Self {
            types,
            enum_values,
            minimum,
            maximum,
            pattern,
            format,
            has_ref,
            is_language_property,
            object_required,
            object_properties,
        }
    }
}

/// Validates a single value against a compiled property schema.
/// Returns the findings encountered and the count of rules skipped ("not checked").
pub fn validate_value(
    entity: &str,
    attribute: &str,
    value: &Value,
    schema: &CompiledPropertySchema,
) -> (Vec<Finding>, usize) {
    let mut findings = Vec::new();
    let mut not_checked = 0;

    if schema.has_ref {
        not_checked += 1;
    }

    if schema.is_language_property {
        if value.is_string() {
            return (findings, not_checked);
        } else if let Some(obj) = value.as_object() {
            if obj.values().all(Value::is_string) {
                return (findings, not_checked);
            }
            findings.push(Finding {
                entity: entity.to_string(),
                attribute: attribute.to_string(),
                rule: "type".to_string(),
                detail: "string".to_string(),
            });
            return (findings, not_checked);
        } else if value.is_null() {
            if schema
                .types
                .as_ref()
                .is_some_and(|ts| ts.iter().any(|t| t == "null"))
            {
                return (findings, not_checked);
            }
            findings.push(Finding {
                entity: entity.to_string(),
                attribute: attribute.to_string(),
                rule: "type".to_string(),
                detail: "string".to_string(),
            });
            return (findings, not_checked);
        } else {
            findings.push(Finding {
                entity: entity.to_string(),
                attribute: attribute.to_string(),
                rule: "type".to_string(),
                detail: "string".to_string(),
            });
            return (findings, not_checked);
        }
    }

    if let Some(ref allowed_types) = schema.types {
        let matches = allowed_types.iter().any(|t| match t.as_str() {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => {
                value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
            }
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => false,
        });

        if !matches {
            findings.push(Finding {
                entity: entity.to_string(),
                attribute: attribute.to_string(),
                rule: "type".to_string(),
                detail: allowed_types.join(", "),
            });
            return (findings, not_checked);
        }
    }

    if value.is_null() {
        return (findings, not_checked);
    }

    if let Some(ref enum_vals) = schema.enum_values {
        if !enum_vals.contains(value) {
            let detail = enum_vals
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            findings.push(Finding {
                entity: entity.to_string(),
                attribute: attribute.to_string(),
                rule: "enum".to_string(),
                detail,
            });
        }
    }

    if let Some(num) = value.as_f64() {
        if let Some(min) = schema.minimum {
            if num < min {
                let detail = if min.fract() == 0.0 {
                    format!("{}", min as i64)
                } else {
                    min.to_string()
                };
                findings.push(Finding {
                    entity: entity.to_string(),
                    attribute: attribute.to_string(),
                    rule: "minimum".to_string(),
                    detail,
                });
            }
        }
        if let Some(max) = schema.maximum {
            if num > max {
                let detail = if max.fract() == 0.0 {
                    format!("{}", max as i64)
                } else {
                    max.to_string()
                };
                findings.push(Finding {
                    entity: entity.to_string(),
                    attribute: attribute.to_string(),
                    rule: "maximum".to_string(),
                    detail,
                });
            }
        }
    }

    if let Some(s) = value.as_str() {
        if let Some(ref pattern_res) = schema.pattern {
            match pattern_res {
                Ok(re) => {
                    if !re.is_match(s) {
                        findings.push(Finding {
                            entity: entity.to_string(),
                            attribute: attribute.to_string(),
                            rule: "pattern".to_string(),
                            detail: re.as_str().to_string(),
                        });
                    }
                }
                Err(_) => {
                    not_checked += 1;
                }
            }
        }

        if let Some(ref fmt) = schema.format {
            match fmt.as_str() {
                "date-time" => {
                    if !is_rfc3339_date_time(s) {
                        findings.push(Finding {
                            entity: entity.to_string(),
                            attribute: attribute.to_string(),
                            rule: "format".to_string(),
                            detail: "date-time".to_string(),
                        });
                    }
                }
                "date" => {
                    if !is_rfc3339_date(s) {
                        findings.push(Finding {
                            entity: entity.to_string(),
                            attribute: attribute.to_string(),
                            rule: "format".to_string(),
                            detail: "date".to_string(),
                        });
                    }
                }
                "uri" if !is_uri(s) => {
                    findings.push(Finding {
                        entity: entity.to_string(),
                        attribute: attribute.to_string(),
                        rule: "format".to_string(),
                        detail: "uri".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    if let Some(obj) = value.as_object() {
        if let Some(ref req_list) = schema.object_required {
            for req in req_list {
                if !obj.contains_key(req) || obj[req].is_null() {
                    findings.push(Finding {
                        entity: entity.to_string(),
                        attribute: attribute.to_string(),
                        rule: "required".to_string(),
                        detail: req.clone(),
                    });
                }
            }
        }
        if let Some(ref inner_props) = schema.object_properties {
            for (prop_name, prop_val) in obj {
                if let Some(prop_schema) = inner_props.get(prop_name) {
                    let (inner_findings, inner_not_checked) =
                        validate_value(entity, attribute, prop_val, prop_schema);
                    findings.extend(inner_findings);
                    not_checked += inner_not_checked;
                }
            }
        }
    }

    (findings, not_checked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn type_validation_integer() {
        let schema = CompiledPropertySchema::compile(&json!({ "type": "integer" }));
        let (f_pass, _) = validate_value("e1", "count", &json!(10), &schema);
        assert!(f_pass.is_empty());

        let (f_pass_f, _) = validate_value("e1", "count", &json!(10.0), &schema);
        assert!(f_pass_f.is_empty());

        let (f_fail, _) = validate_value("e1", "count", &json!(1.5), &schema);
        assert_eq!(f_fail.len(), 1);
        assert_eq!(f_fail[0].rule, "type");
        assert_eq!(f_fail[0].detail, "integer");
    }

    #[test]
    fn type_validation_nullable_string() {
        let schema = CompiledPropertySchema::compile(&json!({ "type": ["string", "null"] }));
        let (f1, _) = validate_value("e1", "desc", &Value::Null, &schema);
        assert!(f1.is_empty());

        let (f2, _) = validate_value("e1", "desc", &json!("hello"), &schema);
        assert!(f2.is_empty());

        let (f3, _) = validate_value("e1", "desc", &json!(123), &schema);
        assert_eq!(f3.len(), 1);
        assert_eq!(f3[0].rule, "type");
    }

    #[test]
    fn format_date_time() {
        let schema =
            CompiledPropertySchema::compile(&json!({ "type": "string", "format": "date-time" }));
        let (f_z, _) = validate_value("e1", "t", &json!("2026-10-08T12:00:00Z"), &schema);
        assert!(f_z.is_empty());

        let (f_off, _) = validate_value("e1", "t", &json!("2026-10-08T15:00:00+03:00"), &schema);
        assert!(f_off.is_empty());

        let (f_neg, _) = validate_value("e1", "t", &json!("2026-10-08T07:00:00-05:00"), &schema);
        assert!(f_neg.is_empty());

        let (f_wrong_day, _) = validate_value("e1", "t", &json!("2026-02-30T10:00:00Z"), &schema);
        assert_eq!(f_wrong_day.len(), 1);
        assert_eq!(f_wrong_day[0].rule, "format");

        let (f_no_sec, _) = validate_value("e1", "t", &json!("2026-10-08T12:00Z"), &schema);
        assert_eq!(f_no_sec.len(), 1);
        assert_eq!(f_no_sec[0].rule, "format");
    }

    #[test]
    fn format_date() {
        let schema =
            CompiledPropertySchema::compile(&json!({ "type": "string", "format": "date" }));
        let (f_pass, _) = validate_value("e1", "d", &json!("2026-10-08"), &schema);
        assert!(f_pass.is_empty());

        let (f_wrong_day, _) = validate_value("e1", "d", &json!("2026-02-30"), &schema);
        assert_eq!(f_wrong_day.len(), 1);

        let (f_wrong_month, _) = validate_value("e1", "d", &json!("2026-13-01"), &schema);
        assert_eq!(f_wrong_month.len(), 1);
    }

    #[test]
    fn format_uri() {
        let schema = CompiledPropertySchema::compile(&json!({ "type": "string", "format": "uri" }));
        let (f1, _) = validate_value("e1", "u", &json!("urn:ngsi-ld:Alert:GUID1"), &schema);
        assert!(f1.is_empty());

        let (f2, _) = validate_value("e1", "u", &json!("https://hel.fi"), &schema);
        assert!(f2.is_empty());

        let (f3, _) = validate_value("e1", "u", &json!("http://foo bar"), &schema);
        assert_eq!(f3.len(), 1);

        let (f4, _) = validate_value("e1", "u", &json!("no-scheme"), &schema);
        assert_eq!(f4.len(), 1);
    }

    #[test]
    fn pattern_validation() {
        let s_digits = CompiledPropertySchema::compile(&json!({ "pattern": "^[0-9]{1,10}$" }));
        let (f_dig, _) = validate_value("e1", "p", &json!("Töölö"), &s_digits);
        assert_eq!(f_dig.len(), 1);
        assert_eq!(f_dig[0].rule, "pattern");
        assert_eq!(f_dig[0].detail, "^[0-9]{1,10}$");

        let s_tag = CompiledPropertySchema::compile(&json!({ "pattern": "^[^<>]{0,500}$" }));
        let (f_tag_pass, _) = validate_value("e1", "p", &json!("Töölö"), &s_tag);
        assert!(f_tag_pass.is_empty());

        let (f_tag_fail, _) = validate_value("e1", "p", &json!("a<b"), &s_tag);
        assert_eq!(f_tag_fail.len(), 1);
    }

    #[test]
    fn invalid_pattern_not_checked() {
        let schema = CompiledPropertySchema::compile(&json!({ "pattern": "[0-9" }));
        let (findings, not_checked) = validate_value("e1", "p", &json!("test"), &schema);
        assert!(findings.is_empty());
        assert_eq!(not_checked, 1);
    }

    #[test]
    fn ref_not_checked() {
        let schema =
            CompiledPropertySchema::compile(&json!({ "$ref": "#/definitions/DivisionLevel" }));
        let (findings, not_checked) = validate_value("e1", "div", &json!("district"), &schema);
        assert!(findings.is_empty());
        assert_eq!(not_checked, 1);
    }

    #[test]
    fn language_property() {
        let schema = CompiledPropertySchema::compile(&json!({
            "type": ["object", "null"],
            "x-ngsi-ld-kind": "LanguageProperty"
        }));

        let (f_str, _) = validate_value("e1", "title", &json!("Mannerheimintie"), &schema);
        assert!(f_str.is_empty());

        let (f_map, _) = validate_value(
            "e1",
            "title",
            &json!({ "fi": "Mannerheimintie", "sv": "Mannerheimvägen" }),
            &schema,
        );
        assert!(f_map.is_empty());

        let (f_bad_map, _) = validate_value("e1", "title", &json!({ "fi": 1 }), &schema);
        assert_eq!(f_bad_map.len(), 1);
        assert_eq!(f_bad_map[0].rule, "type");

        let (f_num, _) = validate_value("e1", "title", &json!(42), &schema);
        assert_eq!(f_num.len(), 1);
        assert_eq!(f_num[0].rule, "type");
    }

    #[test]
    fn geo_property() {
        let schema = CompiledPropertySchema::compile(&json!({
            "type": "object",
            "properties": {
                "type": { "type": "string" },
                "coordinates": {}
            },
            "required": ["type", "coordinates"]
        }));

        let (f_pass, _) = validate_value(
            "e1",
            "location",
            &json!({ "type": "Point", "coordinates": [24.9, 60.2] }),
            &schema,
        );
        assert!(f_pass.is_empty());

        let (f_fail, _) = validate_value("e1", "location", &json!({ "type": "Point" }), &schema);
        assert_eq!(f_fail.len(), 1);
        assert_eq!(f_fail[0].rule, "required");
        assert_eq!(f_fail[0].detail, "coordinates");
    }

    #[test]
    fn enum_validation() {
        let schema = CompiledPropertySchema::compile(&json!({
            "enum": ["working", "outOfService"]
        }));
        let (f_pass, _) = validate_value("e1", "status", &json!("working"), &schema);
        assert!(f_pass.is_empty());

        let (f_fail, _) = validate_value("e1", "status", &json!("broken"), &schema);
        assert_eq!(f_fail.len(), 1);
        assert_eq!(f_fail[0].rule, "enum");
        assert_eq!(f_fail[0].detail, "working, outOfService");
    }

    #[test]
    fn min_max_validation() {
        let schema = CompiledPropertySchema::compile(&json!({
            "minimum": 0,
            "maximum": 100
        }));
        let (f_pass, _) = validate_value("e1", "pct", &json!(50), &schema);
        assert!(f_pass.is_empty());

        let (f_low, _) = validate_value("e1", "pct", &json!(-1), &schema);
        assert_eq!(f_low.len(), 1);
        assert_eq!(f_low[0].rule, "minimum");
        assert_eq!(f_low[0].detail, "0");

        let (f_high, _) = validate_value("e1", "pct", &json!(101), &schema);
        assert_eq!(f_high.len(), 1);
        assert_eq!(f_high[0].rule, "maximum");
        assert_eq!(f_high[0].detail, "100");
    }

    #[test]
    fn every_month_has_its_days() {
        let lengths: Vec<u32> = (1..=12).map(|m| days_in_month(2026, m)).collect();
        assert_eq!(lengths, [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 13), 0);
        assert!(is_rfc3339_date("2026-04-30"));
        assert!(!is_rfc3339_date("2026-04-31"));
    }

    #[test]
    fn a_date_time_with_fractions_and_an_offset_is_one_and_a_broken_one_is_not() {
        for good in [
            "2026-10-08T12:30:00.123Z",
            "2026-10-08t12:30:00z",
            "2026-10-08T12:30:00+03:00",
            "2026-10-08T12:30:00-00:30",
        ] {
            assert!(is_rfc3339_date_time(good), "{good}");
        }
        for bad in [
            "2026-10-08T12:30:00.Z",
            "2026-10-08T12:30:00",
            "2026-10-08T12:30:00+0300",
            "2026-10-08T12:30:00+24:00",
            "2026-10-08T12:30:00+03:60",
            "2026-10-08T12:30:00Q",
            "2026-13-08T12:30:00Z",
            "2026-02-30T12:30:00Z",
            "2026-10-08T24:30:00Z",
            "2026-10-08T12:60:00Z",
            "2026-10-08T12:30:61Z",
            "20x6-10-08T12:30:00Z",
        ] {
            assert!(!is_rfc3339_date_time(bad), "{bad}");
        }
    }

    #[test]
    fn a_schema_that_is_no_object_checks_nothing() {
        let schema = CompiledPropertySchema::compile(&json!(true));
        assert!(schema.types.is_none() && schema.format.is_none() && !schema.has_ref);
        let (findings, _) = validate_value("urn:x", "a", &json!(5), &schema);
        assert!(findings.is_empty());
    }

    #[test]
    fn a_language_property_is_null_only_where_its_schema_allows_null() {
        let nullable = CompiledPropertySchema::compile(
            &json!({ "type": ["object", "null"], "x-ngsi-ld-kind": "LanguageProperty" }),
        );
        let required = CompiledPropertySchema::compile(
            &json!({ "type": "object", "x-ngsi-ld-kind": "LanguageProperty" }),
        );
        assert!(validate_value("e1", "title", &Value::Null, &nullable)
            .0
            .is_empty());
        let (findings, _) = validate_value("e1", "title", &Value::Null, &required);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "type");
    }
}
