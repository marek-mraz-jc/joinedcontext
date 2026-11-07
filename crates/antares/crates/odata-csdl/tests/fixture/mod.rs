// SPDX-License-Identifier: EUPL-1.2
//! The model of one NGSI-LD view as the OData surface describes it (odata-linkml-mapping.md
//! sections 3 and 4): every construct and annotation term the surface uses, so the OASIS
//! schemas, the converter and the vocabularies see each of them once.

use odata_csdl::{
    Annotation, ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Expr, Include,
    Model, NavigationProperty, NavigationPropertyBinding, Property, Reference, Scale, Schema, Srid,
    Term, TypeRef,
};

const VOCABULARIES: &str = "https://oasis-tcs.github.io/odata-vocabularies/vocabularies";

fn vocabulary(name: &str, alias: &str) -> Reference {
    Reference {
        uri: format!("{VOCABULARIES}/Org.OData.{name}.V1.xml"),
        includes: vec![Include {
            namespace: format!("Org.OData.{name}.V1"),
            alias: Some(alias.to_owned()),
        }],
    }
}

fn tag(term: &str) -> Annotation {
    Annotation::new(term, Expr::Bool(true))
}

fn string(text: &str) -> Expr {
    Expr::String(text.to_owned())
}

fn paths(names: &[&str]) -> Expr {
    Expr::Collection(
        names
            .iter()
            .map(|n| Expr::PropertyPath((*n).to_owned()))
            .collect(),
    )
}

