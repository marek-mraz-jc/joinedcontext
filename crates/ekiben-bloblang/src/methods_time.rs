use chrono::{DurationRound, TimeDelta};

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::{get_first_arg, get_optional_arg};
use crate::time_format::{
    apply_timezone, format_go_layout, format_strftime, parse_go_duration_nanos, parse_go_layout,
    parse_strptime, Iso8601Period, GO_RFC3339_NANO,
};
use crate::value::Value;

pub(crate) fn eval_time_family(
    target_val: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    match name {
        "ts_parse" | "parse_timestamp" => Some((|| {
            let layout = ctx
                .eval_expr(get_first_arg(args, "format")?)?
                .get_string()?;
            let s = target_val.get_string()?;
            let dt = parse_go_layout(&s, &layout)?;
            if name == "parse_timestamp" {
                Ok(Value::String(format_go_layout(dt, GO_RFC3339_NANO)))
            } else {
                Ok(Value::Timestamp(dt))
            }
        })()),
        "ts_strptime" | "parse_timestamp_strptime" => Some((|| {
            let format = ctx
                .eval_expr(get_first_arg(args, "format")?)?
                .get_string()?;
            let s = target_val.get_string()?;
            let dt = parse_strptime(&s, &format)?;
            if name == "parse_timestamp_strptime" {
                Ok(Value::String(format_go_layout(dt, GO_RFC3339_NANO)))
            } else {
                Ok(Value::Timestamp(dt))
            }
        })()),
        "ts_format" | "format_timestamp" => Some((|| {
            let layout = match get_optional_arg(args, "format", 0) {
                Some(e) => ctx.eval_expr(e)?.get_string()?,
                None => GO_RFC3339_NANO.to_string(),
            };
            let tz_opt = match get_optional_arg(args, "tz", 1) {
                Some(e) => Some(ctx.eval_expr(e)?.get_string()?),
                None => None,
            };
            let dt = target_val.to_timestamp_coerced()?;
            let dt = apply_timezone(dt, tz_opt.as_deref())?;
            Ok(Value::String(format_go_layout(dt, &layout)))
        })()),
        "ts_strftime" | "format_timestamp_strftime" => Some((|| {
            let format = ctx
                .eval_expr(get_first_arg(args, "format")?)?
                .get_string()?;
            let tz_opt = match get_optional_arg(args, "tz", 1) {
                Some(e) => Some(ctx.eval_expr(e)?.get_string()?),
                None => None,
            };
            let dt = target_val.to_timestamp_coerced()?;
            let dt = apply_timezone(dt, tz_opt.as_deref())?;
            Ok(Value::String(format_strftime(dt, &format)))
        })()),
        "ts_tz" => Some((|| {
            let tz_str = ctx.eval_expr(get_first_arg(args, "tz")?)?.get_string()?;
            let dt = target_val.to_timestamp_coerced()?;
            let converted = apply_timezone(dt, Some(&tz_str))?;
            Ok(Value::Timestamp(converted))
        })()),
        "ts_round" => Some((|| {
            let dur_nanos = ctx.eval_expr(get_first_arg(args, "duration")?)?.get_int()?;
            let dt = target_val.to_timestamp_coerced()?;
            let delta = TimeDelta::nanoseconds(dur_nanos);
            let rounded = dt.duration_round(delta).map_err(|e| e.to_string())?;
            Ok(Value::Timestamp(rounded))
        })()),
        "ts_unix" | "format_timestamp_unix" => Some((|| {
            let dt = target_val.to_timestamp_coerced()?;
            Ok(Value::Int(dt.timestamp()))
        })()),
        "ts_unix_milli" | "format_timestamp_unix_milli" => Some((|| {
            let dt = target_val.to_timestamp_coerced()?;
            Ok(Value::Int(dt.timestamp_millis()))
        })()),
        "ts_unix_micro" | "format_timestamp_unix_micro" => Some((|| {
            let dt = target_val.to_timestamp_coerced()?;
            Ok(Value::Int(dt.timestamp_micros()))
        })()),
        "ts_unix_nano" | "format_timestamp_unix_nano" => Some((|| {
            let dt = target_val.to_timestamp_coerced()?;
            let ns = dt
                .timestamp_nanos_opt()
                .ok_or_else(|| "timestamp out of nanosecond range".to_string())?;
            Ok(Value::Int(ns))
        })()),
        "ts_sub" => Some((|| {
            let t2_val = ctx.eval_expr(get_first_arg(args, "t2")?)?;
            let t1 = target_val.to_timestamp_coerced()?;
            let t2 = t2_val.to_timestamp_coerced()?;
            let diff = (t1 - t2)
                .num_nanoseconds()
                .ok_or_else(|| "timestamp difference overflows nanoseconds".to_string())?;
            Ok(Value::Int(diff))
        })()),
        "ts_add_iso8601" => Some((|| {
            let dur_str = ctx
                .eval_expr(get_first_arg(args, "duration")?)?
                .get_string()?;
            let period = Iso8601Period::parse(&dur_str)?;
            let dt = target_val.to_timestamp_coerced()?;
            Ok(Value::Timestamp(period.add_to(dt)))
        })()),
        "ts_sub_iso8601" => Some((|| {
            let dur_str = ctx
                .eval_expr(get_first_arg(args, "duration")?)?
                .get_string()?;
            let period = Iso8601Period::parse(&dur_str)?.negate();
            let dt = target_val.to_timestamp_coerced()?;
            Ok(Value::Timestamp(period.add_to(dt)))
        })()),
        "parse_duration" => Some((|| {
            let s = target_val.get_string()?;
            parse_go_duration_nanos(&s).map(Value::Int)
        })()),
        "parse_duration_iso8601" => Some((|| {
            let s = target_val.get_string()?;
            let period = Iso8601Period::parse(&s)?;
            let nanos = period.approx_nanos();
            Ok(Value::Int(nanos))
        })()),
        _ => None,
    }
}
