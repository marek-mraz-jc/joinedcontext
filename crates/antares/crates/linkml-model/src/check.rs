// SPDX-License-Identifier: EUPL-1.2
//! Whether an NGSI-LD Entity or fragment, in the expanded form the broker
//! stores (Attribute names as IRIs, `type` a list of IRIs, each Attribute a
//! list of instances), keeps to a Tenant's model (T-5912): per Attribute its
//! kind, its range (a built-in type, an enum, a class), `multivalued` and its
//! UN/CEFACT unit; per class its `required` slots when the write sets the
//! whole Entity. Every write surface reaches the store through the NGSI-LD
//! routes, which call [`entity_in`], so one refusal reads the same on all.
//!
//! What the subset does not hold is not checked: `pattern`, minimum and
//! maximum, closed classes (a schema naming them is refused at load), the
//! type of a Relationship's target (that needs a read of the target; an id
//! tells nothing about it, ADR-N-041). An Attribute no class of the Entity
//! lists, and an Entity of no class of the model, pass: a LinkML class is
//! open by default.
//!
//! A refusal names the Attribute and the rule, never the value (the value
//! may be personal data, and a log line or a Problem Details carries it).
//!
//! ```
//! use linkml_model::check;
//! use serde_json::json;
//!
//! let schema: std::sync::Arc<[u8]> = br#"{"id": "https://e.org/", "name": "s",
//!   "classes": {"Room": {"slots": ["temperature"]}},
//!   "slots": {"temperature": {"range": "float", "required": true}}}"#[..].into();
//! let good = json!({"id": "urn:ngsi-ld:Room:1", "type": ["https://e.org/Room"],
//!   "https://e.org/temperature": [{"type": "Property", "value": 21.5}]});
//! assert!(check::entity_in(1, &schema, &good, true).is_ok());
//! let bad = json!({"id": "urn:ngsi-ld:Room:1", "type": ["https://e.org/Room"],
//!   "https://e.org/temperature": [{"type": "Property", "value": "warm"}]});
//! let refused = check::entity_in(1, &schema, &bad, false).unwrap_err();
//! assert_eq!(refused.attribute, "temperature");
//! assert!(!refused.to_string().contains("warm"));
//! ```

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, LazyLock};

use serde_json::Value;

use crate::model::{known_kind, Builtin, Model, Range};

/// Why a write breaks the model: the Attribute (the slot's name) and the
/// rule it breaks. Its text names no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The slot name of the Attribute.
    pub attribute: String,
    /// The class the slot belongs to.
    pub class: String,
    /// The rule, as a sentence a client can act on.
    pub rule: &'static str,
}

/// The words every [`Violation`]'s text carries: a surface that hides an
/// inner refusal's detail passes on one that holds them.
pub const MARK: &str = "breaks the data model of";

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the attribute {} {MARK} {}: {}",
            self.attribute, self.class, self.rule
        )
    }
}

/// The rules [`entity`] holds a value to (their texts, for tests and docs).
pub mod rule {
    /// The instance is of another NGSI-LD kind than the slot's.
    pub const KIND: &str = "it is of another NGSI-LD attribute kind than the model states";
    /// The value is not of the slot's built-in type.
    pub const STRING: &str = "its value must be a string";
    /// As [`STRING`].
    pub const INTEGER: &str = "its value must be an integer";
    /// As [`STRING`].
    pub const NUMBER: &str = "its value must be a number";
    /// As [`STRING`].
    pub const BOOLEAN: &str = "its value must be true or false";
    /// As [`STRING`].
    pub const DATE: &str = "its value must be a date (YYYY-MM-DD)";
    /// As [`STRING`].
    pub const DATETIME: &str = "its value must be a date-time (ISO 8601 with a time zone)";
    /// As [`STRING`].
    pub const TIME: &str = "its value must be a time of day (hh:mm:ss)";
    /// As [`STRING`].
    pub const URI: &str = "its value must be a URI";
    /// Not among the enum's permissible values.
    pub const ENUM: &str = "its value is not one of the values the model lists";
    /// A list where the model holds one value.
    pub const SINGLE: &str = "it holds one value, not a list";
    /// A Relationship without an `object` URI.
    pub const OBJECT: &str = "its object must be an Entity id (a URI)";
    /// A unit code other than the slot's.
    pub const UNIT: &str = "its unitCode differs from the unit the model states";
    /// A required slot missing from a whole-Entity write.
    pub const REQUIRED: &str = "the model requires it";
    /// A `LanguageProperty` whose texts are not strings.
    pub const LANGUAGE: &str = "its languageMap must map language tags to strings";
}

