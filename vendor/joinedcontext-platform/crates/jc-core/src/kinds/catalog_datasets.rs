//! `spec.catalog.datasets`: several catalogue datasets of one Endpoint, each a narrowed slice of
//! what the Endpoint serves (ADR-N-068, EP-175, EP-177).
//!
//! A slice may only narrow. What it can say is checked here, where the Endpoint alone is known:
//! the names, the types as names, and the query as the broker's own grammar reads it. Whether
//! each type is one the Endpoint serves needs the space's model and the Policies, which
//! `jc_manifest::catalogue` and the publisher check against the repository.

use super::catalog::{Licence, Texts};
use crate::error::{Error, Result};
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most datasets one Endpoint lists.
pub const MAX_DATASETS: usize = 50;

/// The most entity types one dataset names.
pub const MAX_TYPES: usize = 20;

/// The longest `q` of one dataset, in bytes.
pub const MAX_Q_BYTES: usize = 2_000;

/// One catalogue dataset of an Endpoint: a name, its texts, and the rows it holds (EP-175).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CatalogDataset {
    /// The CKAN dataset name, unique in the catalogue: 2 to 100 characters of `a-z`, `0-9`,
    /// `-` and `_`, such as `bbsk-skoly`.
    pub name: String,
    /// The dataset's title per language: `dct:title`.
    pub title: Texts,
    /// The dataset's description per language: `dct:description`.
    pub description: Texts,
    /// The entity types the dataset holds, each one the Endpoint serves, such as `[School]`.
    pub types: Vec<String>,
    /// An NGSI-LD query its rows match (CIM 009 clause 4.9), such as `schoolType=="gymnazium"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Keywords per language, in place of `spec.catalog.keywords`: `dcat:keyword`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keywords: BTreeMap<String, Vec<String>>,
    /// The licence from the EU licence table, in place of `spec.catalog.license`: `dct:license`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<Licence>,
}

impl CatalogDataset {
    /// The `type` parameter of the dataset's reads: its types joined with `,`.
    pub fn type_parameter(&self) -> String {
        self.types.join(",")
    }

    /// The query string of the dataset's NGSI-LD and CSV reads, percent-encoded: `type=` its
    /// types, then `q=` its query when it has one (EP-176).
    pub fn query(&self) -> String {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("type", &self.type_parameter());
        if let Some(q) = &self.q {
            query.append_pair("q", q);
        }
        query.finish()
    }
}

/// Validates the list: its length, each entry, and that no name is listed twice (EP-177).
pub fn validate(datasets: &[CatalogDataset]) -> Result<()> {
    if datasets.len() > MAX_DATASETS {
        return Err(Error::Invalid {
            field: "spec.catalog.datasets".to_owned(),
            reason: format!("an Endpoint lists at most {MAX_DATASETS} datasets"),
        });
    }
    let mut names = BTreeSet::new();
    for (index, dataset) in datasets.iter().enumerate() {
        let field = |member: &str| format!("spec.catalog.datasets[{index}].{member}");
        one(dataset, &field)?;
        if !names.insert(dataset.name.as_str()) {
            return Err(Error::Invalid {
                field: field("name"),
                reason: format!(
                    "`{}` is listed twice: a dataset name is unique in the catalogue",
                    dataset.name
                ),
            });
        }
    }
    Ok(())
}

fn one(dataset: &CatalogDataset, field: &dyn Fn(&str) -> String) -> Result<()> {
    let invalid = |member: &str, reason: String| Error::Invalid {
        field: field(member),
        reason,
    };
    super::ckan::validate_slug("spec.catalog.datasets[].name", &dataset.name).map_err(|_| {
        invalid(
            "name",
            "a CKAN name is 2 to 100 characters of a-z, 0-9, `-` and `_`, such as `bbsk-skoly`"
                .to_owned(),
        )
    })?;
    for (member, texts) in [
        ("title", &dataset.title),
        ("description", &dataset.description),
    ] {
        if texts.values().all(|text| text.trim().is_empty()) {
            return Err(invalid(
                member,
                "give the text in at least one language".to_owned(),
            ));
        }
        for (language, text) in texts {
            names::validate_locale(language).map_err(|_| {
                invalid(
                    member,
                    format!("`{language}` is no ISO 639-1 language code"),
                )
            })?;
            if text.trim().is_empty() {
                return Err(invalid(
                    member,
                    format!("`{language}` is listed with no text"),
                ));
            }
        }
    }
    if dataset.types.is_empty() || dataset.types.len() > MAX_TYPES {
        return Err(invalid(
            "types",
            format!("name 1 to {MAX_TYPES} entity types the Endpoint serves"),
        ));
    }
    let mut seen = BTreeSet::new();
    for kind in &dataset.types {
        if !is_type_name(kind) {
            return Err(invalid(
                "types",
                format!("`{kind}` is no NGSI-LD type name, such as `School`"),
            ));
        }
        if !seen.insert(kind.as_str()) {
            return Err(invalid("types", format!("`{kind}` is listed twice")));
        }
    }
    if let Some(q) = &dataset.q {
        check_q(q).map_err(|reason| invalid("q", reason))?;
    }
    for (language, words) in &dataset.keywords {
        names::validate_locale(language).map_err(|_| {
            invalid(
                "keywords",
                format!("`{language}` is no ISO 639-1 language code"),
            )
        })?;
        if words
            .iter()
            .any(|w| w.trim().is_empty() || w.len() > 100 || w.chars().any(char::is_control))
        {
            return Err(invalid(
                "keywords",
                "a keyword is one line of at most 100 bytes".to_owned(),
            ));
        }
    }
    Ok(())
}

