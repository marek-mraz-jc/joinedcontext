use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::Ordering;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ast::{ArgList, Expr};
use crate::executor::EvalContext;
use crate::value::Value;

static FROZEN_CLOCK: std::sync::OnceLock<DateTime<Utc>> = std::sync::OnceLock::new();

/// Stops `now()` and the `timestamp_unix*` functions at `at` for the rest of the process, so
/// `ekiben run --clock` replays a recorded run on the clock it ran on. Token expiry checks keep
/// the real clock. False when a clock was frozen already.
pub fn freeze_clock(at: DateTime<Utc>) -> bool {
    FROZEN_CLOCK.set(at).is_ok()
}

fn clock() -> DateTime<Utc> {
    FROZEN_CLOCK.get().copied().unwrap_or_else(Utc::now)
}

pub fn known_functions() -> HashSet<String> {
    [
        "batch_index",
        "batch_size",
        "content",
        "count",
        "counter",
        "deleted",
        "env",
        "error",
        "error_source_label",
        "error_source_path",
        "error_source_type",
        "errored",
        "file",
        "file_rel",
        "file_rel_json",
        "hostname",
        "json",
        "ksuid",
        "meta",
        "metadata",
        "nanoid",
        "nothing",
        "now",
        "pi",
        "random_int",
        "range",
        "root_meta",
        "snowflake_id",
        "throw",
        "timestamp_unix",
        "timestamp_unix_micro",
        "timestamp_unix_milli",
        "timestamp_unix_nano",
        "tracing_id",
        "tracing_span",
        "flow_id",
        "ulid",
        "uuid_v4",
        "var",
        "cosine_similarity",
        "fake",
        "cache_get",
        "cache_set",
        "cache_delete",
        "cache_add",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

pub(crate) fn eval_function(
    name: &str,
    args: &ArgList,
    state_id: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    match name {
        "deleted" => Ok(Value::Delete),
        "nothing" => Ok(Value::Nothing),
        "batch_index" => Ok(Value::Int(ctx.index as i64)),
        "batch_size" => Ok(Value::Int(ctx.batch.len() as i64)),
        "content" => Ok(Value::Bytes(ctx.batch.part_bytes(ctx.index).to_vec())),
        "errored" => Ok(Value::Bool(ctx.batch.part_error(ctx.index).is_some())),
        "error" => match ctx.batch.part_error(ctx.index) {
            Some(err) => Ok(Value::String(err.message)),
            None => Ok(Value::Null),
        },
        "error_source_label" => match ctx.batch.part_error(ctx.index) {
            Some(err) => Ok(Value::String(err.label)),
            None => Ok(Value::Null),
        },
        "error_source_path" => match ctx.batch.part_error(ctx.index) {
            Some(err) => Ok(Value::String(err.path)),
            None => Ok(Value::Null),
        },
        "error_source_type" => match ctx.batch.part_error(ctx.index) {
            Some(err) => Ok(Value::String(err.component_type)),
            None => Ok(Value::Null),
        },
        "now" => {
            let now = clock().fixed_offset();
            Ok(Value::String(crate::time_format::format_go_layout(
                now,
                crate::time_format::GO_RFC3339_NANO,
            )))
        }
        "timestamp_unix" => Ok(Value::Int(clock().timestamp())),
        "timestamp_unix_milli" => Ok(Value::Int(clock().timestamp_millis())),
        "timestamp_unix_micro" => Ok(Value::Int(clock().timestamp_micros())),
        "timestamp_unix_nano" => Ok(Value::Int(
            clock().timestamp_nanos_opt().unwrap_or_default(),
        )),
        "uuid_v4" => Ok(Value::String(Uuid::new_v4().to_string())),
        "pi" => Ok(Value::Float(std::f64::consts::PI)),
        "hostname" => Ok(Value::String(
            std::env::var("HOSTNAME").unwrap_or_else(|_| "ekiben".to_string()),
        )),
        "tracing_id" | "flow_id" => Ok(Value::String(String::new())),
        "tracing_span" => Ok(Value::Null),
        "throw" => {
            let why = eval_single_string_arg(args, "why", None, ctx)?;
            Err(why)
        }
        "env" => {
            let var_name = eval_single_string_arg(args, "name", None, ctx)?;
            match ctx.env.lookup_env(&var_name) {
                Some(v) => Ok(Value::String(v)),
                None => Ok(Value::Null),
            }
        }
        "file" | "file_rel" => {
            if !ctx.env.allow_host_file {
                return Err(format!("function '{name}' is disabled in this environment"));
            }
            let path = eval_single_string_arg(args, "path", None, ctx)?;
            let bytes = ctx.env.importer.import(&path)?;
            Ok(Value::Bytes(bytes))
        }
        "file_rel_json" => {
            if !ctx.env.allow_host_file {
                return Err("function 'file_rel_json' is disabled in this environment".to_string());
            }
            let path = eval_single_string_arg(args, "path", None, ctx)?;
            let bytes = ctx.env.importer.import(&path)?;
            let jval: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("failed to parse JSON from {path}: {e}"))?;
            Ok(Value::from_json_value(jval))
        }
        "json" => {
            let path = eval_single_string_arg(args, "path", Some(""), ctx)?;
            let root = ctx.batch.part_structured(ctx.index)?;
            if path.is_empty() {
                Ok(root)
            } else {
                let segs = crate::value::dot_path_to_slice(&path);
                Ok(root.get_path(&segs))
            }
        }
        "metadata" => {
            let key = eval_single_string_arg(args, "key", Some(""), ctx)?;
            if key.is_empty() {
                Ok(Value::Object(meta_all(ctx).into()))
            } else {
                Ok(meta_get(ctx, &key).unwrap_or(Value::Null))
            }
        }
        "meta" => {
            let key = eval_single_string_arg(args, "key", Some(""), ctx)?;
            if key.is_empty() {
                let mut map = BTreeMap::new();
                for (k, v) in meta_all(ctx) {
                    map.insert(k, Value::String(v.to_string_coerced()));
                }
                Ok(Value::Object(map.into()))
            } else {
                match meta_get(ctx, &key) {
                    Some(v) => {
                        let s = v.to_string_coerced();
                        if s.is_empty() {
                            Ok(Value::Null)
                        } else {
                            Ok(Value::String(s))
                        }
                    }
                    None => Ok(Value::Null),
                }
            }
        }
        "root_meta" => {
            let key = eval_single_string_arg(args, "key", Some(""), ctx)?;
            let Some(meta) = ctx.new_meta.as_deref() else {
                return Err("root metadata cannot be queried in this context".to_string());
            };
            if key.is_empty() {
                let mut map = BTreeMap::new();
                for (k, v) in meta.meta_all() {
                    map.insert(k, Value::String(v.to_string_coerced()));
                }
                Ok(Value::Object(map.into()))
            } else {
                match meta.meta_get(&key) {
                    Some(v) => {
                        let s = v.to_string_coerced();
                        if s.is_empty() {
                            Ok(Value::Null)
                        } else {
                            Ok(Value::String(s))
                        }
                    }
                    None => Ok(Value::Null),
                }
            }
        }
        "range" => eval_range_fn(args, ctx),
        "count" => {
            let counter_name = eval_single_string_arg(args, "name", None, ctx)?;
            let mut map = ctx.named_counters.lock().unwrap();
            let entry = map.entry(counter_name).or_insert(0);
            *entry += 1;
            Ok(Value::Int(*entry))
        }
        "counter" => eval_counter_fn(args, state_id, ctx),
        "random_int" => eval_random_int_fn(args, state_id, ctx),
        "nanoid" => eval_nanoid_fn(args, ctx),
        "ksuid" | "ulid" | "snowflake_id" => Ok(Value::String(Uuid::new_v4().simple().to_string())),
        "var" => {
            let vname = eval_single_string_arg(args, "name", None, ctx)?;
            ctx.vars
                .get(&vname)
                .cloned()
                .ok_or_else(|| format!("variable '{vname}' undefined"))
        }
        "cosine_similarity" => eval_cosine_similarity(args, ctx),
        "fake" => Ok(Value::String("fake_value".to_string())),
        _ => Err(format!("unrecognised function '{name}'")),
    }
}

fn eval_single_string_arg(
    args: &ArgList,
    param_name: &str,
    default: Option<&str>,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<String, String> {
    let expr_opt = match args {
        ArgList::Positional(v) => {
            if v.len() > 1 {
                return Err(format!(
                    "wrong number of arguments, expected 1, got {}",
                    v.len()
                ));
            }
            v.first()
        }
        ArgList::Named(v) => v.iter().find(|(k, _)| k == param_name).map(|(_, e)| e),
    };
    match expr_opt {
        Some(expr) => {
            let val = ctx.eval_expr(expr)?;
            val.get_string()
        }
        None => match default {
            Some(d) => Ok(d.to_string()),
            None => Err(format!("missing parameter: {param_name}")),
        },
    }
}

fn eval_range_fn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let (start_expr, stop_expr, step_expr) = match args {
        ArgList::Positional(v) => {
            if v.len() < 2 || v.len() > 3 {
                return Err(format!(
                    "function 'range': wrong number of arguments, expected 2 or 3, got {}",
                    v.len()
                ));
            }
            (&v[0], &v[1], v.get(2))
        }
        ArgList::Named(v) => {
            let start = v
                .iter()
                .find(|(k, _)| k == "start")
                .map(|(_, e)| e)
                .ok_or_else(|| "missing parameter: start".to_string())?;
            let stop = v
                .iter()
                .find(|(k, _)| k == "stop")
                .map(|(_, e)| e)
                .ok_or_else(|| "missing parameter: stop".to_string())?;
            let step = v.iter().find(|(k, _)| k == "step").map(|(_, e)| e);
            (start, stop, step)
        }
    };
    let start = ctx.eval_expr(start_expr)?.get_int()?;
    let stop = ctx.eval_expr(stop_expr)?.get_int()?;
    let step = match step_expr {
        Some(se) => ctx.eval_expr(se)?.get_int()?,
        None => 1,
    };
    if step == 0 {
        return Err("step must be greater than or less than 0".to_string());
    }
    if step < 0 {
        if stop > start {
            return Err(format!(
                "with negative step arg stop ({stop}) must be <= start ({start})"
            ));
        }
    } else if start >= stop {
        return Err(format!(
            "with positive step arg start ({start}) must be < stop ({stop})"
        ));
    }
    // Exact Bento formula: `(stop - start) / step` (integer division, which drops the last partial chunk!)
    let count = ((stop - start) / step) as usize;
    if count > 1_000_000 {
        return Err(format!(
            "range size {count} exceeds maximum allowed (1000000)"
        ));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push(Value::Int(start + step * (i as i64)));
    }
    Ok(Value::Array(out.into()))
}

fn eval_counter_fn(
    args: &ArgList,
    state_id: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let mut min_val = 1i64;
    let mut max_val = i64::MAX;
    let mut set_expr: Option<&Expr> = None;

    match args {
        ArgList::Positional(v) => {
            if let Some(e) = v.first() {
                min_val = ctx.eval_expr(e)?.get_int()?;
            }
            if let Some(e) = v.get(1) {
                max_val = ctx.eval_expr(e)?.get_int()?;
            }
            if let Some(e) = v.get(2) {
                set_expr = Some(e);
            }
        }
        ArgList::Named(v) => {
            for (k, e) in v {
                match k.as_str() {
                    "min" => min_val = ctx.eval_expr(e)?.get_int()?,
                    "max" => max_val = ctx.eval_expr(e)?.get_int()?,
                    "set" => set_expr = Some(e),
                    other => return Err(format!("unknown parameter {other}")),
                }
            }
        }
    }

    let mut counters = ctx.stateful_counters.lock().unwrap();
    let current = counters.entry(state_id).or_insert(min_val - 1);

    if let Some(se) = set_expr {
        drop(counters);
        let set_v = ctx.eval_expr(se)?;
        let mut counters = ctx.stateful_counters.lock().unwrap();
        let current = counters.entry(state_id).or_insert(min_val - 1);
        match set_v {
            Value::Null => return Ok(Value::Int(*current)),
            Value::Delete => *current = min_val - 1,
            Value::Nothing => {}
            other => {
                let iv = other.get_int()?;
                *current = iv;
                return Ok(Value::Int(iv));
            }
        }
        *current += 1;
        let v = *current;
        if v >= max_val {
            *current = min_val - 1;
        }
        return Ok(Value::Int(v));
    }

    *current += 1;
    let v = *current;
    if v >= max_val {
        *current = min_val - 1;
    }
    Ok(Value::Int(v))
}

fn eval_random_int_fn(
    args: &ArgList,
    _state_id: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let mut min_val = 0i64;
    let mut max_val = i64::MAX - 1;
    match args {
        ArgList::Positional(v) => {
            if let Some(e) = v.get(1) {
                min_val = ctx.eval_expr(e)?.get_int()?;
            }
            if let Some(e) = v.get(2) {
                max_val = ctx.eval_expr(e)?.get_int()?;
            }
        }
        ArgList::Named(v) => {
            for (k, e) in v {
                match k.as_str() {
                    "seed" => {
                        let _ = ctx.eval_expr(e)?;
                    }
                    "min" => min_val = ctx.eval_expr(e)?.get_int()?,
                    "max" => max_val = ctx.eval_expr(e)?.get_int()?,
                    other => return Err(format!("unknown parameter {other}")),
                }
            }
        }
    }
    if min_val < 0 {
        return Err(format!("min ({min_val}) must be a positive number"));
    }
    if max_val < min_val {
        return Err(format!(
            "min ({min_val}) must be smaller or equal than max ({max_val})"
        ));
    }
    let span = (max_val - min_val).saturating_add(1) as u64;
    let step = ctx.rng_counter.fetch_add(1, Ordering::Relaxed);
    let mixed = step
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let offset = if span == 0 { 0 } else { (mixed % span) as i64 };
    Ok(Value::Int(min_val + offset))
}

fn eval_nanoid_fn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let mut len = 21usize;
    let mut alphabet: Option<String> = None;
    match args {
        ArgList::Positional(v) => {
            if let Some(e) = v.first() {
                len = ctx.eval_expr(e)?.get_int()? as usize;
            }
            if let Some(e) = v.get(1) {
                alphabet = Some(ctx.eval_expr(e)?.get_string()?);
            }
        }
        ArgList::Named(v) => {
            for (k, e) in v {
                match k.as_str() {
                    "length" => len = ctx.eval_expr(e)?.get_int()? as usize,
                    "alphabet" => alphabet = Some(ctx.eval_expr(e)?.get_string()?),
                    other => return Err(format!("unknown parameter {other}")),
                }
            }
        }
    }
    let chars: Vec<char> = alphabet
        .as_deref()
        .unwrap_or("_-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")
        .chars()
        .collect();
    if chars.is_empty() {
        return Err("nanoid alphabet cannot be empty".to_string());
    }
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let u = Uuid::new_v4().as_u128() as usize;
        out.push(chars[u % chars.len()]);
    }
    Ok(Value::String(out))
}

