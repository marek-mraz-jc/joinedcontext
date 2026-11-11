//! The reference page of every manifest kind (P-27, DOC-02): `docs/kinds/<Kind>.md`, generated
//! from the jc-core registry's JSON Schema of the whole manifest, so a config repository's author
//! reads the fields `deny_unknown_fields` holds them to instead of guessing them. The test fails
//! when a committed page differs from what the registry renders, when a kind has no page, or when
//! a page names no registry kind.
//!
//! Regenerate after a jc-core pin moves:
//! `JC_WRITE_KIND_DOCS=1 cargo test --test kind_reference_tests` (from crates/portal).

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;

use jc_core::envelope::Scope;
use jc_core::registry::{schema_of, KindInfo, KINDS};
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/kinds")
}

/// The definition a `$ref` names (schemars 0.8 writes `#/definitions/<name>`, and wraps it in a
/// one-element `allOf` when the field carries its own description).
fn ref_name(schema: &Value) -> Option<&str> {
    if let Some([only]) = schema
        .get("allOf")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        return ref_name(only);
    }
    schema.get("$ref")?.as_str()?.strip_prefix("#/definitions/")
}

/// Rustdoc's intra-doc links (`` [`Kind`][crate::kinds::Kind] ``) as their text alone.
fn plain(text: &str) -> String {
    let mut out = text.to_owned();
    while let Some(at) = out.find("][") {
        let (Some(open), Some(close)) = (out[..at].rfind('['), out[at + 2..].find(']')) else {
            break;
        };
        let close = at + 2 + close;
        if out[at + 2..close].contains(char::is_whitespace) {
            break;
        }
        out = format!(
            "{}{}{}",
            &out[..open],
            &out[open + 1..at],
            &out[close + 1..]
        );
    }
    out
}

/// The values of a schema that admits only listed values, a union of them included.
fn values_of(schema: &Value) -> Option<Vec<String>> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Some(values.iter().map(|v| format!("`{v}`")).collect());
    }
    if let Some(value) = schema.get("const") {
        return Some(vec![format!("`{value}`")]);
    }
    let parts = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))?
        .as_array()?;
    let mut all = Vec::new();
    for part in parts {
        all.extend(values_of(part)?);
    }
    Some(all)
}

fn anchor(name: &str) -> String {
    name.to_lowercase()
}

/// One table cell: no line break and no column separator.
fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

