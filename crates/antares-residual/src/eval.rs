// SPDX-License-Identifier: EUPL-1.2
//! The evaluator: a compiled [`Node`] over one normalized NGSI-LD Entity.
//!
//! Every value is an `Option<Val>`, `None` being unknown. Functions and comparisons return
//! unknown for an unknown or mistyped operand, so one missing Attribute never turns into a
//! default value.

use std::borrow::Cow;
use std::cmp::Ordering;

use serde_json::Value;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::{
    base64url, decode_base64, geometry, Cmp, Date, DateTime, Duration, Error, Func, Lit, Member,
    Node, Path, Shape, Time, Truth, MAX_STEPS, MAX_STRING_BYTES,
};

/// A value while evaluating; strings borrow from the Entity or the program when they can.
#[derive(Debug, Clone, PartialEq)]
enum Val<'a> {
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Cow<'a, str>),
    DateTime(DateTime),
    Date(Date),
    Time(Time),
    Duration(Duration),
    Binary(Cow<'a, [u8]>),
    /// A JSON array or object: a list for `any`/`all`, a complex or `GeoJSON` value.
    Json(&'a Value),
}

pub(crate) fn run(root: &Node, entity: &Value) -> Result<Truth, Error> {
    let mut e = Eval {
        entity,
        scope: Vec::new(),
        steps: 0,
    };
    Ok(truth(e.value(root)?.as_ref()))
}

/// The value of `root` for `entity` as JSON: a date, time or date-time as its text, `None`
/// when unknown.
pub(crate) fn run_value(root: &Node, entity: &Value) -> Result<Option<Value>, Error> {
    let mut e = Eval {
        entity,
        scope: Vec::new(),
        steps: 0,
    };
    Ok(e.value(root)?.map(|v| match v {
        Val::Bool(b) => Value::Bool(b),
        Val::Int(i) => Value::from(i),
        Val::Num(n) => Value::from(n),
        Val::Str(s) => Value::String(s.into_owned()),
        Val::DateTime(d) => Value::String(d.to_string()),
        Val::Date(d) => Value::String(d.to_string()),
        Val::Time(t) => Value::String(t.to_string()),
        Val::Duration(d) => Value::String(d.to_string()),
        Val::Binary(b) => Value::String(base64url(&b)),
        Val::Json(v) => v.clone(),
    }))
}

/// `CASEI` and `ACCENTI` of a string.
pub(crate) fn fold_text(f: Func, s: &str) -> String {
    if f == Func::CaseI {
        s.to_lowercase()
    } else {
        s.nfd().filter(|c| !is_combining_mark(*c)).collect()
    }
}

fn truth(v: Option<&Val<'_>>) -> Truth {
    match v {
        Some(Val::Bool(true)) => Truth::True,
        Some(Val::Bool(false)) => Truth::False,
        _ => Truth::Unknown,
    }
}

fn of(t: Truth) -> Option<Val<'static>> {
    match t {
        Truth::True => Some(Val::Bool(true)),
        Truth::False => Some(Val::Bool(false)),
        Truth::Unknown => None,
    }
}

struct Eval<'a> {
    entity: &'a Value,
    /// The members of the enclosing `any`/`all` lists, innermost last.
    scope: Vec<&'a Value>,
    steps: usize,
}

