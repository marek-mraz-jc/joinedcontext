// SPDX-License-Identifier: EUPL-1.2
//! `$filter` against a reference (odata-linkml-mapping 5.2): random
//! filters of the served subset select, through the surface and the
//! broker's `q`, exactly the Entities an OData evaluator written here
//! selects. An absent member is OData `null`: a comparison with it is
//! false except `ne`, which holds (F3), and `not` of a string function on
//! it stays `null`, so the Entity is not selected. Arithmetic, the
//! numeric and string functions, two members compared and the `any`/`all`
//! lambdas are evaluated on the broker's answer (T-3749); an absent list is
//! the empty collection the surface shows.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "label", "on", "tags"]}},
  "slots": {"speed": {"range": "float"}, "label": {"range": "string"}, "on": {"range": "boolean"},
            "tags": {"range": "string",
                     "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}}}
}"#;

/// One Car of the fixture; `None` is an absent Attribute.
struct Car {
    speed: Option<f64>,
    label: Option<&'static str>,
    on: Option<bool>,
    tags: Option<&'static [&'static str]>,
}

const CARS: &[Car] = &[
    Car {
        speed: Some(5.0),
        label: Some("a.b"),
        on: Some(true),
        tags: Some(&["a", "b"]),
    },
    Car {
        speed: Some(10.5),
        label: Some("ab\""),
        on: Some(false),
        tags: Some(&["b"]),
    },
    Car {
        speed: Some(30.0),
        label: None,
        on: Some(true),
        tags: Some(&[]),
    },
    Car {
        speed: None,
        label: Some("b"),
        on: None,
        tags: None,
    },
    Car {
        speed: Some(-1.0),
        label: Some("."),
        on: Some(false),
        tags: Some(&["a"]),
    },
    Car {
        speed: Some(0.0),
        label: Some("\"a"),
        on: Some(true),
        tags: Some(&["\"a", "b"]),
    },
    Car {
        speed: None,
        label: None,
        on: None,
        tags: None,
    },
];

const NUMBERS: &[f64] = &[-1.0, 0.0, 5.0, 10.5, 30.0];
const OPS: &[&str] = &["eq", "ne", "gt", "ge", "lt", "le"];
const FUNCTIONS: &[&str] = &["contains", "startswith", "endswith"];
const ARITH: &[&str] = &["add", "sub", "mul"];
const ROUNDING: &[&str] = &["round", "floor", "ceiling"];
const TAGS: &[&str] = &["a", "b", "\"a"];

