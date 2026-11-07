// SPDX-License-Identifier: EUPL-1.2
//! Both writers on one small model that reaches every branch of each: the outputs are pinned
//! byte for byte, so a change to either representation is a reviewed change.

use crate::*;

fn core() -> Reference {
    Reference {
        uri: "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/Org.OData.Core.V1.xml"
            .into(),
        includes: vec![Include {
            namespace: "Org.OData.Core.V1".into(),
            alias: Some("Core".into()),
        }],
    }
}

fn path_values() -> Expr {
    Expr::Record(vec![
        ("P".into(), Expr::Path("a/b".into())),
        ("Q".into(), Expr::PropertyPath("a".into())),
        ("N".into(), Expr::NavigationPropertyPath("n".into())),
        (
            "E".into(),
            Expr::EnumMember {
                ty: "Core.Permission".into(),
                members: vec!["Read".into(), "Write".into()],
            },
        ),
        ("I".into(), Expr::Int(-7)),
        ("C".into(), Expr::Collection(vec![])),
        ("R".into(), Expr::Record(vec![])),
        (
            "L".into(),
            Expr::Collection(vec![
                Expr::String("x<\"y\"\r\r\nz".into()),
                Expr::Bool(false),
                Expr::Record(vec![("V".into(), Expr::Collection(vec![Expr::Int(1)]))]),
            ]),
        ),
    ])
}

/// The container of [`every_branch`]: bindings with and without a cast, sets with and without
/// annotations.
fn every_container() -> EntityContainer {
    let mut container = EntityContainer::new("Box");
    let mut things = EntitySet::new("Things", "t.Thing");
    things.bindings = vec![
        NavigationPropertyBinding {
            path: "owner".into(),
            target: "Things".into(),
        },
        NavigationPropertyBinding {
            path: "next/t.Thing".into(),
            target: "Things".into(),
        },
    ];
    let mut tagged = EntitySet::new("Tagged", "t.Thing");
    tagged.annotations = vec![Annotation::new(
        "Core.Description",
        Expr::String("only annotated".into()),
    )];
    container.entity_sets = vec![things, tagged, EntitySet::new("Bare", "t.Thing")];
    container.annotations = vec![Annotation::new(
        "Core.Description",
        Expr::String("box".into()),
    )];

    container
}