impl<'a> Eval<'a> {
    fn value(&mut self, node: &'a Node) -> Result<Option<Val<'a>>, Error> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_STEPS {
            return Err(Error::TooComplex);
        }
        Ok(match node {
            Node::Lit(l) => Some(literal(l)),
            Node::Path(p) => path(self.entity, p),
            Node::Var(up, keys) => self
                .scope
                .len()
                .checked_sub(up.saturating_add(1))
                .and_then(|i| self.scope.get(i))
                .and_then(|member| walk(member, keys))
                .and_then(from_json),
            Node::Cmp(op, a, b) => {
                let a = self.value(a)?;
                let b = self.value(b)?;
                of(compare(*op, a.as_ref(), b.as_ref()))
            }
            Node::And(es) => {
                let mut acc = Truth::True;
                for e in es {
                    acc = acc.and(truth(self.value(e)?.as_ref()));
                    if acc == Truth::False {
                        break;
                    }
                }
                of(acc)
            }
            Node::Or(es) => {
                let mut acc = Truth::False;
                for e in es {
                    acc = acc.or(truth(self.value(e)?.as_ref()));
                    if acc == Truth::True {
                        break;
                    }
                }
                of(acc)
            }
            Node::Not(a) => of(!truth(self.value(a)?.as_ref())),
            Node::IsNull(a) => Some(Val::Bool(self.value(a)?.is_none())),
            Node::In(a, list) => match self.value(a)? {
                Some(v) => {
                    let mut acc = Truth::False;
                    for l in list {
                        acc = acc.or(compare(Cmp::Eq, Some(&v), Some(&literal(l))));
                        if acc == Truth::True {
                            break;
                        }
                    }
                    of(acc)
                }
                None => None,
            },
            Node::Any(list, body) => match (self.value(list)?, body) {
                (Some(Val::Json(Value::Array(items))), None) => Some(Val::Bool(!items.is_empty())),
                (Some(Val::Json(Value::Array(items))), Some(body)) => {
                    of(self.lambda(items, body, Truth::False)?)
                }
                _ => None,
            },
            Node::All(list, body) => match self.value(list)? {
                Some(Val::Json(Value::Array(items))) => {
                    of(self.lambda(items, body, Truth::True)?)
                }
                _ => None,
            },
            Node::Match(a, regex) => match self.value(a)? {
                Some(Val::Str(s)) => Some(Val::Bool(regex.is_match(&s))),
                _ => None,
            },
            Node::Call(f, args) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.value(a)?);
                }
                call(*f, vals)
            }
            Node::Spatial(rel, a, b) => match (self.shape(a)?, self.shape(b)?) {
                (Some(a), Some(b)) => Some(Val::Bool(geometry::relate(*rel, &a, &b))),
                _ => None,
            },
        })
    }

    /// A spatial operand's geometry; `None` when the value is no geometry.
    fn shape(&mut self, s: &'a Shape) -> Result<Option<Cow<'a, geo::Geometry<f64>>>, Error> {
        Ok(match s {
            Shape::Fixed(g) => Some(Cow::Borrowed(g)),
            Shape::Value(n) => match self.value(n)? {
                Some(Val::Json(v)) => geometry::value(v).map(Cow::Owned),
                _ => None,
            },
        })
    }

    /// `any` (from `False`, by `or`) or `all` (from `True`, by `and`) over the list members.
    fn lambda(&mut self, items: &'a [Value], body: &'a Node, start: Truth) -> Result<Truth, Error> {
        let mut acc = start;
        for item in items {
            self.scope.push(item);
            let t = truth(self.value(body)?.as_ref());
            self.scope.pop();
            acc = if start == Truth::False {
                acc.or(t)
            } else {
                acc.and(t)
            };
            if acc != start && acc != Truth::Unknown {
                break;
            }
        }
        Ok(acc)
    }
}

fn literal(l: &Lit) -> Val<'_> {
    match l {
        Lit::Bool(b) => Val::Bool(*b),
        Lit::Int(i) => Val::Int(*i),
        Lit::Num(n) => Val::Num(*n),
        Lit::Str(s) => Val::Str(Cow::Borrowed(s)),
        Lit::DateTime(d) => Val::DateTime(*d),
        Lit::Date(d) => Val::Date(*d),
        Lit::Time(t) => Val::Time(*t),
        Lit::Duration(d) => Val::Duration(*d),
        Lit::Binary(b) => Val::Binary(Cow::Borrowed(b)),
        Lit::Geometry(g) => Val::Json(g),
    }
}

