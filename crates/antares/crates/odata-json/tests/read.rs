// SPDX-License-Identifier: EUPL-1.2
//! The body reader against the City model: every mapped write form, and every refusal by the
//! error it answers (ADR-0024 decision 5, mapping rows W1, W2, W4, W6, W10, W11, W16, W20).

#[cfg(test)]
mod service;

#[cfg(test)]
mod w1_to_w20 {

    use crate::service::{minimal, model, resource, ROOT};
    use odata_csdl::{Annotation, EntitySet, EntityType, Expr, Property, TypeRef};
    use odata_json::{
        read_entity_body, read_property_body, read_reference_body, BodyError, BodyLimit,
        BodyLimits, EntityPayload, Feature, Resource, Target, WriteKind,
    };
    use odata_url::{Key, KeyValue, Literal};
    use serde_json::{json, Value};

    fn read(set: &str, kind: WriteKind, body: &Value) -> Result<EntityPayload, BodyError> {
        let model = model();
        read_entity_body(
            &minimal(),
            &resource(&model, set),
            kind,
            body.to_string().as_bytes(),
            &BodyLimits::strict(),
        )
    }

    fn create(body: &Value) -> Result<EntityPayload, BodyError> {
        read("Buildings", WriteKind::Create, body)
    }

    fn update(body: &Value) -> Result<EntityPayload, BodyError> {
        read("Buildings", WriteKind::Update, body)
    }

    fn key(id: &str) -> Key {
        Key::Single(KeyValue::Literal(Literal::String(id.into())))
    }

    fn target(set: &str, id: &str) -> Target {
        Target {
            set: set.into(),
            key: key(id),
        }
    }

    fn invalid(property: &str) -> BodyError {
        BodyError::InvalidValue {
            property: property.into(),
        }
    }

    fn read_only(property: &str) -> BodyError {
        BodyError::ReadOnly {
            property: property.into(),
        }
    }

    fn bind_error(navigation: &str) -> BodyError {
        BodyError::InvalidBind {
            navigation: navigation.into(),
        }
    }

    #[test]
    fn a_create_reads_every_type_and_both_bind_forms() {
        let body = json!({
            "@odata.context": "ignored",
            "@odata.type": "#City.Building",
            "@Core.Description": "an instance annotation",
            "name@odata.type": "#String",
            "id": "b1",
            "name": null,
            "height": 42,
            "area": 1.5,
            "ratio": "-INF",
            "price": 10,
            "count": 9_007_199_254_740_993_i64,
            "byte": 255,
            "sbyte": -128,
            "short": -32768,
            "built": "2026-10-10",
            "active": true,
            "extra": {"anything": [1, "two"]},
            "any": "text",
            "shape": {"type": "Polygon", "coordinates": []},
            "tags": ["a", null],
            "scores": [1, 2],
            "anything": [null, {}],
            "address": {"@odata.type": "#c.Address", "street": "Main", "floors": [1], "location": {"type": "Point"}},
            "homes": [{"street": "Side"}, null],
            "note": "not computed",
            "label": "set once",
            "owner@odata.bind": "People('p1')",
            "auditor": {"@id": format!("{ROOT}People('p2')")},
        });
        let payload = create(&body).unwrap();
        assert_eq!(payload.key, vec![("id".to_owned(), json!("b1"))]);
        assert_eq!(payload.properties.len(), 21);
        assert_eq!(
            payload.properties.first().map(|(name, _)| name.as_str()),
            Some("active")
        );
        assert_eq!(
            payload
                .binds
                .iter()
                .map(|b| (b.navigation.as_str(), &b.target))
                .collect::<Vec<_>>(),
            [
                ("auditor", &target("People", "p2")),
                ("owner", &target("People", "p1"))
            ]
        );
    }

    #[test]
    fn updates_and_derived_sets_read_the_same_way() {
        let payload = update(&json!({"@type": "c.Building", "height": 1, "createdAt": "2026-10-10T00:00:00Z", "manager": {"@odata.id": "People('m')"}})).unwrap();
        assert_eq!(payload.key, vec![]);
        assert_eq!(payload.binds.len(), 1);
        let replaced = read(
            "Buildings",
            WriteKind::Replace,
            &json!({"id": "b1", "height": 2}),
        )
        .unwrap();
        assert_eq!(replaced.key.len(), 1);
        let tower = read(
            "Towers",
            WriteKind::Create,
            &json!({"id": "t1", "height": 3, "floors": 4}),
        )
        .unwrap();
        assert_eq!(tower.properties.len(), 2);
        assert_eq!(
            read("Keyless", WriteKind::Create, &json!({})),
            Ok(EntityPayload::default())
        );
        assert_eq!(
            read("Towers", WriteKind::Create, &json!({"height": 1})),
            Err(BodyError::MissingKey {
                property: "id".into()
            })
        );
    }

