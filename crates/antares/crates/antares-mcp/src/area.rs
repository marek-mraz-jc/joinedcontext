// SPDX-License-Identifier: EUPL-1.2
//! `area_overview` (T-5279): what lies in one area across every Entity type
//! a view shows, in one call. The area is a Point with a radius, a
//! `LineString` with a buffer, or a `Polygon`/`MultiPolygon`; this module checks it
//! at the trust boundary, plans the geoquery each type is read with, measures
//! distances and folds each type's rows into a count and its nearest
//! Entities. The reads themselves run through the view as `query_entities`
//! runs them (`Server::area_overview`), so a type, an Entity or an Attribute
//! the caller may not read never reaches a count or the answer.

use serde_json::{json, Map, Value};

/// The widest radius or buffer, in metres.
pub(crate) const MAX_RADIUS_M: u64 = 5000;
/// The radius of a Point area when the call names none, in metres.
const DEFAULT_RADIUS_M: u64 = 500;
/// The buffer of a `LineString` area when the call names none, in metres.
const DEFAULT_BUFFER_M: u64 = 50;
/// The most positions an area may have.
pub(crate) const MAX_VERTICES: usize = 1000;
/// The largest area when `ANTARES_AREA_MAX_KM2` names none, in km².
pub(crate) const DEFAULT_MAX_KM2: f64 = 100.0;
/// Entities per type by default, and the most a call may ask.
pub(crate) const DEFAULT_K: u64 = 5;
pub(crate) const MAX_K: u64 = 20;
/// The most types one call reads; a view names at most 256.
pub(crate) const MAX_TYPES: usize = 64;
/// Rows of one type read to rank its nearest; past it the ranking is a sample.
pub(crate) const RANK_ROWS: u64 = 200;
/// Rows of one type a `LineString` area reads to measure against the line.
pub(crate) const LINE_ROWS: u64 = 1000;
/// The answer's size: past it the farthest Entities are left out, one round
/// per type at a time, and `truncated` says so.
const ANSWER_BYTES: usize = 18 * 1024;
/// Attributes shown per Entity, and the longest text value kept.
const KEY_ATTRS: usize = 4;
const TEXT_CHARS: usize = 80;
/// Mean Earth radius (IUGG), the one the broker's `near` evaluator uses.
const EARTH_M: f64 = 6_371_008.8;

/// A refused area: a stable slug and the rule, never the value.
#[derive(Debug, PartialEq)]
pub(crate) struct Refusal {
    pub slug: &'static str,
    pub why: String,
}

fn refuse(slug: &'static str, why: impl Into<String>) -> Refusal {
    Refusal {
        slug,
        why: why.into(),
    }
}

type Pos = [f64; 2];

/// A checked area.
#[derive(Debug, Clone)]
pub(crate) enum Shape {
    Point {
        at: Pos,
        radius: f64,
    },
    Line {
        path: Vec<Pos>,
        buffer: f64,
    },
    Area {
        polygons: Vec<Vec<Vec<Pos>>>,
        multi: bool,
    },
}

/// One position: longitude then latitude, in the WGS84 range.
fn position(v: &Value) -> Result<Pos, Refusal> {
    let shape = || refuse("invalid-geometry", "a position is [longitude, latitude]");
    let a = v.as_array().ok_or_else(shape)?;
    let (Some(x), Some(y)) = (
        a.first().and_then(Value::as_f64),
        a.get(1).and_then(Value::as_f64),
    ) else {
        return Err(shape());
    };
    if a.len() > 3 {
        return Err(shape());
    }
    if !((-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y)) {
        return Err(refuse(
            "coordinates-out-of-range",
            "longitude is -180 to 180 and latitude -90 to 90 (WGS84, longitude first)",
        ));
    }
    Ok([x, y])
}

fn positions(v: &Value) -> Result<Vec<Pos>, Refusal> {
    v.as_array()
        .ok_or_else(|| refuse("invalid-geometry", "a list of positions is expected"))?
        .iter()
        .map(position)
        .collect()
}

