//! GML 2 and GML 3 geometries (ISO 19136) as GeoJSON geometries (RFC 7946): the `parse_gml`
//! method, for WFS answers. An ekiben extension; Bento has no counterpart.
//!
//! The target is the GML text of one geometry element, or the object `parse_xml()` makes of
//! it (`{"Polygon": {...}}`). The text keeps the element order a `gml:Curve` with mixed
//! segments needs; `parse_xml` groups siblings by name, so such a curve is refused there.
//!
//! Axis order: an srsName in URN or URL form (`urn:ogc:def:crs:EPSG::4258`,
//! `urn:x-ogc:def:crs:EPSG:3879`, `http://www.opengis.net/def/crs/EPSG/0/4326`) follows the
//! EPSG axis order, which is north first for [`NORTH_FIRST`]; those positions are swapped to
//! GeoJSON's x/y (longitude/latitude, easting/northing). The short `EPSG:4326` form of WFS 1.0
//! and GeoServer's `http://www.opengis.net/gml/srs/epsg.xml#4326` are x/y already.
//!
//! Pure: no I/O. DTDs and entity declarations are refused, only XML's predefined and
//! character references are read, element nesting is bounded by [`MAX_DEPTH`] and
//! MultiGeometry nesting by [`MAX_GEO_DEPTH`]; NaN, infinities and malformed GML are errors,
//! never a panic.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use quick_xml::events::Event;

use crate::geo::{epsg_code, geo_transform, is_known_crs};
use crate::ngsild::MAX_GEO_DEPTH;
use crate::value::Value;

/// The deepest element nesting a GML geometry may have.
pub const MAX_DEPTH: usize = 32;

/// EPSG codes whose EPSG axis order puts north (latitude) first.
pub const NORTH_FIRST: &[u32] = &[4326, 4258, 3879];

/// Elements a geometry may carry beside its positions and members (GML 3.2 7.2.2).
const METADATA: &[&str] = &[
    "name",
    "description",
    "descriptionReference",
    "identifier",
    "metaDataProperty",
];

/// The largest angle one chord of a densified arc spans: 5 degrees.
const ARC_STEP: f64 = PI / 36.0;

/// One element: its local name, attributes by local name, text and children in order.
#[derive(Default)]
struct El {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    kids: Vec<El>,
}

impl El {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The children that are no metadata.
    fn parts(&self) -> impl Iterator<Item = &El> {
        self.kids
            .iter()
            .filter(|k| !METADATA.contains(&k.name.as_str()))
    }

    /// The one geometry inside a property element (`gml:exterior`, `gml:pointMember`).
    fn only(&self) -> Result<&El, String> {
        let mut parts = self.parts();
        match (parts.next(), parts.next()) {
            (Some(k), None) => Ok(k),
            (None, _) => Err(format!("gml:{} holds no geometry", self.name)),
            _ => Err(format!("gml:{} holds more than one geometry", self.name)),
        }
    }

    fn unexpected(&self, kid: &El) -> String {
        format!("unexpected gml:{} in gml:{}", kid.name, self.name)
    }
}

fn local(qname: &str) -> String {
    qname.rsplit(':').next().unwrap_or(qname).to_string()
}

fn xml_err(e: impl std::fmt::Display) -> String {
    format!("failed to parse GML: {e}")
}

