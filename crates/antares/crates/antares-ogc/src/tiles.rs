// SPDX-License-Identifier: EUPL-1.2
//! OGC API - Tiles Part 1 (OGC 20-057) for each collection with a geometry (T-5121): one
//! vector tileset in `WebMercatorQuad` whose tiles are Mapbox Vector Tiles 2.1, one layer named
//! by the collection. A tile is the `items` page of the tile's box (64 tile units of buffer),
//! read through the view as every page is, so the view's rules and the Policy narrow a tile
//! exactly as they narrow a page and a hidden Attribute is never a tag.
//!
//! Decided (worker-t5121, owner may override): the tile is encoded here from the view's scan,
//! not by `ST_AsMVT`, because a surface reaches the store only through Ops (ADR-0027), the
//! view's residual and the Policy run there, and the memory store has no `PostGIS`.
//! ponytail: a store-side `ST_AsMVT` pushdown in Ops when a tile's scan dominates its p95.

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::f64::consts::PI;

/// The one tile matrix set served.
pub(crate) const TMS: &str = "WebMercatorQuad";
/// Its URI in the OGC register.
const TMS_URI: &str = "http://www.opengis.net/def/tilematrixset/OGC/1.0/WebMercatorQuad";
/// The tile media type (Tiles Part 1 `/req/mvt`).
pub(crate) const MVT: &str = "application/vnd.mapbox-vector-tile";
/// The deepest tile matrix a tileset serves; deeper ones are 404 (zoom cap).
pub(crate) const MAX_ZOOM: u8 = 22;
/// The most Features one tile holds: past it the tile holds the first ones in the store's
/// order and [`TRUNCATED`] names the cap.
pub(crate) const MAX_FEATURES: u64 = crate::items::MAX_LIMIT;
/// The largest tile body; above it the tile is 413 (zoom in).
pub(crate) const MAX_BYTES: usize = 4 << 20;
/// The header a tile cut at [`MAX_FEATURES`] carries.
const TRUNCATED: &str = "antares-tile-truncated";
/// How long a client may reuse a tile without asking again (a short cache: the `ETag` is the
/// tile's own digest, so a revalidation after a write reads the new tile).
const CACHE: &str = "private, max-age=60";

/// A collection's vector tilesets (`/req/geodata-tilesets`).
pub(crate) const TILESETS_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector";
/// The tile matrix sets of the endpoint and of a tileset (`/req/tileset/tm-links`).
pub(crate) const SCHEMES_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/tiling-schemes";
const SCHEME_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/tiling-scheme";
const GEODATA_REL: &str = "http://www.opengis.net/def/rel/ogc/1.0/geodata";
const JSON: &str = "application/json";
const EPSG_3857: &str = "http://www.opengis.net/def/crs/EPSG/0/3857";

/// Tile units across a tile (MVT 4.3.1) and the buffer read and kept around it.
const EXTENT: f64 = 4096.0;
const BUFFER: f64 = 64.0;
/// The latitude `WebMercatorQuad` reaches.
const MAX_LAT: f64 = 85.051_128_779_806_59;
/// `WebMercatorQuad` at tile matrix 0 (OGC 17-083r4 Annex D.1).
const SCALE_0: f64 = 559_082_264.028_717_2;
const CELL_0: f64 = 156_543.033_928_041;
const ORIGIN: f64 = 20_037_508.342_789_2;

/// One tile of `WebMercatorQuad`: matrix (zoom), row and column, each within the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tile {
    pub z: u8,
    pub row: u32,
    pub col: u32,
}

impl Tile {
    /// The tile a path names as `{tileMatrix}/{tileRow}/{tileCol}`; outside the set or past
    /// [`MAX_ZOOM`] none (`/req/core/tc-error`: 404).
    pub(crate) fn of(z: &str, row: &str, col: &str) -> Option<Self> {
        let z: u8 = z.parse().ok().filter(|z| *z <= MAX_ZOOM)?;
        let size = 1u32.checked_shl(u32::from(z))?;
        let row: u32 = row.parse().ok().filter(|r| *r < size)?;
        let col: u32 = col.parse().ok().filter(|c| *c < size)?;
        Some(Self { z, row, col })
    }

    fn tiles(self) -> f64 {
        2f64.powi(i32::from(self.z))
    }