fn polygon(v: &Value) -> Result<Vec<Vec<Pos>>, Refusal> {
    let rings = v
        .as_array()
        .filter(|r| !r.is_empty())
        .ok_or_else(|| refuse("invalid-geometry", "a Polygon is a list of rings"))?;
    rings
        .iter()
        .map(|r| {
            let ring = positions(r)?;
            if ring.len() < 4 || ring.first() != ring.last() {
                return Err(refuse(
                    "invalid-polygon",
                    "each ring has at least 4 positions and ends where it starts",
                ));
            }
            if self_intersects(&ring) {
                return Err(refuse("invalid-polygon", "a ring crosses itself"));
            }
            Ok(ring)
        })
        .collect()
}

/// Check `geometry` and `coordinates`, with `radius` for a Point and `buffer`
/// for a `LineString` in metres, against the limits: positions, vertices,
/// ring validity and an area of at most `max_km2`.
pub(crate) fn shape(
    geometry: &str,
    coordinates: &Value,
    radius: Option<u64>,
    buffer: Option<u64>,
    max_km2: f64,
) -> Result<Shape, Refusal> {
    let vertices = count_positions(coordinates);
    if vertices > MAX_VERTICES {
        return Err(refuse(
            "too-many-vertices",
            format!("an area has at most {MAX_VERTICES} positions"),
        ));
    }
    let metres = |m: Option<u64>, default: u64| {
        let m = m.unwrap_or(default);
        if m > MAX_RADIUS_M {
            return Err(refuse(
                "distance-too-large",
                format!("maxDistance is at most {MAX_RADIUS_M} m"),
            ));
        }
        Ok(f64::from(u32::try_from(m).unwrap_or(u32::MAX)))
    };
    let shape = match geometry {
        "Point" => Shape::Point {
            at: position(coordinates)?,
            radius: metres(radius, DEFAULT_RADIUS_M)?,
        },
        "LineString" => {
            let path = positions(coordinates)?;
            if path.len() < 2 {
                return Err(refuse(
                    "invalid-geometry",
                    "a LineString has at least 2 positions",
                ));
            }
            Shape::Line {
                path,
                buffer: metres(buffer, DEFAULT_BUFFER_M)?,
            }
        }
        "Polygon" => Shape::Area {
            polygons: vec![polygon(coordinates)?],
            multi: false,
        },
        "MultiPolygon" => Shape::Area {
            polygons: coordinates
                .as_array()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| refuse("invalid-geometry", "a MultiPolygon is a list of Polygons"))?
                .iter()
                .map(polygon)
                .collect::<Result<_, _>>()?,
            multi: true,
        },
        _ => {
            return Err(refuse(
                "invalid-geometry",
                "geometry is Point, LineString, Polygon or MultiPolygon",
            ))
        }
    };
    if shape.km2() > max_km2 {
        return Err(refuse(
            "area-too-large",
            format!("the area searched is at most {max_km2} km²; split it or draw it smaller"),
        ));
    }
    Ok(shape)
}

fn count_positions(v: &Value) -> usize {
    match v.as_array() {
        Some(a) if a.first().is_some_and(Value::is_number) => 1,
        Some(a) => a.iter().map(count_positions).sum(),
        None => 0,
    }
}

/// Metres east and north of `origin`, on a plane tangent there: good to
/// 0.1 % across the 100 km² an area may cover.
fn plane(origin: Pos, p: Pos) -> Pos {
    let k = EARTH_M * std::f64::consts::PI / 180.0;
    [
        (p[0] - origin[0]) * k * origin[1].to_radians().cos(),
        (p[1] - origin[1]) * k,
    ]
}

fn cross(o: Pos, a: Pos, b: Pos) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Whether `p`, on the line through `a` and `b`, lies between them.
fn on_segment(a: Pos, b: Pos, p: Pos) -> bool {
    let between = |v: f64, x: f64, y: f64| (v - x) * (v - y) <= 0.0;
    [between(p[0], a[0], b[0]), between(p[1], a[1], b[1])] == [true, true]
}

/// The side of the line `o`-`a` that `b` lies on: left, right, or on it.
fn side(o: Pos, a: Pos, b: Pos) -> (bool, bool) {
    let c = cross(o, a, b);
    (c > 0.0, c < 0.0)
}

