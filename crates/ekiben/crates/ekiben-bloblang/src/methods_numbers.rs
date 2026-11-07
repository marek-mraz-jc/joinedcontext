use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::get_first_arg;
use crate::value::{check_int_bits, check_uint_bits, Value};

pub(crate) fn eval_number_family(
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    match name {
        "ceil" => Some(match target_val {
            Value::Float(f) => {
                let c = f.ceil();
                if c >= (i64::MIN as f64) && c < (i64::MAX as f64) {
                    Ok(Value::Int(c as i64))
                } else {
                    Ok(Value::Float(c))
                }
            }
            Value::Int(i) => Ok(Value::Int(*i)),
            Value::Uint(u) => Ok(Value::Uint(*u)),
            other => Err(format!("expected number value, got {}", other.type_name())),
        }),
        "floor" => Some(match target_val {
            Value::Float(f) => {
                let fl = f.floor();
                if fl >= (i64::MIN as f64) && fl < (i64::MAX as f64) {
                    Ok(Value::Int(fl as i64))
                } else {
                    Ok(Value::Float(fl))
                }
            }
            Value::Int(i) => Ok(Value::Int(*i)),
            Value::Uint(u) => Ok(Value::Uint(*u)),
            other => Err(format!("expected number value, got {}", other.type_name())),
        }),
        "round" => Some(match target_val {
            Value::Float(f) => {
                // Go math.Round: rounds half away from zero (same as f64::round in Rust)
                let r = f.round();
                if r >= (i64::MIN as f64) && r < (i64::MAX as f64) {
                    Ok(Value::Int(r as i64))
                } else {
                    Ok(Value::Float(r))
                }
            }
            Value::Int(i) => Ok(Value::Int(*i)),
            Value::Uint(u) => Ok(Value::Uint(*u)),
            other => Err(format!("expected number value, got {}", other.type_name())),
        }),
        "abs" => Some(match target_val {
            Value::Float(f) => Ok(Value::Float(f.abs())),
            Value::Int(i) => {
                if *i == i64::MIN {
                    Ok(Value::Int(i64::MAX))
                } else {
                    Ok(Value::Int(i.abs()))
                }
            }
            Value::Uint(u) => Ok(Value::Uint(*u)),
            other => Err(format!("expected number value, got {}", other.type_name())),
        }),
        "log" => Some(target_val.get_number().map(|f| Value::Float(f.ln()))),
        "log10" => Some(target_val.get_number().map(|f| Value::Float(f.log10()))),
        "sin" => Some(target_val.get_number().map(|f| Value::Float(f.sin()))),
        "cos" => Some(target_val.get_number().map(|f| Value::Float(f.cos()))),
        "tan" => Some(target_val.get_number().map(|f| Value::Float(f.tan()))),
        "pow" => Some((|| {
            let base = target_val.get_number()?;
            let exp_expr = get_first_arg(args, "exponent")?;
            let exp = ctx.eval_expr(exp_expr)?.get_number()?;
            Ok(Value::Float(base.powf(exp)))
        })()),
        "int64" => Some(target_val.to_int_coerced().map(Value::Int)),
        "int32" => Some(
            target_val
                .to_int_coerced()
                .and_then(|v| check_int_bits(v, 32))
                .map(Value::Int),
        ),
        "int16" => Some(
            target_val
                .to_int_coerced()
                .and_then(|v| check_int_bits(v, 16))
                .map(Value::Int),
        ),
        "int8" => Some(
            target_val
                .to_int_coerced()
                .and_then(|v| check_int_bits(v, 8))
                .map(Value::Int),
        ),
        "uint64" => Some(target_val.to_uint_coerced().map(Value::Uint)),
        "uint32" => Some(
            target_val
                .to_uint_coerced()
                .and_then(|v| check_uint_bits(v, 32))
                .map(Value::Uint),
        ),
        "uint16" => Some(
            target_val
                .to_uint_coerced()
                .and_then(|v| check_uint_bits(v, 16))
                .map(Value::Uint),
        ),
        "uint8" => Some(
            target_val
                .to_uint_coerced()
                .and_then(|v| check_uint_bits(v, 8))
                .map(Value::Uint),
        ),
        "float64" => Some(target_val.to_number_coerced().map(Value::Float)),
        "float32" => Some(
            target_val
                .to_number_coerced()
                // Go keeps a float32, which prints with float32's shortest digits; the f64
                // holding those digits prints the same. ponytail: arithmetic on it then sees
                // the decimal, not the exact float32 Go widens.
                .map(|f| {
                    let short = (f as f32).to_string();
                    Value::Float(short.parse().unwrap_or(f64::from(f as f32)))
                }),
        ),
        "max" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            if arr.is_empty() {
                return Err("the array was empty".to_string());
            }
            let mut max_val = f64::NEG_INFINITY;
            let mut all_int = true;
            let mut max_int = i64::MIN;
            for (i, item) in arr.iter().enumerate() {
                let f = item
                    .get_number()
                    .map_err(|e| format!("index {i} of array: {e}"))?;
                if i == 0 || f > max_val {
                    max_val = f;
                }
                if let Value::Int(iv) = item {
                    if i == 0 || *iv > max_int {
                        max_int = *iv;
                    }
                } else {
                    all_int = false;
                }
            }
            if all_int {
                Ok(Value::Int(max_int))
            } else {
                Ok(Value::Float(max_val))
            }
        })()),
        "min" => Some((|| {
            let Value::Array(arr) = target_val else {
                return Err(format!(
                    "expected array value, got {}",
                    target_val.type_name()
                ));
            };
            if arr.is_empty() {
                return Err("the array was empty".to_string());
            }
            let mut min_val = f64::INFINITY;
            let mut all_int = true;
            let mut min_int = i64::MAX;
            for (i, item) in arr.iter().enumerate() {
                let f = item
                    .get_number()
                    .map_err(|e| format!("index {i} of array: {e}"))?;
                if i == 0 || f < min_val {
                    min_val = f;
                }
                if let Value::Int(iv) = item {
                    if i == 0 || *iv < min_int {
                        min_int = *iv;
                    }
                } else {
                    all_int = false;
                }
            }
            if all_int {
                Ok(Value::Int(min_int))
            } else {
                Ok(Value::Float(min_val))
            }
        })()),
        "sum" => Some((|| match target_val {
            Value::Int(_) | Value::Uint(_) | Value::Float(_) => Ok(target_val.clone()),
            Value::Array(arr) => {
                let mut total = 0.0f64;
                let mut all_int = true;
                let mut int_total = 0i64;
                for (i, item) in arr.iter().enumerate() {
                    let n = item.get_number().map_err(|e| format!("index {i}: {e}"))?;
                    total += n;
                    if let Value::Int(iv) = item {
                        int_total = int_total.saturating_add(*iv);
                    } else {
                        all_int = false;
                    }
                }
                if all_int {
                    Ok(Value::Int(int_total))
                } else {
                    Ok(Value::Float(total))
                }
            }
            other => Err(format!("expected array value, got {}", other.type_name())),
        })()),
        "parse_bytes" => Some((|| {
            let s = target_val.get_string()?;
            parse_human_bytes(&s).map(Value::Int)
        })()),
        "parse_big_decimal" => Some((|| {
            let scale_expr = get_first_arg(args, "scale")?;
            let scale = ctx.eval_expr(scale_expr)?.get_int()?;
            if !(0..=16383).contains(&scale) {
                return Err(format!("scale must be between 0 and 16383, got {scale}"));
            }
            let bytes = match target_val {
                Value::Bytes(b) => b.clone(),
                Value::String(s) => s.as_bytes().to_vec(),
                other => return Err(format!("expected bytes value, got {}", other.type_name())),
            };
            if bytes.is_empty() {
                return Err("cannot parse empty bytes as big decimal".to_string());
            }
            if bytes.len() > 16 {
                return Err("big decimal exceeds 128-bit precision".to_string());
            }
            let mut val: i128 = if bytes[0] & 0x80 != 0 { -1 } else { 0 };
            for &b in &bytes {
                val = (val << 8) | (b as i128);
            }
            if scale == 0 {
                return Ok(Value::String(val.to_string()));
            }
            let sign = if val < 0 { "-" } else { "" };
            let abs = val.unsigned_abs();
            let factor = 10u128.pow(scale as u32);
            let whole = abs / factor;
            let frac = abs % factor;
            Ok(Value::String(format!(
                "{sign}{whole}.{frac:0>width$}",
                width = scale as usize
            )))
        })()),
        _ => None,
    }
}

fn parse_human_bytes(s: &str) -> Result<i64, String> {
    let trimmed = s.trim();
    let num_end = trimmed
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    if num_end == 0 {
        return Err(format!("failed to parse bytes '{s}'"));
    }
    let num: f64 = trimmed[..num_end]
        .parse()
        .map_err(|e| format!("failed to parse bytes '{s}': {e}"))?;
    let unit = trimmed[num_end..].trim().to_ascii_lowercase();
    let mult: f64 = match unit.as_str() {
        "" | "b" => 1.0,
        "kb" | "k" => 1_000.0,
        "kib" | "ki" => 1_024.0,
        "mb" | "m" => 1_000_000.0,
        "mib" | "mi" => 1_048_576.0,
        "gb" | "g" => 1_000_000_000.0,
        "gib" | "gi" => 1_073_741_824.0,
        "tb" | "t" => 1_000_000_000_000.0,
        "tib" | "ti" => 1_099_511_627_776.0,
        _ => return Err(format!("unrecognized byte unit in '{s}'")),
    };
    Ok((num * mult).round() as i64)
}
