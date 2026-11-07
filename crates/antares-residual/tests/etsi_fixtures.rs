// SPDX-License-Identifier: EUPL-1.2
//! The evaluator over the ETSI NGSI-LD test suite's own Entity fixtures
//! (`ngsi-ld-test-suite/data/entities`, vendored in the workspace; not in the published crate).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ngsild_residual::{compile, eval, Cmp, DateTime, Expr, Lit, Path as Attr, Truth};
use serde_json::Value;
use std::path::Path;

/// Every scalar Property of every suite Entity equals its own value (`True`), differs from
/// another (`False`), and an attribute the Entity lacks is `Unknown`; no fixture errors.
#[test]
fn every_suite_property_compares_to_its_own_value() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite/data/entities");
    let now = DateTime::parse("2026-10-12T00:00:00Z").unwrap();
    let cmp = |op, attr: &str, lit: Lit| {
        let e = Expr::Cmp(
            op,
            Box::new(Expr::Path(Attr::value(attr))),
            Box::new(Expr::Lit(lit)),
        );
        compile(&e, now).unwrap()
    };
    let mut checked = 0;
    for f in std::fs::read_dir(dir).unwrap() {
        let path = f.unwrap().path();
        let Ok(entity @ Value::Object(_)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            eval(&cmp(Cmp::Eq, "absentAttribute", Lit::Int(1)), &entity).unwrap(),
            Truth::Unknown,
            "{name}"
        );
        for (attr, member) in entity.as_object().unwrap() {
            if member.get("type").and_then(Value::as_str) != Some("Property") {
                continue;
            }
            let (own, other) = match &member["value"] {
                Value::String(s) => (Lit::Str(s.clone()), Lit::Str(format!("{s}-other"))),
                Value::Bool(b) => (Lit::Bool(*b), Lit::Bool(!b)),
                Value::Number(n) if n.is_i64() => {
                    let i = n.as_i64().unwrap();
                    (Lit::Int(i), Lit::Int(i.wrapping_add(1)))
                }
                Value::Number(n) => {
                    let x = n.as_f64().unwrap();
                    (Lit::Num(x), Lit::Num(x + 1.0))
                }
                _ => continue,
            };
            assert_eq!(
                eval(&cmp(Cmp::Eq, attr, own), &entity).unwrap(),
                Truth::True,
                "{name} {attr}"
            );
            assert_eq!(
                eval(&cmp(Cmp::Eq, attr, other), &entity).unwrap(),
                Truth::False,
                "{name} {attr}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 40, "only {checked} suite Properties checked");
}