/// Whether segments `ab` and `cd` share a point.
fn crosses(a: Pos, b: Pos, c: Pos, d: Pos) -> bool {
    let (s1, s2, s3, s4) = (side(c, d, a), side(c, d, b), side(a, b, c), side(a, b, d));
    if s1 != s2 && s3 != s4 {
        return true;
    }
    // collinear: a point of one lies on the other
    let on = (false, false);
    [(s1, c, d, a), (s2, c, d, b), (s3, a, b, c), (s4, a, b, d)]
        .into_iter()
        .any(|(s, p, q, r)| s == on && on_segment(p, q, r))
}

/// Whether a closed ring crosses or touches itself anywhere but at the
/// shared ends of neighbouring segments. ponytail: O(n²), 500 000 tests at
/// the 1 000-vertex cap; a sweep line if the cap grows.
fn self_intersects(ring: &[Pos]) -> bool {
    let segs: Vec<(Pos, Pos)> = ring
        .windows(2)
        .filter_map(|w| Some((*w.first()?, *w.get(1)?)))
        .collect();
    let n = segs.len();
    segs.iter().enumerate().any(|(i, &(a, b))| {
        segs.iter()
            .enumerate()
            .skip(i.saturating_add(2))
            .any(|(j, &(c, d))| {
                // the last segment meets the first at the ring's start
                !(i == 0 && j.saturating_add(1) == n) && crosses(a, b, c, d)
            })
    })
}

fn ring_m2(ring: &[Pos], origin: Pos) -> f64 {
    let p: Vec<Pos> = ring.iter().map(|q| plane(origin, *q)).collect();
    let twice: f64 = p
        .windows(2)
        .filter_map(|w| Some(cross([0.0, 0.0], *w.first()?, *w.get(1)?)))
        .sum();
    (twice / 2.0).abs()
}

/// Distance in metres between two positions on the sphere (haversine).
pub(crate) fn haversine(a: Pos, b: Pos) -> f64 {
    let (la, lb) = (a[1].to_radians(), b[1].to_radians());
    let h = ((lb - la) / 2.0).sin().powi(2)
        + la.cos() * lb.cos() * ((b[0] - a[0]).to_radians() / 2.0).sin().powi(2);
    2.0 * EARTH_M * h.sqrt().min(1.0).asin()
}