/// The one element of a GML text.
fn from_text(text: &str) -> Result<El, String> {
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") {
        return Err(xml_err("DTD and entity declarations are forbidden"));
    }
    let mut reader = quick_xml::Reader::from_str(text);
    let mut stack: Vec<El> = Vec::new();
    let mut root = None;
    loop {
        let event = reader.read_event().map_err(xml_err)?;
        let closed = match event {
            Event::Start(e) | Event::Empty(e) if root.is_some() => {
                return Err(xml_err(format!(
                    "unexpected <{}> after the geometry",
                    AsRef::<str>::as_ref(&e)
                )))
            }
            Event::Start(ref e) | Event::Empty(ref e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(xml_err(format!("elements nest deeper than {MAX_DEPTH}")));
                }
                let mut el = El {
                    name: local(e.name().as_ref()),
                    ..El::default()
                };
                for attr in e.attributes() {
                    let attr = attr.map_err(xml_err)?;
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(xml_err)?;
                    el.attrs
                        .push((local(attr.key.as_ref()), value.into_owned()));
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(el);
                    None
                } else {
                    Some(el)
                }
            }
            Event::Text(e) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(e.as_ref());
                }
                None
            }
            Event::CData(e) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(e.as_ref());
                }
                None
            }
            Event::GeneralRef(r) => {
                let ch = r.resolve_char_ref().map_err(xml_err)?;
                let name: &str = r.as_ref();
                let s = match ch {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_predefined_entity(name)
                        .ok_or_else(|| xml_err(format!("unknown entity &{name};")))?
                        .to_string(),
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&s);
                }
                None
            }
            Event::End(_) => stack.pop(),
            Event::DocType(_) => return Err(xml_err("DOCTYPE is forbidden")),
            Event::Eof => break,
            _ => None,
        };
        if let Some(el) = closed {
            match stack.last_mut() {
                Some(parent) => parent.kids.push(el),
                None => root = Some(el),
            }
        }
    }
    root.ok_or_else(|| xml_err("unclosed or empty document"))
}

/// The element `parse_xml()` made of a geometry: `-name` attributes, `#text`, children by name.
fn from_value(name: &str, v: &Value, depth: usize) -> Result<El, String> {
    if depth >= MAX_DEPTH {
        return Err(xml_err(format!("elements nest deeper than {MAX_DEPTH}")));
    }
    let scalar = |v: &Value| match v {
        Value::String(s) | Value::Number(s) => Ok(s.clone()),
        Value::Float(_) | Value::Int(_) | Value::Uint(_) | Value::Bool(_) => Ok(v.to_string()),
        other => Err(format!(
            "parse_gml: gml:{name} holds {}, not text",
            other.type_name()
        )),
    };
    let mut el = El {
        name: name.to_string(),
        ..El::default()
    };
    let Value::Object(m) = v else {
        el.text = scalar(v)?;
        return Ok(el);
    };
    for (k, v) in m.iter() {
        if let Some(a) = k.strip_prefix('-') {
            el.attrs.push((a.to_string(), scalar(v)?));
        } else if k == "#text" {
            el.text = scalar(v)?;
        } else if let Value::Array(items) = v {
            for item in items.iter() {
                el.kids.push(from_value(k, item, depth + 1)?);
            }
        } else {
            el.kids.push(from_value(k, v, depth + 1)?);
        }
    }
    if el.name == "segments" && el.kids.windows(2).any(|w| w[0].name != w[1].name) {
        return Err("parse_gml: parse_xml lost the order of this gml:Curve's segments; pass the GML text instead".to_string());
    }
    Ok(el)
}

/// The geometry element of a `parse_gml` target.
fn root(target: &Value) -> Result<El, String> {
    match target {
        Value::String(s) => from_text(s),
        Value::Bytes(b) => from_text(std::str::from_utf8(b).map_err(xml_err)?),
        Value::Object(m) => {
            let mut els = m.iter().filter(|(k, _)| !k.starts_with(['-', '#']));
            match (els.next(), els.next()) {
                (Some((k, v)), None) => from_value(k, v, 0),
                _ => Err("parse_gml wants the GML text or the object parse_xml() makes of one geometry element, such as {\"Point\": {...}}".to_string()),
            }
        }
        other => Err(format!(
            "parse_gml wants the GML text of a geometry, got {}",
            other.type_name()
        )),
    }
}

/// How positions read: numbers per position and whether the first two swap.
#[derive(Clone, Copy)]
struct Axes {
    dim: Option<usize>,
    swap: bool,
}

struct Conv {
    /// The geometry's srsName as `EPSG:n`, once one is read.
    crs: Option<String>,
}

impl Conv {
    fn axes(&mut self, el: &El, mut ax: Axes) -> Result<Axes, String> {
        if let Some(d) = el.attr("srsDimension") {
            ax.dim = Some(dimension(d)?);
        }
        if let Some(srs) = el.attr("srsName") {
            let (code, swap) = srs_axes(srs)?;
            let crs = format!("EPSG:{code}");
            match &self.crs {
                Some(c) if *c != crs => {
                    return Err(format!(
                        "parse_gml: gml:{} names srsName {srs:?}, the geometry {c}",
                        el.name
                    ))
                }
                _ => self.crs = Some(crs),
            }
            ax.swap = swap;
        }
        Ok(ax)
    }
}

