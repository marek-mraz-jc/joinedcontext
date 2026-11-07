//! OData write bodies (`odata_json::read_entity_body`, threat model O22): never panics under
//! `BodyLimits`, and what it accepts, written back as a body (keys, properties, `@odata.bind`),
//! reads back to the same payload (T-3504).
#![no_main]
use libfuzzer_sys::fuzz_target;
use odata_csdl::{
    ComplexType, CsdlVersion, EntityContainer, EntitySet, EntityType, Model, NavigationProperty, Property, Schema,
    TypeRef,
};
use odata_json::{read_entity_body, BodyError, BodyLimits, Ctx, MetadataLevel, ODataVersion, Resource, WriteKind};
use odata_url::Segment;
use serde_json::{Map, Value};

fn model() -> Model {
    let single = |name: &str, ty: &str| Property::new(name, TypeRef::single(ty));
    let mut building = EntityType::new("Building");
    building.key.push("id".into());
    building.properties = vec![
        single("id", "Edm.String").required(),
        single("name", "Edm.String"),
        single("height", "Edm.Int32"),
        single("area", "Edm.Double"),
        single("extra", "Edm.Untyped"),
        Property::new("tags", TypeRef::collection("Edm.String")),
        single("address", "City.Address"),
    ];
    building.navigation_properties = vec![
        NavigationProperty::new("owner", TypeRef::single("City.Building")),
        NavigationProperty::new("parts", TypeRef::collection("City.Building")),
    ];
    let mut address = ComplexType::new("Address");
    address.properties = vec![single("street", "Edm.String"), Property::new("floors", TypeRef::collection("Edm.Int64"))];
    let mut city = Schema::new("City");
    city.entity_types.push(building);
    city.complex_types.push(address);
    let mut service = EntityContainer::new("Service");
    service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
    city.entity_container = Some(service);
    Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] }
}

fuzz_target!(|data: &[u8]| {
    let model = model();
    let Some(buildings) = Resource::new(&model, "Buildings") else {
        return;
    };
    let ctx = Ctx { service_root: "https://example.org/odata/", level: MetadataLevel::Minimal, version: ODataVersion::V4_01, metadata_etag: None };
    let limits = BodyLimits { max_bytes: 4096, max_properties: 64, max_bind_links: 4, max_collection_len: 32, ..BodyLimits::strict() };
    let Ok(payload) = read_entity_body(&ctx, &buildings, WriteKind::Update, data, &limits) else {
        return;
    };
    let mut body: Map<String, Value> = payload.key.iter().chain(&payload.properties).cloned().collect();
    let mut longest = 0;
    for bind in &payload.binds {
        let url = format!("{}{}", bind.target.set, Segment::Key(bind.target.key.clone()));
        longest = longest.max(url.len());
        body.insert(format!("{}@odata.bind", bind.navigation), Value::String(url));
    }
    let written = Value::Object(body).to_string();
    let again = read_entity_body(&ctx, &buildings, WriteKind::Update, written.as_bytes(), &limits);
    match again {
        // The canonical key spells a space `%20` and may outgrow the literal bound.
        Err(BodyError::InvalidBind { .. }) if longest > limits.bind_url.max_literal_len => {}
        _ => assert_eq!(again.as_ref(), Ok(&payload), "{written}"),
    }
});