/// Distance in metres from `p` to the polyline `path`.
fn to_line(path: &[Pos], p: Pos) -> f64 {
    let at = |q: Pos| plane(p, q);
    path.windows(2)
        .filter_map(|w| {
            let (a, b) = (at(*w.first()?), at(*w.get(1)?));
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx * dx + dy * dy;
            let t = if len > 0.0 {
                ((-a[0] * dx - a[1] * dy) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            Some((a[0] + t * dx).hypot(a[1] + t * dy))
        })
        .fold(f64::INFINITY, f64::min)
}

impl Shape {
    /// The bounding box `[west, south, east, north]` of what is searched.
    pub(crate) fn bbox(&self) -> [f64; 4] {
        let grow = |b: [f64; 4], m: f64| {
            let dy = (m / EARTH_M).to_degrees();
            let lat = b[1].abs().max(b[3].abs()).min(89.0);
            let dx = dy / lat.to_radians().cos();
            [b[0] - dx, b[1] - dy, b[2] + dx, b[3] + dy]
        };
        let of = |ps: &mut dyn Iterator<Item = Pos>| {
            ps.fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                },
            )
        };
        match self {
            Shape::Point { at, radius } => grow([at[0], at[1], at[0], at[1]], *radius),
            Shape::Line { path, buffer } => grow(of(&mut path.iter().copied()), *buffer),
            Shape::Area { polygons, .. } => of(&mut polygons.iter().flatten().flatten().copied()),
        }
    }

    /// The area searched in km²: the circle, the line's box, the polygons
    /// less their holes.
    pub(crate) fn km2(&self) -> f64 {
        let m2 = match self {
            Shape::Point { radius, .. } => std::f64::consts::PI * radius * radius,
            Shape::Line { .. } => {
                let b = self.bbox();
                let (sw, ne) = ([b[0], b[1]], [b[2], b[3]]);
                let p = plane(sw, ne);
                (p[0] * p[1]).abs()
            }
            Shape::Area { polygons, .. } => polygons
                .iter()
                .map(|rings| {
                    let origin = rings
                        .first()
                        .and_then(|r| r.first())
                        .copied()
                        .unwrap_or_default();
                    let mut each = rings.iter().map(|r| ring_m2(r, origin));
                    let outer = each.next().unwrap_or_default();
                    outer - each.sum::<f64>()
                })
                .sum(),
        };
        m2 / 1e6
    }

    /// The point distances are measured from: the Point, or the box's centre.
    fn centre(&self) -> Pos {
        if let Shape::Point { at, .. } = self {
            return *at;
        }
        let b = self.bbox();
        [f64::midpoint(b[0], b[2]), f64::midpoint(b[1], b[3])]
    }

    /// What a distance in the answer is measured from.
    pub(crate) fn from(&self) -> &'static str {
        match self {
            Shape::Point { .. } => "point",
            Shape::Line { .. } => "line",
            Shape::Area { .. } => "centre",
        }
    }

    /// The geoquery each type is read with: `georel`, `geometry` and
    /// `coordinates`. A `LineString` reads its buffered box, a superset the
    /// distance to the line then narrows ([`Shape::keeps`]).
    pub(crate) fn query(&self, radius: Option<f64>) -> (String, &'static str, Value) {
        match self {
            Shape::Point { at, radius: r } => (
                format!("near;maxDistance=={}", radius.unwrap_or(*r).ceil()),
                "Point",
                json!(at),
            ),
            Shape::Line { .. } => {
                let [w, s, e, n] = self.bbox();
                (
                    "intersects".into(),
                    "Polygon",
                    json!([[[w, s], [e, s], [e, n], [w, n], [w, s]]]),
                )
            }
            Shape::Area { polygons, multi } => (
                "intersects".into(),
                if *multi { "MultiPolygon" } else { "Polygon" },
                if *multi {
                    json!(polygons)
                } else {
                    json!(polygons.first())
                },
            ),
        }
    }

    /// The distance of an Entity's location, in metres: to the Point, the
    /// line or the area's centre, from the nearest of its positions.
    /// ponytail: vertices only, so a long line or a large polygon Entity
    /// reads farther than its nearest edge; measure edges if that misranks.
    pub(crate) fn distance(&self, location: &Value) -> Option<f64> {
        let mut ps = Vec::new();
        collect(location.get("coordinates")?, &mut ps);
        let d = |p: Pos| match self {
            Shape::Line { path, .. } => to_line(path, p),
            _ => haversine(self.centre(), p),
        };
        ps.into_iter().map(d).reduce(f64::min)
    }

    /// Whether an Entity at `distance` lies in the area: a `LineString`'s box
    /// is wider than its buffer, every other query is already exact.
    pub(crate) fn keeps(&self, distance: Option<f64>) -> bool {
        match self {
            Shape::Line { buffer, .. } => distance.is_some_and(|d| d <= *buffer),
            _ => true,
        }
    }
}

fn collect(v: &Value, out: &mut Vec<Pos>) {
    let items = v.as_array().map(Vec::as_slice).unwrap_or_default();
    let x = items.first().and_then(Value::as_f64);
    let y = items.get(1).and_then(Value::as_f64);
    out.extend(x.zip(y).map(|(x, y)| [x, y]));
    if x.is_none() {
        for v in items {
            collect(v, out);
        }
    }
}

/// The `GeoJSON` of a normalized Entity's `location`, when it has one.
pub(crate) fn location(entity: &Value) -> Option<&Value> {
    let l = entity.get("location")?;
    l.get("value").or(Some(l))
}

/// What an Attribute shows: a scalar value, its unit and its time.
fn shown(attr: &Value) -> Option<Value> {
    let kind = attr.get("type").and_then(Value::as_str)?;
    if kind != "Property" {
        return None;
    }
    let value = match attr.get("value")? {
        Value::String(s) if s.chars().count() > TEXT_CHARS => Value::String(format!(
            "{}…",
            s.chars().take(TEXT_CHARS).collect::<String>()
        )),
        v @ (Value::String(_) | Value::Number(_) | Value::Bool(_)) => v.clone(),
        _ => return None,
    };
    let mut out = Map::from_iter([("value".to_owned(), value)]);
    for key in ["unitCode", "observedAt"] {
        if let Some(v) = attr.get(key) {
            out.insert(key.to_owned(), v.clone());
        }
    }
    Some(Value::Object(out))
}