    #[test]
    fn values_of_the_wrong_type_are_refused() {
        for (body, error) in [
            (json!({"id": 5}), invalid("id")),
            (json!({"id": null}), invalid("id")),
            (json!({"height": "x"}), invalid("height")),
            (json!({"height": 1.5}), invalid("height")),
            (json!({"height": 2_147_483_648_i64}), invalid("height")),
            (json!({"byte": 256}), invalid("byte")),
            (json!({"sbyte": -129}), invalid("sbyte")),
            (json!({"short": 40000}), invalid("short")),
            (json!({"count": 1.5}), invalid("count")),
            (json!({"count": u64::MAX}), invalid("count")),
            (json!({"area": "x"}), invalid("area")),
            (json!({"price": "1"}), invalid("price")),
            (json!({"built": 1}), invalid("built")),
            // F-R2ANT4-012: a string typed clients cannot read back
            (json!({"built": "yesterday"}), invalid("built")),
            (json!({"built": "2026-13-01"}), invalid("built")),
            (json!({"createdAt": "2026-10-10"}), invalid("createdAt")),
            (
                json!({"createdAt": "2026-10-10T10%3A00Z"}),
                invalid("createdAt"),
            ),
            (json!({"active": "true"}), invalid("active")),
            (json!({"shape": "x"}), invalid("shape")),
            (json!({"odd": 1}), invalid("odd")),
            (json!({"address": "x"}), invalid("address")),
            (json!({"address": {"street": 1}}), invalid("address/street")),
            (json!({"address": {"floors": 1}}), invalid("address/floors")),
            (json!({"tags": "x"}), invalid("tags")),
            (json!({"height": [1]}), invalid("height")),
            (json!({"tags": null}), invalid("tags")),
            (json!({"homes": [1]}), invalid("homes")),
            (
                json!({"height": null}),
                BodyError::NullNotAllowed {
                    property: "height".into(),
                },
            ),
            (
                json!({"scores": [null]}),
                BodyError::NullNotAllowed {
                    property: "scores".into(),
                },
            ),
            (
                json!({"address": {"street": null}}),
                BodyError::NullNotAllowed {
                    property: "address/street".into(),
                },
            ),
            (json!({"secret": 1}), BodyError::UnknownProperty),
            (
                json!({"address": {"secret": 1}}),
                BodyError::UnknownProperty,
            ),
            (
                json!({"@odata.type": "#City.Person"}),
                BodyError::TypeMismatch,
            ),
            (json!({"@odata.type": 5}), BodyError::TypeMismatch),
            (
                json!({"@type": "#Nowhere.Building"}),
                BodyError::TypeMismatch,
            ),
            (
                json!({"address": {"@type": "#City.Building"}}),
                BodyError::TypeMismatch,
            ),
        ] {
            assert_eq!(update(&body), Err(error), "{body}");
        }
        assert_eq!(create(&json!([])), Err(BodyError::NotAnObject));
    }

    /// The City model plus `Minted` (key `Core.ComputedDefaultValue`) and
    /// `Sealed` (key `Core.Computed`): keys the service computes.
    fn read_service_keyed(set: &str, body: &Value) -> Result<EntityPayload, BodyError> {
        let mut model = model();
        let city = model
            .schemas
            .first_mut()
            .ok_or(BodyError::UnknownProperty)?;
        for (name, term) in [
            ("Minted", "Core.ComputedDefaultValue"),
            ("Sealed", "Org.OData.Core.V1.Computed"),
        ] {
            let mut ty = EntityType::new(name);
            ty.key.push("id".into());
            let mut id = Property::new("id", TypeRef::single("Edm.Guid")).required();
            id.annotations.push(Annotation::new(term, Expr::Bool(true)));
            ty.properties = vec![id, Property::new("name", TypeRef::single("Edm.String"))];
            city.entity_types.push(ty);
            let mut set = EntitySet::new(name, format!("City.{name}"));
            set.annotations.push(Annotation::new(
                "Capabilities.InsertRestrictions",
                Expr::Record(vec![(
                    "RequiredProperties".into(),
                    Expr::Collection(vec![Expr::PropertyPath("name".into())]),
                )]),
            ));
            if let Some(service) = city.entity_container.as_mut() {
                service.entity_sets.push(set);
            }
        }
        read_entity_body(
            &minimal(),
            &resource(&model, set),
            WriteKind::Create,
            body.to_string().as_bytes(),
            &BodyLimits::strict(),
        )
    }

