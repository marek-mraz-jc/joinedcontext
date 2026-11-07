// SPDX-License-Identifier: EUPL-1.2
//! The model every odata-json test reads: `City` (alias `c`) with Building, Tower (from
//! Building), Person and the complex Address; one property per primitive type the reader
//! checks; Core and Capabilities restrictions on the Buildings set.

use odata_csdl::{
    Annotation, ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Expr, Include,
    Model, NavigationProperty, Property, Reference, Schema, TypeRef,
};
use odata_json::{Ctx, MetadataLevel, ODataVersion, Resource};

pub const ROOT: &str = "https://example.org/odata/";

fn include(namespace: &str, alias: &str) -> Reference {
    Reference {
        uri: format!("https://oasis-tcs.github.io/odata-vocabularies/vocabularies/{namespace}.xml"),
        includes: vec![Include {
            namespace: namespace.into(),
            alias: Some(alias.into()),
        }],
    }
}

fn restrictions(
    term: &str,
    properties: (&str, &[&str]),
    navigations: (&str, &[&str]),
) -> Annotation {
    let paths = |names: &[&str], path: fn(String) -> Expr| {
        Expr::Collection(names.iter().map(|n| path((*n).into())).collect())
    };
    Annotation::new(
        term,
        Expr::Record(vec![
            (properties.0.into(), paths(properties.1, Expr::PropertyPath)),
            (
                navigations.0.into(),
                paths(navigations.1, Expr::NavigationPropertyPath),
            ),
        ]),
    )
}

fn building() -> EntityType {
    let single = |name: &str, ty: &str| Property::new(name, TypeRef::single(ty));
    let mut computed = single("stamp", "Edm.DateTimeOffset");
    computed
        .annotations
        .push(Annotation::new("Core.Computed", Expr::Bool(true)));
    let mut not_computed = single("note", "Edm.String");
    not_computed
        .annotations
        .push(Annotation::new("Core.Computed", Expr::Bool(false)));
    not_computed
        .annotations
        .push(Annotation::new("Core.Description", Expr::Bool(true)));
    let mut building = EntityType::new("Building");
    building.key.push("id".into());
    building.properties = vec![
        single("id", "Edm.String").required(),
        single("name", "Edm.String"),
        single("height", "Edm.Int32").required(),
        single("area", "Edm.Double"),
        single("ratio", "Edm.Single"),
        single("price", "Edm.Decimal"),
        single("count", "Edm.Int64"),
        single("byte", "Edm.Byte"),
        single("sbyte", "Edm.SByte"),
        single("short", "Edm.Int16"),
        single("built", "Edm.Date"),
        single("active", "Edm.Boolean"),
        single("extra", "Edm.Untyped"),
        single("any", "Edm.PrimitiveType"),
        single("shape", "Edm.GeometryPolygon"),
        single("odd", "Nowhere.Thing"),
        Property::new("tags", TypeRef::collection("Edm.String")),
        Property::new("scores", TypeRef::collection("Edm.Int32")).required(),
        Property::new("anything", TypeRef::collection("Edm.Untyped")),
        single("address", "City.Address"),
        Property::new("homes", TypeRef::collection("c.Address")),
        computed,
        not_computed,
        single("createdAt", "Edm.DateTimeOffset"),
        single("label", "Edm.String"),
    ];
    building.navigation_properties = vec![
        NavigationProperty::new("owner", TypeRef::single("City.Person")),
        NavigationProperty::new("parts", TypeRef::collection("City.Building")),
        NavigationProperty::new("manager", TypeRef::single("c.Person")),
        NavigationProperty::new("auditor", TypeRef::single("City.Person")),
    ];
    building
}

pub fn model() -> Model {
    let single = |name: &str, ty: &str| Property::new(name, TypeRef::single(ty));
    let mut address = ComplexType::new("Address");
    address.properties = vec![
        single("street", "Edm.String").required(),
        Property::new("floors", TypeRef::collection("Edm.Int32")),
        single("location", "Edm.GeographyPoint"),
        single("stamp", "Edm.DateTimeOffset"),
    ];
    let building = building();
    let mut tower = EntityType::new("Tower");
    tower.base_type = Some("City.Building".into());
    tower.properties.push(single("floors", "Edm.Int16"));
    let mut person = EntityType::new("Person");
    person.key.push("id".into());
    person
        .properties
        .push(single("id", "Edm.String").required());
    let mut keyless = EntityType::new("Keyless");
    keyless.properties.push(single("id", "Edm.String"));
    let mut city = Schema::new("City");
    city.alias = Some("c".into());
    city.entity_types = vec![building, tower, person, keyless];
    city.complex_types.push(address);
    let mut buildings = EntitySet::new("Buildings", "City.Building");
    buildings.annotations = vec![
        restrictions(
            "Capabilities.InsertRestrictions",
            ("NonInsertableProperties", &["createdAt", "address/stamp"]),
            ("NonInsertableNavigationProperties", &["manager"]),
        ),
        restrictions(
            "Org.OData.Capabilities.V1.UpdateRestrictions",
            ("NonUpdatableProperties", &["label"]),
            ("NonUpdatableNavigationProperties", &["auditor"]),
        ),
        Annotation::new("Capabilities.InsertRestrictions", Expr::Bool(true)),
        Annotation::new(
            "Capabilities.InsertRestrictions",
            Expr::Record(vec![
                (
                    "NonInsertableProperties".into(),
                    Expr::String("not a path list".into()),
                ),
                (
                    "NonInsertableNavigationProperties".into(),
                    Expr::Collection(vec![Expr::Int(1)]),
                ),
            ]),
        ),
        Annotation::new("Capabilities.Unknown", Expr::Record(vec![])),
    ];
    let mut service = EntityContainer::new("Service");
    service.entity_sets = vec![
        buildings,
        EntitySet::new("Towers", "c.Tower"),
        EntitySet::new("People", "City.Person"),
        EntitySet::new("Keyless", "City.Keyless"),
        EntitySet::new("Ghosts", "City.Ghost"),
    ];
    city.entity_container = Some(service);
    Model {
        version: CsdlVersion::V4_01,
        references: vec![
            include("Org.OData.Core.V1", "Core"),
            include("Org.OData.Capabilities.V1", "Capabilities"),
        ],
        schemas: vec![city],
    }
}

pub fn ctx(level: MetadataLevel, version: ODataVersion) -> Ctx<'static> {
    Ctx {
        service_root: ROOT,
        level,
        version,
        metadata_etag: None,
    }
}

pub fn minimal() -> Ctx<'static> {
    ctx(MetadataLevel::Minimal, ODataVersion::V4_0)
}

pub fn resource<'m>(model: &'m Model, set: &str) -> Resource<'m> {
    Resource::new(model, set).unwrap()
}