/// A JSON value as a typed value: a JSON-LD value object `{"@type": .., "@value": ..}` of a
/// date, time or date-time type is parsed, any other one read through its `@value`.
fn from_json(v: &Value) -> Option<Val<'_>> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(Val::Bool(*b)),
        Value::Number(n) => n
            .as_i64()
            .map(Val::Int)
            .or_else(|| n.as_f64().map(Val::Num)),
        Value::String(s) => Some(Val::Str(Cow::Borrowed(s))),
        Value::Object(m) => match (
            m.get("@type").and_then(Value::as_str).map(local_name),
            m.get("@value"),
        ) {
            (Some("DateTime" | "dateTime"), Some(Value::String(s))) => {
                DateTime::parse(s).map(Val::DateTime)
            }
            (Some("Date" | "date"), Some(Value::String(s))) => Date::parse(s).map(Val::Date),
            (Some("Time" | "time"), Some(Value::String(s))) => Time::parse(s).map(Val::Time),
            (_, Some(inner)) => from_json(inner),
            (_, None) => Some(Val::Json(v)),
        },
        Value::Array(_) => Some(Val::Json(v)),
    }
}

/// A type name after its last `/`, `#` or `:`: `DateTime`, `ngsi-ld:DateTime`, `xsd:dateTime` and
/// their full IRIs read alike.
fn local_name(t: &str) -> &str {
    t.rsplit(['/', '#', ':']).next().unwrap_or(t)
}

fn walk<'a>(mut v: &'a Value, keys: &[String]) -> Option<&'a Value> {
    for k in keys {
        v = v.get(k)?;
    }
    Some(v)
}

/// An Attribute at its default instance: a lone instance, or the one of several without a
/// `datasetId`.
fn instance(v: &Value) -> Option<&Value> {
    match v {
        Value::Array(items) => items.iter().find(|i| i.get("datasetId").is_none()),
        Value::Object(_) => Some(v),
        _ => None,
    }
}

fn path<'a>(entity: &'a Value, p: &Path) -> Option<Val<'a>> {
    match p {
        Path::Id => entity.get("id").and_then(from_json),
        Path::Type => entity.get("type").and_then(from_json),
        Path::CreatedAt => stamp(entity.get("createdAt")),
        Path::ModifiedAt => stamp(entity.get("modifiedAt")),
        Path::Attribute {
            attrs,
            member,
            keys,
        } => {
            let mut attr = entity;
            for name in attrs {
                attr = instance(attr.get(name)?)?;
            }
            match member {
                Member::Value => walk(attr.get(value_key(attr))?, keys).and_then(from_json),
                Member::ObservedAt => stamp(attr.get("observedAt")),
                Member::CreatedAt => stamp(attr.get("createdAt")),
                Member::ModifiedAt => stamp(attr.get("modifiedAt")),
                Member::UnitCode => attr.get("unitCode").and_then(from_json),
            }
        }
    }
}

/// The member that holds an Attribute's value, by its kind (TS 104 176 4.5).
fn value_key(attr: &Value) -> &'static str {
    match attr.get("type").and_then(Value::as_str) {
        Some("Relationship") => "object",
        Some("LanguageProperty") => "languageMap",
        Some("ListProperty") => "valueList",
        Some("ListRelationship") => "objectList",
        Some("VocabProperty") => "vocab",
        Some("JsonProperty") => "json",
        _ => "value",
    }
}

/// A date-time member (`createdAt`, `observedAt`, ...), unknown unless it is one.
fn stamp(v: Option<&Value>) -> Option<Val<'_>> {
    v.and_then(from_json)
        .as_ref()
        .and_then(as_datetime)
        .map(Val::DateTime)
}

fn as_datetime(v: &Val<'_>) -> Option<DateTime> {
    match v {
        Val::DateTime(d) => Some(*d),
        Val::Str(s) => DateTime::parse(s),
        _ => None,
    }
}

fn as_date(v: &Val<'_>) -> Option<Date> {
    match v {
        Val::Date(d) => Some(*d),
        Val::Str(s) => Date::parse(s),
        _ => None,
    }
}

/// A date, or the UTC day of a date-time: a date and a date-time compare by day, a date being
/// the whole day it names (CQL2 21-065r2 7.8 instants at day granularity).
fn day(v: &Val<'_>) -> Option<Date> {
    as_date(v).or_else(|| as_datetime(v).map(DateTime::utc_date))
}

