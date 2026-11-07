#![forbid(unsafe_code)]
//! The 13 core Grafana data frame transformations of R4 (§3.4, §7.1):
//! 1. `organize` (`excludeByName`, `includeByName`, `indexByName`, `renameByName`)
//! 2. `merge`
//! 3. `reduce` (`seriesToRows`, `reduceFields`)
//! 4. `filterByValue` (`include`/`exclude`, `all`/`any`)
//! 5. `calculateField` (`reduceRow`, `cumulativeFunctions`, `windowFunctions`, `binary`, `unary`, `index`)
//! 6. `groupBy` (`groupby`, `aggregate`)
//! 7. `sortBy`
//! 8. `renameByRegex`
//! 9. `joinByField` (and alias `seriesToColumns`)
//! 10. `convertFieldType`
//! 11. `limit`
//! 12. `seriesToRows`
//! 13. `labelsToFields`
//!
//! Other transformation IDs emit a named warning and pass frames through unchanged (R4).

use crate::reduce::{ReducerId, reduce_slice};
use controlroom_model::{
    DataTransformerConfig, FieldColumn, FieldType, Frame, FrameMeta, Notice, NoticeSeverity,
};
use regex::Regex;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Outcome of running a panel's transformation pipeline.
#[derive(Debug, Clone)]
pub struct TransformationOutcome {
    pub frames: Vec<Frame>,
    pub warnings: Vec<String>,
}

/// Applies a sequence of [`DataTransformerConfig`] steps to `frames` (R4).
#[must_use]
pub fn apply_transformations(
    frames: Vec<Frame>,
    transformations: &[DataTransformerConfig],
) -> TransformationOutcome {
    let mut current = frames;
    let mut warnings = Vec::new();

    for tx in transformations {
        if tx.disabled == Some(true) {
            continue;
        }
        match tx.id.as_str() {
            "organize" => current = tx_organize(current, &tx.options),
            "merge" => current = tx_merge(current),
            "reduce" => current = tx_reduce(current, &tx.options),
            "filterByValue" => current = tx_filter_by_value(current, &tx.options),
            "calculateField" => current = tx_calculate_field(current, &tx.options),
            "groupBy" => current = tx_group_by(current, &tx.options),
            "sortBy" => current = tx_sort_by(current, &tx.options),
            "renameByRegex" => current = tx_rename_by_regex(current, &tx.options),
            "joinByField" | "seriesToColumns" => {
                current = tx_join_by_field(current, &tx.options);
            }
            "convertFieldType" => current = tx_convert_field_type(current, &tx.options),
            "limit" => current = tx_limit(current, &tx.options),
            "seriesToRows" => current = tx_series_to_rows(current),
            "labelsToFields" => current = tx_labels_to_fields(current, &tx.options),
            "noop" => {}
            other => {
                let msg = format!(
                    "transformation '{other}' is not executed in this run and passes through unchanged"
                );
                warnings.push(msg.clone());
                if let Some(first) = current.first_mut() {
                    first.meta.notices.push(Notice {
                        severity: NoticeSeverity::Warning,
                        text: msg,
                    });
                }
            }
        }
    }

    TransformationOutcome {
        frames: current,
        warnings,
    }
}

/// 1. `organize`: excludeByName, includeByName, indexByName, renameByName (`organize.ts`).
#[must_use]
pub fn tx_organize(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let exclude = options
        .get("excludeByName")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let include = options
        .get("includeByName")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let has_include = include.values().any(|v| v.as_bool() == Some(true));
    let index_by_name = options
        .get("indexByName")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let rename_by_name = options
        .get("renameByName")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    frames
        .into_iter()
        .map(|mut frame| {
            // 1. Filter
            let filtered: Vec<(usize, FieldColumn)> = frame
                .fields
                .into_iter()
                .enumerate()
                .filter(|(_, f)| {
                    let eff = f.effective_name();
                    if exclude.get(&f.name).and_then(Value::as_bool) == Some(true)
                        || exclude.get(eff).and_then(Value::as_bool) == Some(true)
                    {
                        return false;
                    }
                    if has_include {
                        return include.get(&f.name).and_then(Value::as_bool) == Some(true)
                            || include.get(eff).and_then(Value::as_bool) == Some(true);
                    }
                    true
                })
                .collect();

            // 2. Order by indexByName (falling back to original index)
            let mut indexed: Vec<(i64, usize, FieldColumn)> = filtered
                .into_iter()
                .map(|(orig_idx, f)| {
                    let idx = index_by_name
                        .get(&f.name)
                        .or_else(|| index_by_name.get(f.effective_name()))
                        .and_then(Value::as_i64)
                        .unwrap_or(1_000_000 + orig_idx as i64);
                    (idx, orig_idx, f)
                })
                .collect();
            indexed.sort_by_key(|(idx, orig_idx, _)| (*idx, *orig_idx));

            // 3. Rename
            frame.fields = indexed
                .into_iter()
                .map(|(_, _, mut f)| {
                    if let Some(new_name) = rename_by_name
                        .get(&f.name)
                        .or_else(|| rename_by_name.get(f.effective_name()))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        f.display_name = Some(new_name.to_owned());
                        f.name = new_name.to_owned();
                    }
                    f
                })
                .collect();
            frame
        })
        .collect()
}