fn dimension(d: &str) -> Result<usize, String> {
    match d.trim() {
        "2" => Ok(2),
        "3" => Ok(3),
        other => Err(format!("parse_gml: srsDimension {other:?} is not 2 or 3")),
    }
}

/// The EPSG code an srsName names and whether its positions are north first.
fn srs_axes(srs: &str) -> Result<(u32, bool), String> {
    let short =
        srs.starts_with("EPSG:") || srs.starts_with("http://www.opengis.net/gml/srs/epsg.xml#");
    let code =
        epsg_code(srs).ok_or_else(|| format!("parse_gml: srsName {srs:?} names no EPSG CRS"))?;
    if short || srs.contains("CRS84") {
        return Ok((code, false));
    }
    if !is_known_crs(code) {
        return Err(format!(
            "parse_gml: srsName {srs:?}: the axis order of EPSG:{code} is not known; ask the WFS for srsName=EPSG:{code} (x/y order)"
        ));
    }
    Ok((code, NORTH_FIRST.contains(&code)))
}

fn number(t: &str) -> Result<f64, String> {
    t.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .ok_or_else(|| format!("parse_gml: {t:?} is not a finite number"))
}

type Pos = Vec<f64>;

fn position(mut nums: Vec<f64>, ax: Axes) -> Result<Pos, String> {
    let ok = match ax.dim {
        Some(d) => nums.len() == d,
        None => (2..=3).contains(&nums.len()),
    };
    if !ok {
        return Err(format!(
            "parse_gml: a position of {} numbers, want {}",
            nums.len(),
            ax.dim.map_or("2 or 3".to_string(), |d| d.to_string())
        ));
    }
    if ax.swap {
        nums.swap(0, 1);
    }
    Ok(nums)
}

/// `gml:coordinates` of GML 2: tuples split by `ts`, numbers by `cs`, `decimal` as the point.
fn coordinates(el: &El, ax: Axes) -> Result<Vec<Pos>, String> {
    let cs = el.attr("cs").unwrap_or(",");
    let ts = el.attr("ts").unwrap_or(" ");
    let decimal = el.attr("decimal").unwrap_or(".");
    if cs.is_empty()
        || ts.is_empty()
        || decimal.is_empty()
        || cs == ts
        || decimal == cs
        || decimal == ts
    {
        return Err("parse_gml: gml:coordinates needs distinct cs, ts and decimal".to_string());
    }
    let tuples: Vec<&str> = if ts.trim().is_empty() {
        el.text.split_whitespace().collect()
    } else {
        el.text
            .split(ts)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect()
    };
    tuples
        .into_iter()
        .map(|t| {
            let nums = t
                .split(cs)
                .map(|n| number(&n.trim().replace(decimal, ".")))
                .collect::<Result<Vec<_>, _>>()?;
            position(nums, ax)
        })
        .collect()
}

/// `gml:posList`: whitespace-separated numbers, srsDimension of them per position.
fn pos_list(el: &El, ax: Axes) -> Result<Vec<Pos>, String> {
    let nums = el
        .text
        .split_whitespace()
        .map(number)
        .collect::<Result<Vec<_>, _>>()?;
    let dim = ax.dim.unwrap_or(2);
    if nums.is_empty() || !nums.len().is_multiple_of(dim) {
        return Err(format!(
            "parse_gml: gml:posList holds {} numbers, not a multiple of srsDimension {dim}",
            nums.len()
        ));
    }
    nums.chunks(dim)
        .map(|c| {
            position(
                c.to_vec(),
                Axes {
                    dim: Some(dim),
                    ..ax
                },
            )
        })
        .collect()
}

