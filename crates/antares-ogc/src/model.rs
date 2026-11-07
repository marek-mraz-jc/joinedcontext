// SPDX-License-Identifier: EUPL-1.2
//! The collections of a view (mapping rows M1 to M7, S1, S2, S5, S7): each class of the
//! Tenant's `LinkML` schema whose type the view shows, with the slots it shows of it.

use linkml_model::{Builtin, Range};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The NGSI-LD default `GeoProperty` (4.5.16.1), the primary geometry when no slot is
/// annotated `ogc_role: primary-geometry`.
pub(crate) const LOCATION: &str = "https://uri.etsi.org/ngsi-ld/location";

/// The annotation that gives a slot its OGC API - Features Part 5 role.
const ROLE: &str = "ogc_role";

/// One collection: an Entity type the view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Collection {
    /// The `LinkML` class name, the `collectionId`.
    pub id: String,
    /// The class description, if the schema gives one.
    pub description: Option<String>,
    /// The Entity type's expanded IRI.
    pub type_iri: String,
    /// The primary geometry's Attribute IRI, if the view shows it (M4).
    pub geometry: Option<String>,
    /// The temporal geometry, if the schema declares one the view shows (M6).
    pub time: Option<Time>,
    /// The other Attributes the view shows, in schema order (M5).
    pub properties: Vec<Prop>,
    /// A `{type}-history` collection (T-5100): read from the temporal history, never written.
    pub history: bool,
    /// A named output of the view's `representations.ogc` (T-5216): read only, its filter a
    /// conjunct of every read.
    pub output: Option<antares_ops::outputs::Output>,
}

/// One Attribute of a collection besides its geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prop {
    /// The slot name, the Feature property's name.
    pub name: String,
    /// The Attribute IRI.
    pub iri: String,
    /// The JSON Schema of its value as `format=simplified` writes it (S1).
    pub schema: Value,
    /// Its `ogc_role` annotation (S2).
    pub role: Option<String>,
    /// Its unit's UCUM code.
    pub unit: Option<String>,
    /// Its slot description.
    pub description: Option<String>,
    /// One scalar value a string, number or boolean: `sortby` may name it (S5).
    pub sortable: bool,
    /// The NGSI-LD kind a write gives it (W1): the slot's, `JsonProperty` for an inlined
    /// class.
    pub kind: String,
    /// A Relationship (S7).
    pub reference: bool,
    /// A Relationship's target collection: the class its range names, if the view shows it.
    pub target: Option<String>,
}

/// A temporal geometry (Part 5 Req 10 to 12): Attribute IRIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Time {
    /// `primary-instant`
    Instant(String),
    /// `primary-interval-start` / `primary-interval-end`, either may be undeclared.
    Interval(Option<String>, Option<String>),
}

/// The collection of a named output (docs API/11 EP-159, T-5216) over its one source's
/// collection `base`: its geometry (the first `geometry` path), its time and its properties from
/// the source's roles, read only, narrowed by its filter. `None` when a role names an Attribute
/// the collection does not show, or a geometry that is no `GeoProperty`.
pub(crate) fn output(base: &Collection, o: &antares_ops::outputs::Output) -> Option<Collection> {
    let source = o.sources.first()?;
    let attribute = |name: &str| base.properties.iter().find(|p| p.name == name);
    let iri = |name: &str| attribute(name).map(|p| p.iri.clone());
    let one = |role: &str| source.role(role).first().map(|name| iri(name).ok_or(()));
    let geometry = match source.role("geometry").first() {
        None => base.geometry.clone(),
        Some(name) => Some(
            base.geometry
                .clone()
                .filter(|g| g.rsplit(['/', '#']).next() == Some(*name))
                .or_else(|| {
                    attribute(name)
                        .filter(|p| p.kind == "GeoProperty")
                        .map(|p| p.iri.clone())
                })?,
        ),
    };
    let time = match (one("instant"), one("start"), one("end")) {
        (None, None, None) => base.time.clone(),
        (Some(at), _, _) => Some(Time::Instant(at.ok()?)),
        (None, start, end) => Some(Time::Interval(
            start.transpose().ok()?,
            end.transpose().ok()?,
        )),
    };
    let shown = source.role("properties");
    if shown.iter().any(|name| attribute(name).is_none()) {
        return None;
    }
    let timed: Vec<&String> = match &time {
        Some(Time::Instant(at)) => vec![at],
        Some(Time::Interval(start, end)) => start.iter().chain(end).collect(),
        None => Vec::new(),
    };
    // a time Attribute stays a property: the `datetime` query names it by its term
    let properties = base
        .properties
        .iter()
        .filter(|p| Some(&p.iri) != geometry.as_ref())
        .filter(|p| shown.is_empty() || shown.contains(&p.name.as_str()) || timed.contains(&&p.iri))
        .cloned()
        .collect();
    Some(Collection {
        id: o.name.clone(),
        description: o.description_in("en").map(str::to_owned),
        type_iri: base.type_iri.clone(),
        geometry,
        time,
        properties,
        history: false,
        output: Some(o.clone()),
    })
}