/// 2. `merge`: merges multiple frames sharing common key fields into a single frame (`merge.ts`).
#[must_use]
pub fn tx_merge(frames: Vec<Frame>) -> Vec<Frame> {
    let non_empty: Vec<&Frame> = frames.iter().filter(|f| !f.fields.is_empty()).collect();
    if non_empty.len() <= 1 {
        return frames;
    }

    let mut all_field_names: Vec<(String, FieldType, Option<String>)> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    for frame in &non_empty {
        let mut seen_in_frame = BTreeSet::new();
        for f in &frame.fields {
            if seen_in_frame.insert(f.name.clone()) {
                *counts.entry(f.name.clone()).or_insert(0) += 1;
            }
            if !all_field_names.iter().any(|(n, _, _)| n == &f.name) {
                all_field_names.push((f.name.clone(), f.field_type, f.unit.clone()));
            }
        }
    }

    let key_fields: Vec<String> = all_field_names
        .iter()
        .filter(|(n, _, _)| counts.get(n).copied() == Some(non_empty.len()))
        .map(|(n, _, _)| n.clone())
        .collect();

    if key_fields.is_empty() {
        return frames;
    }

    let mut merged_rows: Vec<BTreeMap<String, Value>> = Vec::new();
    for frame in &non_empty {
        let row_cnt = frame.row_count();
        for r in 0..row_cnt {
            let mut row_map = BTreeMap::new();
            for f in &frame.fields {
                if let Some(v) = f.values.get(r) {
                    row_map.insert(f.name.clone(), v.clone());
                }
            }
            // Try to merge with an existing row that matches on all key_fields and has no conflicting fields
            let mut merged = false;
            for existing in &mut merged_rows {
                let same_key = key_fields
                    .iter()
                    .all(|k| existing.get(k) == row_map.get(k));
                if !same_key {
                    continue;
                }
                let compatible = row_map.iter().all(|(k, v)| match existing.get(k) {
                    None | Some(Value::Null) => true,
                    Some(ev) => ev == v,
                });
                if compatible {
                    for (k, v) in &row_map {
                        existing.insert(k.clone(), v.clone());
                    }
                    merged = true;
                    break;
                }
            }
            if !merged {
                merged_rows.push(row_map);
            }
        }
    }

    let fields = all_field_names
        .into_iter()
        .map(|(name, field_type, unit)| {
            let values = merged_rows
                .iter()
                .map(|r| r.get(&name).cloned().unwrap_or(Value::Null))
                .collect();
            FieldColumn {
                name,
                field_type,
                unit,
                display_name: None,
                labels: BTreeMap::new(),
                values,
            }
        })
        .collect();

    vec![Frame {
        name: "merged".to_owned(),
        ref_id: Some("merge".to_owned()),
        fields,
        meta: FrameMeta::default(),
    }]
}

/// 3. `reduce`: reduces fields to rows (`seriesToRows`) or collapses each field to 1 value (`reduceFields`) (`reduce.ts`).
#[must_use]
pub fn tx_reduce(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let reducers: Vec<(String, ReducerId)> = options
        .get("reducers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| ReducerId::parse(s).ok().map(|r| (s.to_owned(), r)))
                .collect()
        })
        .unwrap_or_else(|| vec![("max".to_owned(), ReducerId::Max)]);

    if reducers.is_empty() {
        return frames;
    }

    let mode = options
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("seriesToRows");

    if mode == "reduceFields" {
        return frames
            .into_iter()
            .filter_map(|mut frame| {
                let mut new_fields = Vec::new();
                for f in &frame.fields {
                    if f.field_type == FieldType::Time {
                        continue;
                    }
                    let nums: Vec<Option<f64>> = (0..f.len()).map(|i| f.f64_at(i)).collect();
                    let non_null: Vec<Option<f64>> =
                        nums.into_iter().filter(|x| x.is_some()).collect();
                    for (_, r_id) in &reducers {
                        let v = reduce_slice(*r_id, &non_null);
                        new_fields.push(FieldColumn::numbers(
                            f.effective_name(),
                            f.labels.clone(),
                            vec![v],
                        ));
                    }
                }
                if new_fields.is_empty() {
                    None
                } else {
                    frame.fields = new_fields;
                    Some(frame)
                }
            })
            .collect();
    }

    // Default: seriesToRows
    let mut field_names = Vec::new();
    let mut calc_cols: BTreeMap<String, Vec<Option<f64>>> = BTreeMap::new();
    for (r_name, _) in &reducers {
        calc_cols.insert(r_name.clone(), Vec::new());
    }

    for frame in &frames {
        for f in &frame.fields {
            if f.field_type == FieldType::Time {
                continue;
            }
            field_names.push(Some(f.effective_name().to_owned()));
            let non_null: Vec<Option<f64>> =
                (0..f.len()).filter_map(|i| f.f64_at(i).map(Some)).collect();
            for (r_name, r_id) in &reducers {
                let v = reduce_slice(*r_id, &non_null);
                if let Some(col) = calc_cols.get_mut(r_name) {
                    col.push(v);
                }
            }
        }
    }

    let mut out_fields = vec![FieldColumn::strings("Field", field_names)];
    for (r_name, _) in &reducers {
        if let Some(vals) = calc_cols.remove(r_name) {
            out_fields.push(FieldColumn::numbers(r_name.clone(), BTreeMap::new(), vals));
        }
    }

    vec![Frame {
        name: "reduce".to_owned(),
        ref_id: Some("reduce".to_owned()),
        fields: out_fields,
        meta: FrameMeta::default(),
    }]
}