    /// The box the tile and its buffer cover, `[west, south, east, north]` in CRS84.
    pub(crate) fn bbox(self) -> [f64; 4] {
        let n = self.tiles();
        let pad = BUFFER / EXTENT;
        let lon = |x: f64| (x / n * 360.0 - 180.0).clamp(-180.0, 180.0);
        let lat = |y: f64| (PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
        let (col, row) = (f64::from(self.col), f64::from(self.row));
        [
            lon(col - pad),
            lat(row + 1.0 + pad),
            lon(col + 1.0 + pad),
            lat(row - pad),
        ]
    }

    /// A CRS84 position in this tile's units, origin top left, y down.
    fn project(self, (lon, lat): Point) -> Point {
        let n = self.tiles();
        let phi = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
        let x = (lon + 180.0) / 360.0 * n;
        let y = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / PI) / 2.0 * n;
        (
            (x - f64::from(self.col)) * EXTENT,
            (y - f64::from(self.row)) * EXTENT,
        )
    }
}

type Point = (f64, f64);
type Line = Vec<Point>;

/// A Feature's geometry in tile units (`GeometryCollection` is no MVT type and is dropped).
enum Shape {
    Points(Vec<Point>),
    Lines(Vec<Line>),
    Polygons(Vec<Vec<Line>>),
}

fn position(v: &Value) -> Option<Point> {
    let a = v.as_array()?;
    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

fn list<T>(v: &Value, each: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    v.as_array()?.iter().map(each).collect()
}

fn line(v: &Value) -> Option<Line> {
    list(v, position)
}

fn shape(geometry: &Value) -> Option<Shape> {
    let c = geometry.get("coordinates")?;
    match geometry.get("type")?.as_str()? {
        "Point" => Some(Shape::Points(vec![position(c)?])),
        "MultiPoint" => list(c, position).map(Shape::Points),
        "LineString" => Some(Shape::Lines(vec![line(c)?])),
        "MultiLineString" => list(c, line).map(Shape::Lines),
        "Polygon" => Some(Shape::Polygons(vec![list(c, line)?])),
        "MultiPolygon" => list(c, |p| list(p, line)).map(Shape::Polygons),
        _ => None,
    }
}

const LO: f64 = -BUFFER;
const HI: f64 = EXTENT + BUFFER;

/// The part of segment `a`-`b` inside the buffered tile (Liang-Barsky), and whether it ends at a
/// cut.
fn segment(a: Point, b: Point) -> Option<(Point, Point, bool)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - LO),
        (dx, HI - a.0),
        (-dy, a.1 - LO),
        (dy, HI - a.1),
    ] {
        if p.abs() < f64::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }
    let at = |t: f64| (a.0 + t * dx, a.1 + t * dy);
    Some((at(t0), at(t1), t1 < 1.0))
}

/// A line clipped to the buffered tile: the pieces inside it. A piece ends where a segment
/// leaves the tile, so the next segment inside starts a new one; one that ends inside is
/// continued by the next segment, which then starts inside too.
fn clip_line(points: &[Point]) -> Vec<Line> {
    let mut pieces = Vec::new();
    let mut piece: Line = Vec::new();
    for (a, b) in points.iter().zip(points.iter().skip(1)) {
        let Some((p, q, cut_out)) = segment(*a, *b) else {
            continue;
        };
        if piece.is_empty() {
            piece.push(p);
        }
        piece.push(q);
        if cut_out {
            pieces.push(std::mem::take(&mut piece));
        }
    }
    pieces.push(piece);
    pieces.retain(|p| !p.is_empty());
    pieces
}

/// A ring clipped to the buffered tile (Sutherland-Hodgman), open: no closing position.
fn clip_ring(ring: &[Point]) -> Line {
    let open = ring.split_last().map_or(ring, |(_, rest)| rest);
    let mut out = open.to_vec();
    for edge in 0..4u8 {
        let inside = |p: Point| match edge {
            0 => p.0 >= LO,
            1 => p.0 <= HI,
            2 => p.1 >= LO,
            _ => p.1 <= HI,
        };
        let cross = |a: Point, b: Point| {
            let (vertical, at) = match edge {
                0 => (true, LO),
                1 => (true, HI),
                2 => (false, LO),
                _ => (false, HI),
            };
            if vertical {
                let t = (at - a.0) / (b.0 - a.0);
                (at, a.1 + t * (b.1 - a.1))
            } else {
                let t = (at - a.1) / (b.1 - a.1);
                (a.0 + t * (b.0 - a.0), at)
            }
        };
        let input = std::mem::take(&mut out);
        let Some(mut prev) = input.last().copied() else {
            break;
        };
        for cur in input {
            match (inside(prev), inside(cur)) {
                (true, true) => out.push(cur),
                (true, false) => out.push(cross(prev, cur)),
                (false, true) => {
                    out.push(cross(prev, cur));
                    out.push(cur);
                }
                (false, false) => {}
            }
            prev = cur;
        }
    }
    out
}