/// Every construct and every optional attribute at least once each way.
pub(crate) fn every_branch() -> Model {
    let mut empty = EntityType::new("Empty");
    empty.is_abstract = true;
    let mut base = EntityType::new("Base");
    base.is_abstract = true;
    base.open = true;
    base.key = vec!["id".into()];
    base.properties = vec![Property::new("id", TypeRef::single("Edm.Guid")).required()];
    let mut thing = EntityType::new("Thing");
    thing.base_type = Some("t.Base".into());
    let mut amount = Property::new("amount", TypeRef::single("Edm.Decimal"));
    amount.precision = Some(9);
    amount.scale = Some(Scale::Digits(2));
    let mut ratio = Property::new("ratio", TypeRef::single("Edm.Decimal"));
    ratio.scale = Some(Scale::Floating);
    let mut any = Property::new("any", TypeRef::single("Edm.Decimal"));
    any.scale = Some(Scale::Variable);
    let mut shape = Property::new("shape", TypeRef::single("Edm.Geometry"));
    shape.srid = Some(Srid::Variable);
    let mut spot = Property::new("spot", TypeRef::single("Edm.GeographyPoint"));
    spot.srid = Some(Srid::Id(4326));
    let mut at = Property::new("at", TypeRef::single("Edm.DateTimeOffset"));
    at.annotations = vec![Annotation::new("Core.Computed", Expr::Bool(true))];
    thing.properties = vec![
        Property::new("tags", TypeRef::collection("Edm.String")),
        Property::new("whole", TypeRef::single("Edm.Decimal")),
        amount,
        ratio,
        any,
        shape,
        spot,
        at,
        Property::new("raw", TypeRef::single("Edm.Untyped")).required(),
        Property::new("where", TypeRef::single("t.Place")),
        Property::new("ownerId", TypeRef::single("Edm.Guid")).required(),
        Property::new("nextId", TypeRef::single("Edm.Guid")),
    ];
    let mut owner = NavigationProperty::new("owner", TypeRef::single("t.Thing"));
    owner.nullable = false;
    owner.partner = Some("parts".into());
    owner.referential_constraints = vec![ReferentialConstraint::new("ownerId", "id")];
    let mut parts = NavigationProperty::new("parts", TypeRef::collection("t.Thing"));
    parts.nullable = false;
    parts.partner = Some("owner".into());
    let mut next = NavigationProperty::new("next", TypeRef::single("t.Base"));
    next.annotations = vec![Annotation::new(
        "Core.Description",
        Expr::String("tab\there".into()),
    )];
    next.referential_constraints = vec![ReferentialConstraint::new("nextId", "id")];
    thing.navigation_properties = vec![owner, parts, next];
    let mut described = Annotation::new("Core.Description", Expr::String("thing".into()));
    described.qualifier = Some("en".into());
    thing.annotations = vec![described];

    let mut place = ComplexType::new("Place");
    place.open = true;
    place.annotations = vec![Annotation::new("t.Note", path_values())];
    let mut note = Term::new("Note", TypeRef::single("t.Place"));
    note.applies_to = vec!["EntityType".into(), "Schema".into()];
    let mut when = Term::new("When", TypeRef::collection("Edm.TimeOfDay"));
    when.nullable = false;
    when.precision = Some(3);
    when.annotations = vec![Annotation::new(
        "Core.Description",
        Expr::String("w".into()),
    )];

    let mut schema = Schema::new("Test.Space");
    schema.alias = Some("t".into());
    schema.entity_types = vec![empty, base, thing];
    schema.complex_types = vec![place, ComplexType::new("Nothing")];
    // A Decimal term without a Scale: 0 in CSDL XML, written out as 0 in CSDL JSON.
    schema.terms = vec![
        note,
        when,
        Term::new("Rate", TypeRef::single("Edm.Decimal")),
    ];
    schema.entity_container = Some(every_container());
    schema.annotations = vec![Annotation::new(
        "Core.SchemaVersion",
        Expr::String("2".into()),
    )];
    // A schema with members but no container.
    let mut shape = ComplexType::new("Shape");
    shape.properties = vec![Property::new("kind", TypeRef::single("Edm.String"))];
    let mut shapes = Schema::new("Shapes.Space");
    shapes.complex_types = vec![shape];
    Model {
        version: CsdlVersion::V4_01,
        references: vec![
            core(),
            Reference {
                uri: "https://example.org/own.xml".into(),
                includes: vec![Include {
                    namespace: "Example.Own".into(),
                    alias: None,
                }],
            },
        ],
        schemas: vec![schema, Schema::new("Empty.Space"), shapes],
    }
}

fn xml(model: &Model) -> String {
    let mut out = Vec::new();
    write_xml(model, &Limits::strict(), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn json(model: &Model) -> String {
    let mut out = Vec::new();
    write_json(model, &Limits::strict(), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// Regenerates the pinned outputs when `ODATA_CSDL_BLESS` is set, else compares with them.
fn pinned(name: &str, written: &str, pinned: &str) {
    if std::env::var_os("ODATA_CSDL_BLESS").is_some() {
        let path = format!("{}/src/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::write(path, written).unwrap();
    } else {
        assert_eq!(
            written, pinned,
            "{name} changed; rerun with ODATA_CSDL_BLESS=1 and review"
        );
    }
}

#[test]
fn the_xml_writer_is_pinned() {
    pinned(
        "every.xml",
        &xml(&every_branch()),
        include_str!("golden/every.xml"),
    );
}

#[test]
fn the_json_writer_is_pinned() {
    pinned(
        "every.json",
        &json(&every_branch()),
        include_str!("golden/every.json"),
    );
}

/// No reference, no annotation, an empty container: the short forms of both writers.
#[test]
fn a_bare_model_writes_its_short_forms() {
    let mut schema = Schema::new("ns");
    schema.entity_container = Some(EntityContainer::new("Box"));
    let bare = Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![schema, Schema::new("Other")],
    };
    let (mut xml, mut json) = (Vec::new(), Vec::new());
    write_xml(&bare, &Limits::strict(), &mut xml).unwrap();
    write_json(&bare, &Limits::strict(), &mut json).unwrap();
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains(r#"<EntityContainer Name="Box"/>"#), "{xml}");
    assert!(
        xml.contains(
            r#"<Schema xmlns="http://docs.oasis-open.org/odata/ns/edm" Namespace="Other"/>"#
        ),
        "{xml}"
    );
    assert!(!xml.contains("edmx:Reference"), "{xml}");
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "$Version": "4.01",
            "$EntityContainer": "ns.Box",
            "ns": { "Box": { "$Kind": "EntityContainer" } },
            "Other": {}
        })
    );
    let (mut xml, mut json) = (Vec::new(), Vec::new());
    let none = Model {
        version: CsdlVersion::V4_01,
        references: vec![],
        schemas: vec![Schema::new("ns")],
    };
    write_xml(&none, &Limits::strict(), &mut xml).unwrap();
    write_json(&none, &Limits::strict(), &mut json).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json).unwrap(),
        serde_json::json!({ "$Version": "4.01", "ns": {} })
    );
}