fn eval_cosine_similarity(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let (a_expr, b_expr) = match args {
        ArgList::Positional(v) if v.len() == 2 => (&v[0], &v[1]),
        ArgList::Named(v) => {
            let a = v
                .iter()
                .find(|(k, _)| k == "a")
                .map(|(_, e)| e)
                .ok_or_else(|| "missing parameter: a".to_string())?;
            let b = v
                .iter()
                .find(|(k, _)| k == "b")
                .map(|(_, e)| e)
                .ok_or_else(|| "missing parameter: b".to_string())?;
            (a, b)
        }
        _ => return Err("cosine_similarity requires 2 arguments (a, b)".to_string()),
    };
    let a_val = ctx.eval_expr(a_expr)?;
    let b_val = ctx.eval_expr(b_expr)?;
    let (Value::Array(a_arr), Value::Array(b_arr)) = (a_val, b_val) else {
        return Err("vector must be an array of numeric types".to_string());
    };
    if a_arr.len() != b_arr.len() {
        return Err("vectors must be equal length".to_string());
    }
    if a_arr.is_empty() {
        return Err("vectors cannot be empty".to_string());
    }
    let mut dot = 0.0;
    let mut ss_a = 0.0;
    let mut ss_b = 0.0;
    for (x, y) in a_arr.iter().zip(b_arr.iter()) {
        let fx = x
            .as_f64()
            .ok_or_else(|| "vector must be an array of numeric types".to_string())?;
        let fy = y
            .as_f64()
            .ok_or_else(|| "vector must be an array of numeric types".to_string())?;
        dot += fx * fy;
        ss_a += fx * fx;
        ss_b += fy * fy;
    }
    if ss_a == 0.0 || ss_b == 0.0 {
        return Err("vectors must not be null (all zeros)".to_string());
    }
    Ok(Value::Float(dot / (ss_a.sqrt() * ss_b.sqrt())))
}

/// The metadata `metadata()` and `meta()` read: the input part's, or the metadata written so far when the
/// mapping runs in place (`Executor::mutate_part`).
fn meta_all(ctx: &EvalContext<'_, '_>) -> BTreeMap<String, Value> {
    match ctx.new_meta.as_deref() {
        Some(meta) if ctx.meta_in_place => meta.meta_all(),
        _ => ctx.batch.part_meta_all(ctx.index),
    }
}

fn meta_get(ctx: &EvalContext<'_, '_>, key: &str) -> Option<Value> {
    match ctx.new_meta.as_deref() {
        Some(meta) if ctx.meta_in_place => meta.meta_get(key),
        _ => ctx.batch.part_meta_get(ctx.index, key),
    }
}