/// A rounded tile coordinate as an integer, without a cast: clipped coordinates lie within
/// `-64..=4160`, so 2^20 bounds every one.
fn whole(v: f64) -> i32 {
    let mut left = v.round().abs().min(1_048_575.0);
    let mut n = 0i32;
    for bit in (0..20u32).rev() {
        let step = 2f64.powi(i32::try_from(bit).unwrap_or(0));
        if left >= step {
            left -= step;
            n |= 1i32.wrapping_shl(bit);
        }
    }
    if v < 0.0 {
        n.wrapping_neg()
    } else {
        n
    }
}

fn zigzag(n: i32) -> u32 {
    (n.wrapping_shl(1) ^ n.wrapping_shr(31)).cast_unsigned()
}

fn zigzag64(n: i64) -> u64 {
    (n.wrapping_shl(1) ^ n.wrapping_shr(63)).cast_unsigned()
}

/// The geometry commands of one Feature (MVT 4.3), deltas from the pen's last position.
#[derive(Default)]
struct Pen {
    at: (i32, i32),
    out: Vec<u32>,
}

impl Pen {
    fn command(&mut self, id: u32, count: usize) {
        let count = u32::try_from(count).unwrap_or(0);
        self.out.push(count.saturating_mul(8) | id);
    }

    fn to(&mut self, p: (i32, i32)) {
        self.out.push(zigzag(p.0.wrapping_sub(self.at.0)));
        self.out.push(zigzag(p.1.wrapping_sub(self.at.1)));
        self.at = p;
    }

    /// `MoveTo` the first position, `LineTo` the rest; `ClosePath` for a ring.
    fn path(&mut self, first: (i32, i32), rest: &[(i32, i32)], ring: bool) {
        self.command(1, 1);
        self.to(first);
        self.command(2, rest.len());
        for p in rest {
            self.to(*p);
        }
        if ring {
            self.command(7, 1);
        }
    }
}

/// Positions in whole tile units, a repeat of the previous one dropped.
fn quantized(points: &[Point]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(points.len());
    for p in points {
        let q = (whole(p.0), whole(p.1));
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    out
}

/// Twice the signed area (surveyor's formula in tile units, y down: positive is clockwise).
fn area(ring: &[(i32, i32)]) -> i64 {
    let next = ring.iter().skip(1).chain(ring.first());
    ring.iter().zip(next).fold(0i64, |sum, (a, b)| {
        let term = i64::from(a.0)
            .saturating_mul(i64::from(b.1))
            .saturating_sub(i64::from(b.0).saturating_mul(i64::from(a.1)));
        sum.saturating_add(term)
    })
}

/// The geometry type and commands of a Feature in `tile`, none when nothing of it is inside.
fn geometry(tile: Tile, geometry: &Value) -> Option<(u32, Vec<u32>)> {
    let project = |points: &[Point]| -> Line { points.iter().map(|p| tile.project(*p)).collect() };
    let mut pen = Pen::default();
    let kind = match shape(geometry)? {
        Shape::Points(points) => {
            let kept: Vec<(i32, i32)> = project(&points)
                .into_iter()
                .filter(|p| (LO..=HI).contains(&p.0) && (LO..=HI).contains(&p.1))
                .map(|p| (whole(p.0), whole(p.1)))
                .collect();
            if !kept.is_empty() {
                pen.command(1, kept.len());
                for p in kept {
                    pen.to(p);
                }
            }
            1
        }
        Shape::Lines(lines) => {
            for l in lines {
                for piece in clip_line(&project(&l)) {
                    // A piece shorter than a tile unit is one position: no line.
                    let piece = quantized(&piece);
                    if let Some((first, rest)) = piece.split_first().filter(|(_, r)| !r.is_empty())
                    {
                        pen.path(*first, rest, false);
                    }
                }
            }
            2
        }
        Shape::Polygons(polygons) => {
            for rings in polygons {
                for (i, ring) in rings.iter().enumerate() {
                    let mut ring = quantized(&clip_ring(&project(ring)));
                    // Rounding can bring the last position onto the first.
                    if ring.first() == ring.last() {
                        ring.pop();
                    }
                    let a = area(&ring);
                    // A ring that has no area in whole tile units is gone; an exterior one drops
                    // its holes too.
                    let Some((first, rest)) = ring.split_first().filter(|_| a != 0) else {
                        if i == 0 {
                            break;
                        }
                        continue;
                    };
                    // MVT 4.3.4.4: exterior rings positive (clockwise), interior negative.
                    let mut rest = rest.to_vec();
                    if (i == 0) != (a > 0) {
                        rest.reverse();
                    }
                    pen.path(*first, &rest, true);
                }
            }
            3
        }
    };
    (!pen.out.is_empty()).then_some((kind, pen.out))
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let low = u8::try_from(v & 0x7f).unwrap_or(0);
        v = v.wrapping_shr(7);
        if v == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn key(out: &mut Vec<u8>, field: u64, wire: u64) {
    varint(out, field.wrapping_shl(3) | wire);
}

fn bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    key(out, field, 2);
    varint(out, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    out.extend_from_slice(bytes);
}

fn packed(out: &mut Vec<u8>, field: u64, values: &[u32]) {
    let mut body = Vec::with_capacity(values.len());
    for v in values {
        varint(&mut body, u64::from(*v));
    }
    bytes_field(out, field, &body);
}

/// A property value as an MVT `Value` message; an array or object is its JSON text, a null
/// none.
fn value(v: &Value) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match v {
        Value::Null => return None,
        Value::Bool(b) => {
            key(&mut out, 7, 0);
            varint(&mut out, u64::from(*b));
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                key(&mut out, 6, 0);
                varint(&mut out, zigzag64(i));
            } else if let Some(u) = n.as_u64() {
                key(&mut out, 5, 0);
                varint(&mut out, u);
            } else {
                key(&mut out, 3, 1);
                out.extend_from_slice(&n.as_f64().unwrap_or(0.0).to_le_bytes());
            }
        }
        Value::String(s) => bytes_field(&mut out, 1, s.as_bytes()),
        other => bytes_field(&mut out, 1, other.to_string().as_bytes()),
    }
    Some(out)
}

