// SPDX-License-Identifier: EUPL-1.2
//! Spatial relations over `GeoJSON` geometries (T-3756), related by `geo`'s DE-9IM matrix. A
//! geometry is read as the NGSI-LD write path and `georel` read it (RFC 7946 3.1 shapes, every
//! position a finite WGS84 longitude and latitude, a ring closed with four or more positions;
//! `GeometryCollection` is no Value geometry, CIM 009 4.6.3), so a residual relation and a pushed
//! `georel` agree (`tests/geometry.rs` holds the two against each other).

use geo::{Coord, Geometry, LineString, MultiLineString, MultiPoint, MultiPolygon, Point};
use geo::{Polygon, Relate};
use serde_json::Value;

use crate::{Error, Spatial};

/// The most positions one geometry holds, a literal or a value: the bound of an NGSI-LD `geoQ`
/// geometry, since every position is an edge each relation walks.
pub const MAX_GEO_VERTICES: usize = 1024;

/// The positions a `coordinates` value holds, at whatever depth its type nests them.
fn positions(v: &Value) -> usize {
    match v.as_array() {
        Some(a) if a.first().is_some_and(Value::is_array) => {
            a.iter().map(positions).fold(0, usize::saturating_add)
        }
        Some(_) => 1,
        None => 0,
    }
}

fn position(p: &Value) -> Option<Coord<f64>> {
    let mut n = p
        .as_array()?
        .iter()
        .map(|c| c.as_f64().filter(|f| f.is_finite()));
    let (x, y) = (n.next()??, n.next()??);
    n.all(|c| c.is_some()).then_some(())?;
    ((-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y)).then_some(Coord { x, y })
}

fn each<T>(v: &Value, f: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    v.as_array()?.iter().map(f).collect()
}

fn line(v: &Value) -> Option<LineString<f64>> {
    let c = each(v, position)?;
    (c.len() >= 2).then(|| LineString::new(c))
}

fn ring(v: &Value) -> Option<LineString<f64>> {
    let a = v.as_array()?;
    (a.len() >= 4 && a.first() == a.last()).then_some(())?;
    Some(LineString::new(each(v, position)?))
}

fn polygon(v: &Value) -> Option<Polygon<f64>> {
    let mut rings = each(v, ring)?.into_iter();
    let exterior = rings.next().unwrap_or_else(|| LineString::new(Vec::new()));
    Some(Polygon::new(exterior, rings.collect()))
}

/// A `GeoJSON` geometry object as a `geo` geometry, or `None` when it is none.
fn parse(v: &Value) -> Option<Geometry<f64>> {
    let c = v.get("coordinates")?;
    Some(match v.get("type")?.as_str()? {
        "Point" => Point(position(c)?).into(),
        "MultiPoint" => MultiPoint::new(each(c, |p| position(p).map(Point))?).into(),
        "LineString" => line(c)?.into(),
        "MultiLineString" => MultiLineString::new(each(c, line)?).into(),
        "Polygon" => polygon(c)?.into(),
        "MultiPolygon" => MultiPolygon::new(each(c, polygon)?).into(),
        _ => return None,
    })
}

fn too_many(v: &Value) -> bool {
    v.get("coordinates")
        .is_some_and(|c| positions(c) > MAX_GEO_VERTICES)
}

/// A geometry literal, parsed once at compile.
pub(crate) fn literal(v: &Value) -> Result<Geometry<f64>, Error> {
    if too_many(v) {
        return Err(Error::TooComplex);
    }
    parse(v).ok_or_else(|| {
        Error::Invalid("a geometry literal that is no GeoJSON geometry in WGS84 range".into())
    })
}

/// An Entity's geometry value; `None` (unknown) when it is no `GeoJSON` geometry or holds more
/// than [`MAX_GEO_VERTICES`] positions.
pub(crate) fn value(v: &Value) -> Option<Geometry<f64>> {
    if too_many(v) {
        None
    } else {
        parse(v)
    }
}

/// Whether `a` stands in `rel` to `b`.
pub(crate) fn relate(rel: Spatial, a: &Geometry<f64>, b: &Geometry<f64>) -> bool {
    let m = a.relate(b);
    match rel {
        Spatial::Intersects => m.is_intersects(),
        Spatial::Within => m.is_within(),
        Spatial::Contains => m.is_contains(),
        // the pushed `georel=equals` also takes an identical geometry as equal, even one whose
        // ring DE-9IM leaves undefined
        Spatial::Equals => a == b || m.is_equal_topo(),
        Spatial::Disjoint => m.is_disjoint(),
        Spatial::Overlaps => m.is_overlaps(),
        Spatial::Touches => m.is_touches(),
        Spatial::Crosses => m.is_crosses(),
    }
}