#[test]
fn a_4_0_model_is_xml_only() {
    let mut model = every_branch();
    model.version = CsdlVersion::V4_0;
    // Edm.Untyped is 4.01: drop the one property that has it.
    model.schemas[0].entity_types[2]
        .properties
        .retain(|p| p.name != "raw");
    assert!(xml(&model).contains(
        r#"<edmx:Edmx xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx" Version="4.0">"#
    ));
    let mut out = Vec::new();
    let refused = write_json(&model, &Limits::strict(), &mut out);
    assert!(matches!(
        refused,
        Err(CsdlError::NotImplemented(Feature::CsdlJson40))
    ));
    assert_eq!(out, Vec::<u8>::new());
}

#[test]
fn an_invalid_model_writes_nothing() {
    let mut model = every_branch();
    model.schemas[0].entity_types[0].is_abstract = false;
    let (mut xml_out, mut json_out) = (Vec::new(), Vec::new());
    for refused in [
        write_xml(&model, &Limits::strict(), &mut xml_out),
        write_json(&model, &Limits::strict(), &mut json_out),
    ] {
        let Err(CsdlError::Invalid(problems)) = refused else {
            panic!("{refused:?}")
        };
        assert_eq!(
            problems[0].to_string(),
            "Test.Space.Empty: entity type without a key"
        );
    }
    assert!(xml_out.is_empty() && json_out.is_empty());
}

/// A sink that takes `room` bytes and then fails.
struct Full {
    room: usize,
}