/// The keys and values of a layer, each written once and named by its index.
#[derive(Default)]
struct Table {
    keys: Vec<String>,
    key_at: HashMap<String, u32>,
    values: Vec<Vec<u8>>,
    value_at: HashMap<Vec<u8>, u32>,
}

impl Table {
    fn index<K: std::hash::Hash + Eq + Clone>(
        all: &mut Vec<K>,
        at: &mut HashMap<K, u32>,
        k: &K,
    ) -> u32 {
        if let Some(i) = at.get(k) {
            return *i;
        }
        let i = u32::try_from(all.len()).unwrap_or(u32::MAX);
        all.push(k.clone());
        at.insert(k.clone(), i);
        i
    }

    /// The tags of a Feature's properties: its URN as `id`, then each property but a null.
    fn tags(&mut self, id: Option<&Value>, properties: Option<&Map<String, Value>>) -> Vec<u32> {
        let mut tags = Vec::new();
        let pairs = id.map(|id| ("id", id)).into_iter().chain(
            properties
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.as_str(), v)),
        );
        for (name, v) in pairs {
            let Some(encoded) = value(v) else { continue };
            tags.push(Self::index(
                &mut self.keys,
                &mut self.key_at,
                &name.to_owned(),
            ));
            tags.push(Self::index(&mut self.values, &mut self.value_at, &encoded));
        }
        tags
    }
}

/// The tile of `features` (`GeoJSON` Features of the collection `layer`): one MVT layer with
/// the Features that reach the buffered tile, `None` when none does (an empty tile).
pub(crate) fn encode(layer: &str, tile: Tile, features: &[Value]) -> Option<Vec<u8>> {
    let mut table = Table::default();
    let mut body = Vec::new();
    key(&mut body, 15, 0);
    varint(&mut body, 2);
    bytes_field(&mut body, 1, layer.as_bytes());
    let mut any = false;
    for f in features {
        let Some((kind, commands)) = f.get("geometry").and_then(|g| geometry(tile, g)) else {
            continue;
        };
        let tags = table.tags(f.get("id"), f.get("properties").and_then(Value::as_object));
        let mut feature = Vec::new();
        if !tags.is_empty() {
            packed(&mut feature, 2, &tags);
        }
        key(&mut feature, 3, 0);
        varint(&mut feature, u64::from(kind));
        packed(&mut feature, 4, &commands);
        bytes_field(&mut body, 2, &feature);
        any = true;
    }
    if !any {
        return None;
    }
    for k in &table.keys {
        bytes_field(&mut body, 3, k.as_bytes());
    }
    for v in &table.values {
        bytes_field(&mut body, 4, v);
    }
    key(&mut body, 5, 0);
    varint(&mut body, 4096);
    let mut tile = Vec::with_capacity(body.len().saturating_add(8));
    bytes_field(&mut tile, 3, &body);
    Some(tile)
}

