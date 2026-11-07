// SPDX-License-Identifier: EUPL-1.2
//! The security property of a view of another model as a property over
//! generated Cars and target-model queries (`view-mapping.md` section 6): a
//! mapped view serves at most what the same view without its mapping
//! serves. Every id it answers is one the plain view answers, every value
//! it answers derives from the plain view's value of the same Entity, and
//! no Attribute the view hides reaches the answer under any name.
//!
//! `PROPTEST_CASES` sets the number of cases (default 32: each case builds
//! a broker).

use crate::common::{prop, put_view, send};
use antares_ops::test_kit::TestBroker;
use axum::http::Method;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const COLOURS: [&str; 3] = ["red", "blue", "green"];

/// A Car: its number, `speed`, `colour` (an index) and `secret`.
type Car = (u8, Option<u8>, Option<usize>, Option<u8>);

fn cars() -> impl Strategy<Value = Vec<Car>> {
    proptest::collection::btree_map(
        0u8..16,
        (
            proptest::option::of(0u8..100),
            proptest::option::of(0usize..3),
            proptest::option::of(0u8..4),
        ),
        0..10,
    )
    .prop_map(|m| m.into_iter().map(|(n, (s, c, x))| (n, s, c, x)).collect())
}

/// One target-model term: on the converted, the mapped and the hidden slot.
fn term() -> impl Strategy<Value = String> {
    let ops = prop_oneof![
        Just("<"),
        Just("<="),
        Just(">"),
        Just(">="),
        Just("=="),
        Just("!=")
    ];
    prop_oneof![
        (ops, 0u16..220).prop_map(|(op, n)| format!("velocity{op}{n}")),
        (
            prop_oneof![Just("=="), Just("!=")],
            prop_oneof![Just("rot"), Just("blau"), Just("green"), Just("x")]
        )
            .prop_map(|(op, v)| format!("paint{op}\"{v}\"")),
        (0u8..4).prop_map(|n| format!("hidden=={n}")),
        prop_oneof![Just("velocity"), Just("!paint"), Just("hidden")].prop_map(str::to_owned),
    ]
}

fn query() -> impl Strategy<Value = String> {
    (
        term(),
        proptest::option::of((prop_oneof![Just(";"), Just("|")], term())),
    )
        .prop_map(|(a, rest)| rest.map_or_else(|| a.clone(), |(join, b)| format!("{a}{join}{b}")))
}

fn view(id: &str, mapping: Option<Value>) -> Value {
    let mut v = json!({
        "id": id, "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "q": "speed<80", "attributes": ["speed", "colour"]}],
        "operations": ["query_entities"]
    });
    if let (Some(m), Some(o)) = (mapping, v.as_object_mut()) {
        o.insert("mapping".into(), m);
    }
    v
}

fn mapping() -> Value {
    json!({
        "version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
        "slots": [
            {"target": "velocity", "source": "speed", "kind": "unitConversion", "factor": 2, "filterable": true},
            {"target": "paint", "source": "colour", "kind": "valueMappings",
             "forward": {"red": "rot", "blue": "blau"}, "filterable": true},
            {"target": "hidden", "source": "secret", "kind": "rename", "filterable": true}
        ]
    })
}

fn encode(q: &str) -> String {
    q.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn case(cars: Vec<Car>, queries: Vec<String>) -> Result<(), TestCaseError> {
    let fail = |e: String| TestCaseError::fail(e);
    let broker = TestBroker::new().await.map_err(fail)?;
    let app = broker.serve(antares_views::Views).map_err(fail)?;
    // a type outside the selector, which also makes the Tenant exist
    let truck = json!({"id": "urn:ngsi-ld:Truck:t", "type": "Truck", "speed": prop(&json!(1))});
    let a = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        &[("NGSILD-Tenant", "city")],
        Some(&truck),
    )
    .await;
    prop_assert_eq!(a.status.as_u16(), 201, "{}", a.text);
    for (n, speed, colour, secret) in cars {
        let mut car = serde_json::Map::new();
        car.insert("id".into(), json!(format!("urn:ngsi-ld:Car:{n}")));
        car.insert("type".into(), json!("Car"));
        let colour = colour.and_then(|c| COLOURS.get(c));
        for (name, value) in [
            ("speed", speed.map(|s| json!(s))),
            ("colour", colour.map(|c| json!(c))),
            ("secret", secret.map(|x| json!(x))),
        ] {
            if let Some(v) = value {
                car.insert(name.into(), prop(&v));
            }
        }
        let car = Value::Object(car);
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            &[("NGSILD-Tenant", "city")],
            Some(&car),
        )
        .await;
        prop_assert_eq!(a.status.as_u16(), 201, "{}", a.text);
    }
    put_view(&app, &view("plain", None)).await.map_err(fail)?;
    put_view(&app, &view("mapped", Some(mapping())))
        .await
        .map_err(fail)?;
    let plain = send(
        &app,
        Method::GET,
        "/x/views/plain/ngsi-ld/v1/entities?type=Car&limit=1000",
        &[],
        None,
    )
    .await;
    prop_assert_eq!(plain.status.as_u16(), 200, "{}", plain.text);
    let served: BTreeMap<String, Value> = plain
        .json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| Some((e.get("id")?.as_str()?.to_owned(), e.clone())))
        .collect();
    for q in queries {
        let uri = format!(
            "/x/views/mapped/ngsi-ld/v1/entities?type=Vehicle&limit=1000&q={}",
            encode(&q)
        );
        let a = send(&app, Method::GET, &uri, &[], None).await;
        prop_assert_eq!(a.status.as_u16(), 200, "{}: {}", q, a.text);
        for e in a.json.as_array().into_iter().flatten() {
            served_by_plain(&q, e, &served)?;
        }
    }
    Ok(())
}

/// `e`, one Entity the mapped view answered for `q`, is one the plain view
/// serves, with no value the plain view does not serve.
fn served_by_plain(
    q: &str,
    e: &Value,
    served: &BTreeMap<String, Value>,
) -> Result<(), TestCaseError> {
    let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
    let Some(source) = served.get(id) else {
        return Err(TestCaseError::fail(format!(
            "{q}: {id} is not served by the plain view"
        )));
    };
    for (name, value) in e.as_object().into_iter().flatten() {
        let from = |attr: &str| source.pointer(&format!("/{attr}/value")).cloned();
        match name.as_str() {
            "id" | "type" => {}
            "velocity" => {
                let speed = from("speed").and_then(|v| v.as_f64());
                prop_assert_eq!(
                    value.pointer("/value").and_then(Value::as_f64),
                    speed.map(|s| s * 2.0),
                    "{}: {}",
                    q,
                    e
                );
            }
            "paint" => {
                let colour = from("colour");
                let want = match colour.as_ref().and_then(Value::as_str) {
                    Some("red") => Some(json!("rot")),
                    Some("blue") => Some(json!("blau")),
                    _ => colour,
                };
                prop_assert_eq!(value.pointer("/value").cloned(), want, "{}: {}", q, e);
            }
            other => {
                return Err(TestCaseError::fail(format!(
                    "{q}: {other} is no value the plain view serves: {e}"
                )))
            }
        }
    }

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(32)
    ))]

    #[test]
    fn a_mapped_view_never_serves_more_than_the_same_view_without_its_mapping(
        cars in cars(),
        queries in proptest::collection::vec(query(), 1..6),
    ) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| TestCaseError::fail(e.to_string()))?
            .block_on(case(cars, queries))?;
    }
}