    #[test]
    fn a_create_leaves_out_a_key_the_service_computes() {
        const GUID: &str = "0f9e8d7c-6b5a-4938-8271-605f4e3d2c1b";
        // Core.ComputedDefaultValue: left out, or named by the client
        let minted = read_service_keyed("Minted", &json!({"name": "a"}));
        assert_eq!(
            minted.map(|p| (p.key.len(), p.properties.len())),
            Ok((0, 1))
        );
        let named = read_service_keyed("Minted", &json!({"id": GUID, "name": "a"}));
        assert_eq!(named.map(|p| p.key), Ok(vec![("id".into(), json!(GUID))]));
        // Core.Computed: left out, and never in a body
        assert!(read_service_keyed("Sealed", &json!({"name": "a"})).is_ok());
        assert_eq!(
            read_service_keyed("Sealed", &json!({"id": GUID})),
            Err(read_only("id"))
        );
        // a key the client names is still required
        assert_eq!(
            read("People", WriteKind::Create, &json!({})),
            Err(BodyError::MissingKey {
                property: "id".into()
            })
        );
    }

    #[test]
    fn a_create_names_every_required_property() {
        let missing = Err(BodyError::MissingProperty {
            property: "name".into(),
        });
        assert_eq!(read_service_keyed("Minted", &json!({})), missing);
        assert_eq!(
            read_service_keyed("Minted", &json!({"name": null})),
            missing
        );
        assert!(read_service_keyed("Minted", &json!({"name": ""})).is_ok());
    }

    #[test]
    fn read_only_properties_are_refused_per_write() {
        assert_eq!(update(&json!({"stamp": "x"})), Err(read_only("stamp")));
        assert_eq!(
            create(&json!({"id": "b", "createdAt": "x"})),
            Err(read_only("createdAt"))
        );
        assert_eq!(
            create(&json!({"id": "b", "address": {"stamp": "x"}})),
            Err(read_only("address/stamp"))
        );
        assert!(update(&json!({"address": {"stamp": "2026-10-10T00:00:00Z"}})).is_ok());
        assert_eq!(update(&json!({"label": "x"})), Err(read_only("label")));
        assert_eq!(
            create(&json!({"id": "b", "manager@odata.bind": "People('m')"})),
            Err(read_only("manager"))
        );
        assert_eq!(
            update(&json!({"auditor": {"@id": "People('a')"}})),
            Err(read_only("auditor"))
        );
        assert_eq!(
            create(&json!({"height": 1})),
            Err(BodyError::MissingKey {
                property: "id".into()
            })
        );
    }

    #[test]
    fn binds_must_address_an_entity_of_the_target_type_here() {
        for url in [
            json!("/odata/People('p1')"),
            json!("https://other.example/odata/People('p1')"),
            json!("Buildings('b1')"),
            json!("People"),
            json!("People('p1')/id"),
            json!("People('p1')?$select=id"),
            json!("$metadata"),
            json!("People("),
            json!("Ghosts('g')"),
            json!("People/$count"),
            json!(5),
            json!(["People('p1')"]),
            json!({"@id": "People('p1')"}),
        ] {
            assert_eq!(
                update(&json!({"owner@odata.bind": url})),
                Err(bind_error("owner")),
                "{url}"
            );
        }
    }

    #[test]
    fn bind_shapes_outside_the_subset_are_answered() {
        for (body, error) in [
            (json!({"owner": "People('p1')"}), bind_error("owner")),
            (
                json!({"owner": [{"@id": "People('p1')"}]}),
                bind_error("owner"),
            ),
            (
                json!({"parts": {"@id": "Buildings('b')"}}),
                bind_error("parts"),
            ),
            (
                json!({"parts@odata.bind": "Buildings('b')"}),
                bind_error("parts"),
            ),
            (
                json!({"owner@odata.bind": "People('p1')", "owner": {"@id": "People('p2')"}}),
                bind_error("owner"),
            ),
            (
                json!({"unknown@odata.bind": "People('p1')"}),
                BodyError::UnknownProperty,
            ),
            (
                json!({"owner@odata.bind": null}),
                BodyError::NotImplemented(Feature::Unbind),
            ),
            (
                json!({"owner": null}),
                BodyError::NotImplemented(Feature::Unbind),
            ),
            (
                json!({"parts@odata.bind": ["Buildings('b')"]}),
                BodyError::NotImplemented(Feature::CollectionBind),
            ),
            (
                json!({"parts": [{"@id": "Buildings('b')"}]}),
                BodyError::NotImplemented(Feature::CollectionBind),
            ),
            (
                json!({"parts": [{"id": "b"}]}),
                BodyError::NotImplemented(Feature::DeepUpdate),
            ),
            (
                json!({"owner": {"id": "p9"}}),
                BodyError::NotImplemented(Feature::DeepUpdate),
            ),
            (
                json!({"owner": {"@id": "People('p1')", "id": "p1"}}),
                BodyError::NotImplemented(Feature::DeepUpdate),
            ),
            (json!({"owner": 5}), bind_error("owner")),
        ] {
            assert_eq!(update(&body), Err(error), "{body}");
        }
        assert_eq!(
            create(&json!({"id": "b", "owner": {"id": "p9"}})),
            Err(BodyError::NotImplemented(Feature::DeepInsert))
        );
        let one = BodyLimits {
            max_bind_links: 1,
            ..BodyLimits::strict()
        };
        let model = model();
        let body =
            json!({"owner@odata.bind": "People('p1')", "auditor@odata.bind": "People('p2')"})
                .to_string();
        let read = read_entity_body(
            &minimal(),
            &resource(&model, "Towers"),
            WriteKind::Update,
            body.as_bytes(),
            &one,
        );
        assert_eq!(
            read,
            Err(BodyError::LimitExceeded {
                limit: BodyLimit::BindLinks,
                max: 1
            })
        );
        assert!(update(
            &json!({"@odata.bind": "People('p1')", "owner@odata.type": "#City.Person"})
        )
        .is_ok());
    }

