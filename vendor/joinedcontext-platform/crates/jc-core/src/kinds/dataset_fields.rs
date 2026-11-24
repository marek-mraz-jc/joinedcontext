//! An Organization's own dataset metadata fields (`spec.datasetFields`, T-7746): at most
//! [`MAX_DATASET_FIELDS`] typed fields its Endpoints fill in `spec.catalog.fields`, carried to the
//! open data portal as the dataset extra `x_<name>`.
//!
//! Every value is text to whoever shows it: a page escapes it, a `url` value is `https://` only,
//! and nothing here is ever HTML.

use crate::error::{Error, Result};
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most fields one Organization defines.
pub const MAX_DATASET_FIELDS: usize = 20;
/// The most values an `enum` field offers.
pub const MAX_ENUM_VALUES: usize = 50;
/// The longest value a field holds, in characters.
pub const MAX_FIELD_VALUE: usize = 2000;
/// The prefix of a field's dataset extra: `x_<name>`.
pub const EXTRA_PREFIX: &str = "x_";

/// What a field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DatasetFieldType {
    /// One line of text.
    Text,
    /// A decimal number, such as `12.5`.
    Number,
    /// A calendar date, `YYYY-MM-DD`.
    Date,
    /// An `https://` address.
    Url,
    /// One of the field's `values`.
    Enum,
}

/// One extra metadata field of an Organization's datasets (T-7746).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DatasetField {
    /// A DNS label, unique in the Organization, such as `funding`; the dataset extra is
    /// `x_<name>` and the DCAT property `https://<installation domain>/ns/dataset-fields/<org>/<name>`.
    pub name: String,
    /// The label the dataset form and page show, per language, such as `{sk: Zdroj financovania}`.
    pub label: BTreeMap<String, String>,
    /// What the field holds: `text`, `number`, `date`, `url` or `enum`.
    #[serde(rename = "type")]
    pub kind: DatasetFieldType,
    /// The values an `enum` field offers, at most 50; only `enum` takes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Whether every dataset of the Organization must fill it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

impl DatasetField {
    /// The dataset extra the value is stored under: `x_<name>`.
    pub fn extra_key(&self) -> String {
        format!("{EXTRA_PREFIX}{}", self.name)
    }

    /// The label in `lang`, else in `fallback`, else in any language, else the name.
    pub fn label_in<'a>(&'a self, lang: &str, fallback: &str) -> &'a str {
        self.label
            .get(lang)
            .or_else(|| self.label.get(fallback))
            .or_else(|| self.label.values().next())
            .map_or(self.name.as_str(), String::as_str)
    }

    /// Whether `value` is one this field holds; the reason a person can act on when not.
    pub fn check_value(&self, value: &str) -> std::result::Result<(), String> {
        if value.chars().count() > MAX_FIELD_VALUE {
            return Err(format!("at most {MAX_FIELD_VALUE} characters"));
        }
        if value.chars().any(char::is_control) {
            return Err("one line of plain text, no control characters".to_owned());
        }
        match self.kind {
            DatasetFieldType::Text => Ok(()),
            DatasetFieldType::Number => value
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && !value.contains(['e', 'E', 'i', 'n', 'N']))
                .map(|_| ())
                .ok_or_else(|| "a number, such as 12.5".to_owned()),
            DatasetFieldType::Date => value
                .parse::<jc_time::Date>()
                .map(|_| ())
                .map_err(|_| "a date, YYYY-MM-DD, such as 2026-11-23".to_owned()),
            DatasetFieldType::Url => is_https_url(value)
                .then_some(())
                .ok_or_else(|| "an https:// address, such as https://example.org/page".to_owned()),
            DatasetFieldType::Enum => {
                if self.values.iter().any(|v| v == value) {
                    Ok(())
                } else {
                    Err(format!("one of: {}", self.values.join(", ")))
                }
            }
        }
    }
}

/// An `https://` address with a host and no space, quote or angle bracket.
pub fn is_https_url(value: &str) -> bool {
    value.strip_prefix("https://").is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty()
            && !host.contains('@')
            && !rest
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || "\"'<>\\`".contains(c))
    })
}

