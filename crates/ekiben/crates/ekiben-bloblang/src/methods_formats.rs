//! `format_msgpack`, `parse_msgpack`, `json_path` and `json_schema` of Bento 1.22.

use serde_json_path::JsonPath;

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::get_arg;
use crate::value::Value;

pub(crate) fn eval_format_family(
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    Some(match name {
        "format_msgpack" => Ok(Value::Bytes(crate::msgpack::encode(target_val))),
        "parse_msgpack" => match target_val {
            Value::Bytes(b) => crate::msgpack::decode(b),
            Value::String(s) => crate::msgpack::decode(s.as_bytes()),
            other => Err(format!("expected bytes value, got {}", other.type_name())),
        },
        "json_path" => get_arg(args, "expression", 0)
            .and_then(|e| ctx.eval_expr(e))
            .and_then(|e| e.get_string())
            .and_then(|expr| json_path(target_val, &expr)),
        "json_schema" => get_arg(args, "schema", 0)
            .and_then(|e| ctx.eval_expr(e))
            .and_then(|s| s.get_string())
            .and_then(|schema| crate::json_schema::check(target_val, &schema))
            .map(|()| target_val.clone()),
        _ => return None,
    })
}

/// PaesslerAG/jsonpath answers a plain path with the value itself and a path with a
/// wildcard, recursive descent, filter, union or slice with the list of matches.
fn json_path(target: &Value, expr: &str) -> Result<Value, String> {
    let path = JsonPath::parse(expr)
        .map_err(|e| format!("failed to evaluate json path expression: {e}"))?;
    let doc = target.to_json_value();
    let nodes = path.query(&doc);
    let several = expr.contains("..")
        || expr.contains('*')
        || expr.contains('?')
        || expr.contains(',')
        || expr.contains(':');
    if several {
        return Ok(Value::Array(
            nodes
                .all()
                .into_iter()
                .cloned()
                .map(Value::from_json_value)
                .collect(),
        ));
    }
    nodes
        .at_most_one()
        .ok()
        .flatten()
        .cloned()
        .map(Value::from_json_value)
        .ok_or_else(|| format!("unknown key {}", expr.trim_start_matches("$.")))
}
