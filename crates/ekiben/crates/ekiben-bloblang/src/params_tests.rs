//! The per-parameter parity gate (T-7079): params.rs equals Bento 1.22's recorded spec, the parser
//! refuses what Bento refuses, and every function and method ekiben runs takes each parameter
//! under the name the spec gives it.

use std::collections::BTreeMap;

use crate::params::{Spec, FUNCTIONS, METHODS};
use crate::{Environment, Value};

fn recording() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/parity/bento-params.json"
    );
    let text = std::fs::read_to_string(path).expect("tests/parity/bento-params.json");
    serde_json::from_str(&text).expect("bento-params.json is JSON")
}

type Row = (bool, Vec<(String, bool, Option<serde_json::Value>)>);

fn table(spec: &[Spec]) -> BTreeMap<String, Row> {
    spec.iter()
        .map(|(name, variadic, params)| {
            let params = params
                .iter()
                .map(|(p, optional, default)| {
                    let default = default.map(|d| serde_json::from_str(d).expect("default"));
                    (p.to_string(), *optional, default)
                })
                .collect();
            (name.to_string(), (*variadic, params))
        })
        .collect()
}

fn recorded(rec: &serde_json::Value, kind: &str) -> BTreeMap<String, Row> {
    rec[kind]
        .as_object()
        .expect(kind)
        .iter()
        .map(|(name, item)| {
            let params = item["params"]
                .as_array()
                .expect("params")
                .iter()
                .map(|p| {
                    let name = p["name"].as_str().expect("name").to_string();
                    let optional = p["optional"].as_bool().expect("optional");
                    (name, optional, p.get("default").cloned())
                })
                .collect();
            let variadic = item["variadic"].as_bool().expect("variadic");
            (name.clone(), (variadic, params))
        })
        .collect()
}

#[test]
fn the_table_is_the_recorded_bento_spec() {
    let rec = recording();
    assert_eq!(
        table(FUNCTIONS),
        recorded(&rec, "functions"),
        "regenerate: scripts/parity-inventory.py --write-params-rs"
    );
    assert_eq!(
        table(METHODS),
        recorded(&rec, "methods"),
        "regenerate: scripts/parity-inventory.py --write-params-rs"
    );
}

fn compile_err(mapping: &str) -> String {
    match Environment::new().compile_mapping(mapping) {
        Ok(_) => panic!("{mapping} compiled"),
        Err(e) => e.to_string(),
    }
}

/// Each message is what `bento blobl` 1.22 prints for the same mapping.
#[test]
fn calls_outside_the_spec_are_refused_at_parse_as_bento_does() {
    let cases = [
        (r#"root = random_int(sed: 1)"#, "unknown parameter sed"),
        (r#"root = this.uppercase(foo: 1)"#, "unknown parameter foo"),
        (
            r#"root = this.replace_all(old: "a", nw: "b")"#,
            "unknown parameter nw, did you mean new?",
        ),
        (
            r#"root = this.replace_all(new: "a", zz: "b")"#,
            "unknown parameter zz, did you mean old?",
        ),
        (
            r#"root = this.replace_all(xx: "a", yy: "b")"#,
            "unknown parameters xx, yy, expected old, new",
        ),
        (
            r#"root = this.replace_all(xx: "a")"#,
            "unknown parameter xx, expected old, new",
        ),
        (
            r#"root = this.split(delimiter: ",", ff: 1)"#,
            "unknown parameter ff",
        ),
        (
            r#"root = this.split(",", 2)"#,
            "wrong number of arguments, expected 1, got 2",
        ),
        (
            r#"root = this.trim(1, 2)"#,
            "wrong number of arguments, expected 1, got 2",
        ),
        (
            r#"root = now(1)"#,
            "wrong number of arguments, expected 0, got 1",
        ),
    ];
    for (mapping, want) in cases {
        let got = compile_err(mapping);
        assert!(got.contains(want), "{mapping}: {got:?} lacks {want:?}");
    }
    // variadic calls take any number of positional arguments
    assert!(Environment::new()
        .compile_mapping(r#"root = "%v-%v".format(1, 2)"#)
        .is_ok());
}

/// Calls every function and method ekiben runs with all its parameters named as the spec names them
/// (the default where the spec has one, else a string). A parameter the implementation knows under
/// another name answers `unknown parameter` or `missing parameter` here, so a rename on either side
/// turns this red.
#[test]
fn every_parameter_is_taken_under_its_spec_name() {
    let env = Environment::new();
    let mut failures = Vec::new();
    for (kind, spec) in [("function", FUNCTIONS), ("method", METHODS)] {
        for (name, variadic, params) in spec {
            let known = match kind {
                "function" => env.has_function(name),
                _ => env.has_method(name),
            };
            if !known || *variadic || params.is_empty() {
                continue;
            }
            let args: Vec<String> = params
                .iter()
                .map(|(p, _, default)| {
                    let v = match default {
                        Some(d) if d.starts_with('{') => "0".to_string(),
                        Some(d) => (*d).to_string(),
                        None => "\"x\"".to_string(),
                    };
                    format!("{p}: {v}")
                })
                .collect();
            let call = match kind {
                "function" => format!("{name}({})", args.join(", ")),
                _ => format!("this.{name}({})", args.join(", ")),
            };
            let mapping = format!("root = {call}");
            let err = match env.compile_mapping(&mapping) {
                Err(e) => Some(e.to_string()),
                Ok(exec) => exec
                    .exec_on_value(Value::String("a".to_string()))
                    .err()
                    .map(|e| e.to_string()),
            };
            if let Some(e) = err {
                if e.contains("unknown parameter") || e.contains("missing parameter") {
                    failures.push(format!("{mapping}: {e}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} calls refuse a spec name:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