/// Every position of a point list (LineString, LinearRing, a curve segment), in order.
fn points(c: &mut Conv, el: &El, ax: Axes) -> Result<Vec<Pos>, String> {
    let ax = c.axes(el, ax)?;
    let mut out = Vec::new();
    for kid in el.parts() {
        let ax = c.axes(kid, ax)?;
        match kid.name.as_str() {
            "pos" => out.push(position(
                kid.text
                    .split_whitespace()
                    .map(number)
                    .collect::<Result<_, _>>()?,
                ax,
            )?),
            "posList" => out.extend(pos_list(kid, ax)?),
            "coordinates" => out.extend(coordinates(kid, ax)?),
            "coord" => out.push(coord(kid, ax)?),
            "pointProperty" | "pointRep" => out.push(point(c, kid.only()?, ax)?),
            "Point" => out.push(point(c, kid, ax)?),
            _ => return Err(el.unexpected(kid)),
        }
    }
    if out.is_empty() {
        return Err(format!("parse_gml: gml:{} holds no position", el.name));
    }
    Ok(out)
}

/// `gml:coord` of GML 2: X, Y and an optional Z child.
fn coord(el: &El, ax: Axes) -> Result<Pos, String> {
    let mut nums = Vec::new();
    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
        match el.kids.iter().find(|k| k.name == *axis) {
            Some(k) => nums.push(number(k.text.trim())?),
            None if i == 2 => {}
            None => return Err(format!("parse_gml: gml:coord needs {axis}")),
        }
    }
    position(nums, Axes { dim: None, ..ax })
}

fn point(c: &mut Conv, el: &El, ax: Axes) -> Result<Pos, String> {
    if el.name != "Point" {
        return Err(format!("parse_gml: gml:{} is not a gml:Point", el.name));
    }
    let mut ps = points(c, el, ax)?;
    match ps.len() {
        1 => Ok(ps.remove(0)),
        n => Err(format!("parse_gml: a gml:Point holds {n} positions")),
    }
}

/// Appends a part of a ring or curve, dropping the position the two share.
fn join(acc: &mut Vec<Pos>, part: Vec<Pos>) {
    let mut part = part.into_iter().peekable();
    if acc.last().is_some() && acc.last() == part.peek() {
        part.next();
    }
    acc.extend(part);
}

/// The positions of a curve: LineString, LinearRing, Curve segments, a Ring of curve members.
fn line(c: &mut Conv, el: &El, ax: Axes) -> Result<Vec<Pos>, String> {
    let ax = c.axes(el, ax)?;
    let ps = match el.name.as_str() {
        "LineString" | "LinearRing" | "LineStringSegment" => points(c, el, ax)?,
        "ArcString" | "Arc" => arcs(points(c, el, ax)?)?,
        "Curve" => {
            let mut acc = Vec::new();
            for kid in el.parts() {
                if kid.name != "segments" {
                    return Err(el.unexpected(kid));
                }
                for seg in kid.parts() {
                    let part = line(c, seg, ax)?;
                    join(&mut acc, part);
                }
            }
            acc
        }
        "Ring" | "CompositeCurve" => {
            let mut acc = Vec::new();
            for kid in el.parts() {
                if kid.name != "curveMember" {
                    return Err(el.unexpected(kid));
                }
                let part = line(c, kid.only()?, ax)?;
                join(&mut acc, part);
            }
            acc
        }
        other => return Err(format!("parse_gml: gml:{other} is not a curve")),
    };
    if ps.len() < 2 {
        return Err(format!("parse_gml: gml:{} needs two positions", el.name));
    }
    Ok(ps)
}

fn ring(c: &mut Conv, el: &El, ax: Axes) -> Result<Vec<Pos>, String> {
    let ps = line(c, el.only()?, ax)?;
    if ps.len() < 4 || ps.first() != ps.last() {
        return Err(format!(
            "parse_gml: the ring in gml:{} needs four positions, the last equal to the first",
            el.name
        ));
    }
    Ok(ps)
}