/// A plain NGSI-LD type name as a dataset names it: a letter, then letters, digits and `_`.
pub fn is_type_name(kind: &str) -> bool {
    (1..=100).contains(&kind.len())
        && kind.starts_with(|c: char| c.is_ascii_alphabetic())
        && kind.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Why `q` is no dataset query, `Ok` when it is one: bounded, free of anything a URL reads as a
/// separator, and parsed by the broker's own grammar (EP-177).
pub fn check_q(q: &str) -> std::result::Result<(), String> {
    if q.trim().is_empty() {
        return Err("an empty q narrows nothing: drop it".to_owned());
    }
    if q.len() > MAX_Q_BYTES {
        return Err(format!("q is at most {MAX_Q_BYTES} bytes"));
    }
    if let Some(c) = q
        .chars()
        .find(|c| matches!(c, '&' | '#' | '?') || c.is_control())
    {
        return Err(format!(
            "q holds {c:?}, which a URL reads as the start of another part: write the query \
             without it"
        ));
    }
    antares_ql::parse_q(q)
        .map(|_| ())
        .map_err(|why| format!("q is no NGSI-LD query: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset(name: &str) -> CatalogDataset {
        CatalogDataset {
            name: name.to_owned(),
            title: BTreeMap::from([("sk".to_owned(), "Školy".to_owned())]),
            description: BTreeMap::from([("sk".to_owned(), "Školy kraja".to_owned())]),
            types: vec!["School".to_owned()],
            q: None,
            keywords: BTreeMap::new(),
            license: None,
        }
    }

    fn refused(datasets: &[CatalogDataset]) -> String {
        validate(datasets).expect_err("refused").to_string()
    }

    #[test]
    fn the_query_encodes_type_and_q() {
        let mut gymnazia = dataset("bbsk-gymnazia");
        gymnazia.types.push("Kindergarten".to_owned());
        gymnazia.q = Some("schoolType==\"gymnázium\";a>1".to_owned());
        assert_eq!(
            gymnazia.query(),
            "type=School%2CKindergarten&q=schoolType%3D%3D%22gymn%C3%A1zium%22%3Ba%3E1"
        );
        assert_eq!(dataset("a-b").query(), "type=School");
    }

    #[test]
    fn a_list_of_slices_validates() {
        let mut gymnazia = dataset("bbsk-gymnazia");
        gymnazia.q = Some("schoolType==\"gymnazium\"".to_owned());
        validate(&[dataset("bbsk-skoly"), gymnazia]).expect("valid");
    }

    #[test]
    fn a_name_listed_twice_is_refused() {
        let why = refused(&[dataset("bbsk-skoly"), dataset("bbsk-skoly")]);
        assert!(
            why.contains("datasets[1].name") && why.contains("twice"),
            "{why}"
        );
    }

    #[test]
    fn a_name_ckan_refuses_is_refused() {
        assert!(refused(&[dataset("Skoly")]).contains("datasets[0].name"));
    }

    #[test]
    fn a_dataset_without_types_or_with_a_bad_type_is_refused() {
        let mut none = dataset("a-b");
        none.types.clear();
        assert!(refused(&[none]).contains("types"));
        let mut bad = dataset("a-b");
        bad.types = vec!["School&type=Secret".to_owned()];
        assert!(refused(&[bad]).contains("no NGSI-LD type name"));
        let mut twice = dataset("a-b");
        twice.types = vec!["School".to_owned(), "School".to_owned()];
        assert!(refused(&[twice]).contains("twice"));
    }

    #[test]
    fn a_q_that_could_widen_the_read_is_refused() {
        for (q, said) in [
            ("a==1&type=Secret", "start of another part"),
            ("a==1#x", "start of another part"),
            ("a==1?", "start of another part"),
            ("a==1\nb", "start of another part"),
            ("a===", "no NGSI-LD query"),
            ("   ", "narrows nothing"),
        ] {
            let mut sliced = dataset("a-b");
            sliced.q = Some(q.to_owned());
            let why = refused(&[sliced]);
            assert!(
                why.contains("datasets[0].q") && why.contains(said),
                "{q}: {why}"
            );
        }
        let mut long = dataset("a-b");
        long.q = Some(format!("a=={}", "1".repeat(MAX_Q_BYTES)));
        assert!(refused(&[long]).contains("at most"));
    }

    #[test]
    fn a_dataset_without_title_or_description_is_refused() {
        let mut untitled = dataset("a-b");
        untitled.title.clear();
        assert!(refused(&[untitled]).contains("datasets[0].title"));
        let mut blank = dataset("a-b");
        blank.description = BTreeMap::from([("en".to_owned(), " ".to_owned())]);
        assert!(refused(&[blank]).contains("datasets[0].description"));
    }

    #[test]
    fn more_than_fifty_datasets_are_refused() {
        let many: Vec<_> = (0..=MAX_DATASETS)
            .map(|i| dataset(&format!("d-{i}")))
            .collect();
        assert!(refused(&many).contains("at most 50"));
    }
}
