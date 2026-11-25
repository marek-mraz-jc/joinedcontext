//! Any text as a math expression: parsing under a small depth cap never panics or recurses past
//! the cap, and a parsed expression evaluates over a series, a number and a missing value to an
//! answer or a refusal (T-7849).
#![no_main]

use daiya_expr::{ExprValue, MathExpr};
use libfuzzer_sys::fuzz_target;
use std::collections::{BTreeMap, HashMap};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(expr) = MathExpr::parse_with_max_depth(text, 16) else {
        return;
    };
    let series = ExprValue::Series {
        name: "A".into(),
        labels: BTreeMap::from([("host".into(), "a".into())]),
        points: vec![(1000, Some(1.5)), (2000, None), (3000, Some(f64::NAN))],
        notices: Vec::new(),
    };
    let number = ExprValue::Number {
        name: "B".into(),
        labels: BTreeMap::from([("host".into(), "a".into())]),
        value: Some(-2.0),
        notices: Vec::new(),
    };
    let vars = HashMap::from([
        ("A".to_owned(), vec![series]),
        ("B".to_owned(), vec![number]),
        ("N".to_owned(), vec![ExprValue::NoData]),
    ]);
    let _ = expr.eval("C", &vars);
});