/// The answer for a tile of `features`: `204` when none reaches it, `413` above `max` bytes,
/// `304` when the client holds its `ETag` (`held`: the `If-None-Match` values), else the tile
/// with its `ETag`, a short `Cache-Control` and, when the page was cut, [`TRUNCATED`].
pub(crate) fn reply(
    layer: &str,
    tile: Tile,
    features: &[Value],
    truncated: bool,
    held: &[&str],
    max: usize,
) -> Response {
    let Some(body) = encode(layer, tile, features) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if body.len() > max {
        return crate::problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the tile is above 4 MiB: ask a deeper tile matrix",
        );
    }
    let tag = etag(&body);
    let fresh = held
        .iter()
        .flat_map(|v| v.split(','))
        .any(|t| [tag.as_str(), "*"].contains(&t.trim()));
    // A weak digest in hex is a header value.
    let mut headers = vec![
        (
            ETAG,
            HeaderValue::from_str(&tag).unwrap_or(HeaderValue::from_static("W/\"0\"")),
        ),
        (CACHE_CONTROL, HeaderValue::from_static(CACHE)),
    ];
    if fresh {
        return (StatusCode::NOT_MODIFIED, HeaderMap::from_iter(headers)).into_response();
    }
    headers.push((CONTENT_TYPE, HeaderValue::from_static(MVT)));
    headers.extend(truncated.then_some((HeaderName::from_static(TRUNCATED), MAX_FEATURES.into())));
    (StatusCode::OK, HeaderMap::from_iter(headers), body).into_response()
}

/// A weak `ETag` of a tile body: the tile's own digest, so it changes when the data does.
fn etag(body: &[u8]) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    format!("W/\"{:016x}\"", h.finish())
}

fn link(rel: &str, media: &str, href: &str) -> Value {
    json!({"rel": rel, "type": media, "href": href})
}

/// `tileMatrixSetLimits`: every tile of each matrix up to [`MAX_ZOOM`].
fn limits() -> Vec<Value> {
    (0..=MAX_ZOOM)
        .map(|z| {
            let last = 1u32
                .checked_shl(u32::from(z))
                .map_or(0, |n| n.saturating_sub(1));
            json!({"tileMatrix": z.to_string(), "minTileRow": 0, "maxTileRow": last, "minTileCol": 0, "maxTileCol": last})
        })
        .collect()
}

/// The tileset of a collection at `at` (`.../collections/{c}`) under the endpoint `base`
/// (`/req/tileset`): metadata, the tile template and the matrix set.
pub(crate) fn tileset(base: &str, at: &str, collection: &str) -> Value {
    let own = format!("{at}/tiles/{TMS}");
    json!({
        "title": format!("{collection} vector tiles"),
        "dataType": "vector",
        "crs": EPSG_3857,
        "tileMatrixSetURI": TMS_URI,
        "tileMatrixSetLimits": limits(),
        "layers": [{"id": collection, "dataType": "vector", "minTileMatrix": "0", "maxTileMatrix": MAX_ZOOM.to_string()}],
        "links": [
            link("self", JSON, &own),
            link(SCHEME_REL, JSON, &format!("{base}/tileMatrixSets/{TMS}")),
            link(GEODATA_REL, JSON, at),
            {"rel": "item", "type": MVT, "templated": true,
             "href": format!("{own}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}")},
        ],
    })
}

/// A collection's tilesets (`/req/tilesets-list`): its one `WebMercatorQuad` tileset.
pub(crate) fn tilesets(base: &str, at: &str, collection: &str) -> Value {
    json!({
        "links": [link("self", JSON, &format!("{at}/tiles"))],
        "tilesets": [{
            "title": format!("{collection} vector tiles"),
            "dataType": "vector",
            "crs": EPSG_3857,
            "tileMatrixSetURI": TMS_URI,
            "links": [
                link("self", JSON, &format!("{at}/tiles/{TMS}")),
                link(SCHEME_REL, JSON, &format!("{base}/tileMatrixSets/{TMS}")),
            ],
        }],
    })
}

/// `/tileMatrixSets`: the one set served.
pub(crate) fn schemes(base: &str) -> Value {
    let at = format!("{base}/tileMatrixSets");
    json!({
        "links": [link("self", JSON, &at)],
        "tileMatrixSets": [{
            "id": TMS, "title": "Google Maps Compatible for the World", "uri": TMS_URI,
            "links": [link("self", JSON, &format!("{at}/{TMS}"))],
        }],
    })
}