/// The type of a property as a reader writes it, with each named definition it reaches queued.
fn type_of(schema: &Value, queue: &mut VecDeque<String>) -> String {
    if let Some(name) = ref_name(schema) {
        queue.push_back(name.to_owned());
        return format!("[{name}](#{})", anchor(name));
    }
    if let Some(values) = values_of(schema) {
        return match values.as_slice() {
            [one] => one.clone(),
            many => format!("one of {}", many.join(", ")),
        };
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(parts) = schema.get(key).and_then(Value::as_array) {
            let parts: Vec<String> = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) != Some("null"))
                .map(|part| type_of(part, queue))
                .collect();
            return parts.join(" or ");
        }
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => return "any".to_owned(),
    };
    types
        .iter()
        .map(|t| match *t {
            "array" => match schema.get("items") {
                Some(items) => format!("list of {}", type_of(items, queue)),
                None => "list".to_owned(),
            },
            "object" => match schema.get("additionalProperties") {
                Some(values) if values.is_object() => {
                    format!("map of {}", type_of(values, queue))
                }
                _ => "object".to_owned(),
            },
            other => match schema.get("format").and_then(Value::as_str) {
                Some(format) => format!("{other} ({format})"),
                None => other.to_owned(),
            },
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

/// The section of one object or enum schema: its description and one row per field.
fn section(out: &mut String, title: &str, schema: &Value, queue: &mut VecDeque<String>) {
    out.push_str(&format!("\n## {title}\n\n"));
    if let Some(text) = schema.get("description").and_then(Value::as_str) {
        out.push_str(&plain(text.trim()));
        out.push_str("\n\n");
    }
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    match schema.get("properties").and_then(Value::as_object) {
        Some(props) if !props.is_empty() => {
            out.push_str("| Field | Type | Required | Description |\n|---|---|---|---|\n");
            for (name, prop) in props {
                let description = prop
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                out.push_str(&format!(
                    "| `{name}` | {} | {} | {} |\n",
                    cell(&type_of(prop, queue)),
                    if required.contains(name.as_str()) {
                        "yes"
                    } else {
                        "no"
                    },
                    cell(&plain(description))
                ));
            }
        }
        _ => out.push_str(&format!("Type: {}\n", cell(&type_of(schema, queue)))),
    }
}

/// The reference page of one kind.
fn page(info: &KindInfo) -> String {
    let schema = schema_of(info.kind).expect("every registry kind has a schema");
    let definitions = schema.get("definitions").cloned().unwrap_or(Value::Null);
    let scope = match info.scope {
        Scope::Organization => "the Organization (namespace `org`)",
        Scope::Project => "a project (namespace: the project's slug)",
        Scope::OrganizationOrProject => "the Organization (`org`) or a project",
    };
    let mut out = format!(
        "# {kind}\n\n<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; \
         do not edit. -->\n\n\
         - apiVersion: `joinedcontext.com/v1alpha1`, kind: `{kind}`\n\
         - Lives in {scope}\n\
         - API: `/api/v1/projects/{{project}}/{plural}`\n\
         - Repository path: `{path}`\n",
        kind = info.kind,
        plural = info.plural,
        path = info.path_template,
    );
    if let Some(path) = info.project_path_template {
        out.push_str(&format!("- Repository path in a project: `{path}`\n"));
    }
    if let Some(path) = info.spaceless_path_template {
        out.push_str(&format!("- Repository path without a space: `{path}`\n"));
    }
    out.push_str(
        "\nEvery field below is checked with `deny_unknown_fields`: a field this page does not \
         name is refused at propose, approve and load (`jcctl validate --as-portal`).\n",
    );
    let mut queue = VecDeque::new();
    let spec = &schema["properties"]["spec"];
    let spec_name = ref_name(spec).map(str::to_owned);
    let mut seen = BTreeSet::new();
    match &spec_name {
        Some(name) => queue.push_back(name.clone()),
        None => section(&mut out, "spec", spec, &mut queue),
    }
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(def) = definitions.get(&name) else {
            continue;
        };
        let title = if Some(&name) == spec_name.as_ref() {
            format!("spec ({name})")
        } else {
            name.clone()
        };
        section(&mut out, &title, def, &mut queue);
    }
    out
}

fn index() -> String {
    let mut out = String::from(
        "# Manifest kinds\n\n<!-- Generated by crates/portal/tests/kind_reference_tests.rs; do not edit. -->\n\n\
         One page per kind of `apiVersion: joinedcontext.com/v1alpha1`, generated from the jc-core \
         registry's JSON Schema (P-27).\n\n",
    );
    for info in KINDS {
        out.push_str(&format!(
            "- [{kind}]({kind}.md): `{plural}`\n",
            kind = info.kind,
            plural = info.plural
        ));
    }
    out
}

/// Every registry kind's page and the index, as the registry renders them.
fn expected() -> Vec<(String, String)> {
    let mut pages: Vec<(String, String)> = KINDS
        .iter()
        .map(|info| (format!("{}.md", info.kind), page(info)))
        .collect();
    pages.push(("README.md".to_owned(), index()));
    pages
}

#[test]
fn every_kind_has_the_reference_page_its_schema_generates() {
    let dir = dir();
    if std::env::var_os("JC_WRITE_KIND_DOCS").is_some() {
        std::fs::create_dir_all(&dir).expect("docs/kinds");
        for (file, text) in expected() {
            std::fs::write(dir.join(file), text).expect("a page is written");
        }
    }
    let mut stale = Vec::new();
    for (file, text) in expected() {
        if std::fs::read_to_string(dir.join(&file)).ok().as_deref() != Some(text.as_str()) {
            stale.push(file);
        }
    }
    assert!(
        stale.is_empty(),
        "docs/kinds differs from the registry for {stale:?}: run \
         JC_WRITE_KIND_DOCS=1 cargo test --test kind_reference_tests and commit docs/kinds"
    );
    let want: BTreeSet<String> = expected().into_iter().map(|(file, _)| file).collect();
    let extra: Vec<String> = std::fs::read_dir(&dir)
        .expect("docs/kinds")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|file| !want.contains(file))
        .collect();
    assert!(
        extra.is_empty(),
        "docs/kinds has pages no registry kind generates: {extra:?}"
    );
}

/// The generator itself: a kind's page names its required spec fields and reaches the
/// definitions they use, and the index lists every kind.
#[test]
fn a_page_names_the_required_fields_and_the_types_they_reach() {
    let space = page(jc_core::registry::by_kind("ContextSpace").expect("ContextSpace"));
    assert!(space.starts_with("# ContextSpace\n"), "{space}");
    assert!(space.contains("## spec ("), "{space}");
    assert!(
        space.contains("| `keyRef` | [SecretRef](#secretref) | yes |"),
        "{space}"
    );
    assert!(space.contains("one of `\"fill\"`, `\"refuse\"`"), "{space}");
    assert!(
        !space.contains("][crate::"),
        "rustdoc links read as text: {space}"
    );
    let index = index();
    assert_eq!(index.matches("](").count(), KINDS.len());
    for kind in [
        "DataSpaceParticipant",
        "DataOffer",
        "DataAgreement",
        "Organization",
    ] {
        assert!(index.contains(&format!("[{kind}]({kind}.md)")), "{kind}");
    }
}
