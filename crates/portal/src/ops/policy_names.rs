//! A Policy's attribute lists against its space's data model (T-6283).
//!
//! `information[].propertyNames` and `relationshipNames` narrow a grant to the Attributes they
//! name. A name the model does not define narrows the grant to nothing a broker ever stores, so a
//! steward's typo grants less than meant and still plans green. The dry run reads the space's
//! LinkML source and names each such entry as an error on its field. A class the model's own
//! source does not hold whole (an imported class, or one whose `is_a` or mixin is imported) is
//! not judged here: Model Tools resolves imports, and a guess would refuse a valid Policy.
//!
//! The same holds for the keys of an entry's `attributes` capability block (ADR-N-066, DM-148),
//! and a public grant never reads the versions of an attribute the model marks `personalData`
//! (DM-151): the dry run names the key that gives them.

use std::collections::BTreeSet;

use serde_json::Value;

use super::verdict::{Finding, Level};
use crate::state::AppState;

/// The NGSI-LD core GeoProperties every Entity may carry, whatever its class lists.
const CORE: [&str; 3] = ["location", "observationSpace", "operationSpace"];

/// One error finding per attribute name of `manifest` (a Policy of `project`) that its space's
/// model does not define for any type of the same `information` entry. Empty for another kind, or
/// when the space, its model or the model's source cannot be read.
pub async fn findings(state: &AppState, project: &str, manifest: &Value) -> Vec<Finding> {
    if manifest["kind"].as_str() != Some("Policy") || !needs_model(&manifest["spec"]) {
        return Vec::new();
    }
    match space_model(state, project, &manifest["spec"]).await {
        Some((model, source)) => of_model(&manifest["spec"], &model, &source),
        None => Vec::new(),
    }
}

/// The findings for a Policy `spec` against the LinkML `source` of its space's DataModel `model`.
/// Empty when the spec names nothing the model judges or the source does not parse.
fn of_model(spec: &Value, model: &str, source: &str) -> Vec<Finding> {
    if !needs_model(spec) {
        return Vec::new();
    }
    let Ok(linkml) = serde_norway::from_str(source) else {
        return Vec::new();
    };
    let mut out = unknown_names(spec, &linkml, model);
    out.extend(public_personal_versions(
        spec,
        &crate::personal_data::marks(source),
    ));
    out
}

/// A warning on an Endpoint whose `spec.audit` is `changes`: ADR-N-066 2.5 moves the audit into
/// `versions: [keep]` on the `*` entry of the Endpoint's write grants, and the gateway reads the
/// old field for one release more (DM-150).
pub fn superseded_audit(manifest: &Value) -> Option<Finding> {
    (manifest["kind"].as_str() == Some("Endpoint")
        && manifest["spec"]["audit"].as_str() == Some("changes"))
    .then(|| Finding {
        level: Level::Warning,
        path: "spec.audit".into(),
        message: "spec.audit is superseded: give the Endpoint's write grants \
                  `attributes: {\"*\": {versions: [keep]}}` and drop spec.audit (ADR-N-066 2.5); \
                  the gateway reads it for one release more"
            .into(),
    })
}

/// The name and LinkML source of the DataModel the spec's space names.
async fn space_model(state: &AppState, project: &str, spec: &Value) -> Option<(String, String)> {
    let space = crate::resource::reference_name(&spec["contextSpaceRef"])?;
    let space = state.mirror.get(project, "ContextSpace", space)?;
    let (namespace, model) =
        crate::resource::model_reference(&space.spec["dataModelRef"], project)?;
    let linkml = crate::api::datamodels::read_source(state, namespace, model)
        .await
        .ok()?;
    Some((model.to_owned(), linkml))
}