fn as_duration(v: &Val<'_>) -> Option<Duration> {
    match v {
        Val::Duration(d) => Some(*d),
        Val::Str(s) => Duration::parse(s),
        _ => None,
    }
}

fn as_binary<'v>(v: &'v Val<'_>) -> Option<Cow<'v, [u8]>> {
    match v {
        Val::Binary(b) => Some(Cow::Borrowed(b)),
        Val::Str(s) => decode_base64(s).map(Cow::Owned),
        _ => None,
    }
}

fn as_time(v: &Val<'_>) -> Option<Time> {
    match v {
        Val::Time(t) => Some(*t),
        Val::Str(s) => Time::parse(s),
        _ => None,
    }
}

/// An `i64` as the nearest `f64` (numeric promotion), without a lossy cast: the high and low
/// 32 bits are each exact, so the sum rounds once.
fn int_to_f64(i: i64) -> f64 {
    let high = f64::from(i32::try_from(i.div_euclid(1 << 32)).unwrap_or_default());
    let low = f64::from(u32::try_from(i.rem_euclid(1 << 32)).unwrap_or_default());
    high * 4_294_967_296.0 + low
}

fn number(v: &Val<'_>) -> Option<f64> {
    match v {
        Val::Int(i) => Some(int_to_f64(*i)),
        Val::Num(n) => Some(*n),
        _ => None,
    }
}

/// How two values order; `None` when they do not compare. A string compares with a date,
/// time or date-time as the value it spells, since NGSI-LD often stores one as plain text.
fn order(a: &Val<'_>, b: &Val<'_>) -> Option<Ordering> {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => Some(x.cmp(y)),
        (Val::Int(_) | Val::Num(_), Val::Int(_) | Val::Num(_)) => {
            number(a)?.partial_cmp(&number(b)?)
        }
        (Val::Str(x), Val::Str(y)) => Some(x.cmp(y)),
        (Val::Bool(x), Val::Bool(y)) => Some(x.cmp(y)),
        (Val::DateTime(_), _) | (_, Val::DateTime(_)) => match (as_datetime(a), as_datetime(b)) {
            (Some(x), Some(y)) => Some(x.unix_micros().cmp(&y.unix_micros())),
            _ => Some(day(a)?.cmp(&day(b)?)),
        },
        (Val::Date(_), _) | (_, Val::Date(_)) => Some(day(a)?.cmp(&day(b)?)),
        (Val::Time(_), _) | (_, Val::Time(_)) => Some(as_time(a)?.cmp(&as_time(b)?)),
        (Val::Duration(_), _) | (_, Val::Duration(_)) => {
            Some(as_duration(a)?.cmp(&as_duration(b)?))
        }
        (Val::Binary(_), _) | (_, Val::Binary(_)) => Some(as_binary(a)?.cmp(&as_binary(b)?)),
        _ => None,
    }
}

fn compare(op: Cmp, a: Option<&Val<'_>>, b: Option<&Val<'_>>) -> Truth {
    let (Some(a), Some(b)) = (a, b) else {
        return Truth::Unknown;
    };
    let ordered = !matches!(op, Cmp::Eq | Cmp::Ne);
    if ordered && (matches!(a, Val::Bool(_)) || matches!(b, Val::Bool(_))) {
        return Truth::Unknown;
    }
    let Some(o) = order(a, b) else {
        return Truth::Unknown;
    };
    let holds = match op {
        Cmp::Eq => o.is_eq(),
        Cmp::Ne => o.is_ne(),
        Cmp::Lt => o.is_lt(),
        Cmp::Le => o.is_le(),
        Cmp::Gt => o.is_gt(),
        Cmp::Ge => o.is_ge(),
    };
    if holds {
        Truth::True
    } else {
        Truth::False
    }
}

fn text<'v>(v: Option<&'v Val<'_>>) -> Option<&'v str> {
    match v {
        Some(Val::Str(s)) => Some(s),
        _ => None,
    }
}

fn int(v: Option<&Val<'_>>) -> Option<i64> {
    match v {
        Some(Val::Int(i)) => Some(*i),
        _ => None,
    }
}