/// A circular arc string (2n+1 positions, each three on one circle) as chords of at most
/// [`ARC_STEP`]; three collinear positions are a straight line. A z is interpolated.
fn arcs(ps: Vec<Pos>) -> Result<Vec<Pos>, String> {
    if ps.len() < 3 || ps.len().is_multiple_of(2) {
        return Err("parse_gml: an arc string holds 2n+1 positions, n at least 1".to_string());
    }
    let mut out = vec![ps[0].clone()];
    for w in ps.windows(3).step_by(2) {
        let (a, b, e) = (&w[0], &w[1], &w[2]);
        // Relative to a, so projected coordinates of 10^7 keep their digits.
        let (bx, by, ex, ey) = (b[0] - a[0], b[1] - a[1], e[0] - a[0], e[1] - a[1]);
        let d = 2.0 * (bx * ey - by * ex);
        let (b2, e2) = (bx * bx + by * by, ex * ex + ey * ey);
        if d.abs() <= 1e-12 * (b2 + e2) {
            out.extend([b.clone(), e.clone()]);
            continue;
        }
        let (ux, uy) = ((ey * b2 - by * e2) / d, (bx * e2 - ex * b2) / d);
        let r = ux.hypot(uy);
        let t0 = (-uy).atan2(-ux);
        let s1 = ((by - uy).atan2(bx - ux) - t0).rem_euclid(2.0 * PI);
        let s2 = ((ey - uy).atan2(ex - ux) - t0).rem_euclid(2.0 * PI);
        let sweep = if s1 <= s2 { s2 } else { s2 - 2.0 * PI };
        // At most 72 chords per arc: the sweep is below one turn.
        let n = (sweep.abs() / ARC_STEP).ceil().max(2.0) as usize;
        for i in 1..n {
            let f = i as f64 / n as f64;
            let t = t0 + sweep * f;
            let mut p = vec![a[0] + ux + r * t.cos(), a[1] + uy + r * t.sin()];
            if let (Some(za), Some(ze)) = (a.get(2), e.get(2)) {
                p.push(za + (ze - za) * f);
            }
            out.push(p);
        }
        out.push(e.clone());
    }
    if out.iter().flatten().any(|f| !f.is_finite()) {
        return Err("parse_gml: an arc has no finite circle".to_string());
    }
    Ok(out)
}

/// The rings of a Polygon or PolygonPatch: the exterior first, then each interior.
fn polygon(c: &mut Conv, el: &El, ax: Axes) -> Result<Vec<Vec<Pos>>, String> {
    let ax = c.axes(el, ax)?;
    let mut exterior = None;
    let mut interiors = Vec::new();
    for kid in el.parts() {
        match kid.name.as_str() {
            "exterior" | "outerBoundaryIs" if exterior.is_none() => {
                exterior = Some(ring(c, kid, ax)?)
            }
            "interior" | "innerBoundaryIs" => interiors.push(ring(c, kid, ax)?),
            _ => return Err(el.unexpected(kid)),
        }
    }
    let exterior =
        exterior.ok_or_else(|| format!("parse_gml: gml:{} needs an exterior", el.name))?;
    Ok(std::iter::once(exterior).chain(interiors).collect())
}

/// The polygons of a surface: a Polygon, or each PolygonPatch of a Surface.
fn surfaces(c: &mut Conv, el: &El, ax: Axes) -> Result<Vec<Vec<Vec<Pos>>>, String> {
    let ax = c.axes(el, ax)?;
    match el.name.as_str() {
        "Polygon" => Ok(vec![polygon(c, el, ax)?]),
        "Surface" => {
            let mut out = Vec::new();
            for kid in el.parts() {
                if kid.name != "patches" {
                    return Err(el.unexpected(kid));
                }
                for patch in kid.parts() {
                    if patch.name != "PolygonPatch" {
                        return Err(kid.unexpected(patch));
                    }
                    out.push(polygon(c, patch, ax)?);
                }
            }
            if out.is_empty() {
                return Err("parse_gml: gml:Surface holds no patch".to_string());
            }
            Ok(out)
        }
        "MultiSurface" | "MultiPolygon" | "CompositeSurface" => {
            let mut out = Vec::new();
            for m in members(el, &["surfaceMember", "polygonMember"], "surfaceMembers")? {
                out.extend(surfaces(c, m, ax)?);
            }
            Ok(out)
        }
        other => Err(format!("parse_gml: gml:{other} is not a surface")),
    }
}

/// The geometries of a multi geometry: each `one` property's geometry and every child of `many`.
fn members<'e>(el: &'e El, one: &[&str], many: &str) -> Result<Vec<&'e El>, String> {
    let mut out = Vec::new();
    for kid in el.parts() {
        if one.contains(&kid.name.as_str()) {
            out.push(kid.only()?);
        } else if kid.name == many {
            out.extend(kid.parts());
        } else {
            return Err(el.unexpected(kid));
        }
    }
    Ok(out)
}