impl std::io::Write for Full {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.room < buf.len() {
            return Err(std::io::Error::other("disk full"));
        }
        self.room = self.room.saturating_sub(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_failing_sink_is_an_io_error_wherever_it_fails() {
    let model = every_branch();
    let sizes = [xml(&model).len(), json(&model).len()];
    for (writer, size) in [(0, sizes[0]), (1, sizes[1])] {
        // Every seventh prefix: the error surfaces from whichever write it hits. Miri interprets
        // each write, so it samples a few hundred bytes apart.
        for room in (0..size).step_by(if cfg!(miri) { 293 } else { 7 }) {
            let mut sink = Full { room };
            let result = if writer == 0 {
                write_xml(&model, &Limits::strict(), &mut sink)
            } else {
                write_json(&model, &Limits::strict(), &mut sink)
            };
            let Err(err @ CsdlError::Io(_)) = result else {
                panic!("{writer} at {room}: {result:?}")
            };
            assert_eq!(err.to_string(), "write failed: disk full");
        }
    }
}

#[test]
fn vocabulary_references_name_their_json_form() {
    let uri = |uri: &str| {
        let mut model = every_branch();
        model.references[0].uri = uri.into();
        json(&model)
    };
    assert!(
        uri("https://sap.github.io/odata-vocabularies/vocabularies/Common.xml")
            .contains("\"https://sap.github.io/odata-vocabularies/vocabularies/Common.json\"")
    );
    assert!(uri(
        "https://oasis-tcs.github.io/odata-vocabularies/vocabularies/Org.OData.Core.V1.json"
    )
    .contains("vocabularies/Org.OData.Core.V1.json\""));
    assert!(
        uri("https://example.org/vocabularies/Org.OData.Core.V1.xml")
            .contains("\"https://example.org/vocabularies/Org.OData.Core.V1.xml\"")
    );
}

#[test]
fn errors_read_as_fixed_text() {
    let problems = [
        Problem::InvalidIdentifier,
        Problem::InvalidNamespace,
        Problem::InvalidQualifiedName,
        Problem::Duplicate,
        Problem::UnknownNamespace,
        Problem::UnknownType,
        Problem::UnknownTerm,
        Problem::EntityTypedProperty,
        Problem::NotAnEntityType,
        Problem::UntypedIn40,
        Problem::FacetNotApplicable,
        Problem::MissingKey,
        Problem::KeyOnDerivedType,
        Problem::KeyNotFound,
        Problem::InvalidKeyProperty,
        Problem::BaseTypeNotFound,
        Problem::NullableCollectionNavigation,
        Problem::InvalidPartner,
        Problem::InvalidReferentialConstraint,
        Problem::InvalidBindingPath,
        Problem::UnknownBindingTarget,
        Problem::UnknownEntitySetType,
        Problem::SecondContainer,
        Problem::InvalidCharacter,
        Problem::InvalidEnumMember,
        Problem::InvalidPath,
        Problem::InvalidAppliesTo,
        Problem::EmptyReference,
    ];
    let texts: std::collections::HashSet<_> = problems.iter().map(|p| p.as_str()).collect();
    assert_eq!(texts.len(), problems.len());
    let invalid = CsdlError::Invalid(vec![
        ValidationError {
            at: "a".into(),
            problem: Problem::Duplicate,
        },
        ValidationError {
            at: "b".into(),
            problem: Problem::InvalidPath,
        },
    ]);
    assert_eq!(
        invalid.to_string(),
        "invalid CSDL model (2 problems); a: name already taken; b: invalid path"
    );
    let limit = CsdlError::LimitExceeded {
        limit: LimitKind::StringLen,
        max: 9,
    };
    assert_eq!(limit.to_string(), "limit exceeded: string length (max 9)");
    assert_eq!(LimitKind::Elements.as_str(), "model elements");
    assert_eq!(LimitKind::ExprDepth.as_str(), "annotation expression depth");
    let features = [
        Feature::DerivedType,
        Feature::StreamProperty,
        Feature::CsdlJson40,
    ];
    for feature in features {
        assert!(CsdlError::NotImplemented(feature)
            .to_string()
            .starts_with("not implemented: "));
    }
    let io: CsdlError = std::io::Error::other("x").into();
    assert!(std::error::Error::source(&io).is_none());
}

#[test]
fn lookups_resolve_aliases_bases_and_references() {
    let mut city = Schema::new("City");
    city.alias = Some("c".into());
    let mut tower = EntityType::new("Tower");
    tower.base_type = Some("c.Building".into());
    let mut building = EntityType::new("Building");
    building.base_type = Some("City.Address".into());
    let mut looped = EntityType::new("Loop");
    looped.base_type = Some("City.Loop".into());
    city.entity_types = vec![tower, building, looped];
    city.complex_types.push(ComplexType::new("Address"));
    let mut service = EntityContainer::new("Service");
    service
        .entity_sets
        .push(EntitySet::new("Towers", "City.Tower"));
    city.entity_container = Some(service);
    let model = Model {
        version: CsdlVersion::V4_01,
        references: vec![core()],
        schemas: vec![Schema::new("Empty"), city],
    };
    assert_eq!(model.namespace("Core"), Some("Org.OData.Core.V1"));
    assert_eq!(
        model.namespace("Org.OData.Core.V1"),
        Some("Org.OData.Core.V1")
    );
    assert_eq!(
        model.full_name("Core.Computed").as_deref(),
        Some("Org.OData.Core.V1.Computed")
    );
    assert_eq!(model.full_name("Computed"), None);
    assert!(model.entity_type("Tower").is_none());
    assert!(model.entity_type("Nowhere.Tower").is_none());
    assert!(model.complex_type("Address").is_none());
    assert!(model.entity_type("c.Address").is_none());
    assert!(model.complex_type("c.Tower").is_none());
    assert!(model.complex_type("c.Address").is_some());
    assert!(model.entity_set("Towers").is_some());
    let names = |ty: &str| -> Vec<String> {
        model
            .entity_type(ty)
            .map(|t| model.lineage(t).iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default()
    };
    // A complex base ends the walk; a cycle stops after as many steps as there are types.
    assert_eq!(names("c.Tower"), ["Tower", "Building"]);
    assert_eq!(names("City.Loop"), ["Loop", "Loop", "Loop"]);
}
