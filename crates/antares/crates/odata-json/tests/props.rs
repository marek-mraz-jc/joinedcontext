// SPDX-License-Identifier: EUPL-1.2
//! Reader round trips: a body built from random values of every declared type reads back to
//! exactly those values, keys and binds (ADR-0024 decision 8).

#[cfg(test)]
mod service;

#[cfg(test)]
mod round_trips {

    use crate::service::{minimal, model, resource};
    use odata_json::{read_entity_body, Bind, BodyLimits, Target, WriteKind};
    use odata_url::{Key, KeyValue, Literal};
    use proptest::prelude::*;
    use serde_json::{json, Map, Value};

    /// A value of each property the strategy fills, by declared type.
    fn properties() -> impl Strategy<Value = Vec<(String, Value)>> {
        let text = || "[ -~]{0,12}".prop_map(Value::from);
        let field = |name: &'static str, value: BoxedStrategy<Value>| {
            proptest::option::of(value).prop_map(move |v| v.map(|v| (name.to_owned(), v)))
        };
        (
            field("name", prop_oneof![text(), Just(Value::Null)].boxed()),
            field("height", any::<i32>().prop_map(Value::from).boxed()),
            field("count", any::<i64>().prop_map(Value::from).boxed()),
            field("byte", any::<u8>().prop_map(Value::from).boxed()),
            field("sbyte", any::<i8>().prop_map(Value::from).boxed()),
            field("short", any::<i16>().prop_map(Value::from).boxed()),
            field("area", (-1e9f64..1e9).prop_map(Value::from).boxed()),
            field("active", any::<bool>().prop_map(Value::from).boxed()),
            field(
                "tags",
                proptest::collection::vec(text(), 0..4)
                    .prop_map(Value::from)
                    .boxed(),
            ),
            field(
                "address",
                (text(), proptest::collection::vec(any::<i32>(), 0..3))
                    .prop_map(|(street, floors)| json!({"street": street, "floors": floors}))
                    .boxed(),
            ),
        )
            .prop_map(|fields| {
                let (name, height, count, byte, sbyte, short, area, active, tags, address) = fields;
                [
                    name, height, count, byte, sbyte, short, area, active, tags, address,
                ]
                .into_iter()
                .flatten()
                .collect()
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 4 } else { 500 }))]

        #[test]
        fn a_body_reads_back_to_its_values(id in "[a-z0-9]{1,8}", owner in proptest::option::of("[a-z0-9]{1,8}"), properties in properties()) {
            let mut body: Map<String, Value> = properties.iter().cloned().collect();
            body.insert("id".into(), Value::from(id.clone()));
            if let Some(owner) = &owner {
                body.insert("owner@odata.bind".into(), Value::from(format!("People('{owner}')")));
            }
            let model = model();
            let text = Value::Object(body).to_string();
            let payload = read_entity_body(&minimal(), &resource(&model, "Buildings"), WriteKind::Create, text.as_bytes(), &BodyLimits::strict()).unwrap();
            let mut expected = properties;
            expected.sort_by(|a, b| a.0.cmp(&b.0));
            prop_assert_eq!(payload.key, vec![("id".to_owned(), Value::from(id))]);
            prop_assert_eq!(payload.properties, expected);
            let binds: Vec<Bind> = owner.into_iter().map(|owner| Bind {
                navigation: "owner".into(),
                target: Target { set: "People".into(), key: Key::Single(KeyValue::Literal(Literal::String(owner))) },
            }).collect();
            prop_assert_eq!(payload.binds, binds);
        }
    }
}