fn floats(p: Pos) -> Value {
    Value::Array(p.into_iter().map(Value::Float).collect())
}

fn nest<T>(items: Vec<T>, f: impl Fn(T) -> Value) -> Value {
    Value::Array(items.into_iter().map(f).collect())
}

fn geojson(kind: &str, key: &str, v: Value) -> Value {
    let mut m = BTreeMap::new();
    m.insert("type".to_string(), Value::String(kind.to_string()));
    m.insert(key.to_string(), v);
    Value::Object(m.into())
}

fn geometry(c: &mut Conv, el: &El, ax: Axes, depth: usize) -> Result<Value, String> {
    if depth > MAX_GEO_DEPTH {
        return Err(format!(
            "parse_gml: the MultiGeometry nests deeper than {MAX_GEO_DEPTH}"
        ));
    }
    let ax = c.axes(el, ax)?;
    let ring_list = |r: Vec<Vec<Pos>>| nest(r, |l| nest(l, floats));
    Ok(match el.name.as_str() {
        "Point" => geojson("Point", "coordinates", floats(point(c, el, ax)?)),
        "LineString" | "Curve" | "CompositeCurve" | "LinearRing" | "Ring" => {
            geojson("LineString", "coordinates", nest(line(c, el, ax)?, floats))
        }
        "Polygon" | "Surface" => {
            let mut ps = surfaces(c, el, ax)?;
            if ps.len() == 1 {
                geojson("Polygon", "coordinates", ring_list(ps.remove(0)))
            } else {
                geojson("MultiPolygon", "coordinates", nest(ps, ring_list))
            }
        }
        "MultiSurface" | "MultiPolygon" | "CompositeSurface" => geojson(
            "MultiPolygon",
            "coordinates",
            nest(surfaces(c, el, ax)?, ring_list),
        ),
        "MultiPoint" => {
            let ps = members(el, &["pointMember"], "pointMembers")?
                .into_iter()
                .map(|p| point(c, p, ax))
                .collect::<Result<Vec<_>, _>>()?;
            geojson("MultiPoint", "coordinates", nest(ps, floats))
        }
        "MultiCurve" | "MultiLineString" => {
            let ls = members(el, &["curveMember", "lineStringMember"], "curveMembers")?
                .into_iter()
                .map(|l| line(c, l, ax))
                .collect::<Result<Vec<_>, _>>()?;
            geojson(
                "MultiLineString",
                "coordinates",
                nest(ls, |l| nest(l, floats)),
            )
        }
        "MultiGeometry" => {
            let gs = members(el, &["geometryMember"], "geometryMembers")?
                .into_iter()
                .map(|g| geometry(c, g, ax, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            geojson("GeometryCollection", "geometries", Value::Array(gs.into()))
        }
        other => {
            return Err(format!(
                "parse_gml: gml:{other} is not a GML geometry parse_gml reads"
            ))
        }
    })
}

/// A GML geometry as a GeoJSON geometry, its positions in x/y order. With `to`, it is
/// reprojected from the geometry's srsName, or `from` where it names none.
pub fn parse_gml(target: &Value, from: Option<&str>, to: Option<&str>) -> Result<Value, String> {
    let el = root(target)?;
    let mut c = Conv { crs: None };
    let mut ax = Axes {
        dim: None,
        swap: false,
    };
    if let Some(f) = from.filter(|_| el.attr("srsName").is_none()) {
        let (code, swap) = srs_axes(f).map_err(|e| format!("parameter from: {e}"))?;
        c.crs = Some(format!("EPSG:{code}"));
        ax.swap = swap;
    }
    let g = geometry(&mut c, &el, ax, 0)?;
    match (to, c.crs) {
        (None, _) => Ok(g),
        (Some(to), Some(from)) => geo_transform(&g, &from, to),
        (Some(_), None) => Err("parameter from: the GML names no srsName; give the CRS its positions are in, such as from: \"EPSG:3879\"".to_string()),
    }
}
