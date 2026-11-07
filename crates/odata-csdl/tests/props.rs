// SPDX-License-Identifier: EUPL-1.2
//! CSDL XML and CSDL JSON describe one model (surfaces-test-plan section 3, threat O17).
//!
//! Random valid models are written both ways; the XML is read back by a small reader and turned
//! into CSDL JSON by the rules of the two specifications (the ones the OASIS converter
//! `odata-csdl-xml2json` applies), and that must equal the JSON writer's output. The CI step
//! `odata_csdl_xml_and_json_agree` runs the OASIS converter itself on the fixture.

mod fixture;

use odata_csdl::{
    validate, write_json, write_xml, Annotation, ComplexType, CsdlVersion, EntityContainer,
    EntitySet, EntityType, Expr, Include, Limits, Model, NavigationProperty,
    NavigationPropertyBinding, Property, Reference, Scale, Schema, Srid, Term, TypeRef,
};
use proptest::prelude::*;
use serde_json::{json, Map, Value};

#[cfg(test)]
mod read {
    use super::*;

    // ---- a reader for the XML the writer produces --------------------------------------------

    #[derive(Debug)]
    pub(crate) struct Element {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<Element>,
        text: String,
    }

    impl Element {
        fn new(name: &str) -> Self {
            Element {
                name: name.to_owned(),
                attrs: vec![],
                children: vec![],
                text: String::new(),
            }
        }

        fn attr(&self, name: &str) -> Option<&str> {
            self.attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        }

        fn local(&self) -> &str {
            self.name.rsplit(':').next().unwrap()
        }
    }