/// How a slot's values are held to their range.
#[derive(Debug)]
enum Kind {
    Type(Builtin),
    Enum(BTreeSet<String>),
    Class,
}

#[derive(Debug)]
struct SlotRule {
    name: String,
    class: String,
    /// The NGSI-LD kind, when the surfaces map it ([`known_kind`]).
    kind: Option<String>,
    range: Kind,
    multivalued: bool,
    /// The UN/CEFACT code of the slot's unit (`unece:` among its mappings).
    unit: Option<String>,
}

/// One class's rules, keyed by slot IRI, with its required slots.
#[derive(Debug, Default)]
struct ClassRules {
    slots: HashMap<String, Arc<SlotRule>>,
    required: Vec<(String, Arc<SlotRule>)>,
}

/// Every class of a model by IRI: built once per model ([`entity_in`]
/// caches it per schema revision), so a write costs map lookups.
#[derive(Debug, Default)]
pub struct Rules {
    classes: HashMap<String, ClassRules>,
}

impl Rules {
    /// The rules of every class of `model`.
    #[must_use]
    pub fn of(model: &Model) -> Self {
        let schema = model.schema();
        let mut classes = HashMap::with_capacity(schema.classes.len());
        for class in schema.classes.keys() {
            let mut rules = ClassRules::default();
            for (name, slot) in model.class_slots(class) {
                let range = match model.range(&slot) {
                    Range::Type(b) => Kind::Type(b),
                    Range::Enum(e) => Kind::Enum(
                        schema
                            .enums
                            .get(e)
                            .map(|e| e.permissible_values.keys().cloned().collect())
                            .unwrap_or_default(),
                    ),
                    Range::Class(_) => Kind::Class,
                };
                let kind = model.kind(&slot);
                let unit = slot.unit.as_ref().and_then(|u| {
                    u.exact_mappings
                        .iter()
                        .find_map(|m| m.strip_prefix("unece:"))
                        .map(str::to_owned)
                });
                let rule = Arc::new(SlotRule {
                    name: name.to_owned(),
                    class: class.clone(),
                    kind: known_kind(kind).then(|| kind.to_owned()),
                    range,
                    multivalued: slot.multivalued == Some(true),
                    unit,
                });
                let iri = model.slot_iri(name);
                // The Entity's own members are no Attributes: `id` and
                // `type` are checked by the NGSI-LD expansion itself.
                let own = slot.identifier == Some(true) || matches!(name, "id" | "type");
                if slot.required == Some(true) && !own {
                    rules.required.push((iri.clone(), Arc::clone(&rule)));
                }
                if !own {
                    rules.slots.insert(iri, rule);
                }
            }
            classes.insert(model.class_iri(class), rules);
        }
        Self { classes }
    }
}

/// [`entity`] against the schema `bytes` of `revision`, read once per
/// revision through [`crate::cache`] under [`crate::SURFACE_LIMITS`], its
/// [`Rules`] built once per read. A schema the surfaces refuse checks
/// nothing: the Tenant keeps the behaviour it had without one.
///
/// # Errors
///
/// The first [`Violation`] of `doc`.
pub fn entity_in(
    revision: u64,
    bytes: &Arc<[u8]>,
    doc: &Value,
    whole: bool,
) -> Result<(), Violation> {
    let (read, _) = crate::cache::read(revision, bytes, &crate::SURFACE_LIMITS);
    let Ok(model) = read else {
        return Ok(());
    };
    entity(&rules_of(revision, &model), doc, whole)
}

/// [`entity_in`] with the refusal as its text: the shape the broker's
/// write routes take a check in (`antares_api::schema_writes::SchemaCheck`).
///
/// # Errors
///
/// The first [`Violation`]'s text.
pub fn entity_text(
    revision: u64,
    bytes: &Arc<[u8]>,
    doc: &Value,
    whole: bool,
) -> Result<(), String> {
    entity_in(revision, bytes, doc, whole).map_err(|v| v.to_string())
}