/// One Entity of the answer: id, name, distance and its first
/// [`KEY_ATTRS`] Attributes, measured ones (a unit or a time) first.
pub(crate) fn compact(entity: &Value, distance: Option<f64>, lang: Option<&str>) -> Value {
    let mut out = Map::new();
    out.insert(
        "id".into(),
        entity.get("id").cloned().unwrap_or(Value::Null),
    );
    let name =
        entity
            .get("name")
            .and_then(|n| match n.get("value").or_else(|| n.get("languageMap")) {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Object(m)) => lang
                    .and_then(|l| m.get(l))
                    .or_else(|| m.values().next())
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            });
    if let Some(name) = name {
        out.insert("name".into(), Value::String(name));
    }
    if let Some(d) = distance {
        out.insert("distance_m".into(), json!(d.round()));
    }
    let mut attrs: Vec<(&String, Value)> = entity
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| !matches!(k.as_str(), "id" | "type" | "name" | "location" | "@context"))
        .filter_map(|(k, v)| Some((k, shown(v)?)))
        .collect();
    // measured first; the sort is stable, so the document order holds within
    attrs.sort_by_key(|(_, v)| v.get("unitCode").is_none() && v.get("observedAt").is_none());
    let attrs: Map<String, Value> = attrs
        .into_iter()
        .take(KEY_ATTRS)
        .map(|(k, v)| (k.clone(), v))
        .collect();
    if !attrs.is_empty() {
        out.insert("attributes".into(), Value::Object(attrs));
    }
    Value::Object(out)
}

/// One type's part of the answer.
#[derive(Debug)]
pub(crate) struct TypePart {
    pub entity_type: String,
    pub count: u64,
    /// false when the count is a lower bound (a `LineString` read past [`LINE_ROWS`])
    pub exact: bool,
    /// false when the nearest were picked from a sample of the type's rows
    pub nearest: bool,
    /// compact Entities, nearest first
    pub entities: Vec<Value>,
}

/// Rank `rows` of one type by distance, drop what lies outside, keep `k`.
/// Returns how many lie inside and the `k` nearest, compacted.
pub(crate) fn rank(
    shape: &Shape,
    rows: &[Value],
    k: usize,
    lang: Option<&str>,
) -> (u64, Vec<Value>) {
    let mut inside: Vec<(f64, &Value)> = rows
        .iter()
        .filter_map(|e| {
            let d = location(e).and_then(|l| shape.distance(l));
            shape.keeps(d).then_some((d.unwrap_or(f64::INFINITY), e))
        })
        .collect();
    inside.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = u64::try_from(inside.len()).unwrap_or(u64::MAX);
    let kept = inside
        .into_iter()
        .take(k)
        .map(|(d, e)| compact(e, d.is_finite().then_some(d), lang))
        .collect();
    (n, kept)
}

/// The answer: the area, then each type with Entities, most first, its
/// Entities cut round by round to [`ANSWER_BYTES`].
pub(crate) fn answer(shape: &Shape, k: u64, mut parts: Vec<TypePart>, types_cut: bool) -> Value {
    parts.retain(|p| p.count > 0);
    parts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.entity_type.cmp(&b.entity_type))
    });
    let mut used = 0_usize;
    let mut kept: Vec<Vec<Value>> = parts.iter().map(|_| Vec::new()).collect();
    let mut truncated = types_cut;
    let rounds = parts.iter().map(|p| p.entities.len()).max().unwrap_or(0);
    'rounds: for round in 0..rounds {
        for (part, out) in parts.iter().zip(kept.iter_mut()) {
            let Some(e) = part.entities.get(round) else {
                continue;
            };
            let size = e.to_string().len();
            if used.saturating_add(size) > ANSWER_BYTES {
                truncated = true;
                break 'rounds;
            }
            used = used.saturating_add(size);
            out.push(e.clone());
        }
    }
    let types: Vec<Value> = parts
        .iter()
        .zip(kept)
        .map(|(p, entities)| {
            json!({
                "type": p.entity_type,
                "count": p.count,
                "countExact": p.exact,
                "ranked": if p.nearest { "nearest" } else { "sample" },
                "entities": entities,
            })
        })
        .collect();
    let b = shape.bbox();
    json!({
        "area": {
            "bbox": b,
            "km2": (shape.km2() * 1000.0).round() / 1000.0,
            "distanceFrom": shape.from(),
        },
        "limit": k,
        "types": types,
        "truncated": truncated,
    })
}

