// SPDX-License-Identifier: EUPL-1.2
//! `cql2-json` (OGC 21-065r2 annex C): a JSON document walked into [`Expr`], refusing every
//! member the schema does not define.

use serde_json::{Map, Value};

use crate::{
    apply, instant, interval_end, is_date, is_timestamp, Budget, Error, Expr, Geometry, Op,
    Position,
};

pub(crate) fn parse(input: &str) -> Result<Expr, Error> {
    let value: Value = serde_json::from_str(input).map_err(|_| Error::new(0, "malformed JSON"))?;
    expr(&value, &mut Budget::new())
}

fn bad(what: &'static str) -> Error {
    Error::new(0, what)
}

fn expr(value: &Value, budget: &mut Budget) -> Result<Expr, Error> {
    budget.node(0)?;
    match value {
        Value::String(s) => Ok(Expr::Str(s.clone())),
        // Without `arbitrary_precision` every JSON number is a finite f64 (serde_json refuses
        // 1e999 while parsing), so `as_f64` cannot be `None` here.
        Value::Number(n) => Ok(Expr::Num(n.as_f64().unwrap_or_default())),
        Value::Bool(b) => Ok(Expr::Bool(*b)),
        Value::Array(items) => Ok(Expr::Array(nested(items, budget)?)),
        Value::Object(members) => object(members, budget),
        Value::Null => Err(bad("null is not a CQL2 value")),
    }
}

fn nested(items: &[Value], budget: &mut Budget) -> Result<Vec<Expr>, Error> {
    budget.enter(0)?;
    let out = items
        .iter()
        .map(|v| expr(v, budget))
        .collect::<Result<Vec<_>, _>>()?;
    budget.leave();
    Ok(out)
}

/// The one member of a single-member object, by name.
fn only<'a>(members: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    (members.len() == 1).then(|| members.get(name)).flatten()
}

fn object(members: &Map<String, Value>, budget: &mut Budget) -> Result<Expr, Error> {
    if let (Some(Value::String(op)), Some(Value::Array(args)), 2) =
        (members.get("op"), members.get("args"), members.len())
    {
        let args = nested(args, budget)?;
        return apply(Op::from_name(op), args, 0);
    }
    if let Some(name) = only(members, "property") {
        return match name {
            Value::String(n) if !n.is_empty() => Ok(Expr::Property(n.clone())),
            _ => Err(bad("a property name is a non-empty string")),
        };
    }
    if let Some(t) = only(members, "timestamp") {
        return match t {
            Value::String(t) if is_timestamp(t) => Ok(Expr::Timestamp(t.clone())),
            _ => Err(bad("a UTC timestamp")),
        };
    }
    if let Some(d) = only(members, "date") {
        return match d {
            Value::String(d) if is_date(d) => Ok(Expr::Date(d.clone())),
            _ => Err(bad("a date YYYY-MM-DD")),
        };
    }
    if let Some(ends) = only(members, "interval") {
        let Some([start, end]) = ends.as_array().map(Vec::as_slice) else {
            return Err(bad("an interval of two ends"));
        };
        return Ok(Expr::Interval(
            Box::new(end_of(start, budget)?),
            Box::new(end_of(end, budget)?),
        ));
    }
    if let Some(corners) = only(members, "bbox") {
        let corners = corners
            .as_array()
            .map(|c| c.iter().map(Value::as_f64).collect::<Option<Vec<_>>>());
        return match corners.flatten() {
            Some(c) if c.len() == 4 || c.len() == 6 => Ok(Expr::BBox(c)),
            _ => Err(bad("a bbox of 4 or 6 numbers")),
        };
    }
    Ok(Expr::Geometry(geometry(members, budget)?))
}

fn end_of(value: &Value, budget: &mut Budget) -> Result<Expr, Error> {
    match value {
        Value::String(s) => instant(s, 0),
        other => interval_end(expr(other, budget)?, 0),
    }
}

/// A `GeoJSON` geometry object (RFC 7946 3.1), `type` plus `coordinates` or `geometries`.
fn geometry(members: &Map<String, Value>, budget: &mut Budget) -> Result<Geometry, Error> {
    let not_cql2 = || bad("an object that is not a CQL2 expression");
    let (Some(Value::String(kind)), 2) = (members.get("type"), members.len()) else {
        return Err(not_cql2());
    };
    if kind == "GeometryCollection" {
        let Some(Value::Array(parts)) = members.get("geometries") else {
            return Err(not_cql2());
        };
        budget.enter(0)?;
        let parts = parts
            .iter()
            .map(|p| {
                p.as_object()
                    .ok_or_else(not_cql2)
                    .and_then(|m| geometry(m, budget))
            })
            .collect::<Result<Vec<_>, _>>()?;
        budget.leave();
        return Ok(Geometry::Collection(parts));
    }
    let c = members.get("coordinates").ok_or_else(not_cql2)?;
    Ok(match kind.as_str() {
        "Point" => Geometry::Point(position(c, budget)?),
        "LineString" => Geometry::LineString(line(c, budget)?),
        "Polygon" => Geometry::Polygon(rings(c, budget)?),
        "MultiPoint" => Geometry::MultiPoint(line(c, budget)?),
        "MultiLineString" => Geometry::MultiLineString(rings(c, budget)?),
        "MultiPolygon" => Geometry::MultiPolygon(many(c, budget, rings)?),
        _ => return Err(not_cql2()),
    })
}

fn position(value: &Value, budget: &mut Budget) -> Result<Position, Error> {
    budget.node(0)?;
    let p = value
        .as_array()
        .and_then(|c| c.iter().map(Value::as_f64).collect::<Option<Vec<_>>>());
    match p {
        Some(p) if (2..=3).contains(&p.len()) => Ok(p),
        _ => Err(bad("a position of 2 or 3 numbers")),
    }
}

/// A non-empty array of `item`s.
fn many<T>(
    value: &Value,
    budget: &mut Budget,
    item: fn(&Value, &mut Budget) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let items = value
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad("a non-empty coordinate array"))?;
    budget.enter(0)?;
    let out = items
        .iter()
        .map(|v| item(v, budget))
        .collect::<Result<Vec<_>, _>>()?;
    budget.leave();
    Ok(out)
}

fn line(value: &Value, budget: &mut Budget) -> Result<Vec<Position>, Error> {
    many(value, budget, position)
}

fn rings(value: &Value, budget: &mut Budget) -> Result<Vec<Vec<Position>>, Error> {
    many(value, budget, line)
}
