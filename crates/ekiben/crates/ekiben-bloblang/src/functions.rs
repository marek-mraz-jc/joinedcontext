use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, PoisonError};

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
        "uuid_v7",
        "var",
        "cosine_similarity",
        "fake",
        "cache_get",
        "cache_set",
        "cache_delete",
        "cache_add",
        "ngsild_urn",
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
        "content" => Ok(Value::Bytes(ctx.batch.part_bytes(ctx.index).into_owned())),
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
        "uuid_v7" => Ok(Value::String(Uuid::now_v7().to_string())),
        "pi" => Ok(Value::Float(std::f64::consts::PI)),
        "hostname" => Ok(Value::String(
            std::env::var("HOSTNAME").unwrap_or_else(|_| "ekiben".to_string()),
        )),
        // ekiben runs without a tracer, so these answer what Bento answers with tracing off.
        "tracing_id" => Ok(Value::String("0".repeat(32))),
        "flow_id" => Ok(Value::String(String::new())),
        "tracing_span" => Ok(Value::Object(Default::default())),
        "throw" => {
            let why = eval_single_string_arg(args, "why", None, ctx)?;
            Err(why)
        }
        "env" => {
            // `no_cache` changes nothing here: ekiben reads the injected variables on every call.
            let var_name = match args {
                ArgList::Positional(v) if v.len() == 2 => {
                    ctx.eval_expr(&v[1])?.get_bool()?;
                    ctx.eval_expr(&v[0])?.get_string()?
                }
                _ => eval_single_string_arg(args, "name", None, ctx)?,
            };
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
        "ksuid" => Ok(Value::String(ksuid())),
        "ulid" => eval_ulid_fn(args, ctx),
        "snowflake_id" => eval_snowflake_fn(args, ctx),
        "var" => {
            let vname = eval_single_string_arg(args, "name", None, ctx)?;
            ctx.vars
                .get(&vname)
                .cloned()
                .ok_or_else(|| format!("variable '{vname}' undefined"))
        }
        "cosine_similarity" => eval_cosine_similarity(args, ctx),
        "fake" => Ok(Value::String("fake_value".to_string())),
        "ngsild_urn" => crate::ngsild::eval_urn(args, ctx),
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
    state_id: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    // Bento: `seed` defaults to `timestamp_unix_nano()` and is resolved once per call site, the
    // generator is Go's math/rand, and the number is `Int63n(max - min + 1) + min`.
    let mut seed_expr: Option<&Expr> = None;
    let mut min_val = 0i64;
    let mut max_val = i64::MAX - 1;
    match args {
        ArgList::Positional(v) => {
            seed_expr = v.first();
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
                    "seed" => seed_expr = Some(e),
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
    if max_val == i64::MAX {
        return Err(format!(
            "max must be smaller than the max allowed for an int64 ({})",
            i64::MAX
        ));
    }
    let seeded = ctx
        .rng_states
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_key(&state_id);
    if !seeded {
        let seed = match seed_expr {
            Some(e) => ctx
                .eval_expr(e)?
                .get_int()
                .map_err(|e| format!("failed to seed random number generator: {e}"))?,
            None => clock().timestamp_nanos_opt().unwrap_or_default(),
        };
        ctx.rng_states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(state_id)
            .or_insert_with(|| crate::gorand::GoRand::new(seed));
    }
    let mut states = ctx
        .rng_states
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let rng = states
        .get_mut(&state_id)
        .ok_or("random_int: the generator was not seeded")?;
    Ok(Value::Int(rng.int63n(max_val - min_val + 1) + min_val))
}

/// Longest id `nanoid` builds; the length is allocated up front.
const NANOID_MAX_LEN: i64 = 65_536;

fn eval_nanoid_fn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let mut len = 21i64;
    let mut alphabet: Option<String> = None;
    match args {
        ArgList::Positional(v) => {
            if let Some(e) = v.first() {
                len = ctx.eval_expr(e)?.get_int()?;
            }
            if let Some(e) = v.get(1) {
                alphabet = Some(ctx.eval_expr(e)?.get_string()?);
            }
        }
        ArgList::Named(v) => {
            for (k, e) in v {
                match k.as_str() {
                    "length" => len = ctx.eval_expr(e)?.get_int()?,
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
    if !(1..=NANOID_MAX_LEN).contains(&len) {
        return Err(format!(
            "nanoid length {len} must be between 1 and {NANOID_MAX_LEN}"
        ));
    }
    let len = len as usize;
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

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// segmentio/ksuid's epoch (seconds).
const KSUID_EPOCH: i64 = 1_400_000_000;
/// bwmarrin/snowflake's default epoch (milliseconds).
const SNOWFLAKE_EPOCH_MS: i64 = 1_288_834_974_657;

/// Last ULID handed out: (milliseconds, 80-bit entropy), so ids within one ms stay sorted.
static ULID_STATE: Mutex<(u64, u128)> = Mutex::new((0, 0));
/// Last snowflake: (milliseconds since its epoch, step).
static SNOWFLAKE_STATE: Mutex<(i64, i64)> = Mutex::new((i64::MIN, 0));

/// 12 random bytes: a v4 UUID's bytes without its version and variant bits.
fn random12() -> [u8; 12] {
    let b = *Uuid::new_v4().as_bytes();
    let mut out = [0u8; 12];
    out[..6].copy_from_slice(&b[..6]);
    out[6..].copy_from_slice(&b[10..]);
    out
}

/// Bento's `ulid(encoding: "crockford"|"hex", random_source: "secure_random"|"fast_random")`;
/// both random sources draw from the OS generator here.
fn eval_ulid_fn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let mut arg = |name: &str, pos: usize, default: &str| -> Result<String, String> {
        match crate::methods::get_optional_arg(args, name, pos) {
            Some(e) => ctx.eval_expr(e)?.get_string(),
            None => Ok(default.to_string()),
        }
    };
    let encoding = arg("encoding", 0, "crockford")?;
    let source = arg("random_source", 1, "secure_random")?;
    if !matches!(source.as_str(), "secure_random" | "fast_random") {
        return Err(format!(
            "invalid ulid random source: {source}, expected secure_random or fast_random"
        ));
    }
    let v = ulid()?;
    match encoding.as_str() {
        "crockford" => Ok(Value::String(crockford(v))),
        "hex" => Ok(Value::String(format!("{v:032x}"))),
        other => Err(format!(
            "invalid ulid encoding: {other}, expected crockford or hex"
        )),
    }
}

fn crockford(v: u128) -> String {
    (0..26usize)
        .map(|i| CROCKFORD[((v >> (125 - 5 * i)) & 31) as usize] as char)
        .collect()
}

/// oklog/ulid: 48-bit milliseconds and 80 random bits, 26 Crockford base32 characters; within
/// one millisecond (or a clock that went back) the entropy grows by one, so ids keep their order.
fn ulid() -> Result<u128, String> {
    const ENTROPY_MAX: u128 = (1 << 80) - 1;
    let ms = u64::try_from(clock().timestamp_millis()).unwrap_or(0) & ((1 << 48) - 1);
    let mut st = ULID_STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if ms > st.0 {
        let entropy = random12()[..10]
            .iter()
            .fold(0u128, |acc, b| (acc << 8) | u128::from(*b));
        *st = (ms, entropy);
    } else if st.1 == ENTROPY_MAX {
        return Err("ulid: monotonic entropy exhausted within one millisecond".to_string());
    } else {
        st.1 += 1;
    }
    Ok((u128::from(st.0) << 80) | st.1)
}

/// segmentio/ksuid: 4-byte seconds since its epoch and 16 random bytes, 27 base62 characters.
fn ksuid() -> String {
    let ts = u32::try_from(clock().timestamp() - KSUID_EPOCH).unwrap_or(0);
    let mut num = [0u8; 20];
    num[..4].copy_from_slice(&ts.to_be_bytes());
    num[4..16].copy_from_slice(&random12());
    num[16..].copy_from_slice(&random12()[..4]);
    let mut out = [b'0'; 27];
    for slot in out.iter_mut().rev() {
        let mut rem = 0u32;
        for b in num.iter_mut() {
            let acc = rem * 256 + u32::from(*b);
            *b = (acc / 62) as u8;
            rem = acc % 62;
        }
        *slot = BASE62[rem as usize];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// bwmarrin/snowflake: 41-bit milliseconds since its epoch, 10-bit node, 12-bit step, as decimal.
fn eval_snowflake_fn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let node = match crate::methods::get_optional_arg(args, "node_id", 0) {
        Some(e) => ctx.eval_expr(e)?.get_int()?,
        None => 1,
    };
    if !(0..=1023).contains(&node) {
        return Err(format!("node_id {node} must be between 0 and 1023"));
    }
    let now = clock().timestamp_millis() - SNOWFLAKE_EPOCH_MS;
    let mut st = SNOWFLAKE_STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if now > st.0 {
        *st = (now, 0);
    } else {
        st.1 = (st.1 + 1) & 4095;
        if st.1 == 0 {
            // The step ran out within one millisecond: move on to the next one.
            st.0 += 1;
        }
    }
    Ok(Value::String(
        ((st.0 << 22) | (node << 12) | st.1).to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::value::Value;
    use crate::Environment;

    fn run(env: &Environment, mapping: &str) -> Value {
        env.compile_mapping(mapping)
            .expect("parses")
            .exec_on_value(Value::Null)
            .expect("runs")
    }

    fn text(v: Value) -> String {
        match v {
            Value::String(s) => s,
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn random_int_with_a_seed_gives_bentos_numbers() {
        // `bento blobl` 1.22.0 on three messages.
        let env = Environment::new();
        let m = env
            .compile_mapping("root = random_int(seed: 1)")
            .expect("parses");
        let got: Vec<Value> = (0..3)
            .map(|_| m.exec_on_value(Value::Null).expect("runs"))
            .collect();
        assert!(matches!(
            got[..],
            [
                Value::Int(5577006791947779410),
                Value::Int(8674665223082153551),
                Value::Int(6129484611666145821)
            ]
        ));
        let ranged = env
            .compile_mapping("root = random_int(1, 0, 100)")
            .expect("parses");
        assert!(matches!(
            ranged.exec_on_value(Value::Null),
            Ok(Value::Int(76))
        ));
        let unseeded = env
            .compile_mapping("root = random_int(max: 10)")
            .expect("parses");
        assert!(matches!(
            unseeded.exec_on_value(Value::Null),
            Ok(Value::Int(0..=10))
        ));
    }

    #[test]
    fn ulids_are_crockford_and_sorted() {
        let env = Environment::new();
        let ids: Vec<String> = (0..200).map(|_| text(run(&env, "root = ulid()"))).collect();
        for id in &ids {
            assert_eq!(id.len(), 26, "{id}");
            assert!(id.bytes().all(|b| super::CROCKFORD.contains(&b)), "{id}");
        }
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
        let hex = text(run(&env, r#"root = ulid("hex", "fast_random")"#));
        assert!(
            hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
            "{hex}"
        );
        assert!(env
            .compile_mapping(r#"root = ulid("base64")"#)
            .expect("parses")
            .exec_on_value(Value::Null)
            .is_err());
    }

    #[test]
    fn ksuids_are_27_base62() {
        let id = text(run(&Environment::new(), "root = ksuid()"));
        assert_eq!(id.len(), 27);
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric()), "{id}");
    }

    #[test]
    fn snowflakes_grow_and_carry_their_node() {
        let env = Environment::new();
        let ids: Vec<i64> = (0..50)
            .map(|_| {
                text(run(&env, "root = snowflake_id(5)"))
                    .parse()
                    .expect("a number")
            })
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert!(ids.iter().all(|id| (id >> 12) & 1023 == 5));
        assert!(env
            .compile_mapping("root = snowflake_id(2000)")
            .expect("parses")
            .exec_on_value(Value::Null)
            .is_err());
    }

    #[test]
    fn uuid_v7_is_version_7() {
        let id = text(run(&Environment::new(), "root = uuid_v7()"));
        assert_eq!(
            uuid::Uuid::parse_str(&id)
                .expect("a uuid")
                .get_version_num(),
            7
        );
    }

    #[test]
    fn tracing_off_answers_and_env_no_cache() {
        let env = Environment::new();
        assert_eq!(text(run(&env, "root = tracing_id()")), "0".repeat(32));
        assert!(matches!(run(&env, "root = tracing_span()"), Value::Object(m) if m.is_empty()));
        assert!(env.compile_mapping("root = timestamp_utc()").is_err());
        let env = Environment::new().with_env_vars(HashMap::from([("A".into(), "b".into())]));
        assert_eq!(text(run(&env, r#"root = env("A", true)"#)), "b");
        assert_eq!(
            text(run(&env, r#"root = env(name: "A", no_cache: true)"#)),
            "b"
        );
    }
}