/// A built string, unknown past [`MAX_STRING_BYTES`].
fn owned(s: String) -> Option<Val<'static>> {
    (s.len() <= MAX_STRING_BYTES).then_some(Val::Str(Cow::Owned(s)))
}

fn count(n: usize) -> Option<Val<'static>> {
    i64::try_from(n).ok().map(Val::Int)
}

/// The local date of a date-time, or a date; a string as either.
fn date_of(v: Option<&Val<'_>>) -> Option<Date> {
    let v = v?;
    match as_datetime(v) {
        Some(dt) => dt.local().map(|(d, _)| d),
        None => as_date(v),
    }
}

/// The local time of a date-time, or a time of day; a string as either.
fn time_of(v: Option<&Val<'_>>) -> Option<Time> {
    let v = v?;
    match as_datetime(v) {
        Some(dt) => dt.local().map(|(_, t)| t),
        None => as_time(v),
    }
}

type IntOp = fn(i64, i64) -> Option<i64>;

/// Checked integer arithmetic for two integers when `int` is given, otherwise numbers; a
/// non-finite result (overflow, division by zero) is unknown.
fn arith<'a>(
    a: Option<&Val<'a>>,
    b: Option<&Val<'a>>,
    int: Option<IntOp>,
    float: fn(f64, f64) -> f64,
) -> Option<Val<'a>> {
    match (a?, b?, int) {
        (Val::Int(lhs), Val::Int(rhs), Some(op)) => op(*lhs, *rhs).map(Val::Int),
        (lhs, rhs, _) => {
            let result = float(number(lhs)?, number(rhs)?);
            result.is_finite().then_some(Val::Num(result))
        }
    }
}

fn rounded<'a>(v: Option<&Val<'a>>, f: fn(f64) -> f64) -> Option<Val<'a>> {
    match v? {
        Val::Int(i) => Some(Val::Int(*i)),
        Val::Num(n) => Some(Val::Num(f(*n))),
        _ => None,
    }
}