/// A filter of the served subset.
#[derive(Debug, Clone)]
enum F {
    Number(&'static str, f64),
    Null(bool),
    In(Vec<f64>),
    Label(bool, String),
    Function(&'static str, String, bool),
    On,
    OnIs(bool),
    /// `speed <arith> n <op> m`.
    Arith(&'static str, f64, &'static str, f64),
    /// `-speed <op> n`.
    Neg(&'static str, f64),
    /// `<rounding>(speed) <op> n`.
    Rounded(&'static str, &'static str, f64),
    /// `length(label) <op> n`.
    Length(&'static str, f64),
    /// `toupper(label) eq s`.
    Upper(String),
    /// `speed <op> speed <arith> n`: two members (F25).
    Members(&'static str, &'static str, f64),
    /// `not (speed <arith> n <op> m)`: `not` over an evaluated comparison.
    NotArith(&'static str, f64, &'static str, f64),
    /// `tags/any(x:x eq s)`, or `tags/any()` with no tag.
    Any(Option<&'static str>),
    /// `tags/all(x:x ne s)`.
    All(&'static str),
    And(Box<F>, Box<F>),
    Or(Box<F>, Box<F>),
}

impl F {
    fn text(&self) -> String {
        match self {
            Self::Number(op, n) => format!("speed {op} {n}"),
            Self::Null(eq) => format!("label {} null", if *eq { "eq" } else { "ne" }),
            Self::In(list) => {
                let list: Vec<String> = list.iter().map(f64::to_string).collect();
                format!("speed in ({})", list.join(","))
            }
            Self::Label(eq, s) => format!("label {} '{s}'", if *eq { "eq" } else { "ne" }),
            Self::Function(name, s, negated) => {
                let call = format!("{name}(label,'{s}')");
                if *negated {
                    format!("not {call}")
                } else {
                    call
                }
            }
            Self::On => "on".to_owned(),
            Self::OnIs(b) => format!("on eq {b}"),
            Self::Arith(arith, n, op, m) => format!("speed {arith} {n} {op} {m}"),
            Self::Neg(op, n) => format!("-speed {op} {n}"),
            Self::Rounded(f, op, n) => format!("{f}(speed) {op} {n}"),
            Self::Length(op, n) => format!("length(label) {op} {n}"),
            Self::Upper(s) => format!("toupper(label) eq '{s}'"),
            Self::Members(op, arith, n) => format!("speed {op} speed {arith} {n}"),
            Self::NotArith(arith, n, op, m) => format!("not (speed {arith} {n} {op} {m})"),
            Self::Any(None) => "tags/any()".to_owned(),
            Self::Any(Some(t)) => format!("tags/any(x:x eq '{t}')"),
            Self::All(t) => format!("tags/all(x:x ne '{t}')"),
            Self::And(l, r) => format!("({}) and ({})", l.text(), r.text()),
            Self::Or(l, r) => format!("({}) or ({})", l.text(), r.text()),
        }
    }

    /// The OData answer for `car`; `null` selects nothing.
    fn holds(&self, car: &Car) -> bool {
        match self {
            Self::Number(op, n) => compare(car.speed, op, *n),
            Self::Arith(arith, n, op, m) => compare(car.speed.map(|s| apply(arith, s, *n)), op, *m),
            Self::Neg(op, n) => compare(car.speed.map(|s| -s), op, *n),
            Self::Rounded(f, op, n) => compare(
                car.speed.map(|s| match *f {
                    "round" => s.round(),
                    "floor" => s.floor(),
                    _ => s.ceil(),
                }),
                op,
                *n,
            ),
            Self::Length(op, n) => compare(
                car.label
                    .map(|l| f64::from(u32::try_from(l.chars().count()).unwrap_or(0))),
                op,
                *n,
            ),
            Self::Upper(s) => car.label.is_some_and(|l| l.to_uppercase() == *s),
            Self::Members(op, arith, n) => match car.speed {
                Some(s) => compare(Some(s), op, apply(arith, s, *n)),
                None => *op == "ne",
            },
            Self::NotArith(arith, n, op, m) => {
                !compare(car.speed.map(|s| apply(arith, s, *n)), op, *m)
            }
            Self::Any(t) => car
                .tags
                .is_some_and(|tags| t.map_or(!tags.is_empty(), |t| tags.contains(&t))),
            Self::All(t) => car.tags.is_none_or(|tags| !tags.contains(t)),
            Self::Null(eq) => car.label.is_none() == *eq,
            Self::In(list) => car
                .speed
                .is_some_and(|s| list.iter().any(|n| (s - n).abs() < f64::EPSILON)),
            Self::Label(eq, s) => car.label.map_or(!*eq, |l| (l == s) == *eq),
            Self::Function(name, s, negated) => car.label.is_some_and(|l| {
                let found = match *name {
                    "contains" => l.contains(s.as_str()),
                    "startswith" => l.starts_with(s.as_str()),
                    _ => l.ends_with(s.as_str()),
                };
                found != *negated
            }),
            Self::On => car.on == Some(true),
            Self::OnIs(b) => car.on == Some(*b),
            Self::And(l, r) => l.holds(car) && r.holds(car),
            Self::Or(l, r) => l.holds(car) || r.holds(car),
        }
    }
}

/// OData's comparison of a number that may be `null` (absent): false,
/// except `ne`, which holds (F3).
fn compare(value: Option<f64>, op: &str, n: f64) -> bool {
    value.map_or(op == "ne", |s| match op {
        "eq" => (s - n).abs() < f64::EPSILON,
        "ne" => (s - n).abs() >= f64::EPSILON,
        "gt" => s > n,
        "ge" => s >= n,
        "lt" => s < n,
        _ => s <= n,
    })
}

fn apply(arith: &str, a: f64, b: f64) -> f64 {
    match arith {
        "add" => a + b,
        "sub" => a - b,
        _ => a * b,
    }
}

fn atom() -> impl Strategy<Value = F> {
    let number = || prop::sample::select(NUMBERS);
    let text = || {
        prop::collection::vec(prop::sample::select(&['a', 'b', '.', '"'][..]), 1..3)
            .prop_map(|c| c.into_iter().collect::<String>())
    };
    prop_oneof![
        (prop::sample::select(OPS), number()).prop_map(|(op, n)| F::Number(op, n)),
        any::<bool>().prop_map(F::Null),
        prop::collection::vec(number(), 1..3).prop_map(F::In),
        (any::<bool>(), text()).prop_map(|(eq, s)| F::Label(eq, s)),
        (prop::sample::select(FUNCTIONS), text(), any::<bool>())
            .prop_map(|(name, s, negated)| F::Function(name, s, negated)),
        Just(F::On),
        any::<bool>().prop_map(F::OnIs),
        evaluated(),
    ]
}

/// The atoms the residual evaluates (T-3749).
fn evaluated() -> impl Strategy<Value = F> {
    let number = || prop::sample::select(NUMBERS);
    let op = || prop::sample::select(OPS);
    let arith = || prop::sample::select(ARITH);
    prop_oneof![
        (arith(), number(), op(), number()).prop_map(|(a, n, o, m)| F::Arith(a, n, o, m)),
        (op(), number()).prop_map(|(o, n)| F::Neg(o, n)),
        (prop::sample::select(ROUNDING), op(), number()).prop_map(|(f, o, n)| F::Rounded(f, o, n)),
        (op(), prop::sample::select(&[0.0, 1.0, 2.0, 3.0][..])).prop_map(|(o, n)| F::Length(o, n)),
        prop::sample::select(&["A.B", "B", "AB\"", "a.b"][..]).prop_map(|s| F::Upper(s.to_owned())),
        (op(), arith(), number()).prop_map(|(o, a, n)| F::Members(o, a, n)),
        (arith(), number(), op(), number()).prop_map(|(a, n, o, m)| F::NotArith(a, n, o, m)),
        prop::option::of(prop::sample::select(TAGS)).prop_map(F::Any),
        prop::sample::select(TAGS).prop_map(F::All),
    ]
}

fn filter() -> impl Strategy<Value = F> {
    atom().prop_recursive(3, 8, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(l, r)| F::And(Box::new(l), Box::new(r))),
            (inner.clone(), inner).prop_map(|(l, r)| F::Or(Box::new(l), Box::new(r))),
        ]
    })
}

/// Percent-encodes all but RFC 3986 unreserved characters and `'(),`.
fn encode(raw: &str) -> String {
    use std::fmt::Write as _;
    raw.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || b"-._~'(),".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}

async fn put(app: &Router, method: &str, uri: &str, body: String) -> Result<(), String> {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .map_err(|e| e.to_string())?;
    let Ok(resp) = app.clone().oneshot(req).await;
    if resp.status() == StatusCode::CREATED {
        Ok(())
    } else {
        Err(format!("{uri}: {}", resp.status()))
    }
}

async fn broker() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:label", "city:on", "city:tags"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    put(&app, "PUT", "/q/views/cars", view.to_string()).await?;
    put(&app, "PUT", "/q/schemas/city", SCHEMA.to_owned()).await?;
    for (n, car) in CARS.iter().enumerate() {
        let mut doc = Map::new();
        doc.insert("id".into(), json!(format!("urn:ngsi-ld:Car:{n}")));
        doc.insert("type".into(), json!(format!("{CITY}Car")));
        if let Some(tags) = car.tags {
            doc.insert(
                format!("{CITY}tags"),
                json!({"type": "ListProperty", "valueList": tags}),
            );
        }
        let values = [
            ("speed", car.speed.map(Value::from)),
            ("label", car.label.map(Value::from)),
            ("on", car.on.map(Value::from)),
        ];
        for (name, value) in values {
            if let Some(value) = value {
                doc.insert(
                    format!("{CITY}{name}"),
                    json!({"type": "Property", "value": value}),
                );
            }
        }
        put(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            Value::Object(doc).to_string(),
        )
        .await?;
    }
    Ok((b, app))
}

/// The ids the surface selects for `filter`, or the status it refused with.
async fn selected(app: &Router, filter: &str) -> Result<BTreeSet<String>, String> {
    let uri = format!(
        "/x/views/cars/odata/v4/Car?$top=1000&$filter={}",
        encode(filter)
    );
    let req = Request::get(uri)
        .body(Body::empty())
        .map_err(|e| e.to_string())?;
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if status != StatusCode::OK {
        return Err(format!("{status} {body}"));
    }
    Ok(body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect())
}

#[test]
fn odata_filter_translation_agrees_with_reference_eval() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (_b, app) = rt.block_on(broker()).expect("broker");
    let mut runner = TestRunner::new(Config {
        cases: 192,
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&filter(), |f| {
        let text = f.text();
        let want: BTreeSet<String> = CARS
            .iter()
            .enumerate()
            .filter(|(_, car)| f.holds(car))
            .map(|(n, _)| format!("urn:ngsi-ld:Car:{n}"))
            .collect();
        let got = rt.block_on(selected(&app, &text));
        prop_assert_eq!(got, Ok(want), "{}", text);
        Ok(())
    });
    assert!(result.is_ok(), "{result:?}");
}