/// The text beside the structured answer: the counts, most first.
pub(crate) fn render(answer: &Value) -> String {
    let types = answer
        .get("types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let km2 = answer
        .pointer("/area/km2")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    if types.is_empty() {
        return format!("No Entity this view shows lies in the area ({km2} km²).");
    }
    let counts: Vec<String> = types
        .iter()
        .map(|t| {
            let more = if t.get("countExact") == Some(&Value::Bool(false)) {
                "+"
            } else {
                ""
            };
            format!(
                "{} {}{more}",
                t.get("type").and_then(Value::as_str).unwrap_or_default(),
                t.get("count").and_then(Value::as_u64).unwrap_or_default()
            )
        })
        .collect();
    format!(
        "{} types in the area ({km2} km²): {}. The nearest of each type are in the structured result.",
        types.len(),
        counts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: f64, s: f64, d: f64) -> Value {
        json!([[[w, s], [w + d, s], [w + d, s + d], [w, s + d], [w, s]]])
    }

    #[test]
    fn each_limit_is_refused_with_its_slug() {
        let slug = |g: &str, c: Value, r: Option<u64>| {
            shape(g, &c, r, None, DEFAULT_MAX_KM2)
                .map(|_| ())
                .map_err(|e| e.slug)
        };
        assert_eq!(
            slug("Point", json!([24.94, 60.17]), Some(5001)),
            Err("distance-too-large")
        );
        assert_eq!(slug("Point", json!([24.94, 60.17]), Some(5000)), Ok(()));
        assert_eq!(
            slug("Point", json!([181.0, 60.0]), None),
            Err("coordinates-out-of-range")
        );
        assert_eq!(
            slug("Point", json!([24.0, -90.5]), None),
            Err("coordinates-out-of-range")
        );
        assert_eq!(
            slug("Point", json!(["a", 1]), None),
            Err("invalid-geometry")
        );
        assert_eq!(
            slug("MultiPoint", json!([[1, 1]]), None),
            Err("invalid-geometry")
        );
        assert_eq!(
            slug("LineString", json!([[24.9, 60.1]]), None),
            Err("invalid-geometry")
        );
        // a bow tie crosses itself
        let bow = json!([[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]]);
        assert_eq!(slug("Polygon", bow, None), Err("invalid-polygon"));
        assert_eq!(
            slug("Polygon", json!([[[0, 0], [0.01, 0], [0, 0.01]]]), None),
            Err("invalid-polygon")
        );
        let many: Vec<Value> = (0..=1000)
            .map(|i| json!([f64::from(i) * 1e-5, 0.0]))
            .collect();
        assert_eq!(
            slug("LineString", json!(many), None),
            Err("too-many-vertices")
        );
        // about 11 x 5.6 km at 60° N is 62 km²; 0.2° is four times that
        assert_eq!(slug("Polygon", square(24.8, 60.1, 0.1), None), Ok(()));
        assert_eq!(
            slug("Polygon", square(24.8, 60.1, 0.2), None),
            Err("area-too-large")
        );
        let both = json!([square(24.8, 60.1, 0.1), square(25.0, 60.1, 0.1)]);
        assert_eq!(slug("MultiPolygon", both, None), Err("area-too-large"));
    }

    #[test]
    fn a_ring_closed_at_its_start_is_valid_and_a_hole_shrinks_the_area() -> Result<(), String> {
        let outer = [
            [0.0, 0.0],
            [0.01, 0.0],
            [0.01, 0.01],
            [0.0, 0.01],
            [0.0, 0.0],
        ];
        assert!(!self_intersects(&outer));
        let whole = shape("Polygon", &json!([outer]), None, None, 100.0).map_err(|e| e.why)?;
        let hole = [
            [0.002, 0.002],
            [0.004, 0.002],
            [0.004, 0.004],
            [0.002, 0.004],
            [0.002, 0.002],
        ];
        let holed =
            shape("Polygon", &json!([outer, hole]), None, None, 100.0).map_err(|e| e.why)?;
        // 0.01° is 1 112 m on the mean sphere
        assert!((whole.km2() - 1.2364).abs() < 0.001, "{}", whole.km2());
        assert!(
            (whole.km2() - holed.km2() - 0.0496).abs() < 0.001,
            "{}",
            holed.km2()
        );
        Ok(())
    }

    #[test]
    fn a_line_keeps_what_lies_within_its_buffer_by_the_metre() -> Result<(), String> {
        let path = json!([[24.94, 60.17], [24.94, 60.17], [24.96, 60.17]]);
        let line = shape("LineString", &path, None, Some(50), 100.0);
        let line = line.map_err(|e| e.why)?;
        // one metre of latitude, at the line's middle
        let m = (1.0 / EARTH_M).to_degrees();
        let at = |metres: f64| json!({"type": "Point", "coordinates": [24.95, 60.17 + metres * m]});
        assert!(line.keeps(line.distance(&at(49.0))));
        assert!(!line.keeps(line.distance(&at(51.0))));
        // past the end the buffer is round
        let end = json!({"type": "Point", "coordinates": [24.96 + 49.0 * m / 60.17_f64.to_radians().cos(), 60.17]});
        assert!(line.keeps(line.distance(&end)));
        let (georel, geometry, _) = line.query(None);
        assert_eq!((georel.as_str(), geometry), ("intersects", "Polygon"));
        Ok(())
    }

    #[test]
    fn rank_orders_by_distance_and_compacts() -> Result<(), String> {
        let point =
            shape("Point", &json!([24.94, 60.17]), Some(1000), None, 100.0).map_err(|e| e.why)?;
        let e = |id: &str, lat: f64| {
            json!({
                "id": id, "type": "Tree",
                "name": {"type": "Property", "value": id},
                "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.94, lat]}},
                "species": {"type": "Property", "value": "x".repeat(100)},
                "height": {"type": "Property", "value": 7, "unitCode": "MTR"},
                "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Org:1"}
            })
        };
        let rows = [e("far", 60.175), e("near", 60.171), e("mid", 60.173)];
        let (n, kept) = rank(&point, &rows, 2, None);
        assert_eq!(n, 3);
        let ids: Vec<&str> = kept.iter().filter_map(|k| k.get("id")?.as_str()).collect();
        assert_eq!(ids, ["near", "mid"]);
        let first = kept.first().cloned().unwrap_or_default();
        assert_eq!(first.pointer("/distance_m"), Some(&json!(111.0)));
        let keys: Vec<&String> = first
            .get("attributes")
            .and_then(Value::as_object)
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        assert_eq!(
            keys,
            ["height", "species"],
            "measured first, no Relationship"
        );
        let species = first
            .pointer("/attributes/species/value")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(species.chars().count(), TEXT_CHARS + 1);
        Ok(())
    }

    #[test]
    fn the_answer_drops_empty_types_and_stays_under_its_size() -> Result<(), String> {
        let point = shape("Point", &json!([24.94, 60.17]), None, None, 100.0).map_err(|e| e.why)?;
        let big = json!({"id": "urn:ngsi-ld:T:1", "pad": "x".repeat(2000)});
        let part = |t: &str, count: u64, n: usize| TypePart {
            entity_type: t.into(),
            count,
            exact: true,
            nearest: true,
            entities: vec![big.clone(); n],
        };
        let a = answer(
            &point,
            5,
            vec![part("A", 3, 3), part("Empty", 0, 0), part("B", 9, 20)],
            false,
        );
        let types = a
            .get("types")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = types
            .iter()
            .filter_map(|t| t.get("type")?.as_str())
            .collect();
        assert_eq!(names, ["B", "A"]);
        assert_eq!(a.get("truncated"), Some(&json!(true)));
        assert!(a.to_string().len() < 20 * 1024);
        // round robin: A keeps all 3 before B fills the rest
        assert_eq!(
            types
                .get(1)
                .and_then(|t| t.get("entities")?.as_array().map(Vec::len)),
            Some(3)
        );
        assert!(render(&a).starts_with("2 types in the area"));
        Ok(())
    }

    #[test]
    fn segments_cross_touch_or_miss() {
        let p = |x: f64, y: f64| [x, y];
        let o = p(0.0, 0.0);
        assert!(crosses(o, p(2.0, 2.0), p(0.0, 2.0), p(2.0, 0.0)), "an X");
        assert!(crosses(o, p(2.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)), "a T");
        assert!(
            crosses(o, p(2.0, 0.0), p(1.0, 0.0), p(3.0, 0.0)),
            "collinear overlap"
        );
        assert!(
            crosses(p(1.0, 0.0), p(3.0, 0.0), o, p(2.0, 0.0)),
            "collinear, the other way"
        );
        assert!(
            !crosses(o, p(1.0, 0.0), p(2.0, 0.0), p(3.0, 0.0)),
            "collinear apart"
        );
        assert!(
            !crosses(o, p(1.0, 0.0), p(0.0, 1.0), p(1.0, 1.0)),
            "parallel"
        );
        assert!(!crosses(o, p(1.0, 1.0), p(2.0, 0.0), p(3.0, -1.0)), "apart");
    }

    #[test]
    fn a_malformed_area_is_invalid_geometry_or_an_invalid_polygon() {
        let slug = |g: &str, c: Value| {
            shape(g, &c, None, None, DEFAULT_MAX_KM2)
                .err()
                .map(|e| e.slug)
        };
        let open = json!([[[0, 0], [0.01, 0], [0.01, 0.01], [0, 0.01]]]);
        let bow = json!([[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]]);
        assert_eq!(slug("Point", json!([1, 2, 3, 4])), Some("invalid-geometry"));
        assert_eq!(slug("Polygon", json!([5])), Some("invalid-geometry"));
        assert_eq!(slug("Polygon", json!([])), Some("invalid-geometry"));
        assert_eq!(slug("Polygon", open), Some("invalid-polygon"));
        assert_eq!(slug("MultiPolygon", json!([])), Some("invalid-geometry"));
        assert_eq!(slug("MultiPolygon", json!([bow])), Some("invalid-polygon"));
        assert_eq!(slug("Point", json!([24.9, 60.1, 12.0])), None);
    }

    #[test]
    fn an_entity_shows_what_it_has() -> Result<(), String> {
        let bare = json!({"id": "urn:ngsi-ld:T:1", "type": "T"});
        assert_eq!(compact(&bare, None, None), json!({"id": "urn:ngsi-ld:T:1"}));
        let named = json!({
            "id": "urn:ngsi-ld:T:2",
            "name": {"type": "LanguageProperty", "languageMap": {"en": "Market", "fi": "Tori"}},
            "tags": {"type": "Property", "value": ["a"]},
            "note": {"type": "Property", "value": "short"}
        });
        let fi = compact(&named, None, Some("fi"));
        assert_eq!(fi.get("name"), Some(&json!("Tori")));
        assert_eq!(
            fi.get("attributes"),
            Some(&json!({"note": {"value": "short"}}))
        );
        let numbered = json!({"id": "urn:ngsi-ld:T:3", "name": {"type": "Property", "value": 7}});
        assert_eq!(compact(&numbered, None, None).get("name"), None);
        let other = compact(&named, None, Some("de"));
        assert_eq!(other.get("name"), Some(&json!("Market")));
        let area = json!({"type": "Polygon", "coordinates": [[[24.0, 60.0], [24.0, 60.001], [24.001, 60.0], [24.0, 60.0]]]});
        let point = shape("Point", &json!([24.0, 60.0]), None, None, 100.0);
        let point = point.map_err(|e| e.why)?;
        assert_eq!(point.distance(&area), Some(0.0));
        assert_eq!(point.distance(&json!({"type": "Point"})), None);
        Ok(())
    }

    #[test]
    fn a_sample_and_a_lower_bound_say_so() -> Result<(), String> {
        let point = shape("Point", &json!([24.94, 60.17]), None, None, 100.0);
        let point = point.map_err(|e| e.why)?;
        let part = TypePart {
            entity_type: "Tree".into(),
            count: 1001,
            exact: false,
            nearest: false,
            entities: Vec::new(),
        };
        let a = answer(&point, 5, vec![part], false);
        assert_eq!(a.pointer("/types/0/ranked"), Some(&json!("sample")));
        assert!(render(&a).contains("Tree 1001+"), "{}", render(&a));
        let none = answer(&point, 5, Vec::new(), false);
        assert!(render(&none).starts_with("No Entity"), "{}", render(&none));
        Ok(())
    }
}