fn call(func: Func, args: Vec<Option<Val<'_>>>) -> Option<Val<'_>> {
    let mut it = args.into_iter();
    let first = it.next().flatten();
    let second = it.next().flatten();
    let third = it.next().flatten();
    let (one, two) = (first.as_ref(), second.as_ref());
    match func {
        Func::Contains => Some(Val::Bool(text(one)?.contains(text(two)?))),
        Func::StartsWith => Some(Val::Bool(text(one)?.starts_with(text(two)?))),
        Func::EndsWith => Some(Val::Bool(text(one)?.ends_with(text(two)?))),
        Func::Length => count(text(one)?.chars().count()),
        Func::IndexOf => {
            let haystack = text(one)?;
            match haystack.find(text(two)?) {
                Some(at) => count(haystack.get(..at)?.chars().count()),
                None => Some(Val::Int(-1)),
            }
        }
        Func::Substring => {
            let start = usize::try_from(int(two)?).ok()?;
            let take = match third {
                Some(Val::Int(n)) => usize::try_from(n).ok()?,
                Some(_) => return None,
                None => usize::MAX,
            };
            owned(text(one)?.chars().skip(start).take(take).collect())
        }
        Func::ToLower => owned(text(one)?.to_lowercase()),
        Func::ToUpper => owned(text(one)?.to_uppercase()),
        Func::Trim => owned(text(one)?.trim().to_owned()),
        Func::Concat => {
            let (head, tail) = (text(one)?, text(two)?);
            (head.len().saturating_add(tail.len()) <= MAX_STRING_BYTES)
                .then(|| Val::Str(Cow::Owned([head, tail].concat())))
        }
        Func::CaseI | Func::AccentI => owned(fold_text(func, text(one)?)),
        Func::Year => date_of(one)?.civil().map(|(y, _, _)| Val::Int(y)),
        Func::Month => date_of(one)?.civil().map(|(_, m, _)| Val::Int(m)),
        Func::Day => date_of(one)?.civil().map(|(_, _, d)| Val::Int(d)),
        Func::Hour => time_of(one)?.fields().map(|(h, _, _, _)| Val::Int(h)),
        Func::Minute => time_of(one)?.fields().map(|(_, m, _, _)| Val::Int(m)),
        Func::Second => time_of(one)?.fields().map(|(_, _, s, _)| Val::Int(s)),
        Func::FractionalSeconds => {
            let (_, _, _, micros) = time_of(one)?.fields()?;
            Some(Val::Num(
                f64::from(i32::try_from(micros).ok()?) / 1_000_000.0,
            ))
        }
        Func::TotalOffsetMinutes => Some(Val::Int(as_datetime(one?)?.offset_minutes())),
        Func::Date => as_datetime(one?)?.local().map(|(d, _)| Val::Date(d)),
        Func::Time => as_datetime(one?)?.local().map(|(_, t)| Val::Time(t)),
        Func::InRange => {
            let (value, low, high) = (number(one?)?, number(two?)?, number(third.as_ref()?)?);
            (low <= value && value <= high).then_some(first)?
        }
        Func::Round => rounded(one, f64::round),
        Func::Floor => rounded(one, f64::floor),
        Func::Ceiling => rounded(one, f64::ceil),
        Func::Add => arith(one, two, Some(i64::checked_add), |x, y| x + y),
        Func::Sub => arith(one, two, Some(i64::checked_sub), |x, y| x - y),
        Func::Mul => arith(one, two, Some(i64::checked_mul), |x, y| x * y),
        Func::Div => arith(one, two, Some(i64::checked_div), |x, y| x / y),
        Func::DivBy => arith(one, two, None, |x, y| x / y),
        Func::Mod => arith(one, two, Some(i64::checked_rem), |x, y| x % y),
        Func::Pow => match (one?, two?) {
            (Val::Int(base), Val::Int(exp)) if *exp >= 0 => u32::try_from(*exp)
                .ok()
                .and_then(|exp| base.checked_pow(exp))
                .map(Val::Int),
            // `powf` is not correctly rounded and may differ by platform (Miri
            // perturbs it on purpose); a division and `sqrt` are, so an integer
            // to a negative integer power and a square root give one answer
            // everywhere.
            (base, exp) => {
                let exact = match (base, exp) {
                    (Val::Int(b), Val::Int(e)) => u32::try_from(e.unsigned_abs())
                        .ok()
                        .and_then(|e| b.checked_pow(e))
                        .map(|power| 1.0 / int_to_f64(power)),
                    _ => None,
                };
                let (base, exp) = (number(base)?, number(exp)?);
                let result = exact.unwrap_or_else(|| {
                    if exp.to_bits() == 0.5_f64.to_bits() {
                        base.sqrt()
                    } else {
                        base.powf(exp)
                    }
                });
                result.is_finite().then_some(Val::Num(result))
            }
        },
        Func::Neg => match one? {
            Val::Int(i) => i.checked_neg().map(Val::Int),
            Val::Num(n) => Some(Val::Num(-n)),
            _ => None,
        },
        // Folded to literals or patterns by `compile`.
        Func::Now | Func::MinDateTime | Func::MaxDateTime | Func::MatchesPattern | Func::Like => {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{call, int_to_f64};
    use crate::Func;

    #[test]
    fn functions_compile_folds_are_unknown_if_ever_called() {
        for f in [
            Func::Now,
            Func::MinDateTime,
            Func::MaxDateTime,
            Func::MatchesPattern,
            Func::Like,
        ] {
            assert_eq!(call(f, vec![]), None);
        }
    }

    #[test]
    fn promotion_rounds_once() {
        assert!((int_to_f64(-1) - -1.0).abs() < f64::EPSILON);
        assert!((int_to_f64(1 << 40) - 1_099_511_627_776.0).abs() < f64::EPSILON);
        assert!((int_to_f64(i64::MAX) - 9_223_372_036_854_775_807.0).abs() < 1.0);
        assert!((int_to_f64(i64::MIN) - -9_223_372_036_854_775_808.0).abs() < 1.0);
    }
}