    #[test]
    fn a_set_without_a_declared_type_is_no_resource() {
        let model = model();
        assert!(Resource::new(&model, "Ghosts").is_none());
    }

    fn property(name: &str, body: &str) -> Result<Value, BodyError> {
        let model = model();
        read_property_body(
            &resource(&model, "Buildings"),
            name,
            body.as_bytes(),
            &BodyLimits::strict(),
        )
    }

    #[test]
    fn a_property_body_holds_its_value_or_its_instance() {
        assert_eq!(
            property("height", r#"{"@odata.context":"x","value":12}"#),
            Ok(json!(12))
        );
        assert_eq!(property("tags", r#"{"value":["a"]}"#), Ok(json!(["a"])));
        assert_eq!(
            property("homes", r#"{"value":[{"street":"a"}]}"#),
            Ok(json!([{"street": "a"}]))
        );
        assert_eq!(
            property("address", r#"{"street":"a"}"#),
            Ok(json!({"street": "a"}))
        );
        for (name, body, error) in [
            ("height", r#"{"value":"x"}"#, invalid("height")),
            (
                "height",
                r#"{"value":1,"other":2}"#,
                BodyError::UnknownProperty,
            ),
            ("height", "{}", BodyError::UnknownProperty),
            ("height", "[]", BodyError::NotAnObject),
            ("secret", r#"{"value":1}"#, BodyError::UnknownProperty),
            ("id", r#"{"value":"b"}"#, read_only("id")),
            ("label", r#"{"value":"x"}"#, read_only("label")),
            ("stamp", r#"{"value":"x"}"#, read_only("stamp")),
            ("address", r#"{"street":1}"#, invalid("address/street")),
        ] {
            assert_eq!(property(name, body), Err(error), "{name} {body}");
        }
    }

    fn reference(navigation: &str, body: &Value) -> Result<Vec<Target>, BodyError> {
        let model = model();
        let text = body.to_string();
        read_reference_body(
            &minimal(),
            &resource(&model, "Buildings"),
            navigation,
            text.as_bytes(),
            &BodyLimits::strict(),
        )
    }

    #[test]
    fn a_reference_body_names_one_target_or_the_full_collection() {
        let p1 = format!("{ROOT}People('p1')");
        assert_eq!(
            reference("owner", &json!({"@odata.id": p1})),
            Ok(vec![target("People", "p1")])
        );
        assert_eq!(
            reference("owner", &json!({"@id": "People('p1')"})),
            Ok(vec![target("People", "p1")])
        );
        assert_eq!(
            reference(
                "parts",
                &json!({"value": [{"@id": "Buildings('a')"}, {"@odata.id": "Buildings('b')"}]})
            ),
            Ok(vec![target("Buildings", "a"), target("Buildings", "b")])
        );
        assert_eq!(reference("parts", &json!({"value": []})), Ok(vec![]));
        for (navigation, body, error) in [
            (
                "owner",
                json!({"@odata.id": "People('p1')", "x": 1}),
                bind_error("owner"),
            ),
            (
                "owner",
                json!({"@odata.id": "Buildings('b')"}),
                bind_error("owner"),
            ),
            ("parts", json!({"value": "x"}), bind_error("parts")),
            ("parts", json!({}), bind_error("parts")),
            ("parts", json!({"value": [{"x": 1}]}), bind_error("parts")),
            ("parts", json!({"value": [5]}), bind_error("parts")),
            (
                "secret",
                json!({"@id": "People('p1')"}),
                BodyError::UnknownProperty,
            ),
            (
                "auditor",
                json!({"@id": "People('p1')"}),
                read_only("auditor"),
            ),
            ("owner", json!(1), BodyError::NotAnObject),
        ] {
            assert_eq!(
                reference(navigation, &body),
                Err(error),
                "{navigation} {body}"
            );
        }
    }
}