    fn unescape(text: &str) -> String {
        let mut pieces = text.split('&');
        let mut out = pieces.next().unwrap().to_owned();
        for piece in pieces {
            let (entity, rest) = piece.split_once(';').unwrap();
            out.push(match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                hex => {
                    char::from_u32(u32::from_str_radix(hex.trim_start_matches("#x"), 16).unwrap())
                        .unwrap()
                }
            });
            out.push_str(rest);
        }
        out
    }

    /// The document element. The writer escapes `<` in text and `"` in attributes, so splitting at
    /// `<` gives one tag per piece, and splitting the tag at `"` alternates names and values.
    pub(crate) fn read_xml(xml: &str) -> Element {
        let (_, body) = xml.split_once("?>").unwrap();
        let mut stack = vec![Element::new("#document")];
        for piece in body.split('<').skip(1) {
            let (tag, text) = piece.split_once('>').unwrap();
            if tag.starts_with('/') {
                let done = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(done);
                continue;
            }
            let (name, attrs) = tag
                .trim_end_matches('/')
                .split_once(' ')
                .unwrap_or((tag.trim_end_matches('/'), ""));
            let mut node = Element::new(name);
            let quoted: Vec<&str> = attrs.split('"').collect();
            for [key, value] in quoted.as_chunks::<2>().0 {
                node.attrs
                    .push((key.trim().trim_end_matches('=').to_owned(), unescape(value)));
            }
            if tag.ends_with('/') {
                stack.last_mut().unwrap().children.push(node);
            } else {
                node.text = unescape(text);
                stack.push(node);
            }
        }
        stack.pop().unwrap().children.remove(0)
    }

    // ---- CSDL XML to CSDL JSON, by the rules of both specifications ----------------------------

    const TEMPORAL: [&str; 3] = ["Edm.DateTimeOffset", "Edm.Duration", "Edm.TimeOfDay"];

    /// `$Type`, `$Collection`, `$Nullable`, `$Precision`, `$Scale`, `$SRID` of a Property or Term.
    fn typed(e: &Element, out: &mut Map<String, Value>, navigation: bool) {
        let raw = e.attr("Type").unwrap();
        let (ty, collection) = match raw.strip_prefix("Collection(") {
            Some(inner) => (inner.trim_end_matches(')'), true),
            None => (raw, false),
        };
        if navigation || ty != "Edm.String" {
            out.insert("$Type".into(), json!(ty));
        }
        if collection {
            out.insert("$Collection".into(), json!(true));
        }
        let nullable = match (e.attr("Nullable"), collection) {
            (Some(v), _) => v == "true",
            (None, false) => true, // absent means true on a single value
            (None, true) => false, // only a navigation leaves it out on a collection
        };
        if nullable {
            out.insert("$Nullable".into(), json!(true));
        }
        match e.attr("Precision") {
            Some(p) => {
                out.insert("$Precision".into(), json!(p.parse::<u64>().unwrap()));
            }
            None if TEMPORAL.contains(&ty) => {
                out.insert("$Precision".into(), json!(0));
            }
            None => {}
        }
        match e.attr("Scale") {
            Some("variable") => {}
            Some(s) => {
                out.insert(
                    "$Scale".into(),
                    s.parse::<u64>().map_or_else(|_| json!(s), |n| json!(n)),
                );
            }
            None if ty == "Edm.Decimal" => {
                out.insert("$Scale".into(), json!(0));
            }
            None => {}
        }
        if let Some(srid) = e.attr("SRID") {
            out.insert("$SRID".into(), json!(srid));
        }
    }

    fn value(e: &Element) -> Value {
        match e.local() {
            "Collection" => Value::Array(e.children.iter().map(value).collect()),
            "Record" => Value::Object(
                e.children
                    .iter()
                    .map(|pv| {
                        (
                            pv.attr("Property").unwrap().to_owned(),
                            annotation_value(pv),
                        )
                    })
                    .collect(),
            ),
            other => inline(other, &e.text),
        }
    }

    fn inline(kind: &str, text: &str) -> Value {
        match kind {
            "Bool" => json!(text == "true"),
            "Int" => json!(text.parse::<i64>().unwrap()),
            "String" | "PropertyPath" | "NavigationPropertyPath" => json!(text),
            "Path" => json!({ "$Path": text }),
            "EnumMember" => json!(text
                .split(' ')
                .map(|m| m.rsplit('/').next().unwrap())
                .collect::<Vec<_>>()
                .join(",")),
            other => panic!("unexpected expression {other}"),
        }
    }

    /// The value of an `Annotation` or `PropertyValue`: an inline attribute, or its one child.
    fn annotation_value(e: &Element) -> Value {
        for kind in [
            "Bool",
            "Int",
            "String",
            "Path",
            "PropertyPath",
            "NavigationPropertyPath",
            "EnumMember",
        ] {
            if let Some(text) = e.attr(kind) {
                return inline(kind, text);
            }
        }
        value(&e.children[0])
    }

    fn annotate(e: &Element, out: &mut Map<String, Value>) {
        for a in e.children.iter().filter(|c| c.local() == "Annotation") {
            let key = match a.attr("Qualifier") {
                Some(q) => format!("@{}#{q}", a.attr("Term").unwrap()),
                None => format!("@{}", a.attr("Term").unwrap()),
            };
            out.insert(key, annotation_value(a));
        }
    }

    fn member(e: &Element) -> Value {
        let mut out = Map::new();
        match e.local() {
            "Property" => typed(e, &mut out, false),
            "NavigationProperty" => {
                out.insert("$Kind".into(), json!("NavigationProperty"));
                typed(e, &mut out, true);
                if let Some(partner) = e.attr("Partner") {
                    out.insert("$Partner".into(), json!(partner));
                }
            }
            other => panic!("unexpected member {other}"),
        }
        annotate(e, &mut out);
        Value::Object(out)
    }

    fn structured(e: &Element, kind: &str) -> Value {
        let mut out = Map::new();
        out.insert("$Kind".into(), json!(kind));
        if let Some(base) = e.attr("BaseType") {
            out.insert("$BaseType".into(), json!(base));
        }
        for (attr, member) in [("Abstract", "$Abstract"), ("OpenType", "$OpenType")] {
            if e.attr(attr) == Some("true") {
                out.insert(member.into(), json!(true));
            }
        }
        for child in &e.children {
            match child.local() {
                "Key" => {
                    let names: Vec<_> = child
                        .children
                        .iter()
                        .map(|r| json!(r.attr("Name").unwrap()))
                        .collect();
                    out.insert("$Key".into(), Value::Array(names));
                }
                "Property" | "NavigationProperty" => {
                    out.insert(child.attr("Name").unwrap().to_owned(), member(child));
                }
                _ => {}
            }
        }
        annotate(e, &mut out);
        Value::Object(out)
    }

    fn reference(child: &Element) -> (String, Value) {
        let includes: Vec<_> = child
            .children
            .iter()
            .map(|i| {
                let mut inc = Map::new();
                inc.insert("$Namespace".into(), json!(i.attr("Namespace").unwrap()));
                if let Some(alias) = i.attr("Alias") {
                    inc.insert("$Alias".into(), json!(alias));
                }
                Value::Object(inc)
            })
            .collect();
        let uri = child.attr("Uri").unwrap();
        let uri = match uri.strip_suffix(".xml") {
            Some(stem)
                if uri.starts_with(
                    "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/",
                ) =>
            {
                format!("{stem}.json")
            }
            _ => uri.to_owned(),
        };
        (uri, json!({ "$Include": includes }))
    }

    fn term(e: &Element) -> Value {
        let mut term = Map::new();
        term.insert("$Kind".into(), json!("Term"));
        typed(e, &mut term, false);
        if let Some(applies) = e.attr("AppliesTo") {
            term.insert(
                "$AppliesTo".into(),
                json!(applies.split(' ').collect::<Vec<_>>()),
            );
        }
        annotate(e, &mut term);
        Value::Object(term)
    }

    fn container(e: &Element) -> Value {
        let mut c = Map::new();
        c.insert("$Kind".into(), json!("EntityContainer"));
        for set in e.children.iter().filter(|s| s.local() == "EntitySet") {
            let mut s = Map::new();
            s.insert("$Collection".into(), json!(true));
            s.insert("$Type".into(), json!(set.attr("EntityType").unwrap()));
            let bindings: Map<String, Value> = set
                .children
                .iter()
                .filter(|b| b.local() == "NavigationPropertyBinding")
                .map(|b| {
                    (
                        b.attr("Path").unwrap().to_owned(),
                        json!(b.attr("Target").unwrap()),
                    )
                })
                .collect();
            if !bindings.is_empty() {
                s.insert("$NavigationPropertyBinding".into(), Value::Object(bindings));
            }
            annotate(set, &mut s);
            c.insert(set.attr("Name").unwrap().to_owned(), Value::Object(s));
        }
        annotate(e, &mut c);
        Value::Object(c)
    }

    /// One schema's members; its container's qualified name goes to `doc` as `$EntityContainer`.
    fn schema(schema: &Element, doc: &mut Map<String, Value>) -> Value {
        let namespace = schema.attr("Namespace").unwrap();
        let mut out = Map::new();
        if let Some(alias) = schema.attr("Alias") {
            out.insert("$Alias".into(), json!(alias));
        }
        for e in &schema.children {
            let name = e.attr("Name").unwrap_or_default().to_owned();
            let member = match e.local() {
                "EntityType" => structured(e, "EntityType"),
                "ComplexType" => structured(e, "ComplexType"),
                "Term" => term(e),
                "EntityContainer" => {
                    doc.insert(
                        "$EntityContainer".into(),
                        json!(format!("{namespace}.{name}")),
                    );
                    container(e)
                }
                _ => continue,
            };
            out.insert(name, member);
        }
        annotate(schema, &mut out);
        Value::Object(out)
    }

    fn xml_to_json(root: &Element) -> Value {
        let mut doc = Map::new();
        doc.insert("$Version".into(), json!(root.attr("Version").unwrap()));
        let mut references = Map::new();
        let mut schemas = Map::new();
        for child in &root.children {
            if child.local() == "Reference" {
                let (uri, value) = reference(child);
                references.insert(uri, value);
            }
            for s in child.children.iter().filter(|c| c.local() == "Schema") {
                let value = schema(s, &mut doc);
                schemas.insert(s.attr("Namespace").unwrap().to_owned(), value);
            }
        }
        if !references.is_empty() {
            doc.insert("$Reference".into(), Value::Object(references));
        }
        doc.extend(schemas);
        Value::Object(doc)
    }

    pub(crate) fn both(model: &Model) -> (Value, Value) {
        let (mut xml, mut json_out) = (Vec::new(), Vec::new());
        write_xml(model, &Limits::strict(), &mut xml).unwrap();
        write_json(model, &Limits::strict(), &mut json_out).unwrap();
        let read = xml_to_json(&read_xml(std::str::from_utf8(&xml).unwrap()));
        (read, serde_json::from_slice(&json_out).unwrap())
    }
}

