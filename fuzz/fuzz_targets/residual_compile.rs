// SPDX-License-Identifier: EUPL-1.2
//! A residual expression built from the input (ADR-0028 rule 7, T-3755): compiling answers a
//! program or a refusal, evaluating a program answers a truth or `TooComplex`, and neither
//! panics, whatever the shape, depth, width, pattern text or argument count. The builder nests
//! past `MAX_DEPTH`, so the bound is what stops it.
#![no_main]
use antares_residual::{compile, eval, Cmp, Date, DateTime, Expr, Func, Lit, Member, Path, Time};
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use serde_json::json;

const NAMES: [&str; 6] = ["name", "pm10", "tags", "when", "doc", "speed"];
const FUNCS: [Func; 37] = [
    Func::Contains, Func::StartsWith, Func::EndsWith, Func::Length, Func::IndexOf, Func::Substring,
    Func::ToLower, Func::ToUpper, Func::Trim, Func::Concat, Func::CaseI, Func::AccentI,
    Func::MatchesPattern, Func::Like, Func::Year, Func::Month, Func::Day, Func::Hour, Func::Minute,
    Func::Second, Func::FractionalSeconds, Func::TotalOffsetMinutes, Func::Date, Func::Time,
    Func::Now, Func::MinDateTime, Func::MaxDateTime, Func::Round, Func::Floor, Func::Ceiling,
    Func::Add, Func::Sub, Func::Mul, Func::Div, Func::DivBy, Func::Mod, Func::Neg,
];
const CMPS: [Cmp; 6] = [Cmp::Eq, Cmp::Ne, Cmp::Lt, Cmp::Le, Cmp::Gt, Cmp::Ge];
const MEMBERS: [Member; 5] =
    [Member::Value, Member::ObservedAt, Member::CreatedAt, Member::ModifiedAt, Member::UnitCode];

fn names(u: &mut Unstructured, most: u32) -> Result<Vec<String>> {
    (0..u.int_in_range(0..=most)?).map(|_| Ok((*u.choose(&NAMES)?).to_owned())).collect()
}

fn lit(u: &mut Unstructured) -> Result<Lit> {
    Ok(match u.int_in_range(0..=6)? {
        0 => Lit::Bool(u.arbitrary()?),
        1 => Lit::Int(u.arbitrary()?),
        2 => Lit::Num(u.arbitrary()?),
        3 => Lit::DateTime(DateTime::from_unix_micros(u.arbitrary()?).unwrap_or(DateTime::MIN)),
        4 => Date::parse(u.arbitrary()?).map_or(Lit::Bool(false), Lit::Date),
        5 => Time::parse(u.arbitrary()?).map_or(Lit::Bool(true), Lit::Time),
        _ => Lit::Str(u.arbitrary()?),
    })
}

fn path(u: &mut Unstructured) -> Result<Path> {
    Ok(match u.int_in_range(0..=4)? {
        0 => Path::Id,
        1 => Path::Type,
        2 => Path::CreatedAt,
        3 => Path::ModifiedAt,
        _ => Path::Attribute { attrs: names(u, 2)?, member: *u.choose(&MEMBERS)?, keys: names(u, 2)? },
    })
}

fn many(u: &mut Unstructured, depth: usize, most: u32) -> Result<Vec<Expr>> {
    (0..u.int_in_range(0..=most)?).map(|_| expr(u, depth + 1)).collect()
}

fn expr(u: &mut Unstructured, depth: usize) -> Result<Expr> {
    if depth > 80 || u.is_empty() {
        return Ok(Expr::Lit(lit(u)?));
    }
    let next = depth + 1;
    Ok(match u.int_in_range(0..=11)? {
        0 => Expr::Lit(lit(u)?),
        1 => Expr::Path(path(u)?),
        2 => Expr::Var { up: u.int_in_range(0..=2)?, keys: names(u, 1)? },
        3 => Expr::Cmp(*u.choose(&CMPS)?, Box::new(expr(u, next)?), Box::new(expr(u, next)?)),
        4 => Expr::And(many(u, depth, 4)?),
        5 => Expr::Or(many(u, depth, 4)?),
        6 => Expr::Not(Box::new(expr(u, next)?)),
        7 => Expr::IsNull(Box::new(expr(u, next)?)),
        8 => Expr::In(Box::new(expr(u, next)?), (0..u.int_in_range(0..=8)?).map(|_| lit(u)).collect::<Result<_>>()?),
        9 => Expr::Any(Box::new(expr(u, next)?), if u.arbitrary()? { Some(Box::new(expr(u, next)?)) } else { None }),
        10 => Expr::All(Box::new(expr(u, next)?), Box::new(expr(u, next)?)),
        _ => Expr::Call(*u.choose(&FUNCS)?, many(u, depth, 3)?),
    })
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(e) = expr(&mut u, 0) else { return };
    let entity = json!({
        "id": "urn:ngsi-ld:Station:1", "type": ["Station", "Sensor"],
        "createdAt": "2026-01-01T00:00:00Z",
        "name": {"type": "Property", "value": "Ångström", "observedAt": "2026-10-11T10:00:00Z",
                 "unitCode": "C62", "pm10": {"type": "Property", "value": 1}},
        "pm10": {"type": "Property", "value": 12},
        "tags": {"type": "ListProperty", "valueList": ["a", 1, {"name": "x"}, [2]]},
        "when": {"type": "Property", "value": {"@type": "DateTime", "@value": "2026-10-11T12:00:00+02:00"}},
        "doc": {"type": "JsonProperty", "json": {"name": {"speed": 3.5}}},
        "speed": [{"type": "Property", "value": 1e308, "datasetId": "urn:ngsi-ld:Dataset:a"},
                  {"type": "Property", "value": -9223372036854775808i64}]
    });
    if let Ok(program) = compile(&e, DateTime::from_unix_micros(0).unwrap_or(DateTime::MIN)) {
        let _ = eval(&program, &entity);
    }
});