/// Each revision's rules with the model they were built from.
type RulesCache = moka::sync::Cache<u64, (Arc<Model>, Arc<Rules>)>;

/// The rules of the model `revision` read as, built once while that read
/// stays in [`crate::cache`].
fn rules_of(revision: u64, model: &Arc<Model>) -> Arc<Rules> {
    static RULES: LazyLock<RulesCache> = LazyLock::new(|| moka::sync::Cache::new(256));
    match RULES.get(&revision) {
        Some((m, r)) if Arc::ptr_eq(&m, model) => r,
        _ => {
            let rules = Arc::new(Rules::of(model));
            RULES.insert(revision, (Arc::clone(model), Arc::clone(&rules)));
            rules
        }
    }
}

/// Whether the expanded Entity or fragment `doc` keeps to `rules`: every
/// Attribute a class of the Entity lists, and with `whole` (a create or a
/// replace) every slot those classes require. `doc`'s `type` names the
/// classes; a fragment without one checks nothing.
///
/// # Errors
///
/// The first [`Violation`], in the document's key order.
pub fn entity(rules: &Rules, doc: &Value, whole: bool) -> Result<(), Violation> {
    let types = match doc.get("type") {
        Some(Value::Array(types)) => types.as_slice(),
        Some(one) => std::slice::from_ref(one),
        None => &[],
    };
    let classes: Vec<&ClassRules> = types
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|t| rules.classes.get(t))
        .collect();
    let members = doc.as_object().into_iter().flatten();
    for (name, instances) in members {
        let Some(slot) = classes.iter().find_map(|c| c.slots.get(name)) else {
            continue;
        };
        let list = match instances {
            Value::Array(list) => list.as_slice(),
            one => std::slice::from_ref(one),
        };
        for inst in list {
            instance(slot, inst).map_err(|rule| violation(slot, rule))?;
        }
    }
    let required = classes.iter().flat_map(|c| &c.required);
    for (iri, slot) in required.filter(|_| whole) {
        if doc.get(iri).is_none() {
            return Err(violation(slot, rule::REQUIRED));
        }
    }
    Ok(())
}

fn violation(slot: &SlotRule, rule: &'static str) -> Violation {
    Violation {
        attribute: slot.name.clone(),
        class: slot.class.clone(),
        rule,
    }
}

/// The NGSI-LD Null of a merge or partial update: it deletes, so it is
/// never held to a range (5.5.12).
const NULL: &str = "urn:ngsi-ld:null";

/// `Ok` when `ok` holds, else the rule.
fn hold(ok: bool, rule: &'static str) -> Result<(), &'static str> {
    if ok {
        Ok(())
    } else {
        Err(rule)
    }
}

/// An Entity id: one URI, or a list of them.
fn ids(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.iter().all(ids),
        other => other.as_str().is_some_and(is_uri),
    }
}

fn instance(slot: &SlotRule, inst: &Value) -> Result<(), &'static str> {
    let kind = inst.get("type").and_then(Value::as_str);
    let want = slot.kind.as_deref();
    hold(kind.is_none() || want.is_none() || kind == want, rule::KIND)?;
    let unit = inst.get("unitCode").and_then(Value::as_str);
    hold(
        unit.is_none() || slot.unit.is_none() || unit == slot.unit.as_deref(),
        rule::UNIT,
    )?;
    let member = |name: &str| inst.get(name).unwrap_or(&Value::Null);
    match kind.or(want).unwrap_or("Property") {
        "Relationship" => hold(
            member("object").is_null() || ids(member("object")),
            rule::OBJECT,
        ),
        "ListRelationship" => {
            let list = member("objectList")
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            let each = |o: &Value| ids(o.get("object").unwrap_or(o));
            hold(list.iter().all(each), rule::OBJECT)
        }
        "LanguageProperty" => {
            let map = member("languageMap");
            let ok = map.is_null() || map.as_object().is_some_and(|m| m.values().all(texts));
            hold(ok, rule::LANGUAGE)
        }
        "ListProperty" => {
            let list = member("valueList")
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            list.iter().try_for_each(|v| value(slot, v))
        }
        "VocabProperty" => values(slot, member("vocab")),
        // A GeoProperty's GeoJSON and a JsonProperty's JSON are checked by
        // the expansion; the model types neither further.
        "GeoProperty" | "JsonProperty" => Ok(()),
        _ => values(slot, member("value")),
    }
}

