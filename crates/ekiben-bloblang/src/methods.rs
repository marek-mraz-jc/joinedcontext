use std::collections::HashSet;

use crate::ast::{ArgList, Expr};
use crate::executor::EvalContext;
use crate::value::Value;

pub fn known_methods() -> HashSet<String> {
    [
        // General & Coercion
        "apply",
        "array",
        "bool",
        "bytes",
        "catch",
        "from",
        "from_all",
        "get",
        "set",
        "map",
        "not",
        "not_null",
        "not_empty",
        "number",
        "or",
        "string",
        "type",
        "bloblang",
        // Numbers
        "abs",
        "ceil",
        "cos",
        "float32",
        "float64",
        "floor",
        "int8",
        "int16",
        "int32",
        "int64",
        "log",
        "log10",
        "max",
        "min",
        "pow",
        "round",
        "sin",
        "sum",
        "tan",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "parse_bytes",
        "parse_big_decimal",
        // Strings, Regex, Parsing, Encoding & Hash
        "capitalize",
        "compress",
        "decompress",
        "decode",
        "encode",
        "escape_html",
        "escape_url_query",
        "filepath_join",
        "filepath_split",
        "format",
        "format_json",
        "format_xml",
        "format_msgpack",
        "parse_msgpack",
        "json_path",
        "json_schema",
        "encrypt_aes",
        "decrypt_aes",
        "compare_argon2",
        "compare_bcrypt",
        "parse_jwt_hs256",
        "parse_jwt_hs384",
        "parse_jwt_hs512",
        "parse_jwt_rs256",
        "parse_jwt_rs384",
        "parse_jwt_rs512",
        "parse_jwt_es256",
        "parse_jwt_es384",
        "parse_jwt_es512",
        "sign_jwt_hs256",
        "sign_jwt_hs384",
        "sign_jwt_hs512",
        "sign_jwt_rs256",
        "sign_jwt_rs384",
        "sign_jwt_rs512",
        "sign_jwt_es256",
        "sign_jwt_es384",
        "sign_jwt_es512",
        "format_yaml",
        "has_prefix",
        "has_suffix",
        "hash",
        "index_of",
        "join",
        "lowercase",
        "parse_csv",
        "parse_form_url_encoded",
        "parse_json",
        "parse_url",
        "parse_xml",
        "parse_yaml",
        "quote",
        "re_find_all",
        "re_find_all_object",
        "re_find_all_submatch",
        "re_find_object",
        "re_match",
        "re_replace",
        "re_replace_all",
        "repeat",
        "replace",
        "replace_all",
        "replace_all_many",
        "replace_many",
        "reverse",
        "slug",
        "split",
        "split_by",
        "strip_html",
        "trim",
        "trim_prefix",
        "trim_suffix",
        "unescape_html",
        "unescape_url_query",
        "unquote",
        "uppercase",
        // Object & Array Manipulation
        "all",
        "any",
        "append",
        "assign",
        "collapse",
        "concat",
        "contains",
        "enumerated",
        "exists",
        "explode",
        "filter",
        "find",
        "find_all",
        "find_all_by",
        "find_by",
        "flatten",
        "fold",
        "index",
        "key_values",
        "keys",
        "length",
        "map_each",
        "map_each_key",
        "merge",
        "slice",
        "sort",
        "sort_by",
        "squash",
        "unique",
        "values",
        "with",
        "without",
        "zip",
        // Time & Duration
        "format_timestamp",
        "format_timestamp_strftime",
        "format_timestamp_unix",
        "format_timestamp_unix_micro",
        "format_timestamp_unix_milli",
        "format_timestamp_unix_nano",
        "parse_duration",
        "parse_duration_iso8601",
        "parse_timestamp",
        "parse_timestamp_strptime",
        "ts_add_iso8601",
        "ts_format",
        "ts_parse",
        "ts_round",
        "ts_strftime",
        "ts_strptime",
        "ts_sub",
        "ts_sub_iso8601",
        "ts_tz",
        "ts_unix",
        "ts_unix_micro",
        "ts_unix_milli",
        "ts_unix_nano",
        "ngsild_geoproperty",
        "ngsild_key_values",
        "ngsild_language_property",
        "ngsild_normalized",
        "ngsild_property",
        "ngsild_relationship",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Methods (methods_numbers.rs; not the int and float casts, which read a json.Number's text) that match a target or its elements on Int, Uint and Float.
const NUMBER_METHODS: &[&str] = &[
    "ceil",
    "floor",
    "round",
    "abs",
    "log",
    "log10",
    "sin",
    "cos",
    "tan",
    "pow",
    "max",
    "min",
    "sum",
    "parse_bytes",
    "parse_big_decimal",
];

/// Dispatches method evaluation across small per-family functions so stack frames remain small.
pub(crate) fn eval_method(
    target_expr: &Expr,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        // Control-flow / context-modifying methods that intercept errors or batch index before evaluating target:
        match name {
            "catch" => return eval_catch(target_expr, args, ctx),
            "or" => return eval_or(target_expr, args, ctx),
            "from" => return eval_from(target_expr, args, ctx),
            "from_all" => return eval_from_all(target_expr, args, ctx),
            "bool" => return eval_bool_method(target_expr, args, ctx),
            "number" => return eval_number_method(target_expr, args, ctx),
            _ => {}
        }

        let mut target_val = ctx.eval_expr(target_expr)?;
        if NUMBER_METHODS.contains(&name) && target_val.holds_number_text() {
            // These match on concrete number types; every other method reads a json.Number through
            // the Value accessors and keeps its text where Bento does.
            target_val = target_val.sanitized_deep();
        }
        eval_method_on_value(target_expr, target_val, name, args, ctx).map_err(|e| {
            let ann = target_expr.annotation();
            // A type error that already names its source (Bento's TypeError.From) keeps its text,
            // and Bento's closure methods (`sort`, `sort_by`) return their error unwrapped.
            if matches!(name, "sort" | "sort_by")
                || e.starts_with(&ann)
                || e.starts_with("entering ")
                || e.contains(&format!(" from {ann}"))
            {
                e
            } else if is_type_error(&e) {
                // Bento's value.TypeError names its source last: "expected X value, got Y from <ann>".
                format!("{e} from {ann}")
            } else {
                format!("{ann}: {e}")
            }
        })
    })
}

fn eval_method_on_value(
    target_expr: &Expr,
    target_val: Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    if let Some(res) = eval_general_method(target_expr, &target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) = crate::methods_numbers::eval_number_family(&target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) = crate::methods_strings::eval_string_family(&target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) =
        crate::methods_structured::eval_structured_family(target_expr, &target_val, name, args, ctx)
    {
        return res;
    }
    if let Some(res) = crate::methods_time::eval_time_family(&target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) = crate::methods_crypto::eval_crypto_family(&target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) = crate::methods_formats::eval_format_family(&target_val, name, args, ctx) {
        return res;
    }
    if let Some(res) = crate::ngsild::eval_ngsild_method(&target_val, name, args, ctx) {
        return res;
    }
    Err(format!("unrecognised method '{name}'"))
}

fn eval_catch(
    target_expr: &Expr,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let fallback_expr = get_first_arg(args, "fallback")?;
    match ctx.eval_expr(target_expr) {
        Ok(v) => Ok(v),
        Err(err) => {
            if err.starts_with("entering ") && err.contains("exceeded maximum allowed stacks") {
                return Err(err);
            }
            ctx.eval_with_value(Value::String(err), fallback_expr)
        }
    }
}

fn eval_or(
    target_expr: &Expr,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let fallback_expr = get_first_arg(args, "fallback")?;
    match ctx.eval_expr(target_expr) {
        Ok(v) if !v.is_null() => Ok(v),
        Err(err)
            if err.starts_with("entering ") && err.contains("exceeded maximum allowed stacks") =>
        {
            Err(err)
        }
        _ => ctx.eval_expr(fallback_expr),
    }
}

fn eval_from(
    target_expr: &Expr,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let idx_expr = get_first_arg(args, "index")?;
    let idx = ctx.eval_expr(idx_expr)?.get_int()? as usize;
    let prev_idx = ctx.index;
    let prev_root = ctx.cached_root_value.take();
    ctx.index = idx;
    let res = ctx.eval_expr(target_expr);
    ctx.index = prev_idx;
    ctx.cached_root_value = prev_root;
    res
}

fn eval_from_all(
    target_expr: &Expr,
    _args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let len = ctx.batch.len();
    let prev_idx = ctx.index;
    let prev_root = ctx.cached_root_value.take();
    let mut results = Vec::with_capacity(len);
    for i in 0..len {
        ctx.index = i;
        ctx.cached_root_value = None;
        let val = ctx.eval_expr(target_expr)?;
        results.push(val);
    }
    ctx.index = prev_idx;
    ctx.cached_root_value = prev_root;
    Ok(Value::Array(results.into()))
}

fn eval_bool_method(
    target_expr: &Expr,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let default_expr = get_optional_arg(args, "default", 0);
    let default_val = match default_expr {
        Some(e) => Some(ctx.eval_expr(e)?.get_bool()?),
        None => None,
    };
    match ctx.eval_expr(target_expr) {
        Ok(v) => match v.to_bool_coerced() {
            Ok(b) => Ok(Value::Bool(b)),
            Err(e) => match default_val {
                Some(d) => Ok(Value::Bool(d)),
                None => Err(format!("{}: {}", target_expr.annotation(), e)),
            },
        },
        Err(e) => match default_val {
            Some(d) => Ok(Value::Bool(d)),
            None => Err(e),
        },
    }
}

/// Text of Bento's `value.TypeError` without a source yet.
fn is_type_error(e: &str) -> bool {
    (e.starts_with("expected ") || e.starts_with("unexpected value"))
        && e.contains(", got ")
        && !e.contains(" from ")
}

fn eval_number_method(
    target_expr: &Expr,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let default_expr = get_optional_arg(args, "default", 0);
    let default_val = match default_expr {
        Some(e) => Some(ctx.eval_expr(e)?.get_number()?),
        None => None,
    };
    match ctx.eval_expr(target_expr) {
        Ok(v) => match v.to_number_coerced() {
            Ok(f) => Ok(Value::Float(f)),
            Err(e) => match default_val {
                Some(d) => Ok(Value::Float(d)),
                // Bento's IToNumber: a parse error is wrapped, a wrong type names its source.
                None if matches!(v, Value::String(_) | Value::Bytes(_)) => {
                    Err(format!("{}: {}", target_expr.annotation(), e))
                }
                None => Err(crate::value::type_error_from(
                    &target_expr.annotation(),
                    &v,
                    &["number"],
                )),
            },
        },
        Err(e) => match default_val {
            Some(d) => Ok(Value::Float(d)),
            None => Err(e),
        },
    }
}

fn eval_general_method(
    _target_expr: &Expr,
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    match name {
        "apply" => Some((|| {
            let map_expr = get_first_arg(args, "mapping")?;
            let map_name = ctx.eval_expr(map_expr)?.get_string()?;
            ctx.exec_named_map(&map_name, target_val.clone())
        })()),
        "array" => Some(Ok(match target_val {
            Value::Array(_) => target_val.clone(),
            other => Value::Array(vec![other.clone()].into()),
        })),
        "bytes" => Some(Ok(Value::Bytes(target_val.to_bytes_coerced()))),
        "string" => Some(Ok(Value::String(target_val.to_string_coerced()))),
        "type" => Some(Ok(Value::String(target_val.type_name().to_string()))),
        "not" => Some(match target_val {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(format!("expected bool value, got {}", other.type_name())),
        }),
        "not_null" => Some(if target_val.is_null() {
            Err("value is null".to_string())
        } else {
            Ok(target_val.clone())
        }),
        "not_empty" => Some(match target_val {
            Value::String(s) => {
                if s.is_empty() {
                    Err("string value is empty".to_string())
                } else {
                    Ok(target_val.clone())
                }
            }
            Value::Array(a) => {
                if a.is_empty() {
                    Err("array value is empty".to_string())
                } else {
                    Ok(target_val.clone())
                }
            }
            Value::Object(o) => {
                if o.is_empty() {
                    Err("object value is empty".to_string())
                } else {
                    Ok(target_val.clone())
                }
            }
            other => Err(format!(
                "expected string, array or object value, got {}",
                other.type_name()
            )),
        }),
        "get" => Some((|| {
            let path_expr = get_first_arg(args, "path")?;
            let path_str = ctx.eval_expr(path_expr)?.get_string()?;
            let segs = crate::value::gabs_path(&path_str);
            Ok(target_val.get_path(&segs))
        })()),
        "set" => Some((|| {
            let path_expr = get_arg(args, "path", 0)?;
            let val_expr = get_arg(args, "value", 1)?;
            let path_str = ctx.eval_expr(path_expr)?.get_string()?;
            let new_val = ctx.eval_expr(val_expr)?;
            let segs = crate::value::gabs_path(&path_str);
            let mut cloned = target_val.clone();
            if matches!(new_val, Value::Delete) {
                cloned.delete_path(&segs);
            } else {
                cloned.set_path(&segs, new_val)?;
            }
            Ok(cloned)
        })()),
        "map" => Some((|| {
            let query_expr = get_first_arg(args, "query")?;
            ctx.eval_with_value(target_val.clone(), query_expr)
        })()),
        "bloblang" => Some((|| {
            let map_expr = get_first_arg(args, "mapping")?;
            let map_str = ctx.eval_expr(map_expr)?.get_string()?;
            let pure_env = crate::env::Environment::new().without_functions(&[
                "env",
                "file",
                "file_rel",
                "file_rel_json",
                "content",
                "json",
                "meta",
                "metadata",
                "root_meta",
            ]);
            let exec = pure_env
                .compile_mapping(&map_str)
                .map_err(|e| e.to_string())?;
            exec.exec_on_value(target_val.clone())
                .map_err(|e| e.to_string())
        })()),
        _ => None,
    }
}

pub(crate) fn get_first_arg<'a>(args: &'a ArgList, name: &str) -> Result<&'a Expr, String> {
    get_arg(args, name, 0)
}

pub(crate) fn get_arg<'a>(args: &'a ArgList, name: &str, pos: usize) -> Result<&'a Expr, String> {
    get_optional_arg(args, name, pos).ok_or_else(|| format!("missing parameter: {name}"))
}

pub(crate) fn get_optional_arg<'a>(args: &'a ArgList, name: &str, pos: usize) -> Option<&'a Expr> {
    match args {
        ArgList::Positional(v) => v.get(pos),
        ArgList::Named(v) => v.iter().find(|(k, _)| k == name).map(|(_, e)| e),
    }
}
