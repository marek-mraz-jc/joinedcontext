use crate::resource::ResourceEnvelope;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SelectorError {
    #[error("malformed selector: {0}")]
    Malformed(String),
    #[error("unknown field: {0}")]
    UnknownField(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Requirement {
    Equals(String, String),
    NotEquals(String, String),
    In(String, Vec<String>),
    NotIn(String, Vec<String>),
    Exists(String),
    DoesNotExist(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelSelector {
    requirements: Vec<Requirement>,
}

fn is_valid_key(k: &str) -> bool {
    !k.is_empty()
        && !k
            .chars()
            .any(|c| c.is_whitespace() || "=!()[],".contains(c))
}

/// Splits on the commas that separate requirements, ignoring the ones inside a `( … )`
/// value set — `tier in (a,b),env=prod` is two requirements, not three.
fn split_requirements(input: &str) -> Result<Vec<&str>, SelectorError> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in input.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    SelectorError::Malformed(format!("unbalanced parenthesis: {input}"))
                })?
            }
            ',' if depth == 0 => {
                parts.push(&input[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(SelectorError::Malformed(format!(
            "unclosed set parenthesis: {input}"
        )));
    }
    parts.push(&input[start..]);
    Ok(parts)
}

impl LabelSelector {
    pub fn parse(input: &str) -> Result<Self, SelectorError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(Self {
                requirements: Vec::new(),
            });
        }

        let mut requirements = Vec::new();
        for part in split_requirements(input)? {
            let part = part.trim();
            if part.is_empty() {
                return Err(SelectorError::Malformed(
                    "empty requirement in selector".into(),
                ));
            }

            if part.contains('(') {
                if !part.ends_with(')') {
                    return Err(SelectorError::Malformed(format!(
                        "unclosed set parenthesis: {part}"
                    )));
                }
                let (before, inside) = part[..part.len() - 1].split_once('(').ok_or_else(|| {
                    SelectorError::Malformed(format!("malformed set requirement: {part}"))
                })?;
                let before = before.trim();
                let values: Vec<String> = inside.split(',').map(|v| v.trim().to_string()).collect();

                if values.is_empty() || values.iter().any(|v| v.is_empty()) {
                    return Err(SelectorError::Malformed(format!(
                        "set values must be non-empty in: {part}"
                    )));
                }

                if let Some(key) = before.strip_suffix("notin") {
                    let key = key.trim();
                    if !is_valid_key(key) {
                        return Err(SelectorError::Malformed(format!("invalid key: {key}")));
                    }
                    requirements.push(Requirement::NotIn(key.to_string(), values));
                } else if let Some(key) = before.strip_suffix("in") {
                    let key = key.trim();
                    if !is_valid_key(key) {
                        return Err(SelectorError::Malformed(format!("invalid key: {key}")));
                    }
                    requirements.push(Requirement::In(key.to_string(), values));
                } else {
                    return Err(SelectorError::Malformed(format!(
                        "expected 'in' or 'notin' before parenthesis in: {part}"
                    )));
                }
            } else if let Some(stripped) = part.strip_prefix('!') {
                let key = stripped.trim();
                if !is_valid_key(key) {
                    return Err(SelectorError::Malformed(format!("invalid key in: {part}")));
                }
                requirements.push(Requirement::DoesNotExist(key.to_string()));
            } else if let Some((k, v)) = part.split_once("==") {
                let (k, v) = (k.trim(), v.trim());
                if !is_valid_key(k) {
                    return Err(SelectorError::Malformed(format!("invalid key in: {part}")));
                }
                requirements.push(Requirement::Equals(k.to_string(), v.to_string()));
            } else if let Some((k, v)) = part.split_once("!=") {
                let (k, v) = (k.trim(), v.trim());
                if !is_valid_key(k) {
                    return Err(SelectorError::Malformed(format!("invalid key in: {part}")));
                }
                requirements.push(Requirement::NotEquals(k.to_string(), v.to_string()));
            } else if let Some((k, v)) = part.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                if !is_valid_key(k) {
                    return Err(SelectorError::Malformed(format!("invalid key in: {part}")));
                }
                requirements.push(Requirement::Equals(k.to_string(), v.to_string()));
            } else {
                let key = part.trim();
                if !is_valid_key(key) {
                    return Err(SelectorError::Malformed(format!("invalid key in: {part}")));
                }
                requirements.push(Requirement::Exists(key.to_string()));
            }
        }

        Ok(Self { requirements })
    }

    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.requirements.iter().all(|req| match req {
            Requirement::Equals(k, v) => labels.get(k).map(|val| val == v).unwrap_or(false),
            Requirement::NotEquals(k, v) => labels.get(k).map(|val| val != v).unwrap_or(true),
            Requirement::In(k, values) => labels
                .get(k)
                .map(|val| values.contains(val))
                .unwrap_or(false),
            Requirement::NotIn(k, values) => labels
                .get(k)
                .map(|val| !values.contains(val))
                .unwrap_or(true),
            Requirement::Exists(k) => labels.contains_key(k),
            Requirement::DoesNotExist(k) => !labels.contains_key(k),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    MetadataName,
    MetadataNamespace,
    StatusPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldRequirement {
    Equals(Field, String),
    NotEquals(Field, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldSelector {
    requirements: Vec<FieldRequirement>,
}

impl FieldSelector {
    pub fn parse(input: &str) -> Result<Self, SelectorError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(Self {
                requirements: Vec::new(),
            });
        }

        let mut requirements = Vec::new();
        for part in split_requirements(input)? {
            let part = part.trim();
            if part.is_empty() {
                return Err(SelectorError::Malformed("empty field requirement".into()));
            }

            let (field_str, is_not_equal, value) = if let Some((k, v)) = part.split_once("==") {
                (k.trim(), false, v.trim())
            } else if let Some((k, v)) = part.split_once("!=") {
                (k.trim(), true, v.trim())
            } else if let Some((k, v)) = part.split_once('=') {
                (k.trim(), false, v.trim())
            } else {
                return Err(SelectorError::Malformed(format!(
                    "missing operator in field requirement: {part}"
                )));
            };

            let field = match field_str {
                "metadata.name" => Field::MetadataName,
                "metadata.namespace" => Field::MetadataNamespace,
                "status.phase" => Field::StatusPhase,
                other => return Err(SelectorError::UnknownField(other.to_string())),
            };

            if is_not_equal {
                requirements.push(FieldRequirement::NotEquals(field, value.to_string()));
            } else {
                requirements.push(FieldRequirement::Equals(field, value.to_string()));
            }
        }

        Ok(Self { requirements })
    }

    pub fn matches(&self, envelope: &ResourceEnvelope) -> bool {
        self.requirements.iter().all(|req| match req {
            FieldRequirement::Equals(field, expected) => match field {
                Field::MetadataName => envelope.metadata.name == *expected,
                Field::MetadataNamespace => {
                    envelope.metadata.namespace.as_deref().unwrap_or_default() == expected.as_str()
                }
                Field::StatusPhase => envelope
                    .status
                    .as_ref()
                    .map(|s| super::phase_str(s.phase) == expected.as_str())
                    .unwrap_or(false),
            },
            FieldRequirement::NotEquals(field, expected) => match field {
                Field::MetadataName => envelope.metadata.name != *expected,
                Field::MetadataNamespace => {
                    envelope.metadata.namespace.as_deref().unwrap_or_default() != expected.as_str()
                }
                Field::StatusPhase => envelope
                    .status
                    .as_ref()
                    .map(|s| super::phase_str(s.phase) != expected.as_str())
                    .unwrap_or(true),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn label_selector_operators() {
        let set = labels(&[
            ("environment", "prod"),
            ("tier", "frontend"),
            ("active", "true"),
        ]);

        let s1 = LabelSelector::parse("environment=prod").expect("parse =");
        assert!(s1.matches(&set));

        let s2 = LabelSelector::parse("environment==prod").expect("parse ==");
        assert!(s2.matches(&set));

        let s3 = LabelSelector::parse("environment!=stage").expect("parse !=");
        assert!(s3.matches(&set));
        let s3_neg = LabelSelector::parse("environment!=prod").expect("parse !=");
        assert!(!s3_neg.matches(&set));

        let s4 = LabelSelector::parse("tier in (frontend, backend)").expect("parse in");
        assert!(s4.matches(&set));

        let s5 = LabelSelector::parse("tier notin (mobile, backend)").expect("parse notin");
        assert!(s5.matches(&set));

        let s6 = LabelSelector::parse("active").expect("parse exists");
        assert!(s6.matches(&set));

        let s7 = LabelSelector::parse("!deprecated").expect("parse does not exist");
        assert!(s7.matches(&set));

        let s7_neg = LabelSelector::parse("!active").expect("parse does not exist");
        assert!(!s7_neg.matches(&set));
    }

    #[test]
    fn label_selector_whitespace_tolerance() {
        let set = labels(&[("joinedcontext.com/domain", "environment")]);
        let s =
            LabelSelector::parse("  joinedcontext.com/domain  =  environment  ").expect("parse");
        assert!(s.matches(&set));
    }

    #[test]
    fn label_selector_malformed_input() {
        assert!(LabelSelector::parse("tier in (frontend").is_err());
        assert!(LabelSelector::parse("tier in ()").is_err());
        assert!(LabelSelector::parse("!").is_err());
        assert!(LabelSelector::parse("")
            .expect("empty selector")
            .matches(&BTreeMap::new()));
    }

    #[test]
    fn field_selector_valid_fields_and_operators() {
        let mut envelope = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: crate::resource::ObjectMeta {
                name: "public-air".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: Some(crate::resource::Status {
                phase: crate::resource::Phase::Live,
                observed_revision: None,
                source_url: None,
                conditions: Vec::new(),
                build: None,
                shard: None,
                domain_verification: None,
                jobs: Vec::new(),
                notifications: None,
            }),
        };

        let fs1 = FieldSelector::parse("metadata.name=public-air").expect("parse name =");
        assert!(fs1.matches(&envelope));

        let fs2 = FieldSelector::parse("metadata.name==public-air").expect("parse name ==");
        assert!(fs2.matches(&envelope));

        let fs3 = FieldSelector::parse("metadata.name!=other").expect("parse name !=");
        assert!(fs3.matches(&envelope));

        let fs4 = FieldSelector::parse("metadata.namespace=ovzdusie").expect("parse ns =");
        assert!(fs4.matches(&envelope));

        let fs5 = FieldSelector::parse("status.phase=Live").expect("parse phase =");
        assert!(fs5.matches(&envelope));

        let fs6 = FieldSelector::parse("status.phase!=Draft").expect("parse phase !=");
        assert!(fs6.matches(&envelope));

        envelope.status = None;
        assert!(fs6.matches(&envelope));
        assert!(!fs5.matches(&envelope));
    }

    #[test]
    fn field_selector_whitespace_tolerance() {
        let envelope = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: crate::resource::ObjectMeta {
                name: "public-air".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        };

        let fs = FieldSelector::parse("  metadata.name  ==  public-air  ").expect("parse");
        assert!(fs.matches(&envelope));
    }

    #[test]
    fn field_selector_unknown_field_is_rejected() {
        let err = FieldSelector::parse("spec.unknown=123").expect_err("unknown field");
        assert_eq!(err, SelectorError::UnknownField("spec.unknown".into()));
    }

    #[test]
    fn field_selector_malformed_is_rejected() {
        let err = FieldSelector::parse("metadata.name").expect_err("missing operator");
        assert!(matches!(err, SelectorError::Malformed(_)));
    }
}