fn record(values: Vec<(&str, Expr)>) -> Expr {
    Expr::Record(values.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// NGSI-LD `DateTime`: `YYYY-MM-DDThh:mm:ss[.ffffff]Z` (TS 104 175 5.2.2.4).
fn microseconds(mut property: Property) -> Property {
    property.precision = Some(6);
    property
}

fn annotated(mut property: Property, annotations: Vec<Annotation>) -> Property {
    property.annotations = annotations;
    property
}

/// The surface's own vocabulary schema `Antares.NgsiLd` (mapping 3.2).
fn ngsi_ld() -> Schema {
    let mut schema = Schema::new("Antares.NgsiLd");
    schema.alias = Some("NgsiLd".to_owned());
    let mut iri = Term::new("Iri", TypeRef::single("Edm.String"));
    iri.nullable = false;
    iri.applies_to = vec![
        "EntityType".into(),
        "Property".into(),
        "NavigationProperty".into(),
    ];
    iri.annotations = vec![Annotation::new(
        "Core.Description",
        string("The NGSI-LD IRI of the type or attribute"),
    )];
    let mut kind = Term::new("Kind", TypeRef::single("Edm.String"));
    kind.applies_to = vec!["Property".into()];
    let mut observed_at = Term::new("observedAt", TypeRef::single("Edm.DateTimeOffset"));
    observed_at.precision = Some(6);
    schema.terms = vec![
        iri,
        kind,
        observed_at,
        Term::new("datasetId", TypeRef::single("Edm.String")),
    ];
    let mut entity = EntityType::new("Entity");
    entity.is_abstract = true;
    entity.key = vec!["id".to_owned()];
    entity.properties = vec![
        Property::new("id", TypeRef::single("Edm.String")).required(),
        annotated(
            Property::new("_types", TypeRef::collection("Edm.String")).required(),
            vec![tag("Core.Computed")],
        ),
    ];
    schema.entity_types = vec![entity];
    let mut dataset = ComplexType::new("DatasetInstance");
    dataset.properties = vec![
        Property::new("attribute", TypeRef::single("Edm.String")).required(),
        Property::new("datasetId", TypeRef::single("Edm.String")),
        Property::new("value", TypeRef::single("Edm.Untyped")),
        microseconds(Property::new(
            "observedAt",
            TypeRef::single("Edm.DateTimeOffset"),
        )),
    ];
    schema.complex_types = vec![dataset];
    schema
}

/// The view's schema: two types, a relationship each way, a complex property, and the
/// Capabilities annotations of mapping section 4.
fn view() -> Schema {
    let mut schema = Schema::new("Antares.View.air_quality");
    schema.alias = Some("View".to_owned());
    schema.annotations = vec![Annotation::new("Core.SchemaVersion", string("1.4.0+v7"))];

    let mut address = ComplexType::new("Address");
    address.properties = vec![
        Property::new("streetAddress", TypeRef::single("Edm.String")),
        Property::new("addressLocality", TypeRef::single("Edm.String")),
    ];
    schema.complex_types = vec![address];

    schema.entity_types = vec![observed(), device()];
    schema.entity_container = Some(container());

    schema
}

/// The whole document.
pub fn ngsi_ld_model() -> Model {
    Model {
        version: CsdlVersion::V4_01,
        references: vec![
            vocabulary("Core", "Core"),
            vocabulary("Capabilities", "Capabilities"),
            vocabulary("Measures", "Measures"),
            vocabulary("Validation", "Validation"),
        ],
        schemas: vec![ngsi_ld(), view()],
    }
}

/// `AirQualityObserved`: every structural mapping construct of mapping section 3.
fn observed() -> EntityType {
    let iri = |text: &str| Annotation::new("NgsiLd.Iri", string(text));
    let mut observed = EntityType::new("AirQualityObserved");
    observed.base_type = Some("NgsiLd.Entity".to_owned());
    observed.annotations = vec![iri(
        "https://smartdatamodels.org/dataModel.Environment/AirQualityObserved",
    )];
    let mut pm25 = Property::new("pm25", TypeRef::single("Edm.Decimal"));
    pm25.scale = Some(Scale::Variable);
    pm25.annotations = vec![
        Annotation::new("Measures.UNECEUnit", string("GQ")),
        Annotation::new("Measures.Unit", string("ug/m3")),
    ];
    let mut location = Property::new("location", TypeRef::single("Edm.GeographyPoint"));
    location.srid = Some(Srid::Id(4326));
    observed.properties = vec![
        annotated(
            Property::new("scope", TypeRef::collection("Edm.String")).required(),
            vec![Annotation::new("NgsiLd.Kind", string("scope"))],
        ),
        annotated(
            microseconds(Property::new(
                "createdAt",
                TypeRef::single("Edm.DateTimeOffset"),
            )),
            vec![tag("Core.Computed")],
        ),
        microseconds(Property::new(
            "dateObserved",
            TypeRef::single("Edm.DateTimeOffset"),
        )),
        annotated(
            Property::new("NO2", TypeRef::single("Edm.Double")),
            vec![iri("https://smartdatamodels.org/dataModel.Environment/NO2")],
        ),
        pm25,
        location,
        annotated(
            Property::new("name", TypeRef::single("Edm.String")),
            vec![tag("Core.IsLanguageDependent")],
        ),
        annotated(
            Property::new("category", TypeRef::single("Edm.String")),
            vec![Annotation::new(
                "Validation.AllowedValues",
                Expr::Collection(vec![
                    record(vec![("Value", string("pm2.5"))]),
                    record(vec![("Value", string("NO2 & \"ozone\" <fine>"))]),
                ]),
            )],
        ),
        annotated(
            Property::new("readings", TypeRef::collection("Edm.Double")),
            vec![tag("Core.Ordered")],
        ),
        Property::new("raw", TypeRef::single("Edm.Untyped")),
        annotated(
            Property::new("refHidden", TypeRef::single("Edm.String")),
            vec![tag("Core.IsURL")],
        ),
        Property::new("address", TypeRef::single("View.Address")),
        annotated(
            Property::new("_datasets", TypeRef::collection("NgsiLd.DatasetInstance")).required(),
            vec![tag("Core.Computed")],
        ),
    ];
    let mut ref_device = NavigationProperty::new("refDevice", TypeRef::single("View.Device"));
    ref_device.partner = Some("observations".to_owned());
    observed.navigation_properties = vec![ref_device];

    observed
}

/// `Device`: the facet defaults and both navigation property directions.
fn device() -> EntityType {
    let mut device = EntityType::new("Device");
    device.base_type = Some("NgsiLd.Entity".to_owned());
    let mut battery = Property::new("battery", TypeRef::single("Edm.Decimal"));
    battery.precision = Some(5);
    battery.scale = Some(Scale::Digits(2));
    let mut drift = Property::new("drift", TypeRef::single("Edm.Duration"));
    drift.precision = Some(3);
    device.properties = vec![
        Property::new("serialNumber", TypeRef::single("Edm.String")),
        // No Precision: 0 in CSDL XML, written out as 0 in CSDL JSON.
        Property::new("lastSeen", TypeRef::single("Edm.DateTimeOffset")),
        battery,
        // No Scale: 0 in CSDL XML, written out as 0 in CSDL JSON.
        Property::new("weight", TypeRef::single("Edm.Decimal")),
        drift,
    ];
    let mut observations = NavigationProperty::new(
        "observations",
        TypeRef::collection("View.AirQualityObserved"),
    );
    observations.nullable = false;
    observations.partner = Some("refDevice".to_owned());
    let mut owner = NavigationProperty::new("owner", TypeRef::single("NgsiLd.Entity"));
    owner.nullable = false;
    device.navigation_properties = vec![observations, owner];
    device
}

/// The Capabilities annotations of mapping section 4 on the `AirQualityObserved` set.
fn observed_set() -> EntitySet {
    let insert = Annotation::new(
        "Capabilities.InsertRestrictions",
        record(vec![
            ("Insertable", Expr::Bool(false)),
            (
                "NonInsertableProperties",
                paths(&["createdAt", "_types", "_datasets"]),
            ),
            (
                "NonInsertableNavigationProperties",
                Expr::Collection(vec![Expr::NavigationPropertyPath("refDevice".to_owned())]),
            ),
        ]),
    );
    let mut observed_set = EntitySet::new("AirQualityObserved", "View.AirQualityObserved");
    observed_set.bindings = vec![NavigationPropertyBinding {
        path: "refDevice".to_owned(),
        target: "Device".to_owned(),
    }];
    observed_set.annotations = vec![
        insert,
        Annotation::new(
            "Capabilities.UpdateRestrictions",
            record(vec![
                ("Updatable", Expr::Bool(true)),
                ("Upsertable", Expr::Bool(false)),
                ("DeltaUpdateSupported", Expr::Bool(false)),
                ("NonUpdatableProperties", paths(&["createdAt"])),
            ]),
        ),
        Annotation::new(
            "Capabilities.DeleteRestrictions",
            record(vec![("Deletable", Expr::Bool(false))]),
        ),
        Annotation::new(
            "Capabilities.FilterRestrictions",
            record(vec![
                ("Filterable", Expr::Bool(true)),
                ("MaxLevels", Expr::Int(1)),
                (
                    "NonFilterableProperties",
                    paths(&["raw", "_datasets", "_types"]),
                ),
            ]),
        ),
        Annotation::new(
            "Capabilities.SortRestrictions",
            record(vec![
                ("Sortable", Expr::Bool(true)),
                ("NonSortableProperties", paths(&["raw", "readings", "name"])),
            ]),
        ),
        Annotation::new(
            "Capabilities.ExpandRestrictions",
            record(vec![
                ("Expandable", Expr::Bool(false)),
                ("NonExpandableProperties", paths(&[])),
            ]),
        ),
        Annotation::new(
            "Capabilities.CountRestrictions",
            record(vec![("Countable", Expr::Bool(true))]),
        ),
        Annotation::new(
            "Capabilities.SearchRestrictions",
            record(vec![("Searchable", Expr::Bool(false))]),
        ),
        Annotation::new(
            "Capabilities.DeepInsertSupport",
            record(vec![("Supported", Expr::Bool(false))]),
        ),
        Annotation::new(
            "Capabilities.DeepUpdateSupport",
            record(vec![("Supported", Expr::Bool(false))]),
        ),
        Annotation::new(
            "Capabilities.NavigationRestrictions",
            record(vec![("RestrictedProperties", Expr::Collection(vec![]))]),
        ),
    ];
    observed_set
}

fn container() -> EntityContainer {
    let mut device_set = EntitySet::new("Device", "View.Device");
    device_set.bindings = vec![
        NavigationPropertyBinding {
            path: "observations".to_owned(),
            target: "AirQualityObserved".to_owned(),
        },
        NavigationPropertyBinding {
            path: "owner/View.Device".to_owned(),
            target: "Device".to_owned(),
        },
    ];
    let mut described = Annotation::new(
        "Core.Description",
        string("Devices, as the view shows them"),
    );
    described.qualifier = Some("en".to_owned());
    device_set.annotations = vec![described];

    let mut container = EntityContainer::new("Container");
    container.entity_sets = vec![observed_set(), device_set];
    container.annotations = vec![
        tag("Capabilities.BatchSupported"),
        Annotation::new(
            "Capabilities.BatchSupport",
            record(vec![
                ("ContinueOnErrorSupported", Expr::Bool(true)),
                (
                    "SupportedFormats",
                    Expr::Collection(vec![string("multipart/mixed"), string("application/json")]),
                ),
            ]),
        ),
        Annotation::new(
            "Capabilities.ConformanceLevel",
            Expr::EnumMember {
                ty: "Capabilities.ConformanceLevelType".to_owned(),
                members: vec!["Minimal".to_owned()],
            },
        ),
        Annotation::new("Capabilities.CrossJoinSupported", Expr::Bool(false)),
        Annotation::new("Capabilities.ComputeSupported", Expr::Bool(false)),
        Annotation::new("Capabilities.KeyAsSegmentSupported", Expr::Bool(false)),
        tag("Capabilities.QuerySegmentSupported"),
        Annotation::new(
            "Capabilities.AsynchronousRequestsSupported",
            Expr::Bool(false),
        ),
    ];
    container
}
