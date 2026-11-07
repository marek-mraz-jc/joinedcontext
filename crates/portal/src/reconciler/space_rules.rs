//! The QualityRule and RetentionRule kinds of a Context Space as the broker reads them (DM-82,
//! DM-85, DM-90, DM-91, DM-97, DM-108; T-4772 split them out of `views`): the retention and
//! quality documents, each rule under every IRI its class is written as, the retention counts
//! folded back into classes, and the quarantine narrowing of a space's permits.
//!
//! Pure functions of the manifests and the model's JSON-LD context: no I/O, no Secret. `views`
//! writes what they return.

use jc_core::kinds::{QualityRule, RetentionRule};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Whether a quality rule can quarantine: its own outcome, or a `maxAge` the sweep applies
/// (DM-87, DM-90).
pub(super) fn quarantines(rule: &QualityRule) -> bool {
    use jc_core::kinds::QualityOutcome::Quarantine;
    rule.write_outcome() == Quarantine
        || (rule.max_age.is_some() && rule.sweep_outcome() == Quarantine)
}

/// The roles that still see a quarantined entity, to act on it (DM-90).
const SEE_QUARANTINE: &[&str] = &["owner", "steward"];

/// Narrows one permit to entities that are not quarantined (DM-90): `!quarantinedAt` AND-ed
/// before its own `q`. A prohibition, and a permit of the owner or steward role, stay as they
/// are; quarantine never widens anything.
pub(super) fn hide_quarantined(policy: &mut Value) {
    let role = policy["assignee"]["kind"] == "role"
        && policy["assignee"]["id"]
            .as_str()
            .is_some_and(|id| SEE_QUARANTINE.contains(&id));
    if policy["effect"] != "permit" || role {
        return;
    }
    let q = match policy["q"].as_str() {
        Some(own) if !own.trim().is_empty() => format!("!quarantinedAt;({own})"),
        _ => "!quarantinedAt".to_owned(),
    };
    policy["q"] = json!(q);
}

/// How many rows of one class retention deletes, or would (DM-85).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionCount {
    /// The class of the space's model.
    pub entity_type: String,
    /// `entity` for current state, `history` for temporal instances.
    pub kind: String,
    /// How many.
    pub count: u64,
}

/// The `entityType` of a space's default retention rule, `*` (DM-97); jc-core's
/// `RETENTION_ANY_TYPE` from the pin that accepts it.
pub(super) const RETENTION_ANY_TYPE: &str = "*";

/// The broker's retention document of one space (DM-82, ADR-N-046): each rule under every IRI
/// its type is written as, the NGSI-LD core context's and the space's model's, once.
pub(super) fn retention_document(rules: &[RetentionRule], context: Option<&Value>) -> Value {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for rule in rules {
        // The space's default is no class: the broker reads `*` as it is (DM-97).
        let iris = if rule.entity_type == RETENTION_ANY_TYPE {
            vec![RETENTION_ANY_TYPE.to_owned()]
        } else {
            type_iris(&rule.entity_type, context)
        };
        for iri in iris {
            if seen.insert(iri.clone()) {
                let mut one = json!({ "type": iri });
                if let Some(current) = &rule.current {
                    one["current"] = json!(current);
                }
                // `forever` reaches the broker as written, so the space's `*` rule and the
                // platform default leave the type alone (DM-97); the `reason` stays in the manifest.
                if let Some(history) = &rule.history {
                    one["history"] = json!(history);
                }
                record_into(&mut one, rule, context);
                out.push(one);
            }
        }
    }
    json!({ "rules": out })
}

/// A rule's recording as the broker reads it (DM-108, T-4669): `record`, `heartbeat`,
/// `deadband` and `minDistance` as written, and each attribute's under every IRI it is written as.
fn record_into(one: &mut Value, rule: &RetentionRule, context: Option<&Value>) {
    let own = json!({ "record": rule.record, "heartbeat": rule.heartbeat,
                      "deadband": rule.deadband, "minDistance": rule.min_distance });
    for (key, value) in own.as_object().into_iter().flatten() {
        if !value.is_null() {
            one[key] = value.clone();
        }
    }
    let attributes: serde_json::Map<String, Value> = rule
        .attributes
        .iter()
        .flat_map(|(name, recording)| {
            let recording = serde_json::to_value(recording).unwrap_or_default();
            type_iris(name, context)
                .into_iter()
                .map(move |iri| (iri, recording.clone()))
        })
        .collect();
    if !attributes.is_empty() {
        one["attributes"] = Value::Object(attributes);
    }
}

