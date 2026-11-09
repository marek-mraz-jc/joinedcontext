// SPDX-License-Identifier: MIT
//! The reference semantics: a [`Cond`] and a sort order evaluated over one JSON row. Every
//! other source is tested to answer what this answers.

use std::cmp::Ordering;

use odata_url::CompareOp;
use serde_json::Value;

use crate::plan::{Cond, SortKey, TextOp};

/// The value at `path` in `row`; `None` when a step is missing or not an object.
fn at<'r>(row: &'r Value, path: &[String]) -> Option<&'r Value> {
    path.iter().try_fold(row, |v, name| v.get(name))
}

/// Whether `row` meets `cond`.
///
/// ```
/// use odata_table::{matches, Cond, TextOp};
/// let row = serde_json::json!({"name": "Main Square", "address": {"city": "Ruzomberok"}});
/// let city = Cond::Text { path: vec!["address".into(), "city".into()], op: TextOp::StartsWith, needle: "Ruz".into() };
/// assert!(matches(&row, &city));
/// assert!(!matches(&row, &Cond::Not(Box::new(city))));
/// ```
#[must_use]
pub fn matches(row: &Value, cond: &Cond) -> bool {
    match cond {
        Cond::Const(b) => *b,
        Cond::And(a, b) => matches(row, a) && matches(row, b),
        Cond::Or(a, b) => matches(row, a) || matches(row, b),
        Cond::Not(a) => !matches(row, a),
        Cond::IsNull { path } => at(row, path).is_none_or(Value::is_null),
        Cond::Compare { path, op, value } => at(row, path)
            .filter(|v| rank(v) == rank(value) && rank(v) != 0)
            .is_some_and(|v| {
                let ordering = compare(Some(v), Some(value));
                match op {
                    CompareOp::Eq => ordering == Ordering::Equal,
                    CompareOp::Ne => ordering != Ordering::Equal,
                    CompareOp::Gt => ordering == Ordering::Greater,
                    CompareOp::Ge => ordering != Ordering::Less,
                    CompareOp::Lt => ordering == Ordering::Less,
                    CompareOp::Le => ordering != Ordering::Greater,
                }
            }),
        Cond::Text { path, op, needle } => match at(row, path) {
            Some(Value::String(s)) => match op {
                TextOp::Contains => s.contains(needle.as_str()),
                TextOp::StartsWith => s.starts_with(needle.as_str()),
                TextOp::EndsWith => s.ends_with(needle.as_str()),
            },
            _ => false,
        },
    }
}

/// The sort rank of a JSON type: missing and `null` first, then booleans, numbers, strings,
/// and objects and arrays last (all equal among themselves).
fn rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) | Value::Object(_) => 4,
    }
}

/// The total order rows sort by: by JSON type rank, then booleans `false` first, numbers by
/// value, strings by their UTF-8 bytes (PostgreSQL `COLLATE "C"`).
///
/// ```
/// use std::cmp::Ordering;
/// use odata_table::compare;
/// let (null, two, ten) = (serde_json::Value::Null, serde_json::json!(2), serde_json::json!(10.5));
/// assert_eq!(compare(Some(&null), Some(&two)), Ordering::Less);
/// assert_eq!(compare(None, Some(&null)), Ordering::Equal);
/// assert_eq!(compare(Some(&ten), Some(&two)), Ordering::Greater);
/// ```
#[must_use]
pub fn compare(a: Option<&Value>, b: Option<&Value>) -> Ordering {
    let (a, b) = (a.unwrap_or(&Value::Null), b.unwrap_or(&Value::Null));
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            x.total_cmp(&y)
        }
        (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => Ordering::Equal,
    })
}

/// `a` against `b` under `order`.
pub(crate) fn ordered(a: &Value, b: &Value, order: &[SortKey]) -> Ordering {
    order
        .iter()
        .map(|key| {
            let o = compare(at(a, &key.path), at(b, &key.path));
            if key.descending {
                o.reverse()
            } else {
                o
            }
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}