#[test]
fn the_fixture_reads_the_same_both_ways() {
    let model = fixture::ngsi_ld_model();
    validate(&model, &Limits::strict()).unwrap();
    let (from_xml, from_json) = read::both(&model);
    assert_eq!(from_xml, from_json);
}

#[cfg(test)]
mod strategy {
    use super::*;

    // ---- random valid models ------------------------------------------------------------------

    const PRIMITIVES: [&str; 12] = [
        "Edm.String",
        "Edm.Boolean",
        "Edm.Int32",
        "Edm.Int64",
        "Edm.Double",
        "Edm.Decimal",
        "Edm.Date",
        "Edm.DateTimeOffset",
        "Edm.Duration",
        "Edm.TimeOfDay",
        "Edm.GeographyPoint",
        "Edm.Untyped",
    ];

    fn text() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9 <>&\"'\t\n\r\u{e9}\u{2603}]{0,12}"
    }

    fn expr() -> impl Strategy<Value = Expr> {
        let leaf = prop_oneof![
            any::<bool>().prop_map(Expr::Bool),
            any::<i64>().prop_map(Expr::Int),
            text().prop_map(Expr::String),
            "[a-z]{1,4}(/[a-z]{1,4}){0,2}".prop_map(Expr::Path),
            "[a-z]{1,4}".prop_map(Expr::PropertyPath),
            "[a-z]{1,4}".prop_map(Expr::NavigationPropertyPath),
            prop::collection::vec("[A-Z][a-z]{0,4}", 1..3).prop_map(|members| Expr::EnumMember {
                ty: "Core.Kind".into(),
                members
            }),
        ];
        leaf.prop_recursive(2, 8, 3, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..3).prop_map(Expr::Collection),
                prop::collection::btree_map("[A-Z][a-z]{0,4}", inner, 0..3)
                    .prop_map(|m| Expr::Record(m.into_iter().collect())),
            ]
        })
    }

    fn annotations() -> impl Strategy<Value = Vec<Annotation>> {
        prop::collection::btree_map(
            ("[A-Z][a-z]{0,5}", prop::option::of("[a-z]{1,3}")),
            expr(),
            0..3,
        )
        .prop_map(|m| {
            m.into_iter()
                .map(|((term, qualifier), value)| Annotation {
                    term: format!("Core.{term}"),
                    qualifier,
                    value,
                })
                .collect()
        })
    }

    /// One structural property: a primitive (with its facets) or one of `complex` complex types.
    fn property(name: String, complex: usize) -> impl Strategy<Value = Property> {
        let ty = if complex == 0 {
            prop::sample::select(PRIMITIVES.to_vec())
                .prop_map(str::to_owned)
                .boxed()
        } else {
            prop_oneof![
                3 => prop::sample::select(PRIMITIVES.to_vec()).prop_map(str::to_owned),
                1 => (0..complex).prop_map(|c| format!("p.C{c}")),
            ]
            .boxed()
        };
        (ty, any::<bool>(), any::<bool>(), any::<u8>(), annotations()).prop_map(
            move |(ty, collection, nullable, facet, annotations)| {
                let mut p = Property::new(
                    name.clone(),
                    TypeRef {
                        name: ty.clone(),
                        collection,
                    },
                );
                p.nullable = nullable;
                p.annotations = annotations;
                match ty.as_str() {
                    "Edm.Decimal" => {
                        p.precision = (facet % 2 == 0).then_some(u32::from(facet % 20) + 1);
                        p.scale = [
                            None,
                            Some(Scale::Variable),
                            Some(Scale::Floating),
                            Some(Scale::Digits(1)),
                        ][usize::from(facet % 4)];
                    }
                    t if t.starts_with("Edm.Date") && t != "Edm.Date"
                        || t == "Edm.Duration"
                        || t == "Edm.TimeOfDay" =>
                    {
                        p.precision = (facet % 2 == 0).then_some(u32::from(facet % 13));
                    }
                    "Edm.GeographyPoint" => {
                        p.srid = [None, Some(Srid::Variable), Some(Srid::Id(4326))]
                            [usize::from(facet % 3)];
                    }
                    _ => {}
                }
                p
            },
        )
    }

    fn properties(prefix: &'static str, complex: usize) -> impl Strategy<Value = Vec<Property>> {
        (0..5_usize).prop_flat_map(move |n| {
            (0..n)
                .map(|i| property(format!("{prefix}{i}"), complex))
                .collect::<Vec<_>>()
        })
    }

    pub(crate) fn model() -> impl Strategy<Value = Model> {
        (1..4_usize, 0..3_usize, any::<bool>()).prop_flat_map(|(types, complex, alias)| {
        let q = if alias { "a" } else { "p" };
        let entity_types = (0..types)
            .map(|i| {
                let navigations = prop::collection::vec((0..=types, any::<bool>(), any::<bool>()), 0..3);
                (properties("v", complex), navigations, annotations(), any::<bool>()).prop_map(
                    move |(properties, navigations, annotations, open)| {
                        let mut t = EntityType::new(format!("T{i}"));
                        t.base_type = Some(format!("{q}.Base"));
                        t.open = open;
                        t.properties = properties;
                        t.annotations = annotations;
                        t.navigation_properties = navigations
                            .into_iter()
                            .enumerate()
                            .map(|(n, (target, collection, nullable))| {
                                let target = if target == types { "Base".to_owned() } else { format!("T{target}") };
                                let mut nav = NavigationProperty::new(
                                    format!("n{n}"),
                                    TypeRef { name: format!("{q}.{target}"), collection },
                                );
                                nav.nullable = nullable && !collection;
                                nav
                            })
                            .collect();
                        t
                    },
                )
            })
            .collect::<Vec<_>>();
        let complex_types = (0..complex)
            .map(|i| {
                // A complex type only uses the ones before it, so no type contains itself.
                (properties("w", i), any::<bool>()).prop_map(move |(properties, open)| {
                    let mut c = ComplexType::new(format!("C{i}"));
                    c.open = open;
                    c.properties = properties;
                    c
                })
            })
            .collect::<Vec<_>>();
        let terms = prop::collection::vec(
            (prop::sample::select(PRIMITIVES.to_vec()), any::<bool>(), any::<bool>(), prop::sample::subsequence(vec!["Property", "EntityType", "EntitySet"], 0..3)),
            0..3,
        );
        (entity_types, complex_types, terms, annotations(), annotations()).prop_map(
            move |(entity_types, complex_types, terms, on_schema, on_container)| {
                let mut base = EntityType::new("Base");
                base.is_abstract = true;
                base.key = vec!["id".into()];
                base.properties = vec![Property::new("id", TypeRef::single("Edm.String")).required()];
                let mut container = EntityContainer::new("C");
                container.annotations = on_container;
                container.entity_sets = entity_types
                    .iter()
                    .map(|t| {
                        let mut set = EntitySet::new(format!("S{}", t.name), format!("{q}.{}", t.name));
                        set.bindings = t
                            .navigation_properties
                            .iter()
                            .map(|nav| {
                                let target = nav.ty.name.rsplit('.').next().unwrap();
                                if target == "Base" {
                                    NavigationPropertyBinding { path: format!("{}/{q}.T0", nav.name), target: "ST0".into() }
                                } else {
                                    NavigationPropertyBinding { path: nav.name.clone(), target: format!("S{target}") }
                                }
                            })
                            .collect();
                        set
                    })
                    .collect();
                let mut schema = Schema::new("p");
                schema.alias = alias.then(|| "a".to_owned());
                schema.entity_types = std::iter::once(base).chain(entity_types).collect();
                schema.complex_types = complex_types;
                schema.terms = terms
                    .into_iter()
                    .enumerate()
                    .map(|(i, (ty, collection, nullable, applies))| {
                        let mut term = Term::new(format!("Term{i}"), TypeRef { name: ty.to_owned(), collection });
                        term.nullable = nullable;
                        term.applies_to = applies.into_iter().map(str::to_owned).collect();
                        term
                    })
                    .collect();
                schema.entity_container = Some(container);
                schema.annotations = on_schema;
                Model {
                    version: CsdlVersion::V4_01,
                    references: vec![Reference {
                        uri: "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/Org.OData.Core.V1.xml".into(),
                        includes: vec![Include { namespace: "Org.OData.Core.V1".into(), alias: Some("Core".into()) }],
                    }],
                    schemas: vec![schema],
                }
            },
        )
    })
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 2 } else { 128 }))]

    #[test]
    fn xml_and_json_describe_one_model(model in strategy::model()) {
        validate(&model, &Limits::strict()).unwrap();
        let (from_xml, from_json) = read::both(&model);
        prop_assert_eq!(from_xml, from_json);
    }
}