/// The broker's quality document of one space (DM-91, ADR-N-046 2.5): each `maxAge` rule
/// under every IRI its type and attribute are written as, the core context's and the model's;
/// the broker judges a rule's attributes of one type together, so an entity written under one
/// IRI is never lifted by the other.
pub(super) fn quality_document(rules: &[QualityRule], context: Option<&Value>) -> Value {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for rule in rules {
        let Some(max_age) = &rule.max_age else {
            continue;
        };
        for type_iri in type_iris(&rule.entity_type, context) {
            for attribute in type_iris(&rule.attribute, context) {
                if seen.insert((rule.name.clone(), type_iri.clone(), attribute.clone())) {
                    out.push(json!({ "name": rule.name, "type": type_iri,
                                     "attribute": attribute, "maxAge": max_age }));
                }
            }
        }
    }
    json!({ "rules": out })
}

/// The IRIs a class name is written as: under the NGSI-LD core context, and under the model's
/// JSON-LD `@context` (its term, a CURIE expanded by the context's prefix, else `@vocab`).
pub(super) fn type_iris(name: &str, context: Option<&Value>) -> Vec<String> {
    let absolute = |iri: &str| iri.contains("://") || iri.starts_with("urn:");
    let mut out = vec![format!(
        "https://uri.etsi.org/ngsi-ld/default-context/{name}"
    )];
    let inner = context.map(|c| c.get("@context").unwrap_or(c));
    let objects: Vec<&Map<String, Value>> = match inner {
        Some(Value::Object(one)) => vec![one],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    };
    let id = |definition: &Value| -> Option<String> {
        definition
            .as_str()
            .or_else(|| definition.get("@id").and_then(Value::as_str))
            .map(str::to_owned)
    };
    for terms in objects {
        let defined = terms
            .get(name)
            .and_then(id)
            .and_then(|iri| match iri.split_once(':') {
                _ if absolute(&iri) => Some(iri),
                Some((prefix, local)) => terms
                    .get(prefix)
                    .and_then(id)
                    .filter(|base| absolute(base))
                    .map(|base| format!("{base}{local}")),
                None => None,
            });
        let vocab = terms
            .get("@vocab")
            .and_then(Value::as_str)
            .filter(|v| absolute(v));
        match (defined, vocab) {
            (Some(iri), _) => out.push(iri),
            (None, Some(vocab)) if !terms.contains_key(name) => out.push(format!("{vocab}{name}")),
            _ => {}
        }
    }
    out
}