/// `/tileMatrixSets/WebMercatorQuad` as OGC 17-083r4 Annex D.1 defines it (matrices 0 to 24).
pub(crate) fn scheme() -> Value {
    let matrices: Vec<Value> = (0..=24u8)
        .map(|z| {
            let n = 2f64.powi(i32::from(z));
            let size = 1u32.checked_shl(u32::from(z)).unwrap_or(0);
            json!({
                "id": z.to_string(),
                "scaleDenominator": SCALE_0 / n,
                "cellSize": CELL_0 / n,
                "cornerOfOrigin": "topLeft",
                "pointOfOrigin": [-ORIGIN, ORIGIN],
                "tileWidth": 256, "tileHeight": 256,
                "matrixWidth": size, "matrixHeight": size,
            })
        })
        .collect();
    json!({
        "id": TMS,
        "title": "Google Maps Compatible for the World",
        "uri": TMS_URI,
        "crs": EPSG_3857,
        "orderedAxes": ["X", "Y"],
        "wellKnownScaleSet": "http://www.opengis.net/def/wkss/OGC/1.0/GoogleMapsCompatible",
        "tileMatrices": matrices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: Tile = Tile {
        z: 0,
        row: 0,
        col: 0,
    };

    fn kind(tile: Tile, g: &Value) -> Option<u32> {
        geometry(tile, g).map(|(k, _)| k)
    }

    fn unzig(v: u32) -> i32 {
        v.wrapping_shr(1).cast_signed() ^ (v & 1).cast_signed().wrapping_neg()
    }

    /// The positions of a one-ring polygon's commands (`MoveTo` 1, `LineTo` 3, `ClosePath`).
    fn square(commands: &[u32]) -> Vec<(i32, i32)> {
        let at = |i: usize| unzig(commands.get(i).copied().unwrap_or(0));
        let mut pen = (0i32, 0i32);
        [(1, 2), (4, 5), (6, 7), (8, 9)]
            .into_iter()
            .map(|(x, y)| {
                pen = (pen.0.wrapping_add(at(x)), pen.1.wrapping_add(at(y)));
                pen
            })
            .collect()
    }

    fn polygon(rings: &Value) -> Value {
        json!({"type": "Polygon", "coordinates": rings})
    }

    fn closes(commands: &[u32]) -> usize {
        commands.iter().filter(|c| **c == 15).count()
    }

    #[test]
    fn a_path_names_a_tile_of_the_set_only() {
        assert_eq!(
            Tile::of("2", "3", "1"),
            Some(Tile {
                z: 2,
                row: 3,
                col: 1
            })
        );
        for (z, row, col) in [
            ("2", "4", "0"),
            ("2", "0", "4"),
            ("23", "0", "0"),
            ("x", "0", "0"),
            ("-1", "0", "0"),
        ] {
            assert_eq!(Tile::of(z, row, col), None, "{z}/{row}/{col}");
        }
    }

    #[test]
    fn the_box_is_the_tile_and_its_buffer() {
        let [w, s, e, n] = WORLD.bbox();
        assert_eq!((w, e, n > 85.0, s < -85.0), (-180.0, 180.0, true, true));
        // Bratislava (17.1, 48.1) at matrix 10 is column 560, row 355.
        let [w, s, e, n] = Tile {
            z: 10,
            row: 355,
            col: 560,
        }
        .bbox();
        assert_eq!(
            (w < 17.1, 17.1 < e, s < 48.1, 48.1 < n),
            (true, true, true, true)
        );
    }

    #[test]
    fn each_geojson_geometry_has_its_mvt_type() {
        let line = json!([[0, 0], [10, 10]]);
        let ring = json!([[0, 0], [10, 0], [10, 10], [0, 0]]);
        for (g, want) in [
            (json!({"type": "Point", "coordinates": [0, 0]}), Some(1)),
            (
                json!({"type": "MultiPoint", "coordinates": [[0, 0], [1, 1]]}),
                Some(1),
            ),
            (json!({"type": "LineString", "coordinates": line}), Some(2)),
            (
                json!({"type": "MultiLineString", "coordinates": [line]}),
                Some(2),
            ),
            (polygon(&json!([ring])), Some(3)),
            (
                json!({"type": "MultiPolygon", "coordinates": [[ring]]}),
                Some(3),
            ),
            (
                json!({"type": "GeometryCollection", "geometries": []}),
                None,
            ),
            (
                json!({"type": "GeometryCollection", "coordinates": []}),
                None,
            ),
            (json!({"type": "Point", "coordinates": ["x", 0]}), None),
        ] {
            assert_eq!(kind(WORLD, &g), want, "{g}");
        }
    }

    #[test]
    fn a_point_is_a_move_to_and_one_outside_is_dropped() {
        let point = json!({"type": "Point", "coordinates": [0.0, 0.0]});
        assert_eq!(
            geometry(WORLD, &point),
            Some((1, vec![9, zigzag(2048), zigzag(2048)]))
        );
        let far = Tile {
            z: 10,
            row: 0,
            col: 0,
        };
        assert_eq!(geometry(far, &point), None);
        let two = json!({"type": "MultiPoint", "coordinates": [[0.0, 0.0], [-179.9, 85.04]]});
        assert_eq!(geometry(far, &two).map(|(_, c)| c.len()), Some(3));
    }

    #[test]
    fn a_line_is_clipped_to_the_buffered_tile() {
        assert_eq!(
            clip_line(&[(-1000.0, 100.0), (5000.0, 100.0)]),
            vec![vec![(LO, 100.0), (HI, 100.0)]]
        );
        // Out, in, out, in: two pieces.
        let pieces = clip_line(&[
            (100.0, -500.0),
            (100.0, 100.0),
            (100.0, 9000.0),
            (200.0, 100.0),
        ]);
        assert_eq!(pieces.len(), 2);
        // Inside: one piece of every position.
        assert_eq!(
            clip_line(&[(1.0, 1.0), (2.0, 2.0), (3.0, 1.0)])
                .first()
                .map(Vec::len),
            Some(3)
        );
        // Parallel to an edge outside it, and past the far edge: nothing.
        for outside in [
            [(-500.0, 0.0), (-500.0, 100.0)],
            [(5000.0, 100.0), (6000.0, 100.0)],
            [(100.0, 5000.0), (100.0, 6000.0)],
            [(5000.0, 100.0), (4900.0, 100.0)],
        ] {
            assert_eq!(clip_line(&outside), Vec::<Line>::new());
        }
    }

    #[test]
    fn a_line_shorter_than_a_tile_unit_is_left_out() {
        let tiny = json!({"type": "LineString", "coordinates": [[0, 0], [0.000_01, 0]]});
        assert_eq!(geometry(WORLD, &tiny), None);
        let line = json!({"type": "LineString", "coordinates": [[0, 0], [10, 0], [10, 10]]});
        let commands = geometry(WORLD, &line).map(|(_, c)| c).unwrap_or_default();
        assert_eq!((commands.len(), closes(&commands)), (8, 0));
    }

    #[test]
    fn repeated_positions_are_dropped() {
        assert_eq!(
            quantized(&[(0.1, 0.1), (0.2, 0.2), (5.0, 5.0)]),
            vec![(0, 0), (5, 5)]
        );
        for (v, n) in [
            (0.0, 0),
            (0.4, 0),
            (0.6, 1),
            (-64.0, -64),
            (4160.2, 4160),
            (-1.5, -2),
        ] {
            assert_eq!(whole(v), n, "{v}");
        }
    }

    #[test]
    fn rings_are_clipped_and_wound_as_mvt_wants() {
        let ring = [
            (-100.0, -100.0),
            (5000.0, -100.0),
            (5000.0, 5000.0),
            (-100.0, 5000.0),
            (-100.0, -100.0),
        ];
        assert_eq!(
            quantized(&clip_ring(&ring)),
            vec![(-64, 4160), (-64, -64), (4160, -64), (4160, 4160)]
        );
        // Either winding in CRS84 is an exterior ring of positive area in tile units.
        for exterior in [
            json!([[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]]),
            json!([[0, 0], [0, 10], [10, 10], [10, 0], [0, 0]]),
        ] {
            let commands = geometry(WORLD, &polygon(&json!([exterior])))
                .map(|(_, c)| c)
                .unwrap_or_default();
            let positions = square(&commands);
            assert_eq!(
                (commands.len(), area(&positions) > 0),
                (11, true),
                "{exterior}"
            );
        }
    }

    #[test]
    fn a_ring_wholly_outside_is_empty() {
        let left = [(-900.0, 0.0), (-800.0, 0.0), (-800.0, 100.0), (-900.0, 0.0)];
        assert_eq!(clip_ring(&left), Vec::<Point>::new());
    }

    #[test]
    fn a_last_position_rounded_onto_the_first_is_dropped() {
        let ring = json!([[0, 0], [10, 0], [10, 10], [0, 0.000_01], [0, 0]]);
        let commands = geometry(WORLD, &polygon(&json!([ring]))).map(|(_, c)| c.len());
        assert_eq!(commands, Some(9));
    }

    #[test]
    fn a_hole_is_kept_and_a_vanished_ring_left_out() {
        let outer = json!([[0, 0], [20, 0], [20, 20], [0, 20], [0, 0]]);
        let tiny = json!([[5, 5], [5.000_01, 5], [5.000_01, 5.000_01], [5, 5]]);
        for (hole, want) in [
            (json!([[5, 5], [5, 15], [15, 15], [15, 5], [5, 5]]), 2),
            (json!([[5, 5], [15, 5], [15, 15], [5, 15], [5, 5]]), 2),
            (tiny.clone(), 1),
        ] {
            let commands = geometry(WORLD, &polygon(&json!([outer, hole])))
                .map(|(_, c)| c)
                .unwrap_or_default();
            assert_eq!(closes(&commands), want);
        }
        // A vanished exterior drops its hole too; nothing else is left.
        assert_eq!(geometry(WORLD, &polygon(&json!([tiny, outer]))), None);
    }

    #[test]
    fn each_value_is_its_mvt_value() {
        let first = |v: Value| value(&v).and_then(|b| b.first().copied());
        assert_eq!(first(json!("red")), Some(0x0a));
        assert_eq!(first(json!(0.5)), Some(0x19));
        assert_eq!(first(json!(-3)), Some(0x30));
        assert_eq!(first(json!(u64::MAX)), Some(0x28));
        assert_eq!(first(json!(true)), Some(0x38));
        assert_eq!(first(json!(["a"])), Some(0x0a));
        assert_eq!(first(Value::Null), None);
        assert_eq!(zigzag64(-1), 1);
    }

    #[test]
    fn a_tile_without_a_feature_inside_is_none() {
        let point = json!({"type": "Point", "coordinates": [17.1, 48.1]});
        let bare = json!({"type": "Feature", "geometry": point, "properties": {"gone": null}});
        assert_eq!(
            encode("Car", WORLD, std::slice::from_ref(&bare)).map(|b| b.is_empty()),
            Some(false)
        );
        let far = Tile {
            z: 10,
            row: 0,
            col: 0,
        };
        assert_eq!(encode("Car", far, &[bare]), None);
        let no_geometry = json!({"type": "Feature", "id": "x", "geometry": null, "properties": {}});
        assert_eq!(encode("Car", WORLD, &[no_geometry]), None);
    }

    #[test]
    fn a_tile_answer_carries_its_cache_headers_and_caps() {
        let f = json!({"type": "Feature", "id": "urn:ngsi-ld:Car:1",
                       "geometry": {"type": "Point", "coordinates": [17.1, 48.1]}, "properties": {"speed": 10}});
        let features = std::slice::from_ref(&f);
        let header = |r: &Response, name: &str| {
            r.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let ok = reply("Car", WORLD, features, false, &[], MAX_BYTES);
        let tag = header(&ok, "etag").unwrap_or_default();
        assert_eq!(
            (
                ok.status(),
                header(&ok, "cache-control"),
                header(&ok, TRUNCATED)
            ),
            (StatusCode::OK, Some(CACHE.to_owned()), None)
        );
        let cut = reply("Car", WORLD, features, true, &["W/\"other\""], MAX_BYTES);
        assert_eq!(
            (cut.status(), header(&cut, TRUNCATED)),
            (StatusCode::OK, Some("10000".to_owned()))
        );
        for held in [tag.as_str(), "*", "W/\"a\", *"] {
            let same = reply("Car", WORLD, features, false, &[held], MAX_BYTES);
            assert_eq!(same.status(), StatusCode::NOT_MODIFIED, "{held}");
        }
        let big = reply("Car", WORLD, features, false, &[], 1);
        assert_eq!(big.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let none = reply("Car", WORLD, &[], false, &[], MAX_BYTES);
        assert_eq!(none.status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn the_matrix_set_matches_the_ogc_definition() {
        let s = scheme();
        assert_eq!(
            s.pointer("/tileMatrices/0/scaleDenominator"),
            Some(&json!(SCALE_0))
        );
        assert_eq!(
            s.pointer("/tileMatrices/24/matrixWidth"),
            Some(&json!(16_777_216))
        );
        assert_eq!(limits().len(), 23);
        let t = tileset("http://b", "http://b/collections/Car", "Car");
        assert_eq!(
            t.pointer("/links/3/href"),
            Some(&json!(
                "http://b/collections/Car/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"
            ))
        );
        assert_eq!(
            tilesets("http://b", "http://b/collections/Car", "Car").pointer("/tilesets/0/dataType"),
            Some(&json!("vector"))
        );
        assert_eq!(
            schemes("http://b").pointer("/tileMatrixSets/0/id"),
            Some(&json!(TMS))
        );
    }
}