/// Checks an Organization's field definitions: at most 20, unique DNS-label names, a label in
/// at least one language, values only on `enum` and at least one there.
pub fn validate(fields: &[DatasetField]) -> Result<()> {
    if fields.len() > MAX_DATASET_FIELDS {
        return Err(invalid(
            "spec.datasetFields",
            format!(
                "at most {MAX_DATASET_FIELDS} fields, found {}",
                fields.len()
            ),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (i, field) in fields.iter().enumerate() {
        let at = format!("spec.datasetFields[{i}]");
        names::validate_dns1123_label(&field.name)
            .map_err(|_| invalid(&format!("{at}.name"), "a DNS label such as `funding`"))?;
        if field.name.len() > 62 {
            return Err(invalid(&format!("{at}.name"), "at most 62 characters"));
        }
        if !seen.insert(field.name.as_str()) {
            return Err(invalid(
                &format!("{at}.name"),
                format!("`{}` is defined twice", field.name),
            ));
        }
        if field.label.values().all(|l| l.trim().is_empty()) {
            return Err(invalid(
                &format!("{at}.label"),
                "a label in at least one language, such as `{sk: Zdroj}`",
            ));
        }
        for (lang, label) in &field.label {
            names::validate_locale(lang)
                .map_err(|_| invalid(&format!("{at}.label"), format!("`{lang}` is no locale")))?;
            if label.chars().count() > 120 || label.chars().any(char::is_control) {
                return Err(invalid(
                    &format!("{at}.label.{lang}"),
                    "one line of at most 120 characters",
                ));
            }
        }
        match field.kind {
            DatasetFieldType::Enum => {
                if field.values.is_empty() || field.values.len() > MAX_ENUM_VALUES {
                    return Err(invalid(
                        &format!("{at}.values"),
                        format!("an enum offers 1 to {MAX_ENUM_VALUES} values"),
                    ));
                }
                let mut distinct = std::collections::BTreeSet::new();
                for value in &field.values {
                    if value.trim().is_empty()
                        || value.chars().count() > 120
                        || value.chars().any(char::is_control)
                        || !distinct.insert(value)
                    {
                        return Err(invalid(
                            &format!("{at}.values"),
                            format!("`{value}`: each value once, one line of 1 to 120 characters"),
                        ));
                    }
                }
            }
            _ if !field.values.is_empty() => {
                return Err(invalid(
                    &format!("{at}.values"),
                    "only an enum field takes values",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Checks the shape of `spec.catalog.fields` without the Organization's definitions: DNS-label
/// names and one-line values of at most 2000 characters. [`check_values`] checks them against
/// the definitions at propose.
pub fn validate_values(values: &BTreeMap<String, String>) -> Result<()> {
    if values.len() > MAX_DATASET_FIELDS {
        return Err(invalid(
            "spec.catalog.fields",
            format!("at most {MAX_DATASET_FIELDS} fields"),
        ));
    }
    for (name, value) in values {
        let at = format!("spec.catalog.fields.{name}");
        if names::validate_dns1123_label(name).is_err() || name.len() > 62 {
            return Err(invalid(
                &at,
                "a field name is a DNS label such as `funding`",
            ));
        }
        if value.chars().count() > MAX_FIELD_VALUE || value.chars().any(char::is_control) {
            return Err(invalid(
                &at,
                format!("one line of at most {MAX_FIELD_VALUE} characters"),
            ));
        }
    }
    Ok(())
}

/// Checks an Endpoint's `spec.catalog.fields` against its Organization's definitions: each
/// value names a defined field and holds what its type says, and every required field is
/// filled. The error names the field, `spec.catalog.fields.<name>`.
pub fn check_values(defs: &[DatasetField], values: &BTreeMap<String, String>) -> Result<()> {
    validate_values(values)?;
    for (name, value) in values {
        let at = format!("spec.catalog.fields.{name}");
        let Some(def) = defs.iter().find(|d| &d.name == name) else {
            return Err(invalid(
                &at,
                "the Organization defines no such dataset field; pick one of its spec.datasetFields",
            ));
        };
        def.check_value(value)
            .map_err(|reason| invalid(&at, reason))?;
    }
    for def in defs.iter().filter(|d| d.required) {
        if values.get(&def.name).is_none_or(|v| v.trim().is_empty()) {
            return Err(invalid(
                &format!("spec.catalog.fields.{}", def.name),
                "the Organization requires this field; fill it",
            ));
        }
    }
    Ok(())
}

fn invalid(field: &str, reason: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.to_owned(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, kind: DatasetFieldType, values: &[&str], required: bool) -> DatasetField {
        DatasetField {
            name: name.to_owned(),
            label: BTreeMap::from([("sk".to_owned(), name.to_uppercase())]),
            kind,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            required,
        }
    }

    fn defs() -> Vec<DatasetField> {
        vec![
            field("funding", DatasetFieldType::Enum, &["eu", "city"], true),
            field("reviewed", DatasetFieldType::Date, &[], false),
            field("budget", DatasetFieldType::Number, &[], false),
            field("page", DatasetFieldType::Url, &[], false),
        ]
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn field_of(err: Error) -> String {
        match err {
            Error::Invalid { field, .. } => field,
            other => format!("not an Invalid error: {other}"),
        }
    }

    #[test]
    fn field_of_names_an_error_of_another_kind() {
        let said = field_of(Error::Locale("xx".to_owned()));
        assert!(said.starts_with("not an Invalid error: "), "{said}");
    }

    #[test]
    fn values_round_trip_and_pass_their_types() {
        let ok = values(&[
            ("funding", "eu"),
            ("reviewed", "2026-11-23"),
            ("budget", "12.5"),
            ("page", "https://bbsk.sk/a?b=1"),
        ]);
        assert!(check_values(&defs(), &ok).is_ok());
    }

    #[test]
    fn each_type_rule_names_the_field() {
        for (pairs, named) in [
            (vec![("funding", "state")], "spec.catalog.fields.funding"),
            (
                vec![("funding", "eu"), ("reviewed", "23.11.2026")],
                "spec.catalog.fields.reviewed",
            ),
            (
                vec![("funding", "eu"), ("budget", "NaN")],
                "spec.catalog.fields.budget",
            ),
            (
                vec![("funding", "eu"), ("page", "http://bbsk.sk")],
                "spec.catalog.fields.page",
            ),
            (
                vec![("funding", "eu"), ("page", "javascript:alert(1)")],
                "spec.catalog.fields.page",
            ),
            (
                vec![("funding", "eu"), ("other", "x")],
                "spec.catalog.fields.other",
            ),
            (
                vec![("reviewed", "2026-11-23")],
                "spec.catalog.fields.funding",
            ),
        ] {
            let err = check_values(&defs(), &values(&pairs)).expect_err("refused");
            assert_eq!(field_of(err), named, "{pairs:?}");
        }
    }

    #[test]
    fn definitions_refuse_what_no_form_can_show() {
        assert!(validate(&defs()).is_ok());
        let too_many: Vec<_> = (0..21)
            .map(|i| field(&format!("f{i}"), DatasetFieldType::Text, &[], false))
            .collect();
        assert_eq!(
            field_of(validate(&too_many).unwrap_err()),
            "spec.datasetFields"
        );
        let twice = vec![defs()[1].clone(), defs()[1].clone()];
        assert_eq!(
            field_of(validate(&twice).unwrap_err()),
            "spec.datasetFields[1].name"
        );
        let bad_name = vec![field("Funding_1", DatasetFieldType::Text, &[], false)];
        assert_eq!(
            field_of(validate(&bad_name).unwrap_err()),
            "spec.datasetFields[0].name"
        );
        let empty_enum = vec![field("f", DatasetFieldType::Enum, &[], false)];
        assert_eq!(
            field_of(validate(&empty_enum).unwrap_err()),
            "spec.datasetFields[0].values"
        );
        let text_values = vec![field("f", DatasetFieldType::Text, &["a"], false)];
        assert_eq!(
            field_of(validate(&text_values).unwrap_err()),
            "spec.datasetFields[0].values"
        );
        let mut no_label = field("f", DatasetFieldType::Text, &[], false);
        no_label.label.clear();
        assert_eq!(
            field_of(validate(&[no_label]).unwrap_err()),
            "spec.datasetFields[0].label"
        );
    }

    #[test]
    fn a_value_is_one_line_and_https_has_a_host() {
        assert!(validate_values(&values(&[("f", "a\nb")])).is_err());
        assert!(!is_https_url("https://"));
        assert!(!is_https_url("https://user@evil.example/"));
        assert!(!is_https_url("https://a.sk/\"onmouseover=x"));
        assert!(is_https_url("https://a.sk"));
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn dataset_field_extra_key_and_label_in_fallbacks() {
        let field = DatasetField {
            name: "funding_source".into(),
            label: BTreeMap::from([
                ("sk".into(), "Zdroj".into()),
                ("en".into(), "Source".into()),
            ]),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        assert_eq!(field.extra_key(), "x_funding_source");
        assert_eq!(field.label_in("sk", "en"), "Zdroj");
        assert_eq!(field.label_in("de", "en"), "Source");
        assert_eq!(field.label_in("de", "cs"), "Source");

        let empty_label = DatasetField {
            name: "bare_field".into(),
            label: BTreeMap::new(),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        assert_eq!(empty_label.label_in("de", "cs"), "bare_field");
    }

    #[test]
    fn check_value_length_control_chars_and_text_kind() {
        let text_field = DatasetField {
            name: "notes".into(),
            label: BTreeMap::from([("en".into(), "Notes".into())]),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        assert!(text_field.check_value("Plain text notes").is_ok());

        let too_long = "a".repeat(MAX_FIELD_VALUE + 1);
        let err = text_field
            .check_value(&too_long)
            .expect_err("too long value rejected");
        assert!(err.contains("at most 2000 characters"));

        let with_ctrl = "Line 1\nLine 2";
        let err = text_field
            .check_value(with_ctrl)
            .expect_err("control char rejected");
        assert!(err.contains("one line of plain text, no control characters"));
    }

    #[test]
    fn validate_field_definitions_name_labels_and_enum_values() {
        let long_name = DatasetField {
            name: "a".repeat(63),
            label: BTreeMap::from([("en".into(), "Label".into())]),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        let err = validate(&[long_name]).expect_err("field name > 62 chars rejected");
        assert!(err.to_string().contains("at most 62 characters"), "{err}");

        let bad_locale = DatasetField {
            name: "valid-name".into(),
            label: BTreeMap::from([("INVALID".into(), "Label".into())]),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        let err = validate(&[bad_locale]).expect_err("bad locale rejected");
        assert!(err.to_string().contains("is no locale"), "{err}");

        let long_label = DatasetField {
            name: "valid-name".into(),
            label: BTreeMap::from([("en".into(), "a".repeat(121))]),
            kind: DatasetFieldType::Text,
            values: Vec::new(),
            required: false,
        };
        let err = validate(&[long_label]).expect_err("label > 120 chars rejected");
        assert!(
            err.to_string()
                .contains("one line of at most 120 characters"),
            "{err}"
        );

        let bad_enum_val = DatasetField {
            name: "status".into(),
            label: BTreeMap::from([("en".into(), "Status".into())]),
            kind: DatasetFieldType::Enum,
            values: vec!["   ".into()],
            required: false,
        };
        let err = validate(&[bad_enum_val]).expect_err("empty enum value rejected");
        assert!(
            err.to_string()
                .contains("each value once, one line of 1 to 120 characters"),
            "{err}"
        );
    }

    #[test]
    fn validate_values_bounds_and_names() {
        let mut map = BTreeMap::new();
        for i in 0..=MAX_DATASET_FIELDS {
            map.insert(format!("field{i}"), "value".into());
        }
        let err = validate_values(&map).expect_err("too many values rejected");
        assert!(err.to_string().contains("at most 20 fields"), "{err}");

        let mut bad_key = BTreeMap::new();
        bad_key.insert("BAD_NAME!".into(), "val".into());
        let err = validate_values(&bad_key).expect_err("invalid key rejected");
        assert!(
            err.to_string()
                .contains("a field name is a DNS label such as `funding`"),
            "{err}"
        );
    }
}