/// 4. `filterByValue`: filters rows by predicates (`filterByValue.ts`).
#[must_use]
pub fn tx_filter_by_value(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let Some(filters) = options.get("filters").and_then(Value::as_array) else {
        return frames;
    };
    if filters.is_empty() {
        return frames;
    }
    let include = options.get("type").and_then(Value::as_str) != Some("exclude");
    let match_all = options.get("match").and_then(Value::as_str) == Some("all");

    frames
        .into_iter()
        .map(|mut frame| {
            let row_cnt = frame.row_count();
            let mut keep_rows = Vec::with_capacity(row_cnt);

            for r in 0..row_cnt {
                let mut matched = match_all;
                for filter in filters {
                    let field_name = filter
                        .get("fieldName")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let cfg = filter.get("config").unwrap_or(&Value::Null);
                    let m_id = cfg.get("id").and_then(Value::as_str).unwrap_or("equal");
                    let m_opts = cfg.get("options").unwrap_or(&Value::Null);

                    let field_hit = frame
                        .fields
                        .iter()
                        .find(|f| f.name == field_name || f.effective_name() == field_name)
                        .is_some_and(|col| eval_value_matcher(col, r, m_id, m_opts));

                    if match_all && !field_hit {
                        matched = false;
                        break;
                    }
                    if !match_all && field_hit {
                        matched = true;
                        break;
                    }
                }
                if (include && matched) || (!include && !matched) {
                    keep_rows.push(r);
                }
            }

            for col in &mut frame.fields {
                col.values = keep_rows
                    .iter()
                    .filter_map(|&r| col.values.get(r).cloned())
                    .collect();
            }
            frame
        })
        .collect()
}

fn eval_value_matcher(col: &FieldColumn, row: usize, id: &str, opts: &Value) -> bool {
    let cell = col.values.get(row).unwrap_or(&Value::Null);
    let target_val = opts.get("value").unwrap_or(opts);
    match id {
        "isNull" => cell.is_null(),
        "isNotNull" => !cell.is_null(),
        "equal" => {
            if let (Some(a), Some(b)) = (col.f64_at(row), target_val.as_f64()) {
                (a - b).abs() == 0.0
            } else {
                value_to_str(cell) == value_to_str(target_val)
            }
        }
        "greater" => col
            .f64_at(row)
            .zip(target_val.as_f64())
            .is_some_and(|(a, b)| a > b),
        "greaterOrEqual" => col
            .f64_at(row)
            .zip(target_val.as_f64())
            .is_some_and(|(a, b)| a >= b),
        "lower" => col
            .f64_at(row)
            .zip(target_val.as_f64())
            .is_some_and(|(a, b)| a < b),
        "lowerOrEqual" => col
            .f64_at(row)
            .zip(target_val.as_f64())
            .is_some_and(|(a, b)| a <= b),
        "between" => {
            let from = opts.get("from").and_then(Value::as_f64);
            let to = opts.get("to").and_then(Value::as_f64);
            col.f64_at(row)
                .zip(from)
                .zip(to)
                .is_some_and(|((v, lo), hi)| v >= lo && v <= hi)
        }
        "regex" => {
            let pat = target_val.as_str().unwrap_or("");
            Regex::new(pat).is_ok_and(|re| re.is_match(&value_to_str(cell)))
        }
        _ => false,
    }
}

