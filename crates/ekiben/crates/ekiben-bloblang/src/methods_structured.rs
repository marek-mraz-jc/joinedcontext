use std::collections::{BTreeMap, HashSet};

use crate::ast::{ArgList, Expr};
use crate::executor::EvalContext;
use crate::methods::{get_arg, get_first_arg, get_optional_arg};
use crate::value::Value;

pub(crate) fn eval_structured_family(
    target_expr: &Expr,
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    match name {
        "length" => Some(match target_val {
            Value::String(s) => Ok(Value::Int(s.len() as i64)),
            Value::Bytes(b) => Ok(Value::Int(b.len() as i64)),
            Value::Array(a) => Ok(Value::Int(a.len() as i64)),
            Value::Object(o) => Ok(Value::Int(o.len() as i64)),
            other => Err(format!(
                "expected string, array or object value, got {}",
                other.type_name()
            )),
        }),
        "index" => Some((|| {
            let idx = ctx.eval_expr(get_first_arg(args, "index")?)?.get_int()?;
            match target_val {
                Value::Array(arr) => {
                    let len = arr.len() as i64;
                    let norm = if idx < 0 { len + idx } else { idx };
                    if norm < 0 || norm >= len {
                        return Err(format!(
                            "index '{norm}' was out of bounds for array size: {len}"
                        ));
                    }
                    Ok(arr[norm as usize].clone())
                }
                Value::Bytes(bytes) => {
                    let len = bytes.len() as i64;
                    let norm = if idx < 0 { len + idx } else { idx };
                    if norm < 0 || norm >= len {
                        return Err(format!(
                            "index '{norm}' was out of bounds for array size: {len}"
                        ));
                    }
                    Ok(Value::Int(bytes[norm as usize] as i64))
                }
                other => Err(format!("expected array value, got {}", other.type_name())),
            }
        })()),
        "slice" => Some((|| {
            let low = ctx.eval_expr(get_arg(args, "low", 0)?)?.get_int()?;
            let high_opt = match get_optional_arg(args, "high", 1) {
                Some(e) => Some(ctx.eval_expr(e)?.get_int()?),
                None => None,
            };
            if let Some(h) = high_opt {
                if h > 0 && low >= h {
                    return Err(format!(
                        "lower slice bound {low} must be lower than upper ({h})"
                    ));
                }
            }
            let calc_bounds = |len: i64| -> Result<(usize, usize), String> {
                let mut high_v = match high_opt {
                    Some(h) if h < 0 => len + h,
                    Some(h) => h,
                    None => len,
                };
                high_v = high_v.clamp(0, len);
                let mut low_v = if low < 0 { (len + low).max(0) } else { low };
                if low_v > len && high_opt.is_none() {
                    low_v = len;
                }
                if low_v > high_v {
                    return Err(format!(
                        "lower slice bound {low_v} must be lower than or equal to upper bound ({high_v}) and target length ({len})"
                    ));
                }
                Ok((low_v as usize, high_v as usize))
            };
            match target_val {
                Value::String(s) => {
                    let (start, end) = calc_bounds(s.len() as i64)?;
                    // Bounds are bytes, as in Go; one inside a character moves back to that
                    // character's start rather than panicking the pipeline thread (T-6820).
                    let floor = |mut i: usize| {
                        while !s.is_char_boundary(i) {
                            i -= 1;
                        }
                        i
                    };
                    Ok(Value::String(s[floor(start)..floor(end)].to_string()))
                }
                Value::Bytes(b) => {
                    let (start, end) = calc_bounds(b.len() as i64)?;
                    Ok(Value::Bytes(b[start..end].to_vec()))
                }
                Value::Array(a) => {
                    let (start, end) = calc_bounds(a.len() as i64)?;
                    Ok(Value::Array(a[start..end].to_vec().into()))
                }
                other => Err(format!(
                    "expected array or string value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "contains" => Some((|| {
            let needle = ctx.eval_expr(get_first_arg(args, "value")?)?;
            match target_val {
                Value::String(s) => Ok(Value::Bool(s.contains(&needle.to_string_coerced()))),
                Value::Bytes(b) => {
                    let nb = needle.to_bytes_coerced();
                    let found = if nb.is_empty() {
                        true
                    } else {
                        b.windows(nb.len()).any(|w| w == nb.as_slice())
                    };
                    Ok(Value::Bool(found))
                }
                Value::Array(arr) => Ok(Value::Bool(arr.iter().any(|x| needle.compare_eq(x)))),
                Value::Object(map) => Ok(Value::Bool(map.values().any(|x| needle.compare_eq(x)))),
                other => Err(format!(
                    "expected string, array or object value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "exists" => Some((|| {
            let path_str = ctx.eval_expr(get_first_arg(args, "path")?)?.get_string()?;
            let segs = crate::value::gabs_path(&path_str);
            Ok(Value::Bool(target_val.exists_path(&segs)))
        })()),
        "keys" => Some(match target_val {
            Value::Object(map) => Ok(Value::Array(
                map.keys().cloned().map(Value::String).collect(),
            )),
            other => Err(format!("expected object value, got {}", other.type_name())),
        }),
        "values" => Some(match target_val {
            Value::Object(map) => Ok(Value::Array(map.values().cloned().collect())),
            other => Err(format!("expected object value, got {}", other.type_name())),
        }),
        "key_values" => Some(match target_val {
            Value::Object(map) => {
                let mut out = Vec::with_capacity(map.len());
                for (k, v) in map {
                    let mut pair = BTreeMap::new();
                    pair.insert("key".to_string(), Value::String(k.clone()));
                    pair.insert("value".to_string(), v.clone());
                    out.push(Value::Object(pair.into()));
                }
                Ok(Value::Array(out.into()))
            }
            other => Err(format!("expected object value, got {}", other.type_name())),
        }),
        "enumerated" => Some(match target_val {
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    let mut obj = BTreeMap::new();
                    obj.insert("index".to_string(), Value::Int(i as i64));
                    obj.insert("value".to_string(), item.clone());
                    out.push(Value::Object(obj.into()));
                }
                Ok(Value::Array(out.into()))
            }
            other => Err(format!("expected array value, got {}", other.type_name())),
        }),
        "append" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut out = arr.clone();
            for e in args.exprs() {
                out.push(ctx.eval_expr(e)?);
            }
            Ok(Value::Array(out))
        })()),
        "concat" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut out = arr.clone();
            for e in args.exprs() {
                let v = ctx.eval_expr(e)?;
                let Value::Array(more) = v else {
                    return Err(format!("expected array value, got {}", v.type_name()));
                };
                out.extend(more);
            }
            Ok(Value::Array(out))
        })()),
        "flatten" => Some(match target_val {
            Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for item in arr {
                    match item {
                        Value::Array(inner) => out.extend(inner.iter().cloned()),
                        other => out.push(other.clone()),
                    }
                }
                Ok(Value::Array(out.into()))
            }
            other => Err(format!("expected array value, got {}", other.type_name())),
        }),
        "join" => Some((|| {
            let delim = match get_optional_arg(args, "delimiter", 0) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => String::new(),
            };
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut parts = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let s = item.get_string().map_err(|e| {
                    format!(
                        "failed to join element {i}: {e} ({})",
                        item.to_display_short()
                    )
                })?;
                parts.push(s);
            }
            Ok(Value::String(parts.join(&delim)))
        })()),
        "split" => Some((|| {
            let delim_val = ctx.eval_expr(get_first_arg(args, "delimiter")?)?;
            match target_val {
                Value::String(s) => {
                    let d = delim_val.get_string()?;
                    let parts: Vec<Value> = go_split(s, &d)
                        .map(|p| Value::String(p.to_string()))
                        .collect();
                    Ok(Value::Array(parts.into()))
                }
                Value::Bytes(b) => {
                    let s = String::from_utf8_lossy(b);
                    let d = delim_val.get_string()?;
                    let parts: Vec<Value> = go_split(&s, &d)
                        .map(|p| Value::Bytes(p.as_bytes().to_vec()))
                        .collect();
                    Ok(Value::Array(parts.into()))
                }
                Value::Array(arr) => {
                    if arr.is_empty() {
                        return Ok(Value::Array(vec![Value::Array(Vec::new().into())].into()));
                    }
                    let mut segments = Vec::new();
                    let mut current = Vec::new();
                    for elem in arr {
                        if delim_val.compare_eq(elem) {
                            segments.push(Value::Array(std::mem::take(&mut current).into()));
                        } else {
                            current.push(elem.clone());
                        }
                    }
                    segments.push(Value::Array(current.into()));
                    Ok(Value::Array(segments.into()))
                }
                other => Err(format!(
                    "expected string or array value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "split_by" => Some((|| {
            let pred = get_first_arg(args, "predicate")?;
            match target_val {
                Value::String(s) => {
                    if s.is_empty() {
                        return Ok(Value::Array(vec![Value::String(String::new())].into()));
                    }
                    let chars: Vec<char> = s.chars().collect();
                    let mut segments = Vec::new();
                    let mut start = 0usize;
                    for (i, &c) in chars.iter().enumerate() {
                        let res = ctx.eval_with_value(Value::String(c.to_string()), pred)?;
                        if matches!(res, Value::Bool(true)) {
                            let seg: String = chars[start..i].iter().collect();
                            segments.push(Value::String(seg));
                            start = i + 1;
                        }
                    }
                    let tail: String = chars[start..].iter().collect();
                    segments.push(Value::String(tail));
                    Ok(Value::Array(segments.into()))
                }
                Value::Array(arr) => {
                    if arr.is_empty() {
                        return Ok(Value::Array(vec![Value::Array(Vec::new().into())].into()));
                    }
                    let mut segments = Vec::new();
                    let mut current = Vec::new();
                    for elem in arr {
                        let res = ctx.eval_with_value(elem.clone(), pred)?;
                        if matches!(res, Value::Bool(true)) {
                            segments.push(Value::Array(std::mem::take(&mut current).into()));
                        } else {
                            current.push(elem.clone());
                        }
                    }
                    segments.push(Value::Array(current.into()));
                    Ok(Value::Array(segments.into()))
                }
                other => Err(format!(
                    "expected string or array value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "filter" => Some((|| {
            let query = get_first_arg(args, "test")?;
            match target_val {
                Value::Array(arr) => {
                    let mut out = Vec::with_capacity(arr.len());
                    for item in arr {
                        let v = ctx.eval_with_value(item.clone(), query)?;
                        if matches!(v, Value::Bool(true)) {
                            out.push(item.clone());
                        }
                    }
                    Ok(Value::Array(out.into()))
                }
                Value::Object(map) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in map {
                        let mut item = BTreeMap::new();
                        item.insert("key".to_string(), Value::String(k.clone()));
                        item.insert("value".to_string(), v.clone());
                        let res = ctx.eval_with_value(Value::Object(item.into()), query)?;
                        if matches!(res, Value::Bool(true)) {
                            out.insert(k.clone(), v.clone());
                        }
                    }
                    Ok(Value::Object(out.into()))
                }
                other => Err(format!(
                    "expected array or object value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "map_each" => Some((|| {
            let query = get_first_arg(args, "query")?;
            match target_val {
                Value::Array(arr) => {
                    let mut out = Vec::with_capacity(arr.len());
                    for (i, item) in arr.iter().enumerate() {
                        let res = ctx
                            .eval_with_value(item.clone(), query)
                            .map_err(|e| format!("failed to process element {i}: {e}"))?;
                        match res {
                            Value::Delete => {}
                            Value::Nothing => out.push(item.clone()),
                            other => out.push(other),
                        }
                    }
                    Ok(Value::Array(out.into()))
                }
                Value::Object(map) => {
                    let mut out = BTreeMap::new();
                    for (k, v) in map {
                        let mut item = BTreeMap::new();
                        item.insert("key".to_string(), Value::String(k.clone()));
                        item.insert("value".to_string(), v.clone());
                        let res = ctx
                            .eval_with_value(Value::Object(item.into()), query)
                            .map_err(|e| format!("failed to process element {k}: {e}"))?;
                        match res {
                            Value::Delete => {}
                            Value::Nothing => {
                                out.insert(k.clone(), v.clone());
                            }
                            other => {
                                out.insert(k.clone(), other);
                            }
                        }
                    }
                    Ok(Value::Object(out.into()))
                }
                other => Err(format!(
                    "expected array or object value, got {}",
                    other.type_name()
                )),
            }
        })()),
        "map_each_key" => Some((|| {
            let query = get_first_arg(args, "query")?;
            let Value::Object(map) = target_val else {
                return Err(format!(
                    "expected object value, got {}",
                    target_val.type_name()
                ));
            };
            let mut out = BTreeMap::new();
            for (k, v) in map {
                let res = ctx.eval_with_value(Value::String(k.clone()), query)?;
                match res {
                    Value::Nothing => {
                        out.insert(k.clone(), v.clone());
                    }
                    Value::String(new_k) => {
                        out.insert(new_k, v.clone());
                    }
                    other => {
                        return Err(format!(
                            "unexpected result from key mapping: expected string value, got {}",
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(Value::Object(out.into()))
        })()),
        "fold" => Some((|| {
            let init_expr = get_arg(args, "initial", 0)?;
            let query_expr = get_arg(args, "query", 1)?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut tally = ctx.eval_expr(init_expr)?;
            for item in arr {
                let mut pair = BTreeMap::new();
                pair.insert("tally".to_string(), tally);
                pair.insert("value".to_string(), item.clone());
                tally = ctx.eval_with_value(Value::Object(pair.into()), query_expr)?;
            }
            Ok(tally)
        })()),
        "all" => Some((|| {
            let query = get_first_arg(args, "test")?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            if arr.is_empty() {
                return Ok(Value::Bool(false));
            }
            for (i, item) in arr.iter().enumerate() {
                let res = ctx
                    .eval_with_value(item.clone(), query)
                    .map_err(|e| format!("element {i}: {e}"))?;
                let b = res.get_bool().map_err(|e| format!("element {i}: {e}"))?;
                if !b {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        })()),
        "any" => Some((|| {
            let query = get_first_arg(args, "test")?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            if arr.is_empty() {
                return Ok(Value::Bool(false));
            }
            for (i, item) in arr.iter().enumerate() {
                let res = ctx
                    .eval_with_value(item.clone(), query)
                    .map_err(|e| format!("element {i}: {e}"))?;
                let b = res.get_bool().map_err(|e| format!("element {i}: {e}"))?;
                if b {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        })()),
        "merge" => Some((|| {
            let with_val = ctx.eval_expr(get_first_arg(args, "with")?)?;
            merge_values(target_val, &with_val, target_expr.annotation())
        })()),
        "assign" => Some((|| {
            let with_val = ctx.eval_expr(get_first_arg(args, "with")?)?;
            assign_values(target_val, &with_val, target_expr.annotation())
        })()),
        "squash" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut acc = Value::Object(BTreeMap::new().into());
            for item in arr {
                acc = merge_owned(acc, item, "squash".to_string())?;
            }
            Ok(acc)
        })()),
        "without" => Some((|| {
            let Value::Object(map) = target_val else {
                return Err(format!(
                    "expected object value, got {}",
                    target_val.type_name()
                ));
            };
            let mut exclude_paths: Vec<Vec<String>> = Vec::new();
            for (i, e) in args.exprs().iter().enumerate() {
                let s = ctx
                    .eval_expr(e)?
                    .get_string()
                    .map_err(|err| format!("argument {i}: {err}"))?;
                exclude_paths.push(crate::value::gabs_path(&s).to_vec());
            }
            Ok(Value::Object(map_without(map, &exclude_paths).into()))
        })()),
        "with" => Some((|| {
            let Value::Object(map) = target_val else {
                return Err(format!(
                    "expected object value, got {}",
                    target_val.type_name()
                ));
            };
            let mut include_paths: Vec<Vec<String>> = Vec::new();
            for (i, e) in args.exprs().iter().enumerate() {
                let s = ctx
                    .eval_expr(e)?
                    .get_string()
                    .map_err(|err| format!("argument {i}: {err}"))?;
                include_paths.push(crate::value::gabs_path(&s).to_vec());
            }
            Ok(Value::Object(map_with(map, &include_paths).into()))
        })()),
        "unique" => Some((|| {
            let emit_opt = get_optional_arg(args, "emit", 0);
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut seen_str = HashSet::new();
            let mut seen_num = HashSet::new();
            let mut out = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let check = match emit_opt {
                    Some(e) => ctx
                        .eval_with_value(item.clone(), e)
                        .map_err(|err| format!("index {i}: {err}"))?,
                    None => item.clone(),
                };
                let is_unique = match &check {
                    Value::String(s) => seen_str.insert(s.clone()),
                    Value::Bytes(b) => seen_str.insert(String::from_utf8_lossy(b).into_owned()),
                    Value::Int(_) | Value::Uint(_) | Value::Float(_) | Value::Number(_) => {
                        let f = check.as_f64().unwrap_or(0.0);
                        seen_num.insert(f.to_bits())
                    }
                    other => {
                        return Err(format!(
                            "index {i}: expected string or number value, got {}",
                            other.type_name()
                        ));
                    }
                };
                if is_unique {
                    out.push(item.clone());
                }
            }
            Ok(Value::Array(out.into()))
        })()),
        "sort" => Some((|| {
            let cmp_opt = get_optional_arg(args, "compare", 0);
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut out: Vec<Value> = arr.iter().cloned().collect();
            let mut err = None;
            if let Some(cmp_expr) = cmp_opt {
                crate::go_sort::sort_slice(&mut out, |l, r| {
                    if err.is_some() {
                        return false;
                    }
                    let mut pair = BTreeMap::new();
                    pair.insert("left".to_string(), l.clone());
                    pair.insert("right".to_string(), r.clone());
                    match ctx.eval_with_value(Value::Object(pair.into()), cmp_expr) {
                        Ok(Value::Bool(b)) => b,
                        Ok(v) => {
                            err = Some(format!(
                                "expected bool value, got {} from sort argument ({})",
                                v.type_name(),
                                v.to_display_short()
                            ));
                            false
                        }
                        Err(e) => {
                            err = Some(e);
                            false
                        }
                    }
                });
            } else {
                go_sort_by_key(&mut out, "sort", "", |v| Ok(v), &mut err);
            }
            if let Some(e) = err {
                return Err(e);
            }
            Ok(Value::Array(out.into()))
        })()),
        "sort_by" => Some((|| {
            let query = get_first_arg(args, "query")?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            // The query is pure, so its answer per element stands for Bento's per-comparison call.
            let mut keyed: Vec<(Result<Value, String>, Value)> = arr
                .iter()
                .map(|item| (ctx.eval_with_value(item.clone(), query), item.clone()))
                .collect();
            let mut err = None;
            let from = format!(" from {}", query.annotation());
            go_sort_by_key(&mut keyed, "sort_by", &from, |(k, _)| k.as_ref(), &mut err);
            if let Some(e) = err {
                return Err(e);
            }
            Ok(Value::Array(keyed.into_iter().map(|(_, v)| v).collect()))
        })()),
        "find" => Some((|| {
            let needle = ctx.eval_expr(get_first_arg(args, "value")?)?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let pos = arr
                .iter()
                .position(|x| needle.compare_eq(x))
                .map(|i| i as i64)
                .unwrap_or(-1);
            Ok(Value::Int(pos))
        })()),
        "find_all" => Some((|| {
            let needle = ctx.eval_expr(get_first_arg(args, "value")?)?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let indices: Vec<Value> = arr
                .iter()
                .enumerate()
                .filter_map(|(i, x)| needle.compare_eq(x).then_some(Value::Int(i as i64)))
                .collect();
            Ok(Value::Array(indices.into()))
        })()),
        "find_by" => Some((|| {
            let query = get_first_arg(args, "query")?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            for (i, item) in arr.iter().enumerate() {
                let b = ctx
                    .eval_with_value(item.clone(), query)
                    .map_err(|e| format!("query returned an error for index {i}: {e}"))?
                    .get_bool()
                    .map_err(|e| {
                        format!("query returned a non-boolean value for index {i}: {e}")
                    })?;
                if b {
                    return Ok(Value::Int(i as i64));
                }
            }
            Ok(Value::Int(-1))
        })()),
        "find_all_by" => Some((|| {
            let query = get_first_arg(args, "query")?;
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let mut out = Vec::new();
            for (i, item) in arr.iter().enumerate() {
                let b = ctx
                    .eval_with_value(item.clone(), query)
                    .map_err(|e| format!("query returned an error for index {i}: {e}"))?
                    .get_bool()
                    .map_err(|e| {
                        format!("query returned a non-boolean value for index {i}: {e}")
                    })?;
                if b {
                    out.push(Value::Int(i as i64));
                }
            }
            Ok(Value::Array(out.into()))
        })()),
        "collapse" => Some((|| {
            let include_empty = match get_optional_arg(args, "include_empty", 0) {
                Some(e) => ctx.eval_expr(e)?.get_bool()?,
                None => false,
            };
            let mut out = BTreeMap::new();
            collapse_recursive(target_val, String::new(), include_empty, &mut out);
            Ok(Value::Object(out.into()))
        })()),
        "explode" => Some((|| {
            let path_str = ctx.eval_expr(get_first_arg(args, "path")?)?.get_string()?;
            let segs = crate::value::gabs_path(&path_str);
            let Value::Object(root_map) = target_val else {
                return Err(format!(
                    "expected object value, got {}",
                    target_val.type_name()
                ));
            };
            let target = target_val.get_path(&segs);
            let base_map = map_without(root_map, &[segs.to_vec()]);
            match target {
                Value::Array(arr) => {
                    let mut out = Vec::with_capacity(arr.len());
                    for ele in arr {
                        let mut copy = Value::Object(base_map.clone().into());
                        let _ = copy.set_path(&segs, ele);
                        out.push(copy);
                    }
                    Ok(Value::Array(out.into()))
                }
                Value::Object(obj) => {
                    let mut out = BTreeMap::new();
                    for (k, ele) in obj {
                        let mut copy = Value::Object(base_map.clone().into());
                        let _ = copy.set_path(&segs, ele);
                        out.insert(k, copy);
                    }
                    Ok(Value::Object(out.into()))
                }
                other => Err(format!(
                    "expected array or object value at path '{path_str}', found: {}",
                    other.type_name()
                )),
            }
        })()),
        "zip" => Some((|| {
            let Value::Array(first) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            let exprs = args.exprs();
            if exprs.is_empty() {
                return Err("zip requires at least one argument".to_string());
            }
            let mut others = Vec::with_capacity(exprs.len());
            for e in exprs {
                let v = ctx.eval_expr(e)?;
                let Value::Array(arr) = v else {
                    return Err(format!("expected array value, got {}", v.type_name()));
                };
                if arr.len() != first.len() {
                    return Err("can't zip different length array values".to_string());
                }
                others.push(arr);
            }
            let mut out = Vec::with_capacity(first.len());
            for (i, item) in first.iter().enumerate() {
                let mut tuple = Vec::with_capacity(1 + others.len());
                tuple.push(item.clone());
                for o in &others {
                    tuple.push(o[i].clone());
                }
                out.push(Value::Array(tuple.into()));
            }
            Ok(Value::Array(out.into()))
        })()),
        _ => None,
    }
}

/// Merges `src` into `dst` following `gabs.Merge` semantics:
/// when both `dst` and `src` are objects and share a key with non-object values,
/// the values are combined into an array!
pub fn merge_values(dst: &Value, src: &Value, ann: String) -> Result<Value, String> {
    merge_owned(dst.clone(), src, ann)
}

/// [`merge_values`] over an owned `dst`: a container nobody else holds is extended in place, so a
/// `squash` over n objects stays linear.
fn merge_owned(dst: Value, src: &Value, ann: String) -> Result<Value, String> {
    match (dst, src) {
        (Value::Array(mut out), Value::Array(b)) => {
            out.extend(b.iter().cloned());
            Ok(Value::Array(out))
        }
        (Value::Array(mut out), other) => {
            out.push(other.clone());
            Ok(Value::Array(out))
        }
        (Value::Object(mut out), Value::Object(src_map)) => {
            for (k, sv) in src_map {
                match out.remove(k) {
                    None => {
                        out.insert(k.clone(), sv.clone());
                    }
                    Some(dv) => {
                        let merged = match (dv, sv) {
                            (Value::Object(d_sub), s_sub @ Value::Object(_)) => {
                                merge_owned(Value::Object(d_sub), s_sub, String::new())?
                            }
                            (Value::Array(mut d_arr), Value::Array(s_arr)) => {
                                d_arr.extend(s_arr.iter().cloned());
                                Value::Array(d_arr)
                            }
                            (Value::Array(mut d_arr), s_other) => {
                                d_arr.push(s_other.clone());
                                Value::Array(d_arr)
                            }
                            (d_other, Value::Array(s_arr)) => {
                                let mut arr = vec![d_other];
                                arr.extend(s_arr.iter().cloned());
                                Value::Array(arr.into())
                            }
                            (d_other, s_other) => {
                                Value::Array(vec![d_other, s_other.clone()].into())
                            }
                        };
                        out.insert(k.clone(), merged);
                    }
                }
            }
            Ok(Value::Object(out))
        }
        // gabs.Merge ignores a source that is not an object.
        (keep @ Value::Object(_), _) => Ok(keep),
        (other, _) => Err(crate::value::type_error_from(
            &ann,
            &other,
            &["object", "array"],
        )),
    }
}

/// Assigns `src` into `dst` following `gabs.MergeFn(..., assigner)` semantics:
/// colliding non-object keys are overwritten by `src`, while nested objects are recursively assigned!
pub fn assign_values(dst: &Value, src: &Value, ann: String) -> Result<Value, String> {
    match (dst, src) {
        (Value::Array(a), Value::Array(b)) => {
            let mut out = a.clone();
            out.extend(b.iter().cloned());
            Ok(Value::Array(out))
        }
        (Value::Array(a), other) => {
            let mut out = a.clone();
            out.push(other.clone());
            Ok(Value::Array(out))
        }
        (Value::Object(dst_map), Value::Object(src_map)) => {
            let mut out = dst_map.clone();
            for (k, sv) in src_map {
                match out.remove(k) {
                    Some(Value::Object(d_sub)) if matches!(sv, Value::Object(_)) => {
                        let merged = assign_values(&Value::Object(d_sub), sv, String::new())?;
                        out.insert(k.clone(), merged);
                    }
                    _ => {
                        out.insert(k.clone(), sv.clone());
                    }
                }
            }
            Ok(Value::Object(out))
        }
        // gabs.MergeFn ignores a source that is not an object.
        (Value::Object(_), _) => Ok(dst.clone()),
        (other, _) => Err(crate::value::type_error_from(
            &ann,
            other,
            &["object", "array"],
        )),
    }
}

pub fn map_without(
    map: &BTreeMap<String, Value>,
    paths: &[Vec<String>],
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let mut excluded = false;
        let mut nested: Vec<Vec<String>> = Vec::new();
        for p in paths {
            if p.first().is_some_and(|first| first == k) {
                if p.len() > 1 {
                    nested.push(p[1..].to_vec());
                } else {
                    excluded = true;
                }
            }
        }
        if !excluded {
            if !nested.is_empty() {
                if let Value::Object(sub_map) = v {
                    out.insert(
                        k.clone(),
                        Value::Object(map_without(sub_map, &nested).into()),
                    );
                } else {
                    out.insert(k.clone(), v.clone());
                }
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

pub fn map_with(map: &BTreeMap<String, Value>, paths: &[Vec<String>]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let mut included = false;
        let mut nested: Vec<Vec<String>> = Vec::new();
        for p in paths {
            if p.first().is_some_and(|first| first == k) {
                included = true;
                if p.len() > 1 {
                    nested.push(p[1..].to_vec());
                }
            }
        }
        if included {
            if !nested.is_empty() {
                if let Value::Object(sub_map) = v {
                    out.insert(k.clone(), Value::Object(map_with(sub_map, &nested).into()));
                } else {
                    out.insert(k.clone(), v.clone());
                }
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Bento's `sort`/`sort_by` comparison under Go's `sort.Slice`: the left key's type picks number or
/// string order, the first error ends every later comparison as `false` and is the method's error.
fn go_sort_by_key<T>(
    items: &mut [T],
    method: &str,
    from: &str,
    key: for<'a> fn(&'a T) -> Result<&'a Value, &'a String>,
    err: &mut Option<String>,
) {
    crate::go_sort::sort_slice_at(items, |i, l, j, r| {
        if err.is_some() {
            return false;
        }
        match key_less(key(l), key(r), from) {
            Ok(b) => b,
            Err((left, e)) => {
                *err = Some(match e {
                    KeyErr::Eval(e) => e,
                    KeyErr::Type(e) => {
                        format!("{method} element {}: {e}", if left { i } else { j })
                    }
                });
                false
            }
        }
    });
}

enum KeyErr {
    Eval(String),
    Type(String),
}

fn key_less(
    l: Result<&Value, &String>,
    r: Result<&Value, &String>,
    from: &str,
) -> Result<bool, (bool, KeyErr)> {
    let l = l.map_err(|e| (true, KeyErr::Eval(e.clone())))?;
    let r = r.map_err(|e| (false, KeyErr::Eval(e.clone())))?;
    let type_err = |v: &Value, want: &str| {
        KeyErr::Type(format!(
            "expected {want} value, got {}{from} ({})",
            v.type_name(),
            v.to_display_short()
        ))
    };
    match l {
        Value::Int(_) | Value::Uint(_) | Value::Float(_) | Value::Number(_) => {
            let lf = l.as_f64().unwrap_or(0.0);
            let rf = r.as_f64().ok_or_else(|| (false, type_err(r, "number")))?;
            Ok(lf < rf)
        }
        Value::String(_) | Value::Bytes(_) => {
            let rb = match r {
                Value::String(s) => s.as_bytes(),
                Value::Bytes(b) => b.as_slice(),
                _ => return Err((false, type_err(r, "string"))),
            };
            let lb = match l {
                Value::String(s) => s.as_bytes(),
                Value::Bytes(b) => b.as_slice(),
                _ => &[],
            };
            Ok(lb < rb)
        }
        _ => Err((true, type_err(l, "number or string"))),
    }
}

fn collapse_recursive(
    val: &Value,
    prefix: String,
    include_empty: bool,
    out: &mut BTreeMap<String, Value>,
) {
    match val {
        Value::Object(map) => {
            if map.is_empty() {
                if include_empty && !prefix.is_empty() {
                    out.insert(prefix, Value::Object(BTreeMap::new().into()));
                }
                return;
            }
            for (k, v) in map {
                let next_p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collapse_recursive(v, next_p, include_empty, out);
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                if include_empty && !prefix.is_empty() {
                    out.insert(prefix, Value::Array(Vec::new().into()));
                }
                return;
            }
            for (i, v) in arr.iter().enumerate() {
                let next_p = if prefix.is_empty() {
                    i.to_string()
                } else {
                    format!("{prefix}.{i}")
                };
                collapse_recursive(v, next_p, include_empty, out);
            }
        }
        other => {
            out.insert(prefix, other.clone());
        }
    }
}

/// Go's `strings.Split`: an empty separator splits after each UTF-8 character (`"ab"` -> `["a","b"]`,
/// `""` -> `[]`), where Rust's `split("")` also yields an empty first and last part.
fn go_split<'a>(s: &'a str, sep: &'a str) -> Box<dyn Iterator<Item = &'a str> + 'a> {
    if sep.is_empty() {
        Box::new(s.char_indices().map(move |(i, c)| &s[i..i + c.len_utf8()]))
    } else {
        Box::new(s.split(sep))
    }
}