/// The NGSI-LD Attribute kinds (TS 104 175 4.5) a Feature property reads and writes: a slot of
/// any other kind is no property of the collection, so a write naming it is 400 and it is in
/// neither `/schema` nor the queryables, never stored as a `Property`.
pub(crate) const KINDS: [&str; 8] = [
    "Property",
    "Relationship",
    "GeoProperty",
    "JsonProperty",
    "VocabProperty",
    "LanguageProperty",
    "ListProperty",
    "ListRelationship",
];

/// Whether the `ogc_role` annotation `role` fits the slot `p` (Part 5 `core-roles-features`):
/// the geometry a `GeoProperty`, a time a scalar text Property (a date, a date-time, or a
/// string holding one, as Smart Data Models write `startDate`). Any other role is kept.
fn fits(role: &str, p: &Prop) -> bool {
    match role {
        "primary-geometry" => p.kind == "GeoProperty",
        "primary-instant" | "primary-interval-start" | "primary-interval-end" => {
            p.kind == "Property"
                && p.schema.get("type").and_then(Value::as_str) == Some("string")
                && p.schema.get("enum").is_none()
        }
        _ => true,
    }
}

/// A `collectionId` is a path segment as written: letters, digits, `_`, `-`, `.`.
fn path_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

/// The collections of `schema` the view shows (`shown`: type IRI to Attribute IRIs), by id.
pub(crate) fn build(
    schema: &linkml_model::Model,
    shown: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, Collection> {
    let classes: BTreeMap<&str, (&linkml_model::Class, &BTreeSet<String>)> = schema
        .schema()
        .classes
        .iter()
        .filter(|(name, _)| path_safe(name))
        .filter_map(|(name, class)| {
            Some((name.as_str(), (class, shown.get(&schema.class_iri(name))?)))
        })
        .collect();
    let mut out = BTreeMap::new();
    for (&class_name, &(class, attrs)) in &classes {
        let type_iri = schema.class_iri(class_name);
        let mut slots: Vec<Prop> = schema
            .class_slots(class_name)
            .into_iter()
            .map(|(name, slot)| prop(schema, name, &slot, |c| classes.contains_key(c)))
            .filter(|p| attrs.contains(&p.iri) && KINDS.contains(&p.kind.as_str()))
            .collect();
        for p in &mut slots {
            if p.role.as_deref().is_some_and(|r| !fits(r, p)) {
                p.role = None;
            }
        }
        if let Some(guide) = antares_ops::guide::of(class_name) {
            guided(guide, &mut slots);
        }
        let role = |wanted: &str| {
            slots
                .iter()
                .find(|p| p.role.as_deref() == Some(wanted))
                .map(|p| p.iri.clone())
        };
        let geometry = role("primary-geometry")
            .or_else(|| attrs.contains(LOCATION).then(|| LOCATION.to_owned()));
        let time = match (
            role("primary-instant"),
            role("primary-interval-start"),
            role("primary-interval-end"),
        ) {
            (Some(at), _, _) => Some(Time::Instant(at)),
            (None, None, None) => None,
            (None, start, end) => Some(Time::Interval(start, end)),
        };
        let properties = slots
            .into_iter()
            .filter(|p| Some(&p.iri) != geometry.as_ref())
            .collect();
        out.insert(
            class_name.to_owned(),
            Collection {
                history: false,
                output: None,
                id: class_name.to_owned(),
                description: class.description.clone(),
                type_iri,
                geometry,
                time,
                properties,
            },
        );
    }
    out
}

/// The roles the type's query guide gives (T-4537) to the shown slots, where the schema
/// annotates none of that kind: the time axis and a `GeoProperty` place.
fn guided(guide: &antares_ops::guide::Guide, slots: &mut [Prop]) {
    use antares_ops::guide::Time;
    let annotated = |slots: &[Prop], roles: &[&str]| {
        slots
            .iter()
            .any(|p| p.role.as_deref().is_some_and(|r| roles.contains(&r)))
    };
    let give = |slots: &mut [Prop], name: &str, role: &str| {
        for p in slots
            .iter_mut()
            .filter(|p| p.name == name && p.role.is_none())
        {
            p.role = Some(role.to_owned());
        }
    };
    let times = [
        "primary-instant",
        "primary-interval-start",
        "primary-interval-end",
    ];
    if !annotated(slots, &times) {
        let names: Vec<String> = slots.iter().map(|p| p.name.clone()).collect();
        match guide.time_in(|a| names.iter().any(|n| n == a)) {
            Some(Time::Instant(at)) => give(slots, at, "primary-instant"),
            Some(Time::Range(start, end)) => {
                give(slots, start, "primary-interval-start");
                give(slots, end, "primary-interval-end");
            }
            None => {}
        }
    }
    if !annotated(slots, &["primary-geometry"]) {
        let place = guide.place.iter().find(|a| {
            slots
                .iter()
                .any(|p| p.name == **a && p.kind == "GeoProperty")
        });
        if let Some(place) = place {
            give(slots, place, "primary-geometry");
        }
    }
}

/// One slot as a Feature property: its value's JSON Schema by NGSI-LD kind and range, as
/// `antares-odata` shapes it (odata-linkml-mapping 3.4).
fn prop(
    schema: &linkml_model::Model,
    name: &str,
    slot: &linkml_model::Slot,
    shown: impl Fn(&str) -> bool,
) -> Prop {
    let kind = schema.kind(slot);
    let range = schema.range(slot);
    let inlined = slot.inlined == Some(true) && matches!(range, Range::Class(_));
    let relationship = !inlined && matches!(kind, "Relationship" | "ListRelationship");
    let one = match (kind, range) {
        // A kind this mapping does not know: any JSON value, neither sorted nor
        // filtered (T-5207, mapping-contract rule 4).
        _ if !linkml_model::known_kind(kind) => json!({}),
        _ if relationship => json!({"type": "string", "format": "uri"}),
        ("GeoProperty", _) => json!({"format": "geometry-any"}),
        ("JsonProperty" | "LanguageProperty", _) | (_, Range::Class(_)) => {
            json!({"type": "object"})
        }
        (_, Range::Enum(e)) => {
            let values = schema.schema().enums.get(e).into_iter();
            json!({"type": "string", "enum": values.flat_map(|v| v.permissible_values.keys()).collect::<Vec<_>>()})
        }
        (_, Range::Type(b)) => builtin(b),
    };
    let many = slot.multivalued == Some(true) || kind.starts_with("List");
    let scalar = one.get("type").is_some_and(|t| t != "object");
    Prop {
        name: name.to_owned(),
        iri: schema.slot_iri(name),
        sortable: scalar && !many && !relationship,
        schema: if many {
            json!({"type": "array", "items": one})
        } else {
            one
        },
        role: slot.annotations.get(ROLE).map(|a| a.value.clone()),
        unit: slot.unit.as_ref().and_then(|u| u.ucum_code.clone()),
        description: slot.description.clone(),
        kind: if inlined { "JsonProperty" } else { kind }.to_owned(),
        reference: relationship,
        target: match range {
            Range::Class(c) if relationship && shown(c) => Some(c.to_owned()),
            _ => None,
        },
    }
}

/// The JSON Schema of a `LinkML` builtin type.
fn builtin(b: Builtin) -> Value {
    match b {
        Builtin::Integer => json!({"type": "integer"}),
        Builtin::Float | Builtin::Double | Builtin::Decimal => json!({"type": "number"}),
        Builtin::Boolean => json!({"type": "boolean"}),
        Builtin::Date => json!({"type": "string", "format": "date"}),
        Builtin::Datetime => json!({"type": "string", "format": "date-time"}),
        Builtin::Time => json!({"type": "string", "format": "time"}),
        Builtin::Uri | Builtin::Uriorcurie => json!({"type": "string", "format": "uri"}),
        Builtin::String => json!({"type": "string"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One slot per NGSI-LD kind and `LinkML` range the surface writes a schema for.
    const KINDS: &str = r#"{"id": "https://e.org/s", "name": "s", "default_prefix": "e",
                "prefixes": {"e": {"prefix_prefix": "e", "prefix_reference": "https://e.org/"}},
                "classes": {"A": {"slots": ["i", "f", "b", "d", "dt", "t", "u", "s", "c", "inl", "lr", "rel", "json", "lang", "geo", "vocab", "e", "le", "nk"]},
                            "B": {}},
                "slots": {"i": {"range": "integer"}, "f": {"range": "double"}, "b": {"range": "boolean"},
                          "d": {"range": "date"}, "dt": {"range": "datetime"}, "t": {"range": "time"},
                          "u": {"range": "uriorcurie"}, "s": {}, "c": {"range": "B"},
                          "inl": {"range": "B", "inlined": true},
                          "lr": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListRelationship"}}},
                          "rel": {"range": "B", "multivalued": true},
                          "json": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "JsonProperty"}}},
                          "lang": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "LanguageProperty"}}},
                          "geo": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
                          "vocab": {"range": "E", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "VocabProperty"}}},
                          "e": {"range": "E"},
                          "le": {"range": "E", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
                          "nk": {"range": "integer", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "TimeSeriesProperty"}}}},
                "enums": {"E": {"permissible_values": {"x": {}}}}}"#;

    #[test]
    fn every_kind_and_range_has_its_schema() {
        let m = schema(KINDS);
        let shown = |name: &str| name == "B";
        let props: BTreeMap<&str, Prop> = m
            .class_slots("A")
            .into_iter()
            .map(|(name, slot)| (name, prop(&m, name, &slot, shown)))
            .collect();
        let got = |n: &str| {
            props
                .get(n)
                .map(|p| (p.schema.clone(), p.sortable, p.reference, p.target.clone()))
        };
        let uri = json!({"type": "string", "format": "uri"});
        let want = [
            ("i", json!({"type": "integer"}), true, false, None),
            ("f", json!({"type": "number"}), true, false, None),
            ("b", json!({"type": "boolean"}), true, false, None),
            (
                "d",
                json!({"type": "string", "format": "date"}),
                true,
                false,
                None,
            ),
            (
                "dt",
                json!({"type": "string", "format": "date-time"}),
                true,
                false,
                None,
            ),
            (
                "t",
                json!({"type": "string", "format": "time"}),
                true,
                false,
                None,
            ),
            ("u", uri.clone(), true, false, None),
            ("s", json!({"type": "string"}), true, false, None),
            ("c", uri.clone(), false, true, Some("B".to_owned())),
            ("inl", json!({"type": "object"}), false, false, None),
            (
                "lr",
                json!({"type": "array", "items": uri.clone()}),
                false,
                true,
                None,
            ),
            (
                "rel",
                json!({"type": "array", "items": uri}),
                false,
                true,
                Some("B".to_owned()),
            ),
            ("json", json!({"type": "object"}), false, false, None),
            // T-5207: a kind a later NGSI-LD version adds is any value, never sorted
            ("nk", json!({}), false, false, None),
            ("lang", json!({"type": "object"}), false, false, None),
            ("geo", json!({"format": "geometry-any"}), false, false, None),
            (
                "vocab",
                json!({"type": "string", "enum": ["x"]}),
                true,
                false,
                None,
            ),
            (
                "e",
                json!({"type": "string", "enum": ["x"]}),
                true,
                false,
                None,
            ),
            (
                "le",
                json!({"type": "array", "items": {"type": "string", "enum": ["x"]}}),
                false,
                false,
                None,
            ),
        ];
        for (name, schema, sortable, reference, target) in want {
            assert_eq!(
                got(name),
                Some((schema, sortable, reference, target)),
                "{name}"
            );
        }
        assert!(!shown("A"));
    }

    use linkml_model::{Limits, Model};

    const LIMITS: Limits = Limits {
        max_bytes: 1 << 16,
        max_depth: 16,
        max_elements: 1024,
    };

    fn schema(json: &str) -> Model {
        Model::read(json.as_bytes(), &LIMITS).unwrap_or_else(|e| unreachable!("{e:?}"))
    }

    const CITY: &str = r#"{
      "id": "https://example.org/city", "name": "city",
      "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"},
                   "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
      "default_prefix": "city",
      "classes": {
        "Car": {"slots": ["speed", "where", "seen"], "description": "A car"},
        "Stop": {"slots": ["name", "location", "from", "until"]},
        "Hidden": {"slots": ["name"]}
      },
      "slots": {
        "speed": {"range": "float"},
        "where": {"range": "string", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}, "ogc_role": {"tag": "ogc_role", "value": "primary-geometry"}}},
        "seen": {"range": "datetime", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-instant"}}},
        "name": {"range": "string"},
        "location": {"slot_uri": "ngsi:location", "range": "string"},
        "from": {"range": "datetime", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-interval-start"}}},
        "until": {"range": "datetime"}
      }
    }"#;

    fn shown(rows: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        rows.iter()
            .map(|(t, attrs)| {
                (
                    (*t).to_owned(),
                    attrs.iter().map(|a| (*a).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn collections_are_the_shown_classes_with_their_roles() {
        let m = schema(CITY);
        let c = build(
            &m,
            &shown(&[
                (
                    "https://example.org/city/Car",
                    &[
                        "https://example.org/city/speed",
                        "https://example.org/city/where",
                        "https://example.org/city/seen",
                    ],
                ),
                (
                    "https://example.org/city/Stop",
                    &[
                        "https://example.org/city/name",
                        LOCATION,
                        "https://example.org/city/from",
                    ],
                ),
            ]),
        );
        assert_eq!(
            c.keys().collect::<Vec<_>>(),
            ["Car", "Stop"],
            "Hidden is not shown"
        );
        let car = c.get("Car").unwrap_or_else(|| unreachable!());
        assert_eq!(car.description.as_deref(), Some("A car"));
        assert_eq!(
            car.geometry.as_deref(),
            Some("https://example.org/city/where")
        );
        assert_eq!(
            car.time,
            Some(Time::Instant("https://example.org/city/seen".into()))
        );
        assert_eq!(
            car.properties
                .iter()
                .map(|p| (p.name.clone(), p.iri.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    "speed".to_owned(),
                    "https://example.org/city/speed".to_owned()
                ),
                ("seen".into(), "https://example.org/city/seen".into())
            ]
        );
        let stop = c.get("Stop").unwrap_or_else(|| unreachable!());
        assert_eq!(
            stop.geometry.as_deref(),
            Some(LOCATION),
            "location is the default"
        );
        assert_eq!(
            stop.time,
            Some(Time::Interval(
                Some("https://example.org/city/from".into()),
                None
            )),
            "until is hidden"
        );
        assert_eq!(
            stop.properties.len(),
            2,
            "name and from; location is the geometry"
        );
    }

    #[test]
    fn a_hidden_geometry_or_time_is_none() {
        let m = schema(CITY);
        let c = build(
            &m,
            &shown(&[(
                "https://example.org/city/Car",
                &["https://example.org/city/speed"],
            )]),
        );
        let car = c.get("Car").unwrap_or_else(|| unreachable!());
        assert_eq!((car.geometry.as_ref(), car.time.as_ref()), (None, None));
    }

    #[test]
    fn a_class_name_that_is_no_path_segment_is_left_out() {
        assert!(path_safe("Car_2.v-1"));
        assert!(!path_safe("Car 2"));
        assert!(!path_safe(""));
        assert!(!path_safe("Car/2"));
    }

    /// T-5204: a type no code knows gets its place and time from its model's annotations
    /// alone, and a role on a slot of the wrong kind is dropped. (A kind NGSI-LD does not define
    /// is refused when the schema loads; [`KINDS`] keeps one out should that ever loosen.)
    #[test]
    fn roles_come_from_the_model_and_are_checked() {
        let ann = |kind: &str, role: &str| {
            format!(
                r#"{{"ngsi_ld_kind": {{"tag": "ngsi_ld_kind", "value": "{kind}"}}, "ogc_role": {{"tag": "ogc_role", "value": "{role}"}}}}"#
            )
        };
        let m = schema(&format!(
            r#"{{"id": "https://e.org/s", "name": "s", "default_prefix": "s",
                 "prefixes": {{"s": {{"prefix_prefix": "s", "prefix_reference": "https://e.org/s/"}}}},
                 "classes": {{"Kiosk": {{"slots": ["spot", "opened", "rank"]}},
                              "Booth": {{"slots": ["size", "count", "area"]}}}},
                 "slots": {{"spot": {{"range": "string", "annotations": {}}},
                            "opened": {{"range": "string", "annotations": {}}},
                            "rank": {{"range": "integer", "annotations": {{"ogc_role": {{"tag": "ogc_role", "value": "label"}}}}}},
                            "size": {{"range": "float", "annotations": {}}},
                            "count": {{"range": "integer", "annotations": {}}},
                            "area": {{"range": "string", "annotations": {}}}}}}}"#,
            ann("GeoProperty", "primary-geometry"),
            ann("Property", "primary-instant"),
            ann("Property", "primary-geometry"),
            ann("Property", "primary-instant"),
            ann("GeoProperty", "primary-instant"),
        ));
        let all = |t: &str, slots: &[&str]| {
            (
                format!("https://e.org/s/{t}"),
                slots
                    .iter()
                    .map(|s| format!("https://e.org/s/{s}"))
                    .collect(),
            )
        };
        let c = build(
            &m,
            &BTreeMap::from([
                all("Kiosk", &["spot", "opened", "rank"]),
                all("Booth", &["size", "count", "area"]),
            ]),
        );
        let kiosk = c.get("Kiosk").unwrap_or_else(|| unreachable!());
        assert_eq!(kiosk.geometry.as_deref(), Some("https://e.org/s/spot"));
        assert_eq!(
            kiosk.time,
            Some(Time::Instant("https://e.org/s/opened".into()))
        );
        let names: Vec<&str> = kiosk.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["opened", "rank"]);
        let rank = kiosk.properties.iter().find(|p| p.name == "rank");
        assert_eq!(
            rank.and_then(|p| p.role.as_deref()),
            Some("label"),
            "a role of no place or time is kept"
        );
        let booth = c.get("Booth").unwrap_or_else(|| unreachable!());
        assert_eq!(booth.geometry, None, "a float is no geometry");
        assert_eq!(booth.time, None, "an integer and a GeoProperty are no time");
    }
}