fn value_to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 5. `calculateField`: adds or replaces a calculated column (`calculateField.ts`).
#[must_use]
pub fn tx_calculate_field(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let mode = options
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("reduceRow");
    let alias = options
        .get("alias")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let replace_fields = options
        .get("replaceFields")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    frames
        .into_iter()
        .map(|mut frame| {
            let row_cnt = frame.row_count();
            let (default_name, vals): (String, Vec<Option<f64>>) = match mode {
                "index" => {
                    let as_pct = options
                        .get("index")
                        .and_then(|i| i.get("asPercentile"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let v = (0..row_cnt)
                        .map(|i| {
                            if as_pct && row_cnt > 0 {
                                Some((i as f64) / (row_cnt as f64))
                            } else {
                                Some(i as f64)
                            }
                        })
                        .collect();
                    ("Row".to_owned(), v)
                }
                "binary" => {
                    let bin = options.get("binary").unwrap_or(&Value::Null);
                    let op = bin.get("operator").and_then(Value::as_str).unwrap_or("+");
                    let left_vals = resolve_binary_operand(&frame, bin.get("left"), row_cnt);
                    let right_vals = resolve_binary_operand(&frame, bin.get("right"), row_cnt);
                    let mut v = Vec::with_capacity(row_cnt);
                    for i in 0..row_cnt {
                        let a = left_vals.get(i).copied().flatten().unwrap_or(0.0);
                        let b = right_vals.get(i).copied().flatten().unwrap_or(0.0);
                        let res = match op {
                            "+" => a + b,
                            "-" => a - b,
                            "*" => a * b,
                            "/" => {
                                if b == 0.0 {
                                    f64::NAN
                                } else {
                                    a / b
                                }
                            }
                            _ => a + b,
                        };
                        v.push(Some(res));
                    }
                    ("math".to_owned(), v)
                }
                "cumulativeFunctions" => {
                    let cum = options.get("cumulative").unwrap_or(&Value::Null);
                    let reducer = cum
                        .get("reducer")
                        .and_then(Value::as_str)
                        .unwrap_or("sum");
                    let target = cum.get("field").and_then(Value::as_str);
                    let col = frame.fields.iter().find(|f| {
                        target.map_or(f.field_type == FieldType::Number, |t| {
                            f.name == t || f.effective_name() == t
                        })
                    });
                    let mut total = 0.0;
                    let mut v = Vec::with_capacity(row_cnt);
                    for i in 0..row_cnt {
                        total += col.and_then(|c| c.f64_at(i)).unwrap_or(0.0);
                        if reducer == "mean" {
                            v.push(Some(total / ((i + 1) as f64)));
                        } else {
                            v.push(Some(total));
                        }
                    }
                    (format!("cumulative {reducer}"), v)
                }
                _ => {
                    // reduceRow
                    let red = options.get("reduce").unwrap_or(&Value::Null);
                    let r_str = red
                        .get("reducer")
                        .and_then(Value::as_str)
                        .unwrap_or("sum");
                    let r_id = ReducerId::parse(r_str).unwrap_or(ReducerId::Sum);
                    let num_cols: Vec<&FieldColumn> = frame
                        .fields
                        .iter()
                        .filter(|f| f.field_type == FieldType::Number)
                        .collect();
                    let mut v = Vec::with_capacity(row_cnt);
                    for i in 0..row_cnt {
                        let row_vals: Vec<Option<f64>> =
                            num_cols.iter().filter_map(|c| c.f64_at(i).map(Some)).collect();
                        v.push(reduce_slice(r_id, &row_vals));
                    }
                    (r_str.to_owned(), v)
                }
            };

            let col_name = alias.unwrap_or(&default_name).to_owned();
            let new_col = FieldColumn::numbers(col_name, BTreeMap::new(), vals);
            if replace_fields {
                let mut kept: Vec<FieldColumn> = frame
                    .fields
                    .into_iter()
                    .filter(|f| f.field_type == FieldType::Time)
                    .take(1)
                    .collect();
                kept.push(new_col);
                frame.fields = kept;
            } else {
                frame.fields.push(new_col);
            }
            frame
        })
        .collect()
}

fn resolve_binary_operand(
    frame: &Frame,
    spec: Option<&Value>,
    row_cnt: usize,
) -> Vec<Option<f64>> {
    let Some(spec) = spec else {
        return vec![Some(0.0); row_cnt];
    };
    if let Some(s) = spec.as_str() {
        if let Some(col) = frame
            .fields
            .iter()
            .find(|f| f.name == s || f.effective_name() == s)
        {
            return (0..row_cnt).map(|i| col.f64_at(i)).collect();
        }
        if let Ok(fixed) = s.parse::<f64>() {
            return vec![Some(fixed); row_cnt];
        }
    }
    if let Some(name) = spec
        .get("matcher")
        .and_then(|m| m.get("options"))
        .and_then(Value::as_str)
    {
        if let Some(col) = frame
            .fields
            .iter()
            .find(|f| f.name == name || f.effective_name() == name)
        {
            return (0..row_cnt).map(|i| col.f64_at(i)).collect();
        }
    }
    if let Some(fixed_str) = spec.get("fixed").and_then(Value::as_str) {
        if let Ok(fixed) = fixed_str.parse::<f64>() {
            return vec![Some(fixed); row_cnt];
        }
    }
    if let Some(fixed_num) = spec.get("fixed").and_then(Value::as_f64) {
        return vec![Some(fixed_num); row_cnt];
    }
    vec![Some(0.0); row_cnt]
}

/// 6. `groupBy`: groups rows by key columns and aggregates remaining columns (`groupBy.ts`).
#[must_use]
pub fn tx_group_by(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let Some(fields_cfg) = options.get("fields").and_then(Value::as_object) else {
        return frames;
    };
    let mut group_keys = Vec::new();
    let mut agg_specs: Vec<(String, Vec<String>)> = Vec::new();

    for (f_name, cfg) in fields_cfg {
        let op = cfg.get("operation").and_then(Value::as_str).unwrap_or("");
        if op == "groupby" {
            group_keys.push(f_name.clone());
        } else if op == "aggregate" {
            let aggs: Vec<String> = cfg
                .get("aggregations")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !aggs.is_empty() {
                agg_specs.push((f_name.clone(), aggs));
            }
        }
    }

    if group_keys.is_empty() {
        return frames;
    }

    frames
        .into_iter()
        .filter_map(|frame| {
            let key_cols: Vec<&FieldColumn> = group_keys
                .iter()
                .filter_map(|k| {
                    frame
                        .fields
                        .iter()
                        .find(|f| &f.name == k || f.effective_name() == k)
                })
                .collect();
            if key_cols.is_empty() {
                return None;
            }

            let row_cnt = frame.row_count();
            let mut group_order: Vec<String> = Vec::new();
            let mut group_rows: BTreeMap<String, Vec<usize>> = BTreeMap::new();

            for r in 0..row_cnt {
                let key = key_cols
                    .iter()
                    .map(|c| value_to_str(c.values.get(r).unwrap_or(&Value::Null)))
                    .collect::<Vec<_>>()
                    .join("\u{0}");
                if !group_rows.contains_key(&key) {
                    group_order.push(key.clone());
                }
                group_rows.entry(key).or_default().push(r);
            }

            let mut new_fields = Vec::new();
            for kc in &key_cols {
                let vals: Vec<Value> = group_order
                    .iter()
                    .map(|k| {
                        let first_row = group_rows[k][0];
                        kc.values.get(first_row).cloned().unwrap_or(Value::Null)
                    })
                    .collect();
                new_fields.push(FieldColumn {
                    name: kc.name.clone(),
                    field_type: kc.field_type,
                    unit: kc.unit.clone(),
                    display_name: kc.display_name.clone(),
                    labels: BTreeMap::new(),
                    values: vals,
                });
            }

            for (agg_field, aggs) in &agg_specs {
                let Some(col) = frame
                    .fields
                    .iter()
                    .find(|f| &f.name == agg_field || f.effective_name() == agg_field)
                else {
                    continue;
                };
                for agg_name in aggs {
                    let Ok(r_id) = ReducerId::parse(agg_name) else {
                        continue;
                    };
                    let vals: Vec<Option<f64>> = group_order
                        .iter()
                        .map(|k| {
                            let rows = &group_rows[k];
                            let slice: Vec<Option<f64>> =
                                rows.iter().filter_map(|&r| col.f64_at(r).map(Some)).collect();
                            reduce_slice(r_id, &slice)
                        })
                        .collect();
                    new_fields.push(FieldColumn::numbers(
                        format!("{} ({agg_name})", col.effective_name()),
                        BTreeMap::new(),
                        vals,
                    ));
                }
            }

            Some(Frame {
                name: frame.name,
                ref_id: frame.ref_id,
                fields: new_fields,
                meta: frame.meta,
            })
        })
        .collect()
}

/// 7. `sortBy`: sorts rows by the first configured field (`sortBy.ts`).
#[must_use]
pub fn tx_sort_by(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let Some(sort_arr) = options.get("sort").and_then(Value::as_array) else {
        return frames;
    };
    let Some(first) = sort_arr.first() else {
        return frames;
    };
    let target_field = first
        .get("field")
        .or_else(|| first.get("displayName"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let desc = first.get("desc").and_then(Value::as_bool).unwrap_or(false);

    frames
        .into_iter()
        .map(|mut frame| {
            let Some(col_idx) = frame
                .fields
                .iter()
                .position(|f| f.name == target_field || f.effective_name() == target_field)
            else {
                return frame;
            };
            sort_frame_by_col(&mut frame, col_idx, desc);
            frame
        })
        .collect()
}

pub fn sort_frame_by_col(frame: &mut Frame, col_idx: usize, desc: bool) {
    let row_cnt = frame.row_count();
    let mut indices: Vec<usize> = (0..row_cnt).collect();
    let col = &frame.fields[col_idx];
    indices.sort_by(|&a, &b| {
        let ord = match (col.f64_at(a), col.f64_at(b)) {
            (Some(fa), Some(fb)) => fa.partial_cmp(&fb).unwrap_or(Ordering::Equal),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => {
                let sa = value_to_str(col.values.get(a).unwrap_or(&Value::Null));
                let sb = value_to_str(col.values.get(b).unwrap_or(&Value::Null));
                sa.cmp(&sb)
            }
        };
        if desc { ord.reverse() } else { ord }
    });
    for f in &mut frame.fields {
        f.values = indices
            .iter()
            .filter_map(|&i| f.values.get(i).cloned())
            .collect();
    }
}

/// 8. `renameByRegex`: renames fields matching `regex` using `renamePattern` (`renameByRegex.ts`).
#[must_use]
pub fn tx_rename_by_regex(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let pattern = options
        .get("regex")
        .and_then(Value::as_str)
        .unwrap_or("(.*)");
    let replacement = options
        .get("renamePattern")
        .and_then(Value::as_str)
        .unwrap_or("$1");
    let Ok(re) = Regex::new(pattern) else {
        return frames;
    };

    frames
        .into_iter()
        .map(|mut frame| {
            for f in &mut frame.fields {
                let current = f.effective_name().to_owned();
                if re.is_match(&current) {
                    let renamed = re.replace_all(&current, replacement).into_owned();
                    f.display_name = Some(renamed.clone());
                    f.name = renamed;
                }
            }
            frame
        })
        .collect()
}

/// 9. `joinByField` (and alias `seriesToColumns`): joins multiple frames by a key or time field (`joinByField.ts`).
#[must_use]
pub fn tx_join_by_field(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    if frames.len() <= 1 {
        return frames;
    }
    let by_field = options
        .get("byField")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let mode = options
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("outer");

    // Pick join field name (explicit byField or first Time column or first column)
    let join_name = by_field.map(str::to_owned).unwrap_or_else(|| {
        frames[0]
            .fields
            .iter()
            .find(|f| f.field_type == FieldType::Time)
            .or_else(|| frames[0].fields.first())
            .map_or_else(|| "Time".to_owned(), |f| f.name.clone())
    });

    let join_type = frames[0]
        .fields
        .iter()
        .find(|f| f.name == join_name || f.effective_name() == join_name)
        .map_or(FieldType::Time, |f| f.field_type);

    let mut all_keys_order: Vec<String> = Vec::new();
    let mut key_raw_val: BTreeMap<String, Value> = BTreeMap::new();
    let mut frame_maps: Vec<BTreeMap<String, usize>> = Vec::with_capacity(frames.len());

    for frame in &frames {
        let mut map = BTreeMap::new();
        if let Some(j_col) = frame
            .fields
            .iter()
            .find(|f| f.name == join_name || f.effective_name() == join_name)
        {
            for (r, v) in j_col.values.iter().enumerate() {
                let k = value_to_str(v);
                if !key_raw_val.contains_key(&k) {
                    all_keys_order.push(k.clone());
                    key_raw_val.insert(k.clone(), v.clone());
                }
                map.insert(k, r);
            }
        }
        frame_maps.push(map);
    }

    let kept_keys: Vec<String> = if mode == "inner" {
        all_keys_order
            .into_iter()
            .filter(|k| frame_maps.iter().all(|m| m.contains_key(k)))
            .collect()
    } else {
        all_keys_order
    };

    let mut out_fields = vec![FieldColumn {
        name: join_name.clone(),
        field_type: join_type,
        unit: None,
        display_name: None,
        labels: BTreeMap::new(),
        values: kept_keys
            .iter()
            .map(|k| key_raw_val.get(k).cloned().unwrap_or(Value::Null))
            .collect(),
    }];

    for (f_idx, frame) in frames.iter().enumerate() {
        let row_lookup = &frame_maps[f_idx];
        for col in &frame.fields {
            if col.name == join_name || col.effective_name() == join_name {
                continue;
            }
            let values = kept_keys
                .iter()
                .map(|k| {
                    row_lookup
                        .get(k)
                        .and_then(|&r| col.values.get(r).cloned())
                        .unwrap_or(Value::Null)
                })
                .collect();
            out_fields.push(FieldColumn {
                name: col.name.clone(),
                field_type: col.field_type,
                unit: col.unit.clone(),
                display_name: col.display_name.clone(),
                labels: col.labels.clone(),
                values,
            });
        }
    }

    vec![Frame {
        name: "joined".to_owned(),
        ref_id: Some("joinByField".to_owned()),
        fields: out_fields,
        meta: FrameMeta::default(),
    }]
}

/// 10. `convertFieldType`: converts field types (`convertFieldType.ts`).
#[must_use]
pub fn tx_convert_field_type(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let Some(conversions) = options.get("conversions").and_then(Value::as_array) else {
        return frames;
    };

    frames
        .into_iter()
        .map(|mut frame| {
            for conv in conversions {
                let target = conv
                    .get("targetField")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let dest = conv
                    .get("destinationType")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                for f in &mut frame.fields {
                    if f.name != target && f.effective_name() != target {
                        continue;
                    }
                    match dest {
                        "number" => {
                            f.field_type = FieldType::Number;
                            f.values = f
                                .values
                                .iter()
                                .map(|v| match v {
                                    Value::Number(n) => Value::Number(n.clone()),
                                    Value::String(s) => s
                                        .replace(',', "")
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .and_then(serde_json::Number::from_f64)
                                        .map_or(Value::Null, Value::Number),
                                    Value::Bool(b) => Value::from(if *b { 1.0 } else { 0.0 }),
                                    _ => Value::Null,
                                })
                                .collect();
                        }
                        "string" => {
                            f.field_type = FieldType::String;
                            f.values = f
                                .values
                                .iter()
                                .map(|v| match v {
                                    Value::Null => Value::Null,
                                    Value::String(s) => Value::String(s.clone()),
                                    other => Value::String(other.to_string()),
                                })
                                .collect();
                        }
                        "boolean" | "bool" => {
                            f.field_type = FieldType::Bool;
                            f.values = f
                                .values
                                .iter()
                                .map(|v| match v {
                                    Value::Bool(b) => Value::Bool(*b),
                                    Value::Number(n) => {
                                        Value::Bool(n.as_f64().is_some_and(|x| x != 0.0))
                                    }
                                    Value::String(s) => {
                                        Value::Bool(!s.is_empty() && s != "false" && s != "0")
                                    }
                                    _ => Value::Bool(false),
                                })
                                .collect();
                        }
                        "time" => {
                            f.field_type = FieldType::Time;
                            f.values = (0..f.len())
                                .map(|i| f.time_ms_at(i).map_or(Value::Null, Value::from))
                                .collect();
                        }
                        _ => {}
                    }
                }
            }
            frame
        })
        .collect()
}

/// 11. `limit`: limits rows to the first N (or last |N| when negative) (`limit.ts`).
#[must_use]
pub fn tx_limit(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let limit = match options.get("limitField") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(10),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(10),
        _ => 10,
    };

    frames
        .into_iter()
        .map(|mut frame| {
            let row_cnt = frame.row_count() as i64;
            let abs_limit = limit.unsigned_abs() as usize;
            if row_cnt > abs_limit as i64 {
                for f in &mut frame.fields {
                    if limit >= 0 {
                        f.values.truncate(abs_limit);
                    } else {
                        let start = f.values.len().saturating_sub(abs_limit);
                        f.values = f.values.split_off(start);
                    }
                }
            }
            frame
        })
        .collect()
}

/// 12. `seriesToRows`: combines multiple time series into `[Time, Metric, Value]` sorted descending (`seriesToRows.ts`).
#[must_use]
pub fn tx_series_to_rows(frames: Vec<Frame>) -> Vec<Frame> {
    let mut rows: Vec<(i64, String, Value)> = Vec::new();
    for frame in &frames {
        let Some(t_col) = frame
            .fields
            .iter()
            .find(|f| f.field_type == FieldType::Time)
        else {
            continue;
        };
        for val_col in frame
            .fields
            .iter()
            .filter(|f| f.field_type != FieldType::Time)
        {
            let metric = if !frame.name.is_empty() {
                frame.name.clone()
            } else {
                val_col.effective_name().to_owned()
            };
            let len = t_col.len().min(val_col.len());
            for i in 0..len {
                let t = t_col.time_ms_at(i).unwrap_or(0);
                let v = val_col.values.get(i).cloned().unwrap_or(Value::Null);
                rows.push((t, metric.clone(), v));
            }
        }
    }
    if rows.is_empty() {
        return frames;
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    let times: Vec<i64> = rows.iter().map(|(t, _, _)| *t).collect();
    let metrics: Vec<Option<String>> = rows.iter().map(|(_, m, _)| Some(m.clone())).collect();
    let values: Vec<Value> = rows.into_iter().map(|(_, _, v)| v).collect();

    vec![Frame {
        name: "seriesToRows".to_owned(),
        ref_id: Some("seriesToRows".to_owned()),
        fields: vec![
            FieldColumn::time("Time", times),
            FieldColumn::strings("Metric", metrics),
            FieldColumn {
                name: "Value".to_owned(),
                field_type: FieldType::Number,
                unit: None,
                display_name: None,
                labels: BTreeMap::new(),
                values,
            },
        ],
        meta: FrameMeta::default(),
    }]
}

/// 13. `labelsToFields`: extracts field labels into columns or rows (`labelsToFields.ts`).
#[must_use]
pub fn tx_labels_to_fields(frames: Vec<Frame>, options: &Value) -> Vec<Frame> {
    let mode = options
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("columns");
    let keep_labels: Option<BTreeSet<String>> = options
        .get("keepLabels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .filter(|s: &BTreeSet<String>| !s.is_empty());
    let value_label = options.get("valueLabel").and_then(Value::as_str);

    if mode == "rows" {
        let mut out = Vec::new();
        for frame in &frames {
            for f in &frame.fields {
                if f.labels.is_empty() {
                    continue;
                }
                let mut keys = Vec::new();
                let mut vals = Vec::new();
                for (k, v) in &f.labels {
                    if keep_labels.as_ref().is_none_or(|set| set.contains(k)) {
                        keys.push(Some(k.clone()));
                        vals.push(Some(v.clone()));
                    }
                }
                if !keys.is_empty() {
                    out.push(Frame {
                        name: f.effective_name().to_owned(),
                        ref_id: frame.ref_id.clone(),
                        fields: vec![
                            FieldColumn::strings("label", keys),
                            FieldColumn::strings("value", vals),
                        ],
                        meta: frame.meta.clone(),
                    });
                }
            }
        }
        return out;
    }

    frames
        .into_iter()
        .map(|mut frame| {
            let row_cnt = frame.row_count();
            let mut extracted: BTreeMap<String, String> = BTreeMap::new();
            for f in &mut frame.fields {
                if f.labels.is_empty() {
                    continue;
                }
                for (k, v) in std::mem::take(&mut f.labels) {
                    if keep_labels.as_ref().is_some_and(|set| !set.contains(&k)) {
                        continue;
                    }
                    if value_label == Some(k.as_str()) {
                        f.name = v;
                        f.display_name = None;
                        continue;
                    }
                    extracted.insert(k, v);
                }
            }
            for (k, v) in extracted {
                frame
                    .fields
                    .push(FieldColumn::strings(k, vec![Some(v); row_cnt]));
            }
            frame
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organize_excludes_reorders_and_renames_like_bbsk_and_helsinki() {
        let frame = Frame {
            name: "table".into(),
            ref_id: Some("A".into()),
            fields: vec![
                FieldColumn::strings("id", vec![Some("urn:1".into())]),
                FieldColumn::strings("type", vec![Some("AirQualityObserved".into())]),
                FieldColumn::time("observedAt", vec![1000]),
                FieldColumn::strings("name", vec![Some("Mannerheimintie".into())]),
                FieldColumn::numbers("pm25", BTreeMap::new(), vec![Some(8.4)]),
                FieldColumn::numbers("pm10", BTreeMap::new(), vec![Some(14.1)]),
            ],
            meta: FrameMeta::default(),
        };

        let opts = json!({
            "excludeByName": {
                "id": true,
                "type": true,
                "observedAt": true
            },
            "indexByName": {
                "name": 0,
                "pm25": 1,
                "pm10": 2
            },
            "renameByName": {
                "name": "Station",
                "pm25": "PM2.5",
                "pm10": "PM10"
            }
        });

        let out = tx_organize(vec![frame], &opts);
        let col_names: Vec<&str> = out[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(col_names, ["Station", "PM2.5", "PM10"]);
    }

    #[test]
    fn all_13_transformations_execute_cleanly() {
        let mut labels = BTreeMap::new();
        labels.insert("env".into(), "prod".into());
        let f1 = Frame {
            name: "s1".into(),
            ref_id: Some("A".into()),
            fields: vec![
                FieldColumn::time("Time", vec![1000, 2000, 3000]),
                FieldColumn::numbers("val", labels, vec![Some(10.0), Some(30.0), Some(20.0)]),
            ],
            meta: FrameMeta::default(),
        };

        // limit
        let limited = tx_limit(vec![f1.clone()], &json!({"limitField": 2}));
        assert_eq!(limited[0].row_count(), 2);

        // sortBy desc
        let sorted = tx_sort_by(
            vec![f1.clone()],
            &json!({"sort": [{"field": "val", "desc": true}]}),
        );
        assert_eq!(sorted[0].fields[1].f64_at(0), Some(30.0));

        // filterByValue > 15
        let filtered = tx_filter_by_value(
            vec![f1.clone()],
            &json!({
                "type": "include",
                "match": "all",
                "filters": [{"fieldName": "val", "config": {"id": "greater", "options": {"value": 15.0}}}]
            }),
        );
        assert_eq!(filtered[0].row_count(), 2);

        // calculateField binary
        let calc = tx_calculate_field(
            vec![f1.clone()],
            &json!({
                "mode": "binary",
                "alias": "doubled",
                "binary": {"left": "val", "operator": "*", "right": {"fixed": "2"}}
            }),
        );
        assert_eq!(calc[0].fields[2].name, "doubled");
        assert_eq!(calc[0].fields[2].f64_at(0), Some(20.0));

        // renameByRegex
        let renamed = tx_rename_by_regex(
            vec![f1.clone()],
            &json!({"regex": "^v(.*)", "renamePattern": "metric_$1"}),
        );
        assert_eq!(renamed[0].fields[1].name, "metric_al");

        // labelsToFields
        let l2f = tx_labels_to_fields(vec![f1.clone()], &json!({"mode": "columns"}));
        assert!(l2f[0].fields.iter().any(|f| f.name == "env"));

        // seriesToRows
        let s2r = tx_series_to_rows(vec![f1]);
        assert_eq!(s2r[0].fields.len(), 3);
        assert_eq!(s2r[0].row_count(), 3);
    }
}