/// The broker's retention count `answer` folded back into the space's classes (DM-85, DM-97):
/// each IRI a rule of `rules` (else `held`) or `held` is written as names its class, the
/// platform default's `@platform` row is the space's `*` row, an unknown IRI keeps its last
/// segment; counts of one class and kind add up, saturating.
pub(super) fn retention_rows(
    answer: &Value,
    rules: Option<&[RetentionRule]>,
    held: &[RetentionRule],
    context: Option<&Value>,
) -> Vec<RetentionCount> {
    let classes: BTreeMap<String, &str> = rules
        .unwrap_or(held)
        .iter()
        .chain(held)
        .flat_map(|rule| {
            type_iris(&rule.entity_type, context)
                .into_iter()
                .map(|iri| (iri, rule.entity_type.as_str()))
        })
        .collect();
    let mut out: BTreeMap<(String, String), u64> = BTreeMap::new();
    for row in answer["counts"].as_array().into_iter().flatten() {
        let iri = match row["type"].as_str().unwrap_or_default() {
            "@platform" => RETENTION_ANY_TYPE,
            iri => iri,
        };
        let class = classes.get(iri).map_or_else(
            || iri.rsplit(['/', '#']).next().unwrap_or(iri).to_owned(),
            |class| (*class).to_owned(),
        );
        let kind = row["kind"].as_str().unwrap_or_default().to_owned();
        let total = out.entry((class, kind)).or_default();
        *total = total.saturating_add(row["count"].as_u64().unwrap_or_default());
    }
    out.into_iter()
        .map(|((entity_type, kind), count)| RetentionCount {
            entity_type,
            kind,
            count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_is_named_by_the_core_context_and_by_the_models() {
        let core = "https://uri.etsi.org/ngsi-ld/default-context/Bus";
        assert_eq!(type_iris("Bus", None), [core]);
        for (context, model) in [
            (
                json!({"@context": {"Bus": "https://e.org/Bus"}}),
                Some("https://e.org/Bus"),
            ),
            (
                json!({"@context": {"x": {"@id": "https://e.org/x/", "@prefix": true},
                                 "Bus": {"@id": "x:Bus"}}}),
                Some("https://e.org/x/Bus"),
            ),
            (
                json!({"@context": [core, {"@vocab": "https://e.org/v/"}]}),
                Some("https://e.org/v/Bus"),
            ),
            (
                json!({"@context": {"Bus": {"@id": "nope:Bus"}, "@vocab": "https://e.org/v/"}}),
                None,
            ),
            (json!({"@context": {"Bus": {"@id": "Bus"}}}), None),
            (json!({"@context": "https://e.org/ctx.jsonld"}), None),
        ] {
            let mut want = vec![core.to_owned()];
            want.extend(model.map(str::to_owned));
            assert_eq!(type_iris("Bus", Some(&context)), want, "{context}");
        }
        let rules = [
            RetentionRule {
                entity_type: "Bus".into(),
                current: None,
                history: Some("PT8H".into()),
                reason: None,
                ..Default::default()
            },
            RetentionRule {
                entity_type: "Bus".into(),
                current: Some("PT1M".into()),
                history: None,
                reason: None,
                ..Default::default()
            },
        ];
        assert_eq!(
            retention_document(&rules, None),
            json!({"rules": [{"type": core, "history": "PT8H"}]}),
            "one rule per IRI, the first wins"
        );
    }

    /// `forever`, `off` and a duration reach the broker as written, so the space's `*` rule and
    /// the platform default leave a type kept forever alone; `*` is sent as itself, once
    /// (DM-82, DM-97, T-4275).
    #[test]
    fn forever_and_the_spaces_default_reach_the_broker_as_written() {
        let rule = |entity_type: &str, current: Option<&str>, history: &str| RetentionRule {
            entity_type: entity_type.into(),
            current: current.map(Into::into),
            history: Some(history.into()),
            reason: (history == "forever").then(|| "the archive is the record".into()),
            ..Default::default()
        };
        let rules = [
            rule("Bus", Some("P1D"), "forever"),
            rule("Stop", Some("P1D"), "off"),
            rule("Air", Some("P1D"), "P30D"),
            rule("*", None, "P90D"),
        ];
        let core = |name: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{name}");
        let context = json!({"@context": {"Air": "https://e.org/Air"}});
        assert_eq!(
            retention_document(&rules, Some(&context)),
            json!({"rules": [
                {"type": core("Bus"), "current": "P1D", "history": "forever"},
                {"type": core("Stop"), "current": "P1D", "history": "off"},
                {"type": core("Air"), "current": "P1D", "history": "P30D"},
                {"type": "https://e.org/Air", "current": "P1D", "history": "P30D"},
                {"type": "*", "history": "P90D"},
            ]})
        );
    }

    /// A rule's recording reaches the broker as written, each attribute under every IRI it is
    /// written as; a rule without one sends none (DM-108, T-4669).
    #[test]
    fn a_rules_recording_reaches_the_broker_with_its_attributes_under_both_iris() {
        let rule: RetentionRule = serde_json::from_value(json!({
            "entityType": "Spot", "history": "P90D", "record": "onChange", "heartbeat": "PT1H",
            "deadband": 0.5, "attributes": {"count": {"record": "every"}},
        }))
        .expect("a rule");
        let plain = RetentionRule {
            entity_type: "Bus".into(),
            history: Some("P30D".into()),
            ..Default::default()
        };
        let core = |name: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{name}");
        let context =
            json!({"@context": {"Spot": "https://e.org/Spot", "count": "https://e.org/count"}});
        let attributes =
            json!({core("count"): {"record": "every"}, "https://e.org/count": {"record": "every"}});
        let spot = |iri: String| {
            json!({"type": iri, "history": "P90D", "record": "onChange",
                                        "heartbeat": "PT1H", "deadband": 0.5, "attributes": attributes})
        };
        assert_eq!(
            retention_document(&[rule, plain], Some(&context)),
            json!({"rules": [
                spot(core("Spot")),
                spot("https://e.org/Spot".into()),
                {"type": core("Bus"), "history": "P30D"},
            ]})
        );
    }

    #[test]
    fn a_rule_quarantines_by_its_outcome_or_its_max_age() {
        let rule = |extra: Value| -> jc_core::kinds::QualityRule {
            let mut base = json!({"name": "r", "entityType": "Vehicle", "attribute": "speed"});
            if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
                b.extend(e.clone());
            }
            serde_json::from_value(base).expect("rule")
        };
        assert!(!quarantines(&rule(json!({"kind": "range", "max": 1}))));
        assert!(quarantines(&rule(
            json!({"kind": "range", "max": 1, "outcome": "quarantine"})
        )));
        assert!(quarantines(&rule(
            json!({"kind": "freshness", "maxAge": "PT5M"})
        )));
        assert!(!quarantines(&rule(
            json!({"kind": "freshness", "maxAge": "PT5M", "outcome": "flag"})
        )));
        assert!(!quarantines(&rule(
            json!({"kind": "freshness", "maxAhead": "PT30S"})
        )));
    }

    fn quality(extra: Value) -> QualityRule {
        let mut base = json!({"name": "fresh", "entityType": "Bus", "attribute": "speed",
                              "kind": "freshness"});
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        serde_json::from_value(base).expect("rule")
    }

    /// DM-91: only a `maxAge` rule reaches the broker, under every IRI pair of its class and
    /// attribute, once per name; a rule without one sends nothing (T-4772).
    #[test]
    fn a_max_age_rule_reaches_the_broker_under_every_iri_pair_once() {
        let core = |name: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{name}");
        let context = json!({"@context": {"Bus": "https://e.org/Bus"}});
        let aged = quality(json!({"maxAge": "PT5M"}));
        let rules = [
            aged.clone(),
            aged,
            quality(json!({"name": "ahead", "maxAhead": "PT30S"})),
        ];
        let one = |ty: String| {
            json!({"name": "fresh", "type": ty, "attribute": core("speed"),
                                        "maxAge": "PT5M"})
        };
        assert_eq!(
            quality_document(&rules, Some(&context)),
            json!({"rules": [one(core("Bus")), one("https://e.org/Bus".into())]})
        );
        assert_eq!(quality_document(&rules[2..], None), json!({"rules": []}));
    }

    /// DM-90: the owner and the steward role keep seeing a quarantined entity; a role of
    /// another name, a principal named `owner`, and a blank `q` are narrowed (T-4772).
    #[test]
    fn hiding_quarantined_spares_only_the_owner_and_steward_roles() {
        let narrowed = |policy: Value| {
            let mut policy = policy;
            hide_quarantined(&mut policy);
            policy["q"].clone()
        };
        for id in ["owner", "steward"] {
            let role = json!({"effect": "permit", "assignee": {"kind": "role", "id": id}});
            assert_eq!(narrowed(role), Value::Null, "{id}");
        }
        let viewer = json!({"effect": "permit", "assignee": {"kind": "role", "id": "viewer"}});
        assert_eq!(narrowed(viewer), json!("!quarantinedAt"));
        let user = json!({"effect": "permit", "assignee": {"kind": "user", "id": "owner"}});
        assert_eq!(narrowed(user), json!("!quarantinedAt"));
        let blank = json!({"effect": "permit", "q": "  "});
        assert_eq!(narrowed(blank), json!("!quarantinedAt"));
        let deny = json!({"effect": "prohibition", "q": "a==1"});
        assert_eq!(narrowed(deny), json!("a==1"));
    }

    /// DM-85, DM-97: the broker's IRIs fold back into the rules' classes, the `@platform` row
    /// into the space's `*`, an unknown IRI into its last segment after `/` or `#`; one class
    /// and kind adds up and saturates; without draft rules the held ones name the classes.
    #[test]
    fn retention_rows_fold_iris_into_classes_and_add_up() {
        let rule = |entity_type: &str| RetentionRule {
            entity_type: entity_type.into(),
            history: Some("P1D".into()),
            ..Default::default()
        };
        let context = json!({"@context": {"Bus": "https://e.org/Bus", "Air": "https://e.org/Air"}});
        let answer = json!({"counts": [
            {"type": "https://e.org/Bus", "kind": "history", "count": 2},
            {"type": "https://uri.etsi.org/ngsi-ld/default-context/Bus", "kind": "history",
             "count": 3},
            {"type": "https://e.org/Air", "kind": "entity", "count": 1},
            {"type": "@platform", "kind": "history", "count": 4},
            {"type": "https://x.org/v#Tram", "kind": "entity", "count": u64::MAX},
            {"type": "https://x.org/v#Tram", "kind": "entity", "count": 1},
        ]});
        let row = |entity_type: &str, kind: &str, count: u64| RetentionCount {
            entity_type: entity_type.into(),
            kind: kind.into(),
            count,
        };
        let draft = [rule("Bus")];
        let held = [rule("Air")];
        assert_eq!(
            retention_rows(&answer, Some(&draft), &held, Some(&context)),
            [
                row("*", "history", 4),
                row("Air", "entity", 1),
                row("Bus", "history", 5),
                row("Tram", "entity", u64::MAX),
            ]
        );
        let held_only = retention_rows(&answer, None, &[rule("Bus")], Some(&context));
        assert!(
            held_only.contains(&row("Bus", "history", 5)),
            "{held_only:?}"
        );
        assert!(
            held_only.contains(&row("Air", "entity", 1)),
            "{held_only:?}"
        );
        assert!(retention_rows(&json!({}), None, &[], None).is_empty());
    }
}