/// Whether the spec needs its space's model: it lists an attribute name or capability key, or a
/// holder lists `readVersions`, whose default reads every attribute's versions (DM-151).
fn needs_model(spec: &Value) -> bool {
    has_names(spec)
        || holders(spec).any(|(_, holder)| {
            holder["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|a| a.as_str() == Some("readVersions")))
        })
}

/// Whether the spec or one of its rules lists an attribute name or capability key.
fn has_names(spec: &Value) -> bool {
    entries(spec).any(|(_, info)| {
        ["propertyNames", "relationshipNames"].iter().any(|list| {
            info[*list]
                .as_array()
                .is_some_and(|names| !names.is_empty())
        }) || info["attributes"]
            .as_object()
            .is_some_and(|keys| !keys.is_empty())
    })
}

/// The spec and each of its rules, with their field paths: what holds a grant.
fn holders(spec: &Value) -> impl Iterator<Item = (String, &Value)> {
    let own = std::iter::once(("spec".to_owned(), spec));
    let rules = spec["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(n, rule)| (format!("spec.rules.{n}"), rule));
    own.chain(rules)
}

/// The `information` entries of one holder, with their field paths.
fn entries_of(at: String, holder: &Value) -> impl Iterator<Item = (String, &Value)> {
    holder["information"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(move |(n, info)| (format!("{at}.information.{n}"), info))
}

/// Every `information` entry with its field path: the spec's own, then each rule's.
fn entries(spec: &Value) -> impl Iterator<Item = (String, &Value)> {
    holders(spec).flat_map(|(at, holder)| entries_of(at, holder))
}

/// The entity types an entry's selectors name.
fn entry_types(info: &Value) -> Vec<&str> {
    info["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|selector| selector["type"].as_str())
        .collect()
}

/// One error per entry that lets a public grant read the versions of an attribute its model
/// marks personal (DM-151). The attribute's `versions` falls back to `*`'s, then to the
/// default (ADR-N-066 2.1): read whenever the grant lists the action `readVersions`.
fn public_personal_versions(spec: &Value, marks: &crate::personal_data::Marks) -> Vec<Finding> {
    let mut out = Vec::new();
    for (at, holder) in holders(spec) {
        let public = holder["effect"].as_str() != Some("prohibition")
            && holder["assignee"]["kind"].as_str() == Some("role")
            && holder["assignee"]["id"].as_str() == Some("public");
        if !public {
            continue;
        }
        let default_reads = holder["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|a| a.as_str() == Some("readVersions")));
        for (at, info) in entries_of(at, holder) {
            let caps = info["attributes"].as_object();
            let listed: Vec<&str> = ["propertyNames", "relationshipNames"]
                .iter()
                .flat_map(|list| info[*list].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .collect();
            let personal: BTreeSet<&str> = entry_types(info)
                .iter()
                .filter_map(|t| marks.get(*t))
                .flatten()
                .map(String::as_str)
                .filter(|name| listed.is_empty() || listed.contains(name))
                .collect();
            let versions = |key: &str| caps.and_then(|c| c.get(key)?["versions"].as_array());
            for name in personal {
                let (path, reads) = match versions(name)
                    .map(|v| (name, v))
                    .or_else(|| versions("*").map(|v| ("*", v)))
                {
                    Some((key, list)) => (
                        format!("{at}.attributes.{key}.versions"),
                        list.iter().any(|v| v.as_str() == Some("read")),
                    ),
                    None => (format!("{at}.attributes"), default_reads),
                };
                if reads {
                    out.push(Finding {
                        level: Level::Error,
                        path,
                        message: format!(
                            "'{name}' is personal data in the model, so a public grant never \
                             reads its versions; set `{name}: {{versions: []}}` or drop read"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// The findings for `spec` against the parsed LinkML `linkml` of the DataModel `model`.
fn unknown_names(spec: &Value, linkml: &serde_norway::Value, model: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (at, info) in entries(spec) {
        let types = entry_types(info);
        let mut known: BTreeSet<String> = CORE.iter().map(|name| (*name).to_owned()).collect();
        let mut judged = !types.is_empty();
        for entity_type in &types {
            match class_slots(linkml, entity_type) {
                Some(slots) => known.extend(slots),
                None => judged = false,
            }
        }
        if !judged {
            continue;
        }
        for list in ["propertyNames", "relationshipNames"] {
            for (n, name) in info[list].as_array().into_iter().flatten().enumerate() {
                let Some(name) = name.as_str() else { continue };
                // An expanded or compact IRI is no slot name to look up.
                if name.contains(':') || known.contains(name) {
                    continue;
                }
                out.push(unknown(
                    format!("{at}.{list}.{n}"),
                    name,
                    &types,
                    &known,
                    model,
                ));
            }
        }
        // A capability key names an Attribute too (ADR-N-066, DM-148).
        for name in info["attributes"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            if name == "*" || name.contains(':') || known.contains(name) {
                continue;
            }
            out.push(unknown(
                format!("{at}.attributes.{name}"),
                name,
                &types,
                &known,
                model,
            ));
        }
    }
    out
}

/// The error for `name`, which no type of `types` declares.
fn unknown(
    path: String,
    name: &str,
    types: &[&str],
    known: &BTreeSet<String>,
    model: &str,
) -> Finding {
    Finding {
        level: Level::Error,
        path,
        message: format!(
            "'{name}' is no Attribute of {} in the DataModel {model}; name one of: {}",
            types.join(", "),
            known.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The slots and inline attributes of `class` and of every class it reaches by `is_a` and
/// `mixins`, all in this source; `None` when one of them is not defined here (imported).
fn class_slots(linkml: &serde_norway::Value, class: &str) -> Option<BTreeSet<String>> {
    let names = |value: Option<&serde_norway::Value>| -> Vec<String> {
        match value {
            Some(serde_norway::Value::Sequence(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            Some(serde_norway::Value::Mapping(entries)) => entries
                .keys()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect(),
            Some(serde_norway::Value::String(one)) => vec![one.clone()],
            _ => Vec::new(),
        }
    };
    let mut slots = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut todo = vec![class.to_owned()];
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        // `Entity` is the NGSI-LD core class every model imports; its Attributes are CORE.
        if name == "Entity" {
            continue;
        }
        let definition = linkml
            .get("classes")
            .and_then(|classes| classes.get(&name))?;
        slots.extend(names(definition.get("slots")));
        slots.extend(names(definition.get("attributes")));
        todo.extend(names(definition.get("is_a")));
        todo.extend(names(definition.get("mixins")));
    }
    Some(slots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MODEL: &str = r#"
classes:
  Located: { slots: [address] }
  OnStreetParking:
    is_a: Entity
    mixins: [Located]
    slots: [availableSpotNumber, refParkingGroup]
    attributes: { stewardNote: { range: string } }
  Imported:
    is_a: SomeImportedClass
    slots: [x]
"#;

    fn model() -> serde_norway::Value {
        serde_norway::from_str(MODEL).expect("the test model parses")
    }

    #[test]
    fn a_name_the_class_lacks_is_an_error_on_its_field() {
        let spec = json!({
            "information": [{
                "entities": [{"type": "OnStreetParking"}],
                "propertyNames": ["availableSpotNumber", "noSuchAttribute", "address", "location", "stewardNote"],
                "relationshipNames": ["refParkingGroup", "refNowhere"],
            }],
        });
        let found = unknown_names(&spec, &model(), "parking");
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "spec.information.0.propertyNames.1",
                "spec.information.0.relationshipNames.1"
            ]
        );
        assert!(found.iter().all(|f| f.level == Level::Error));
        assert!(
            found[0].message.contains("'noSuchAttribute'"),
            "{}",
            found[0].message
        );
        assert!(
            found[0].message.contains("availableSpotNumber"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn each_rule_is_checked_under_its_own_path() {
        let spec = json!({
            "rules": [
                {"information": [{"entities": [{"type": "OnStreetParking"}], "propertyNames": ["address"]}]},
                {"information": [{"entities": [{"type": "OnStreetParking"}], "propertyNames": ["typo"]}]},
            ],
        });
        assert!(has_names(&spec));
        let found = unknown_names(&spec, &model(), "parking");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "spec.rules.1.information.0.propertyNames.0");
    }

    #[test]
    fn what_the_source_cannot_judge_passes() {
        let spec = json!({
            "information": [
                // a class another model defines
                {"entities": [{"type": "AirQualityObserved"}], "propertyNames": ["pm10"]},
                // a class whose parent another model defines
                {"entities": [{"type": "Imported"}], "propertyNames": ["inherited"]},
                // an IRI, and an entry of two types where one is judged
                {"entities": [{"type": "OnStreetParking"}, {"type": "Elsewhere"}], "propertyNames": ["other"]},
                {"entities": [{"type": "OnStreetParking"}], "propertyNames": ["https://smartdatamodels.org/name"]},
            ],
        });
        assert_eq!(unknown_names(&spec, &model(), "parking"), Vec::new());
        assert!(!has_names(
            &json!({"information": [{"entities": [{"type": "OnStreetParking"}]}]})
        ));
    }

    #[test]
    fn an_endpoint_audit_is_named_superseded() {
        let audited = json!({"kind": "Endpoint", "spec": {"audit": "changes"}});
        let found = superseded_audit(&audited).expect("a warning");
        assert_eq!(
            (found.level, found.path.as_str()),
            (Level::Warning, "spec.audit")
        );
        assert!(
            found.message.contains("versions: [keep]"),
            "{}",
            found.message
        );
        assert_eq!(
            superseded_audit(&json!({"kind": "Endpoint", "spec": {}})),
            None
        );
        assert_eq!(
            superseded_audit(&json!({"kind": "Policy", "spec": {"audit": "changes"}})),
            None
        );
    }

    #[test]
    fn a_capability_key_the_class_lacks_is_an_error_on_its_key() {
        let spec = json!({
            "information": [{
                "entities": [{"type": "OnStreetParking"}],
                "attributes": {"*": {"read": true}, "address": {"history": true}, "typo": {"read": false}},
            }],
        });
        assert!(has_names(&spec));
        let found = unknown_names(&spec, &model(), "parking");
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["spec.information.0.attributes.typo"]);
    }

    fn marks() -> crate::personal_data::Marks {
        crate::personal_data::marks(
            "classes:\n  OnStreetParking:\n    attributes:\n      ownerName:\n        annotations: { personalData: true }\n      address: {}\n",
        )
    }

    #[test]
    fn a_public_grant_never_reads_the_versions_of_personal_data() {
        let public = |attributes: Value| {
            json!({
                "assignee": {"kind": "role", "id": "public"},
                "information": [{"entities": [{"type": "OnStreetParking"}], "attributes": attributes}],
            })
        };
        assert_eq!(
            marks().get("OnStreetParking").map(|m| m.len()),
            Some(1),
            "{:?}",
            marks()
        );
        let found =
            public_personal_versions(&public(json!({"*": {"versions": ["read"]}})), &marks());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "spec.information.0.attributes.*.versions");
        assert!(
            found[0].message.contains("'ownerName'"),
            "{}",
            found[0].message
        );
        let own = public_personal_versions(
            &public(json!({"ownerName": {"versions": ["read"]}})),
            &marks(),
        );
        assert_eq!(
            own[0].path,
            "spec.information.0.attributes.ownerName.versions"
        );
        // The attribute's own empty list narrows `*`; a non-personal attribute stays public.
        let narrowed = json!({"*": {"versions": ["read"]}, "ownerName": {"versions": []}});
        assert_eq!(
            public_personal_versions(&public(narrowed), &marks()),
            Vec::new()
        );
        let other = json!({"address": {"versions": ["read"]}});
        assert_eq!(
            public_personal_versions(&public(other), &marks()),
            Vec::new()
        );
        // An internal grant may read them.
        let mut internal = public(json!({"*": {"versions": ["read"]}}));
        internal["assignee"]["id"] = json!("city-staff");
        assert_eq!(public_personal_versions(&internal, &marks()), Vec::new());
        // A rule names its own assignee.
        let ruled = json!({
            "rules": [{
                "assignee": {"kind": "role", "id": "public"},
                "information": [{"entities": [{"type": "OnStreetParking"}], "attributes": {"*": {"versions": ["read"]}}}],
            }],
        });
        let found = public_personal_versions(&ruled, &marks());
        assert_eq!(
            found[0].path,
            "spec.rules.0.information.0.attributes.*.versions"
        );
        // Without a versions key the default reads when the grant lists readVersions.
        let mut bare = public(Value::Null);
        bare["information"][0]
            .as_object_mut()
            .map(|info| info.remove("attributes"));
        bare["actions"] = json!(["readVersions"]);
        let found = public_personal_versions(&bare, &marks());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "spec.information.0.attributes");
        let mut history = public(json!({"*": {"history": true}}));
        history["actions"] = json!(["readVersions"]);
        let found = public_personal_versions(&history, &marks());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "spec.information.0.attributes");
        // An explicit list without read wins over the default.
        let mut narrowed = public(json!({"*": {"versions": []}}));
        narrowed["actions"] = json!(["readVersions"]);
        assert_eq!(public_personal_versions(&narrowed, &marks()), Vec::new());
        // Without readVersions the default reads nothing.
        assert_eq!(
            public_personal_versions(&public(json!({"*": {"history": true}})), &marks()),
            Vec::new()
        );
    }

    #[test]
    fn a_public_read_versions_grant_naming_only_types_is_judged() {
        let spec = json!({
            "assignee": {"kind": "role", "id": "public"},
            "actions": ["readVersions"],
            "information": [{"entities": [{"type": "OnStreetParking"}]}],
        });
        let source = "classes:\n  OnStreetParking:\n    attributes:\n      ownerName:\n        annotations: { personalData: true }\n      address: {}\n";
        let found = of_model(&spec, "parking", source);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "spec.information.0.attributes");
        // Without readVersions and names nothing is judged.
        let mut plain = spec.clone();
        plain["actions"] = json!([]);
        assert!(!needs_model(&plain));
        assert_eq!(of_model(&plain, "parking", source), Vec::new());
    }
}