fn texts(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.iter().all(Value::is_string),
        other => other.is_string(),
    }
}

/// One value or, on a `multivalued` slot, a list of them.
fn values(slot: &SlotRule, v: &Value) -> Result<(), &'static str> {
    match v {
        // absent (a fragment that sets only metadata) or the NGSI-LD Null
        Value::Null => Ok(()),
        Value::String(s) if s == NULL => Ok(()),
        Value::Array(items) if slot.multivalued => items.iter().try_for_each(|i| value(slot, i)),
        Value::Array(_) => Err(rule::SINGLE),
        one => value(slot, one),
    }
}

fn value(slot: &SlotRule, v: &Value) -> Result<(), &'static str> {
    // A JSON-LD typed value (`{"@type": "DateTime", "@value": ...}`) is
    // held by its `@value`.
    let v = v.get("@value").unwrap_or(v);
    match &slot.range {
        // An inlined class is an object value; a class-ranged Property
        // holding an id is a reference by value. Neither is typed further.
        Kind::Class => Ok(()),
        Kind::Enum(allowed) => hold(v.as_str().is_some_and(|s| allowed.contains(s)), rule::ENUM),
        Kind::Type(b) => builtin(*b, v),
    }
}

fn builtin(b: Builtin, v: &Value) -> Result<(), &'static str> {
    let text = v.as_str();
    let (ok, why) = match b {
        Builtin::String => (v.is_string(), rule::STRING),
        // an integer may arrive as 4.0: what JSON number parsers make of 4
        Builtin::Integer => (v.as_f64().is_some_and(whole_number), rule::INTEGER),
        Builtin::Float | Builtin::Double | Builtin::Decimal => (v.is_number(), rule::NUMBER),
        Builtin::Boolean => (v.is_boolean(), rule::BOOLEAN),
        Builtin::Date => (text.is_some_and(is_date), rule::DATE),
        Builtin::Datetime => (text.is_some_and(is_datetime), rule::DATETIME),
        Builtin::Time => (text.is_some_and(is_time), rule::TIME),
        Builtin::Uri | Builtin::Uriorcurie => (text.is_some_and(is_uri), rule::URI),
    };
    hold(ok, why)
}

/// JSON numbers are finite, so a whole one is its own truncation.
fn whole_number(f: f64) -> bool {
    f.trunc().to_bits() == f.to_bits()
}

/// `scheme:rest`, no white space: what RFC 3986 and a CURIE share.
fn is_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !s.chars().any(char::is_whitespace)
}

/// `n` ASCII digits from the front of `s`, as a number, and the rest.
fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let (head, rest) = (s.get(..n)?, s.get(n..)?);
    let number = head.bytes().try_fold(0_u32, |acc, b| {
        let digit = b
            .is_ascii_digit()
            .then(|| u32::from(b.saturating_sub(b'0')))?;
        acc.checked_mul(10)?.checked_add(digit)
    })?;
    Some((number, rest))
}

/// `YYYY-MM-DD` and the rest of `s`.
fn date(s: &str) -> Option<&str> {
    let (_, s) = digits(s, 4)?;
    let (month, s) = digits(s.strip_prefix('-')?, 2)?;
    let (day, s) = digits(s.strip_prefix('-')?, 2)?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some(s)
}

/// `hh:mm:ss` with optional fraction, and the rest of `s`.
fn clock(s: &str) -> Option<&str> {
    let (h, s) = digits(s, 2)?;
    let (m, s) = digits(s.strip_prefix(':')?, 2)?;
    let (sec, mut s) = digits(s.strip_prefix(':')?, 2)?;
    if let Some(frac) = s.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = frac.get(n..)?;
    }
    (h <= 23 && m <= 59 && sec <= 60).then_some(s)
}

