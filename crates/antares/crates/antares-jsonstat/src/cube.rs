// SPDX-License-Identifier: EUPL-1.2
//! The `KeyPerformanceIndicator` Entities of one source table mapped back to a JSON-stat 2.0
//! dataset (docs/src/jsonstat.md section 2): the dimensions in `DATAcube`'s order (territory,
//! period, indicator, then the table's other keys and the unit), each cell one Entity, a cell no
//! Entity holds `null` and never 0.

use antares_ops::default_instance as attribute;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The category of a key dimension an Entity does not carry.
pub const NONE: &str = "_";

/// The most cells a dataset writes as an array; a sparser or larger one is an object of the
/// cells it holds (JSON-stat 2.0 `value`, "object" form).
const DENSE_CELLS: usize = 1_000_000;

/// The dimensions, as the Attribute each comes from, and the JSON-stat role of the first three.
const DIMENSIONS: [&str; 5] = [
    "refArea",
    "refPeriod",
    "indicator",
    "dimensionKey",
    "unitText",
];

/// An Attribute's text: a Property's string, or a `LanguageProperty`'s text in its first
/// language (a request's `lang` turns it into a Property before it gets here).
fn text(entity: &Value, name: &str) -> Option<String> {
    let a = attribute(entity, name)?;
    match (a.get("value"), a.get("languageMap")) {
        (Some(Value::String(s)), _) => Some(s.clone()),
        (_, Some(Value::Object(map))) => map.values().find_map(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// One cell: its category per dimension and its value.
struct Cell {
    at: [String; 5],
    value: Value,
}

/// The cell an Entity is, or `None` without a territory, a period, an indicator or a number.
fn cell(entity: &Value) -> Option<Cell> {
    let value = attribute(entity, "kpiValue")?.get("value")?;
    if !value.is_number() {
        return None;
    }
    let category = |d: &str| text(entity, d);
    Some(Cell {
        at: [
            category("refArea")?,
            category("refPeriod")?,
            category("indicator")?,
            category("dimensionKey").unwrap_or_else(|| NONE.to_owned()),
            category("unitText").unwrap_or_else(|| NONE.to_owned()),
        ],
        value: value.clone(),
    })
}

/// The dataset of table `code` from its `entities`, and how many Entities it left out: those
/// that are no cell, and a second Entity of a cell already held.
#[must_use]
pub fn dataset(code: &str, entities: &[Value]) -> (Value, usize) {
    let mut cells: BTreeMap<[String; 5], Value> = BTreeMap::new();
    let (mut labels, mut sources, mut pages, mut updated) = (
        BTreeMap::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        None::<String>,
    );
    for entity in entities {
        let Some(c) = cell(entity) else { continue };
        if let Some(name) = text(entity, "name") {
            labels.entry(c.at[2].clone()).or_insert(name);
        }
        sources.extend(text(entity, "source"));
        pages.extend(text(entity, "isPartOf"));
        let seen = attribute(entity, "kpiValue")
            .and_then(|k| k.get("observedAt")?.as_str().map(str::to_owned))
            .or_else(|| text(entity, "dateObserved"));
        updated = updated.max(seen);
        cells.entry(c.at).or_insert(c.value);
    }
    let skipped = entities.len().saturating_sub(cells.len());
    // a key dimension no Entity carries is no dimension
    let used: Vec<usize> = (0..DIMENSIONS.len())
        .filter(|&d| d < 3 || cells.keys().any(|at| at.get(d).is_some_and(|c| c != NONE)))
        .collect();
    let categories: Vec<Vec<String>> = used
        .iter()
        .map(|&d| {
            let set: BTreeSet<&String> = cells.keys().filter_map(|at| at.get(d)).collect();
            let mut list: Vec<String> = set.into_iter().cloned().collect();
            if d == 1 {
                // DATAcube lists the newest period first
                list.reverse();
            }
            list
        })
        .collect();
    let sizes: Vec<usize> = categories.iter().map(Vec::len).collect();
    let mut dimension = Map::new();
    for (&d, list) in used.iter().zip(&categories) {
        let name = DIMENSIONS.get(d).copied().unwrap_or_default();
        let mut category = Map::new();
        category.insert("index".into(), json!(list));
        if d == 2 {
            category.insert("label".into(), json!(labels));
        }
        dimension.insert(
            name.to_owned(),
            json!({ "label": name, "category": category }),
        );
    }
    let value = values(&cells, &used, &categories);
    let ids: Vec<&str> = used
        .iter()
        .filter_map(|&d| DIMENSIONS.get(d).copied())
        .collect();
    let mut out = Map::new();
    out.insert("version".into(), json!("2.0"));
    out.insert("class".into(), json!("dataset"));
    out.insert("label".into(), json!(code));
    if !sources.is_empty() {
        let list: Vec<&str> = sources.iter().map(String::as_str).collect();
        out.insert("source".into(), json!(list.join(", ")));
    }
    if let Some(at) = updated {
        out.insert("updated".into(), json!(at));
    }
    out.insert("id".into(), json!(ids));
    out.insert("size".into(), json!(sizes));
    out.insert(
        "role".into(),
        json!({ "geo": ["refArea"], "time": ["refPeriod"], "metric": ["indicator"] }),
    );
    out.insert("dimension".into(), Value::Object(dimension));
    out.insert("value".into(), value);
    out.insert(
        "note".into(),
        json!(["A cell the source table left empty is null, never 0."]),
    );
    out.insert(
        "extension".into(),
        json!({ "source": sources, "isPartOf": pages }),
    );
    (Value::Object(out), skipped)
}

/// The `value` member: an array in row-major order with `null` for a missing cell, or, when the
/// cube is larger than [`DENSE_CELLS`] or less than a quarter full, an object of the held cells
/// by their index.
fn values(
    cells: &BTreeMap<[String; 5], Value>,
    used: &[usize],
    categories: &[Vec<String>],
) -> Value {
    let positions: Vec<BTreeMap<&String, usize>> = categories
        .iter()
        .map(|list| list.iter().enumerate().map(|(i, c)| (c, i)).collect())
        .collect();
    let total = categories
        .iter()
        .try_fold(1usize, |n, list| n.checked_mul(list.len()));
    let index = |at: &[String; 5]| {
        used.iter()
            .zip(&positions)
            .zip(categories)
            .try_fold(0usize, |i, ((&d, pos), list)| {
                let p = pos.get(at.get(d)?)?;
                i.checked_mul(list.len())?.checked_add(*p)
            })
    };
    let held: Vec<(usize, &Value)> = cells
        .iter()
        .filter_map(|(at, v)| Some((index(at)?, v)))
        .collect();
    match total {
        Some(n) if n <= DENSE_CELLS && n <= held.len().saturating_mul(4) => {
            let by: BTreeMap<usize, &Value> = held.into_iter().collect();
            (0..n)
                .map(|i| by.get(&i).map_or(Value::Null, |v| (*v).clone()))
                .collect()
        }
        _ => Value::Object(
            held.into_iter()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member at JSON pointer `path`, `null` when there is none.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }

    fn kpi(area: &str, year: &str, key: Option<&str>, value: &Value) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(format!("urn:{area}-{year}")));
        m.insert("refArea".into(), json!({"type": "Property", "value": area}));
        m.insert(
            "refPeriod".into(),
            json!([{"type": "Property", "value": year}]),
        );
        m.insert(
            "indicator".into(),
            json!({"type": "Property", "value": "U"}),
        );
        m.insert(
            "name".into(),
            json!({"type": "LanguageProperty", "languageMap": {"en": "Births", "sk": "Narodení"}}),
        );
        m.insert(
            "kpiValue".into(),
            json!({"type": "Property", "value": value}),
        );
        m.insert(
            "dateObserved".into(),
            json!({"type": "Property", "value": "2026-01-01T00:00:00Z"}),
        );
        if let Some(k) = key {
            m.insert(
                "dimensionKey".into(),
                json!({"type": "Property", "value": k}),
            );
        }
        Value::Object(m)
    }

    #[test]
    fn a_key_dimension_appears_when_an_entity_carries_it_and_a_missing_key_is_none() {
        let (d, skipped) = dataset(
            "t",
            &[
                kpi("SK0", "2024", Some("M"), &json!(1)),
                kpi("SK0", "2024", None, &json!(2)),
            ],
        );
        assert_eq!(skipped, 0);
        assert_eq!(
            at(&d, "/id"),
            json!(["refArea", "refPeriod", "indicator", "dimensionKey"])
        );
        assert_eq!(
            at(&d, "/dimension/dimensionKey/category/index"),
            json!(["M", NONE])
        );
        assert_eq!(at(&d, "/value"), json!([1, 2]));
        assert_eq!(at(&d, "/dimension/indicator/category/label/U"), "Births");
        assert_eq!(at(&d, "/updated"), "2026-01-01T00:00:00Z");
        assert_eq!(at(&d, "/source"), Value::Null);
    }

    /// F-ANT5-009: a kpiValue whose instances all carry a datasetId is read from the
    /// first one, so the cell stays in the cube.
    #[test]
    fn a_named_only_kpi_value_is_still_a_cell() {
        let mut m: Map<String, Value> =
            serde_json::from_value(kpi("SK0", "2024", None, &json!(1))).unwrap_or_default();
        m.insert(
            "kpiValue".into(),
            json!([{"type": "Property", "datasetId": "urn:ngsi-ld:Dataset:sus", "value": 7}]),
        );
        let e = Value::Object(m);
        let (d, skipped) = dataset("t", &[e]);
        assert_eq!(skipped, 0);
        assert_eq!(at(&d, "/value"), json!([7]));
    }

    #[test]
    fn what_is_no_cell_or_a_second_entity_of_a_cell_is_left_out() {
        let mut nameless = kpi("SK0", "2023", None, &json!(5));
        nameless.as_object_mut().map(|m| m.remove("name"));
        let mut untexted = kpi("SK0", "2022", None, &json!(6));
        untexted
            .as_object_mut()
            .map(|m| m.insert("refArea".into(), json!({"value": 7})));
        let mut unvalued = kpi("SK0", "2021", None, &json!(1));
        unvalued.as_object_mut().map(|m| m.remove("kpiValue"));
        let (d, skipped) = dataset(
            "t",
            &[
                kpi("SK0", "2024", None, &json!(1)),
                kpi("SK0", "2024", None, &json!(9)),
                kpi("SK0", "2020", None, &json!("n/a")),
                nameless,
                untexted,
                unvalued,
            ],
        );
        assert_eq!(skipped, 4);
        assert_eq!(at(&d, "/value"), json!([1, 5]));
    }

    #[test]
    fn a_sparse_or_large_cube_is_an_object_of_its_cells() {
        let entities: Vec<Value> = (0..5)
            .map(|i| {
                kpi(
                    &format!("A{i}"),
                    &format!("{}", 2000_u32.saturating_add(i)),
                    None,
                    &json!(i),
                )
            })
            .collect();
        let (d, _) = dataset("t", &entities);
        assert_eq!(at(&d, "/size"), json!([5, 5, 1]));
        assert_eq!(
            at(&d, "/value"),
            json!({"4": 0, "8": 1, "12": 2, "16": 3, "20": 4})
        );
        // past a million cells a cube is an object whatever its fill
        let wide: Vec<Value> = (0..1001_u32)
            .map(|i| kpi(&format!("A{i}"), &format!("P{i}"), None, &json!(i)))
            .collect();
        let (d, _) = dataset("t", &wide);
        assert_eq!(at(&d, "/value").as_object().map(Map::len), Some(1001));
        let (empty, skipped) = dataset("t", &[]);
        assert_eq!((at(&empty, "/value"), skipped), (json!([]), 0));
    }
}