/// `Z`, `±hh:mm`, or nothing when `optional`.
fn zone(s: &str, optional: bool) -> bool {
    if s.is_empty() {
        return optional;
    }
    if s == "Z" {
        return true;
    }
    let Some(rest) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) else {
        return false;
    };
    let Some((h, rest)) = digits(rest, 2) else {
        return false;
    };
    let Some((m, rest)) = rest.strip_prefix(':').and_then(|r| digits(r, 2)) else {
        return false;
    };
    rest.is_empty() && h <= 23 && m <= 59
}

fn is_date(s: &str) -> bool {
    date(s).is_some_and(str::is_empty)
}

fn is_time(s: &str) -> bool {
    clock(s).is_some_and(|rest| zone(rest, true))
}

fn is_datetime(s: &str) -> bool {
    date(s)
        .and_then(|r| r.strip_prefix('T'))
        .and_then(clock)
        .is_some_and(|rest| zone(rest, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NS: &str = "https://e.org/";

    fn rules() -> Rules {
        let json = br#"{"id": "https://e.org/", "name": "s",
          "classes": {
            "Thing": {"slots": ["name", "id"]},
            "Sensor": {"is_a": "Thing", "slots": ["pm10", "count", "on", "day", "at", "clock",
              "homepage", "level", "tags", "owner", "label", "temperature", "readings", "terms", "area",
              "odd", "inline", "crew", "doc", "type", "nominal"],
              "slot_usage": {"pm10": {"required": true}}}},
          "enums": {"Level": {"permissible_values": {"low": {}, "high": {}}}},
          "slots": {
            "id": {"identifier": true, "required": true},
            "name": {"range": "string"},
            "pm10": {"range": "float"},
            "count": {"range": "integer"},
            "on": {"range": "boolean"},
            "day": {"range": "date"},
            "at": {"range": "datetime"},
            "clock": {"range": "time"},
            "homepage": {"range": "uri"},
            "level": {"range": "Level"},
            "tags": {"range": "string", "multivalued": true},
            "owner": {"range": "Thing"},
            "label": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "LanguageProperty"}}},
            "temperature": {"range": "float", "unit": {"ucum_code": "Cel", "exact_mappings": ["unece:CEL"]}},
            "readings": {"range": "float", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
            "terms": {"range": "Level", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "VocabProperty"}}},
            "area": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
            "odd": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "TimeSeriesProperty"}}},
            "inline": {"range": "Thing", "inlined": true},
            "crew": {"range": "Thing", "multivalued": true, "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}},
            "doc": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "JsonProperty"}}},
            "type": {"required": true},
            "nominal": {"range": "float", "unit": {"ucum_code": "1"}}
          }}"#;
        Rules::of(&Model::read(json, &crate::SURFACE_LIMITS).unwrap())
    }

    fn sensor(attrs: &[(&str, Value)]) -> Value {
        let mut doc = json!({"id": "urn:ngsi-ld:Sensor:1", "type": [format!("{NS}Sensor")],
            "https://e.org/pm10": [{"type": "Property", "value": 1.5}]});
        for (name, inst) in attrs {
            doc[format!("{NS}{name}")] = json!([inst]);
        }
        doc
    }

    fn prop(v: Value) -> Value {
        let mut p = json!({"type": "Property"});
        p["value"] = v;
        p
    }

    fn refused(attrs: &[(&str, Value)]) -> Option<&'static str> {
        entity(&rules(), &sensor(attrs), false)
            .err()
            .map(|v| v.rule)
    }

    #[test]
    fn each_range_takes_its_type_and_refuses_another() {
        let table: [(&str, Value, Value, &str); 9] = [
            ("name", json!("Kiosk"), json!(7), rule::STRING),
            ("pm10", json!(12), json!("abc"), rule::NUMBER),
            ("count", json!(3), json!(2.5), rule::INTEGER),
            ("on", json!(true), json!("yes"), rule::BOOLEAN),
            ("day", json!("2026-10-28"), json!("2026-13-01"), rule::DATE),
            (
                "at",
                json!("2026-10-28T10:00:00.5Z"),
                json!("2026-10-28T10:00:00"),
                rule::DATETIME,
            ),
            (
                "clock",
                json!("23:59:59+01:00"),
                json!("24:00:00"),
                rule::TIME,
            ),
            (
                "homepage",
                json!("https://e.org/x"),
                json!("not a uri"),
                rule::URI,
            ),
            ("level", json!("low"), json!("medium"), rule::ENUM),
        ];
        for (name, good, bad, why) in table {
            assert_eq!(refused(&[(name, prop(good))]), None, "{name} good");
            assert_eq!(refused(&[(name, prop(bad))]), Some(why), "{name} bad");
        }
    }

    #[test]
    fn a_typed_date_time_counts_by_its_value() {
        let typed = json!({"@type": "DateTime", "@value": "2026-10-28T10:00:00Z"});
        assert_eq!(refused(&[("at", prop(typed))]), None);
        assert_eq!(refused(&[("count", prop(json!(4.0)))]), None);
    }

    #[test]
    fn multivalued_takes_a_list_and_a_single_slot_refuses_one() {
        assert_eq!(refused(&[("tags", prop(json!(["a", "b"])))]), None);
        assert_eq!(
            refused(&[("tags", prop(json!(["a", 2])))]),
            Some(rule::STRING)
        );
        assert_eq!(
            refused(&[("name", prop(json!(["a", "b"])))]),
            Some(rule::SINGLE)
        );
    }

    #[test]
    fn the_kind_must_be_the_models() {
        let rel = json!({"type": "Relationship", "object": "urn:ngsi-ld:Thing:1"});
        assert_eq!(refused(&[("owner", rel)]), None);
        assert_eq!(refused(&[("owner", prop(json!("x")))]), Some(rule::KIND));
        assert_eq!(
            refused(&[("pm10", json!({"type": "Relationship", "object": "urn:x:1"}))]),
            Some(rule::KIND)
        );
        let not_uri = json!({"type": "Relationship", "object": "a b"});
        assert_eq!(refused(&[("owner", not_uri)]), Some(rule::OBJECT));
    }

    #[test]
    fn the_other_kinds_hold_their_members() {
        let lang =
            json!({"type": "LanguageProperty", "languageMap": {"en": "Room", "sk": ["Izba"]}});
        assert_eq!(refused(&[("label", lang)]), None);
        let bad = json!({"type": "LanguageProperty", "languageMap": {"en": 3}});
        assert_eq!(refused(&[("label", bad)]), Some(rule::LANGUAGE));
        let list = json!({"type": "ListProperty", "valueList": [1, 2.5]});
        assert_eq!(refused(&[("readings", list)]), None);
        let list = json!({"type": "ListProperty", "valueList": [1, "x"]});
        assert_eq!(refused(&[("readings", list)]), Some(rule::NUMBER));
        let vocab = json!({"type": "VocabProperty", "vocab": "high"});
        assert_eq!(refused(&[("terms", vocab)]), None);
        let vocab = json!({"type": "VocabProperty", "vocab": "huge"});
        assert_eq!(refused(&[("terms", vocab)]), Some(rule::ENUM));
        let geo = json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1, 2]}});
        assert_eq!(refused(&[("area", geo)]), None);
    }

    #[test]
    fn a_unit_other_than_the_models_is_refused() {
        let cel = json!({"type": "Property", "value": 20, "unitCode": "CEL"});
        let fah = json!({"type": "Property", "value": 68, "unitCode": "FAH"});
        assert_eq!(refused(&[("temperature", cel)]), None);
        assert_eq!(refused(&[("temperature", prop(json!(20)))]), None);
        assert_eq!(refused(&[("temperature", fah)]), Some(rule::UNIT));
    }

    #[test]
    fn required_holds_only_for_a_whole_entity() {
        let r = rules();
        let mut doc = sensor(&[]);
        doc.as_object_mut().unwrap().remove("https://e.org/pm10");
        let v = entity(&r, &doc, true).unwrap_err();
        assert_eq!((v.attribute.as_str(), v.rule), ("pm10", rule::REQUIRED));
        assert!(
            entity(&r, &doc, false).is_ok(),
            "a fragment may leave it out"
        );
        // `id` is required and an identifier: the Entity's own member.
        assert!(entity(&r, &sensor(&[]), true).is_ok());
    }

    #[test]
    fn open_by_default_and_null_deletes() {
        assert_eq!(refused(&[("unlisted", prop(json!(1)))]), None);
        let other = json!({"id": "urn:ngsi-ld:X:1", "type": ["https://o.org/X"],
            "https://e.org/pm10": [{"type": "Property", "value": "abc"}]});
        assert!(entity(&rules(), &other, true).is_ok());
        let untyped = json!({"https://e.org/pm10": [{"type": "Property", "value": "abc"}]});
        assert!(entity(&rules(), &untyped, false).is_ok());
        assert_eq!(refused(&[("pm10", prop(json!("urn:ngsi-ld:null")))]), None);
    }

    #[test]
    fn a_refusal_names_the_attribute_and_never_the_value() {
        let v = entity(
            &rules(),
            &sensor(&[("pm10", prop(json!("s3cr3t-value")))]),
            false,
        )
        .unwrap_err();
        let text = v.to_string();
        assert!(text.contains("pm10"), "{text}");
        assert!(text.contains("Sensor"), "{text}");
        assert!(!text.contains("s3cr3t"), "{text}");
    }

    #[test]
    fn a_refused_schema_checks_nothing_and_rules_are_cached() {
        let bad: Arc<[u8]> = b"{not json"[..].into();
        let doc = sensor(&[("pm10", prop(json!("abc")))]);
        assert!(entity_in(5_912_001, &bad, &doc, true).is_ok());
        let json: Arc<[u8]> = br#"{"id": "https://e.org/", "name": "s",
          "classes": {"Sensor": {"slots": ["pm10"]}}, "slots": {"pm10": {"range": "float"}}}"#[..]
            .into();
        assert!(entity_in(5_912_002, &json, &doc, false).is_err());
        let (read, _) = crate::cache::read(5_912_002, &json, &crate::SURFACE_LIMITS);
        let model = read.unwrap();
        assert!(Arc::ptr_eq(
            &rules_of(5_912_002, &model),
            &rules_of(5_912_002, &model)
        ));
    }

    #[test]
    fn date_and_time_parsers_hold_their_edges() {
        let uris = [
            ("urn:ngsi-ld:Sensor:1", true),
            ("a+b-c.d:x", true),
            ("noscheme", false),
            (":x", false),
            ("1x:y", false),
            ("a_b:y", false),
            ("x:", false),
            ("x:a b", false),
        ];
        let dates = [
            ("2026-10-28", true),
            ("abcd-01-01", false),
            ("2026/01/01", false),
            ("2026-1x-01", false),
            ("2026-01/01", false),
            ("2026-01-0x", false),
            ("2026-01-01x", false),
            ("2026-00-01", false),
            ("2026-12-32", false),
            ("2026-13-01", false),
            ("2026-12-00", false),
            ("2026-1-01", false),
        ];
        let times = [
            ("10:00:00", true),
            ("10:00:00.5Z", true),
            ("23:59:60-01:30", true),
            ("1x:00:00", false),
            ("10-00:00", false),
            ("10:0x:00", false),
            ("10:00-00", false),
            ("10:00:0x", false),
            ("10:00:00.", false),
            ("24:00:00", false),
            ("10:60:00", false),
            ("10:00:61", false),
            ("10:00", false),
            ("10:00:00x", false),
            ("10:00:00+0x:00", false),
            ("10:00:00+01-00", false),
            ("10:00:00+01:0x", false),
            ("10:00:00+01:00x", false),
            ("10:00:00+24:00", false),
            ("10:00:00+01:60", false),
            ("10:00:00+01", false),
        ];
        let datetimes = [
            ("2026-10-28T10:00:00+02:00", true),
            ("2026-10-28T10:00:00", false),
            ("2026-10-28 10:00:00Z", false),
            ("2026-10-28T1:00:00Z", false),
            ("2026-13-28T10:00:00Z", false),
        ];
        for (text, ok) in uris {
            assert_eq!(is_uri(text), ok, "uri {text}");
        }
        for (text, ok) in dates {
            assert_eq!(is_date(text), ok, "date {text}");
        }
        for (text, ok) in times {
            assert_eq!(is_time(text), ok, "time {text}");
        }
        for (text, ok) in datetimes {
            assert_eq!(is_datetime(text), ok, "date-time {text}");
        }
        assert_eq!(
            [whole_number(4.0), whole_number(-0.0), whole_number(4.5)],
            [true, true, false]
        );
    }

    #[test]
    fn every_kind_and_member_shape_is_held_or_passed() {
        let table: Vec<(&str, Value, Option<&str>)> = vec![
            // a fragment's instance without `type`, a bare instance object
            ("pm10", json!({"value": "x"}), Some(rule::NUMBER)),
            ("pm10", json!("not an instance"), None),
            (
                "pm10",
                json!({"type": "Property", "observedAt": "2026-10-28T10:00:00Z"}),
                None,
            ),
            (
                "pm10",
                json!({"type": "Property", "value": 1, "unitCode": "GQ"}),
                None,
            ),
            (
                "odd",
                json!({"type": "Property", "value": "anything"}),
                None,
            ),
            (
                "odd",
                json!({"type": "TimeSeriesProperty", "value": "anything"}),
                None,
            ),
            ("owner", json!({"type": "Relationship"}), None),
            (
                "owner",
                json!({"type": "Relationship", "object": ["urn:a:1", "urn:a:2"]}),
                None,
            ),
            (
                "owner",
                json!({"type": "Relationship", "object": ["urn:a:1", 2]}),
                Some(rule::OBJECT),
            ),
            (
                "owner",
                json!({"type": "Property", "value": {"name": "inline"}}),
                Some(rule::KIND),
            ),
            (
                "inline",
                json!({"type": "Property", "value": {"name": "inline"}}),
                None,
            ),
            (
                "crew",
                json!({"type": "ListRelationship", "objectList": [{"object": "urn:a:1"}, "urn:a:2"]}),
                None,
            ),
            (
                "crew",
                json!({"type": "ListRelationship", "objectList": [{"object": "a b"}]}),
                Some(rule::OBJECT),
            ),
            ("crew", json!({"type": "ListRelationship"}), None),
            ("label", json!({"type": "LanguageProperty"}), None),
            (
                "label",
                json!({"type": "LanguageProperty", "languageMap": "en"}),
                Some(rule::LANGUAGE),
            ),
            (
                "label",
                json!({"type": "LanguageProperty", "languageMap": {"en": ["a", 1]}}),
                Some(rule::LANGUAGE),
            ),
            ("readings", json!({"type": "ListProperty"}), None),
            ("terms", json!({"type": "VocabProperty"}), None),
            (
                "doc",
                json!({"type": "JsonProperty", "json": {"a": 1}}),
                None,
            ),
            (
                "level",
                json!({"type": "Property", "value": 3}),
                Some(rule::ENUM),
            ),
        ];
        for (name, inst, want) in table {
            let mut doc = sensor(&[]);
            doc[format!("{NS}{name}")] = inst.clone();
            let got = entity(&rules(), &doc, false).err().map(|v| v.rule);
            assert_eq!(got, want, "{name} {inst}");
        }
        // `type` as one string, and a document that is no object
        let one = json!({"type": "https://e.org/Sensor", "https://e.org/pm10": [{"value": "x"}]});
        assert!(entity(&rules(), &one, false).is_err());
        assert!(entity(&rules(), &json!(["no", "entity"]), true).is_ok());
    }

    #[test]
    fn the_text_form_and_a_colliding_revision() {
        let json: Arc<[u8]> = br#"{"id": "https://e.org/", "name": "s",
          "classes": {"Sensor": {"slots": ["pm10"]}}, "slots": {"pm10": {"range": "float"}}}"#[..]
            .into();
        let doc = sensor(&[("pm10", prop(json!("abc")))]);
        let text = entity_text(5_912_003, &json, &doc, false).unwrap_err();
        assert!(
            text.starts_with("the attribute pm10 breaks the data model of Sensor"),
            "{text}"
        );
        assert_eq!(entity_text(5_912_003, &json, &sensor(&[]), true), Ok(()));
        // another model under a revision already held is built again, never shared
        let (read, _) = crate::cache::read(5_912_003, &json, &crate::SURFACE_LIMITS);
        let held = read.unwrap();
        let other = Arc::new(Model::clone(&held));
        let first = rules_of(5_912_003, &held);
        assert!(!Arc::ptr_eq(&first, &rules_of(5_912_003, &other)));
    }
}
