// SPDX-License-Identifier: EUPL-1.2
//! Geoquery Language (CIM 009 clause 4.10) compiled to PostGIS
//! over the extracted `entities.location` column (GIST-indexed).
//!
//! Same one-directional contract as the other compilers: this may only
//! NARROW, and `antares_ql::geo::GeoQuery::matches` stays the arbiter. Three
//! places where the two engines could disagree are handled by deliberately
//! widening rather than by hoping they agree:
//!
//! 1. **Rows without an extracted geometry.** `location` holds the DEFAULT
//!    GeoProperty and only when the entity carries exactly one instance of it
//!    (clause 4.5.5 multi-instance sets have no single-geometry spelling, and a
//!    GEOMETRYCOLLECTION would make `within` mean "all of them", which is
//!    stricter). Rows that CARRY the geoproperty but defeated extraction are
//!    flagged `location_ambiguous` at write time; every predicate ORs that
//!    flag, so those rows reach the evaluator, while rows with no geoproperty
//!    at all (which can never match) are excluded in SQL. Both OR arms are
//!    index-shaped (GIST + partial index → BitmapOr).
//! 2. **`near` metric.** The evaluator measures haversine on a sphere;
//!    PostGIS `geography` measures on the WGS84 spheroid. They differ by up
//!    to ~0.5 %, which at the boundary of a radius is the difference between
//!    keeping and losing a matching row — so the compiled radius is inflated
//!    and a compiled `minDistance` floor is deflated.
//! 3. **`near` against an extended query geometry.** The evaluator measures
//!    from the query geometry's FIRST vertex; PostGIS measures from its
//!    nearest point. For `maxDistance` that only widens, but for
//!    `minDistance` it narrows — so `near` compiles only for a Point query
//!    geometry, where the two are the same point by definition.
//!
//! 4. **Row-level security.** No PostGIS operator is leakproof, so under the
//!    tables' RLS the GIST index on `location` is never an index condition
//!    for the broker's role and every geoquery read its whole Tenant
//!    (T-4852). Each predicate is therefore ANDed with a geohash cover of its
//!    region over `location_cell` (migration 0015), btree text comparisons
//!    that are leakproof; the cover is a superset (`cell_cover`), the PostGIS
//!    predicate still narrows it, and the evaluator still decides.
//!
//! The query geometry travels as bound GeoJSON text (`ST_GeomFromGeoJSON
//! ($n)`), never as SQL text, and so does every distance.

use serde_json::Value;

/// A compiled geoquery: a SQL boolean expression plus its binds, numbered
/// from the offset passed to [`compile_geo`].
pub struct CompiledGeo {
    pub sql: String,
    /// GeoJSON documents, in placeholder order.
    pub geo_binds: Vec<String>,
    /// Distances in metres, in placeholder order (after the geo binds).
    pub num_binds: Vec<f64>,
    /// Geohash cell bounds of the region's cover, in placeholder order
    /// (after the numeric binds); empty when no cover narrows the predicate.
    pub cell_binds: Vec<String>,
}

pub use antares_store::filter::{GeoSpec, Rel, LOCATION_IRI};

/// Compile a geoquery over `col` (a `geometry(Geometry,4326)`).
///
/// A `geoproperty` other than the default `location` has no extracted column,
/// so it returns `None` and the evaluator does the work (the documented
/// fallback). `first_bind` numbers the geo binds; numeric binds follow them.
pub fn compile_geo(
    spec: &GeoSpec<'_>,
    col: &'static str,
    first_bind: usize,
) -> Option<CompiledGeo> {
    if !spec.geoproperty_iri.is_empty() && spec.geoproperty_iri != LOCATION_IRI {
        return None;
    }
    // The un-extractable row always survives (module docs, point 1) — via the
    // `location_ambiguous` column, not `location IS NULL`: rows WITHOUT any
    // default GeoProperty can never match and are excluded in SQL, and the OR
    // over two indexable conditions BitmapOrs (GIST + partial index) instead
    // of forcing a sequential scan.
    let mut c = predicate(spec, col, first_bind)?;
    // The cover is the leakproof index path (module docs, point 4); the
    // PostGIS predicate stays beside it as the narrowing filter.
    let pred = match cell_cover(spec) {
        Some(cells) => {
            let n = first_bind
                .saturating_add(c.geo_binds.len())
                .saturating_add(c.num_binds.len());
            let (cover, binds) = cover_sql(&cells, &format!("{col}_cell"), n);
            c.cell_binds = binds;
            format!("{cover} AND ({})", c.sql)
        }
        None => c.sql,
    };
    c.sql = format!("(({pred}) OR location_ambiguous)");
    Some(c)
}

/// The same predicate over a PER-INSTANCE geometry column
/// (`attr_instances.geo_value`, 5.7.4.4 S3): each row is one instance, so
/// there is no ambiguity flag — a NULL `geo_value` (a value the extractor
/// could not take) is the "reaches the evaluator" arm instead. No geoproperty
/// restriction: the caller binds the attr IRI itself.
pub fn compile_geo_instance(
    spec: &GeoSpec<'_>,
    col: &'static str,
    first_bind: usize,
) -> Option<CompiledGeo> {
    let mut c = predicate(spec, col, first_bind)?;
    c.sql = format!("(({}) OR {col} IS NULL)", c.sql);
    Some(c)
}

fn predicate(spec: &GeoSpec<'_>, col: &str, first_bind: usize) -> Option<CompiledGeo> {
    let GeoSpec {
        rel,
        geometry,
        coordinates,
        ..
    } = spec;
    let geojson = serde_json::to_string(&serde_json::json!({
        "type": geometry, "coordinates": coordinates
    }))
    .ok()?;

    let geo_binds = vec![geojson];
    let mut num_binds: Vec<f64> = Vec::new();
    // placeholder for the single geometry bind
    let g = format!("ST_SetSRID(ST_GeomFromGeoJSON(${first_bind}), 4326)");

    let pred = match rel {
        Rel::Near { max, min } => {
            if *geometry != "Point" {
                return None; // see module docs, point 3
            }
            // 4.10 PositiveNumber is an RFC 8259 Number, but `inf` and `NaN`
            // both parse as `f64`: inflating one gives a bound that EXCLUDES
            // every row rather than widening. Refuse instead of narrowing.
            if [max, min].into_iter().flatten().any(|d| !d.is_finite()) {
                return None;
            }
            let mut parts = Vec::new();
            // Numeric binds are numbered after ALL geo binds; there is exactly
            // one geo bind here, hence the +1 base.
            if let Some(m) = max {
                let n = first_bind.saturating_add(1).saturating_add(num_binds.len());
                // widen: spheroid-vs-sphere slack, plus a metre of absolute
                // slack so a 0 m radius still behaves
                num_binds.push(m * 1.005 + 1.0);
                parts.push(format!(
                    "ST_DWithin({col}::geography, {g}::geography, ${n})"
                ));
            }
            if let Some(m) = min {
                let n = first_bind.saturating_add(1).saturating_add(num_binds.len());
                num_binds.push((m * 0.995 - 1.0).max(0.0));
                parts.push(format!(
                    "ST_Distance({col}::geography, {g}::geography) >= ${n}"
                ));
            }
            if parts.is_empty() {
                return None; // `near` with neither bound is not a filter
            }
            // T-6056: a bounding-box test before the geography distance,
            // which casts and measures every row the cover read (~45 % of
            // the distinct-1000 rig's statement time); `&&` compares boxes
            // rounded outwards, so it only drops rows outside the radius.
            // Only a box inside [-180, 180) x [-90, 90), as `cell_cover`
            // keeps: past the antimeridian `&&` would drop a point on the
            // other side of it
            if let Some((x0, y0, x1, y1)) = max
                .and_then(|m| near_box(coordinates, m))
                .map(|(x0, y0, x1, y1)| (x0 - EDGE, y0 - EDGE, x1 + EDGE, y1 + EDGE))
                .filter(|&(x0, y0, x1, y1)| in_range(x0, y0, x1, y1))
            {
                let n = first_bind.saturating_add(1).saturating_add(num_binds.len());
                num_binds.extend([x0, y0, x1, y1]);
                parts.insert(
                    0,
                    format!(
                        "{col} && ST_MakeEnvelope(${n}, ${}, ${}, ${}, 4326)",
                        n.saturating_add(1),
                        n.saturating_add(2),
                        n.saturating_add(3)
                    ),
                );
            }
            parts.join(" AND ")
        }
        Rel::Within => format!("ST_Within({col}, {g})"),
        Rel::Contains => format!("ST_Contains({col}, {g})"),
        Rel::Intersects => format!("ST_Intersects({col}, {g})"),
        Rel::Disjoint => format!("ST_Disjoint({col}, {g})"),
        Rel::Overlaps => format!("ST_Overlaps({col}, {g})"),
        Rel::Equals => format!("ST_Equals({col}, {g})"),
    };
    Some(CompiledGeo {
        sql: pred,
        geo_binds,
        num_binds,
        cell_binds: Vec::new(),
    })
}

/// Cells a cover may hold. Eight keeps the statement at most 8 range probes
/// plus one prefix probe; a region wider than eight cells of the coarsest
/// precision (45° x 45°) gets no cover and reads the Tenant, as before.
const MAX_COVER_CELLS: f64 = 8.0;

/// Degrees a cover reaches past the region on every side, so a point on the
/// region's closed edge also lies inside a half-open geohash cell of it.
const EDGE: f64 = 1e-7;

const BASE32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// The geohash of a WGS84 point at `chars` characters, the encoding of
/// PostGIS `ST_GeoHash`: longitude bit first, a coordinate on a split goes
/// to the upper half.
pub fn geohash(lon: f64, lat: f64, chars: usize) -> String {
    let (mut x, mut y) = ((-180.0_f64, 180.0_f64), (-90.0_f64, 90.0_f64));
    let mut out = String::with_capacity(chars);
    let mut on_lon = true;
    while out.len() < chars {
        let mut ch = 0_usize;
        for _ in 0..5 {
            let (r, v) = if on_lon { (&mut x, lon) } else { (&mut y, lat) };
            let mid = (r.0 + r.1) / 2.0;
            ch <<= 1;
            if v >= mid {
                ch |= 1;
                r.0 = mid;
            } else {
                r.1 = mid;
            }
            on_lon = !on_lon;
        }
        // five bits: always inside the alphabet
        if let Some(b) = BASE32.get(ch) {
            out.push(char::from(*b));
        }
    }
    out
}

/// The geohash cells of one precision that tile the region's bounding box,
/// or `None` when no cover can narrow (`disjoint`, `near` without a maximum,
/// a box across the antimeridian or a pole, more than `MAX_COVER_CELLS`).
/// A SUPERSET: a stored geometry the predicate can match touches the box,
/// and its `location_cell` (migration 0015: the cell holding its whole
/// bounding box, `''` when none does) then either lies inside one of these
/// cells or is a prefix of one.
fn cell_cover(spec: &GeoSpec<'_>) -> Option<Vec<String>> {
    let (x0, y0, x1, y1) = match &spec.rel {
        Rel::Disjoint | Rel::Near { max: None, .. } => return None,
        Rel::Near { max: Some(m), .. } => near_box(spec.coordinates, *m)?,
        _ => bbox(spec.coordinates)?,
    };
    let (x0, y0, x1, y1) = (x0 - EDGE, y0 - EDGE, x1 + EDGE, y1 + EDGE);
    if !in_range(x0, y0, x1, y1) {
        return None;
    }
    // the finest precision whose cells tile the box in at most eight
    let mut best = None;
    for chars in 1..=12_u32 {
        let w = 360.0 / f64::from(1_u32 << (chars.saturating_mul(5).div_ceil(2)).min(31));
        let h = 180.0 / f64::from(1_u32 << (chars.saturating_mul(5) / 2).min(31));
        let span =
            |a: f64, b: f64, o: f64, s: f64| ((b + o) / s).floor() - ((a + o) / s).floor() + 1.0;
        if span(x0, x1, 180.0, w) * span(y0, y1, 90.0, h) > MAX_COVER_CELLS {
            break;
        }
        best = Some((chars as usize, w, h));
    }
    let (chars, w, h) = best?;
    let (i0, i1) = (((x0 + 180.0) / w).floor(), ((x1 + 180.0) / w).floor());
    let (j0, j1) = (((y0 + 90.0) / h).floor(), ((y1 + 90.0) / h).floor());
    let mut cells = Vec::new();
    let mut i = i0;
    while i <= i1 {
        let mut j = j0;
        while j <= j1 {
            // each cell by its centre, away from every split
            cells.push(geohash(
                -180.0 + (i + 0.5) * w,
                -90.0 + (j + 0.5) * h,
                chars,
            ));
            j += 1.0;
        }
        i += 1.0;
    }
    cells.sort_unstable();
    cells.dedup();
    Some(refine(cells, (x0, y0, x1, y1)))
}

/// Ranges a refined cover may read (`cover_sql` merges sibling cells into
/// one range each).
const MAX_COVER_RANGES: usize = 16;

/// `cells` with each cell replaced by its children (one character more)
/// that touch the box, while the cover stays within `MAX_COVER_RANGES`
/// runs of siblings. Eight cells of one precision can hold up to 32 times
/// the box (a geohash character divides a cell in 32), and every row in
/// them is a heap read under RLS: an 800 m `near` read 5 220 rows for 245
/// matches (T-6056). A stored cell touching the box lies in a kept child
/// or is a prefix of one, so the cover stays a superset.
fn refine(cells: Vec<String>, (x0, y0, x1, y1): (f64, f64, f64, f64)) -> Vec<String> {
    let mut runs = cells.len();
    let mut out = Vec::new();
    for cell in cells {
        let kids: Vec<String> = BASE32
            .iter()
            .map(|b| format!("{cell}{}", char::from(*b)))
            .filter(|k| {
                let (a0, b0, a1, b1) = cell_box(k);
                a0 <= x1 && a1 >= x0 && b0 <= y1 && b1 >= y0
            })
            .collect();
        let more = sibling_runs(&kids).saturating_sub(1);
        if kids.len() < BASE32.len() && runs.saturating_add(more) <= MAX_COVER_RANGES {
            runs = runs.saturating_add(more);
            out.extend(kids);
        } else {
            out.push(cell);
        }
    }
    out.sort_unstable();
    out
}

/// The region of a geohash cell, the inverse of [`geohash`].
fn cell_box(cell: &str) -> (f64, f64, f64, f64) {
    let (mut x, mut y) = ((-180.0_f64, 180.0_f64), (-90.0_f64, 90.0_f64));
    let mut on_lon = true;
    for c in cell.bytes() {
        let bits = BASE32.iter().position(|b| *b == c).unwrap_or(0);
        for k in (0..5).rev() {
            let r = if on_lon { &mut x } else { &mut y };
            let mid = (r.0 + r.1) / 2.0;
            if (bits >> k) & 1 == 1 {
                r.0 = mid;
            } else {
                r.1 = mid;
            }
            on_lon = !on_lon;
        }
    }
    (x.0, y.0, x.1, y.1)
}

/// Whether `b` is the cell right after `a` among one parent's children.
fn next_sibling(a: &str, b: &str) -> bool {
    let index = |s: &str| {
        s.bytes()
            .last()
            .and_then(|c| BASE32.iter().position(|x| *x == c))
    };
    a.len() == b.len()
        && a.get(..a.len().saturating_sub(1)) == b.get(..b.len().saturating_sub(1))
        && matches!((index(a), index(b)), (Some(i), Some(j)) if j == i.saturating_add(1))
}

/// The ranges of sorted `cells` once runs of consecutive siblings merge.
fn sibling_runs(cells: &[String]) -> usize {
    cells.len().saturating_sub(
        cells
            .windows(2)
            .filter(|w| matches!(w, [a, b] if next_sibling(a, b)))
            .count(),
    )
}

/// Whether a box lies inside [-180, 180) x [-90, 90), where it neither
/// wraps the antimeridian nor reaches past a pole.
fn in_range(x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    x0 >= -180.0 && x1 < 180.0 && y0 >= -90.0 && y1 < 90.0
}

/// The box holding every point within `m` metres of the Point
/// `coordinates`, widened as `predicate` widens the radius; `None` around
/// a pole.
fn near_box(coordinates: &Value, m: f64) -> Option<(f64, f64, f64, f64)> {
    let [x, y, ..] = coordinates.as_array().map(Vec::as_slice)? else {
        return None;
    };
    let (x, y) = (x.as_f64()?, y.as_f64()?);
    // the radius `predicate` binds, as an angle on a sphere of
    // 110 km per degree: shorter than any WGS84 degree of latitude
    let r = ((m * 1.005 + 1.0) / 110_000.0).to_radians();
    let lat = y.to_radians();
    // the circle's widest longitude reach (spherical cap)
    if !r.is_finite() || r.sin() >= lat.cos() {
        return None;
    }
    let dx = (r.sin() / lat.cos()).asin().to_degrees();
    let dy = r.to_degrees();
    Some((x - dx, y - dy, x + dx, y + dy))
}

/// The bounding box of a GeoJSON `coordinates` member, any nesting depth.
fn bbox(v: &Value) -> Option<(f64, f64, f64, f64)> {
    match v.as_array()?.as_slice() {
        [x, y, ..] if x.is_number() => {
            let (x, y) = (x.as_f64()?, y.as_f64()?);
            Some((x, y, x, y))
        }
        parts => parts.iter().try_fold(None, |acc, p| {
            let b = bbox(p)?;
            Some(Some(match acc {
                None => b,
                Some((a0, b0, a1, b1)) => (b.0.min(a0), b.1.min(b0), b.2.max(a1), b.3.max(b1)),
            }))
        })?,
    }
}

/// The cover as leakproof btree conditions on `cell_col` (`text COLLATE
/// "C"`): a stored cell inside a cover cell (`>= c AND < c~`; `~` sorts
/// after every geohash character), or one that is a prefix of a cover cell
/// (a geometry too large for a finer cell). Binds numbered from `first`.
fn cover_sql(cells: &[String], cell_col: &str, first: usize) -> (String, Vec<String>) {
    let mut binds = Vec::new();
    let mut ors = Vec::new();
    // a run of consecutive siblings is one range, from its first cell to
    // past its last
    let mut runs: Vec<(&String, &String)> = Vec::new();
    for c in cells {
        match runs.last_mut() {
            Some((_, end)) if next_sibling(end, c) => *end = c,
            _ => runs.push((c, c)),
        }
    }
    for (start, end) in runs {
        let n = first.saturating_add(binds.len());
        let m = n.saturating_add(1);
        ors.push(format!("({cell_col} >= ${n} AND {cell_col} < ${m})"));
        binds.push(start.clone());
        binds.push(format!("{end}~"));
    }
    let mut prefixes: Vec<&str> = cells
        .iter()
        .flat_map(|c| (0..c.len()).map(move |k| &c[..k]))
        .collect();
    prefixes.sort_unstable();
    prefixes.dedup();
    let list: Vec<String> = prefixes
        .iter()
        .enumerate()
        .map(|(k, _)| format!("${}", first.saturating_add(binds.len()).saturating_add(k)))
        .collect();
    ors.push(format!("{cell_col} = ANY(ARRAY[{}])", list.join(", ")));
    binds.extend(prefixes.into_iter().map(str::to_owned));
    (format!("({})", ors.join(" OR ")), binds)
}

/// Extract the geometry to store in `entities.location` at write time.
/// `Some(geojson)` only for exactly one default-GeoProperty instance carrying
/// a GeoJSON value — see module docs, point 1, for why more than one is
/// deliberately `None`.
pub fn extract_location(doc: &Value) -> Option<String> {
    let instances = doc.get(LOCATION_IRI)?;
    let inst = match instances {
        Value::Array(a) => match a.as_slice() {
            [one] => one,
            _ => return None, // multi-instance: let the evaluator judge
        },
        v => v,
    };
    let value = inst.get("value").or_else(|| inst.get("object"))?;
    // must look like a GeoJSON geometry; anything else is not indexable and
    // must not silently become a NULL that looks like "no location".
    // GeometryCollection is deliberately absent: the PostGIS relate
    // predicates refuse one, so an extracted collection turns every later
    // geoquery into a database error — and it can never match anyway, since
    // `GeoQuery::matches_geometry` reads a geometry's `coordinates` and a
    // collection has none. It is flagged ambiguous instead and judged by the
    // evaluator like any other unextractable geoproperty.
    let t = value.get("type")?.as_str()?;
    if !matches!(
        t,
        "Point" | "MultiPoint" | "LineString" | "MultiLineString" | "Polygon" | "MultiPolygon"
    ) {
        return None;
    }
    serde_json::to_string(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn coords() -> Value {
        json!([2.29, 48.85])
    }

    fn spec<'a>(rel: Rel, geometry: &'a str, coordinates: &'a Value, gp: &'a str) -> GeoSpec<'a> {
        GeoSpec {
            rel,
            geometry,
            coordinates,
            geoproperty_iri: gp,
        }
    }

    #[test]
    fn relations_bind_the_geometry_and_never_splice_it() {
        let c = compile_geo(&spec(Rel::Within, "Point", &coords(), ""), "location", 4)
            .expect("compiles");
        assert!(!c.sql.contains("48.85"), "sql: {}", c.sql);
        assert!(c.sql.contains("ST_GeomFromGeoJSON($4)"), "sql: {}", c.sql);
        assert!(c.sql.contains("ST_Within(location,"));
        assert_eq!(c.geo_binds.len(), 1);
        assert!(c.geo_binds[0].contains("48.85"));
    }

    #[test]
    fn unextracted_rows_always_survive() {
        for rel in [
            Rel::Within,
            Rel::Contains,
            Rel::Intersects,
            Rel::Disjoint,
            Rel::Overlaps,
            Rel::Equals,
        ] {
            let c =
                compile_geo(&spec(rel, "Point", &coords(), ""), "location", 1).expect("compiles");
            assert!(
                c.sql.ends_with(" OR location_ambiguous)"),
                "a row carrying an unextractable geoproperty must reach the evaluator: {}",
                c.sql
            );
        }
    }

    #[test]
    fn near_widens_max_and_deflates_min() {
        let c = compile_geo(
            &spec(
                Rel::Near {
                    max: Some(2000.0),
                    min: None,
                },
                "Point",
                &coords(),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        assert!(c.sql.contains("ST_DWithin(location::geography"));
        assert!(
            c.num_binds[0] > 2000.0,
            "radius must widen: {:?}",
            c.num_binds
        );

        let c = compile_geo(
            &spec(
                Rel::Near {
                    max: None,
                    min: Some(2000.0),
                },
                "Point",
                &coords(),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        assert!(c.sql.contains("ST_Distance(location::geography"));
        assert!(
            c.num_binds[0] < 2000.0,
            "floor must deflate: {:?}",
            c.num_binds
        );
        assert_eq!(
            compile_geo(
                &spec(
                    Rel::Near {
                        max: Some(0.0),
                        min: Some(0.0)
                    },
                    "Point",
                    &coords(),
                    ""
                ),
                "location",
                1
            )
            .expect("compiles")
            .num_binds[1],
            0.0,
            "a deflated floor never goes negative"
        );
    }

    /// T-6070: a radius reaching past the antimeridian gets no box, whose
    /// `&&` would drop the points beyond it, and no cover.
    #[test]
    fn a_near_across_the_antimeridian_gets_no_box() {
        let c = compile_geo(
            &spec(
                Rel::Near {
                    max: Some(10_000.0),
                    min: None,
                },
                "Point",
                &json!([179.99, 0.0]),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        assert!(!c.sql.contains("ST_MakeEnvelope"), "{}", c.sql);
        assert_eq!(c.num_binds.len(), 1, "the radius alone");
        assert!(c.cell_binds.is_empty(), "no cover: {}", c.sql);
    }

    /// F-R2ANT3-017: a Point with an altitude (GeoJSON allows a third
    /// position element) keeps the box and the cover a 2D one gets.
    #[test]
    fn a_near_around_a_point_with_altitude_keeps_its_box_and_cover() {
        let c = compile_geo(
            &spec(
                Rel::Near {
                    max: Some(2000.0),
                    min: None,
                },
                "Point",
                &json!([13.4, 52.5, 34.0]),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        assert!(c.sql.contains("ST_MakeEnvelope"), "{}", c.sql);
        assert!(!c.cell_binds.is_empty(), "a cover: {}", c.sql);
    }

    /// Both `near` bounds in one predicate: the geometry keeps the offset and
    /// the two distances take the next two placeholders, in the order the
    /// caller appends them (geo binds first, then the numbers).
    #[test]
    fn both_near_bounds_take_distinct_placeholders_after_the_geometry() {
        let c = compile_geo(
            &spec(
                Rel::Near {
                    max: Some(2000.0),
                    min: Some(500.0),
                },
                "Point",
                &coords(),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        assert_eq!(c.geo_binds.len(), 1);
        assert_eq!(c.num_binds.len(), 6, "max, min and the box");
        assert!(c.sql.contains("ST_GeomFromGeoJSON($1)"), "{}", c.sql);
        assert!(c.sql.contains("$2)"), "maxDistance placeholder: {}", c.sql);
        assert!(
            c.sql.contains(">= $3"),
            "minDistance placeholder: {}",
            c.sql
        );
        assert!(
            c.sql.contains("ST_MakeEnvelope($4, $5, $6, $7, 4326)"),
            "box after: {}",
            c.sql
        );
        let past = 8 + c.cell_binds.len();
        assert!(
            c.sql.contains("location_cell >= $8"),
            "cover after: {}",
            c.sql
        );
        assert!(!c.sql.contains(&format!("${past}")), "overshoot: {}", c.sql);
    }

    /// `maxDistance`/`minDistance` reach this compiler as `f64`, and `inf`
    /// parses as one. A non-finite bound would compile to a comparison that
    /// EXCLUDES every row rather than widening — refuse it instead.
    #[test]
    fn a_non_finite_distance_is_left_to_the_evaluator() {
        for (max, min) in [
            (Some(f64::INFINITY), None),
            (None, Some(f64::INFINITY)),
            (Some(f64::NAN), None),
            (None, Some(f64::NAN)),
            (Some(2000.0), Some(f64::INFINITY)),
        ] {
            assert!(
                compile_geo(
                    &spec(Rel::Near { max, min }, "Point", &coords(), ""),
                    "location",
                    1
                )
                .is_none(),
                "non-finite bound must not compile: {max:?}/{min:?}"
            );
        }
    }

    /// `geometry` is a client string. It is a JSON member of the bound
    /// document, never a fragment of the statement.
    #[test]
    fn the_query_geometry_type_travels_in_the_bound_geojson() {
        let c = compile_geo(
            &spec(
                Rel::Within,
                "Polygon'); DROP TABLE entities; --",
                &coords(),
                "",
            ),
            "location",
            1,
        )
        .expect("compiles");
        for needle in ["DROP", "TABLE", "--", "'"] {
            assert!(!c.sql.contains(needle), "{needle:?} leaked: {}", c.sql);
        }
        assert!(
            c.sql
                .starts_with("((((location_cell >= $2 AND location_cell < $3)"),
            "{}",
            c.sql
        );
        assert!(
            c.sql.ends_with(
                " AND (ST_Within(location, ST_SetSRID(ST_GeomFromGeoJSON($1), 4326)))) \
                 OR location_ambiguous)"
            ),
            "{}",
            c.sql
        );
        assert!(c.geo_binds[0].contains("DROP"), "{}", c.geo_binds[0]);
    }

    /// The encoding PostGIS `ST_GeoHash` stores (`SELECT ST_GeoHash(
    /// ST_MakePoint(10, 45))`, PostGIS 3.5), splits going to the upper cell.
    #[test]
    fn geohash_matches_postgis() {
        assert_eq!(geohash(10.0, 45.0, 12), "u0p0581b0bh2");
        assert_eq!(geohash(11.25, 46.0, 3), "u20");
        assert_eq!(geohash(11.249_999_9, 46.0, 3), "u0p");
    }

    /// T-4852: the cover is a superset. Every point the region holds, its
    /// edges and corners included, has a stored cell (a 12-character
    /// geohash) inside one cover cell, and a geometry whose cell is a prefix
    /// of a cover cell is kept by the prefix list.
    #[test]
    fn the_cell_cover_keeps_every_point_of_the_region() {
        let kept = |cells: &[String], stored: &str| {
            cells
                .iter()
                .any(|c| stored.starts_with(c.as_str()) || c.starts_with(stored))
        };
        let poly = json!([[
            [10.0, 45.0],
            [10.0, 45.05],
            [10.05, 45.05],
            [10.05, 45.0],
            [10.0, 45.0]
        ]]);
        let s = spec(Rel::Intersects, "Polygon", &poly, "");
        let cells = cell_cover(&s).expect("a small box has a cover");
        assert!(
            sibling_runs(&cells) <= MAX_COVER_RANGES
                && cells.iter().all(|c| c.len() == 5 || c.len() == 6),
            "{cells:?}"
        );
        for i in 0..=20 {
            for j in 0..=20 {
                let (x, y) = (10.0 + 0.0025 * f64::from(i), 45.0 + 0.0025 * f64::from(j));
                assert!(
                    kept(&cells, &geohash(x, y, 12)),
                    "({x}, {y}) lost: {cells:?}"
                );
            }
        }
        assert!(kept(&cells, "u0p0") && kept(&cells, ""), "ancestors stay");
        assert!(!kept(&cells, &geohash(10.2, 45.0, 12)), "the cover narrows");

        // near: every point within the radius, measured on the sphere
        let centre = json!([16.37, 48.21]);
        let s = spec(
            Rel::Near {
                max: Some(5000.0),
                min: None,
            },
            "Point",
            &centre,
            "",
        );
        let cells = cell_cover(&s).expect("a 5 km circle has a cover");
        for k in 0..72 {
            let a = f64::from(k * 5).to_radians();
            let (lat, lon, d) = (
                48.21_f64.to_radians(),
                16.37_f64.to_radians(),
                5000.0_f64 / 6_371_008.8,
            );
            let y = (lat.sin() * d.cos() + lat.cos() * d.sin() * a.cos()).asin();
            let x = lon + (a.sin() * d.sin() * lat.cos()).atan2(d.cos() - lat.sin() * y.sin());
            assert!(
                kept(&cells, &geohash(x.to_degrees(), y.to_degrees(), 12)),
                "bearing {k}"
            );
        }
    }

    /// T-6056: the refined cover holds a fraction of its eight cells' area,
    /// keeps every point of the region, and reads each run of siblings as
    /// one range.
    #[test]
    fn the_refined_cover_reads_less_than_its_cells() {
        let centre = json!([19.3001, 48.52]);
        let s = spec(
            Rel::Near {
                max: Some(800.0),
                min: None,
            },
            "Point",
            &centre,
            "",
        );
        let cells = cell_cover(&s).expect("an 800 m circle has a cover");
        let area = |cs: &[String]| {
            cs.iter()
                .map(|c| {
                    let (a0, b0, a1, b1) = cell_box(c);
                    (a1 - a0) * (b1 - b0)
                })
                .sum::<f64>()
        };
        let parents: Vec<String> = {
            let mut p: Vec<String> = cells.iter().map(|c| geohash_parent(c, 5)).collect();
            p.dedup();
            p
        };
        assert!(cells.iter().any(|c| c.len() == 6), "{cells:?}");
        assert!(
            area(&cells) * 2.0 < area(&parents),
            "{cells:?} of {parents:?}"
        );
        let (sql, binds) = cover_sql(&cells, "location_cell", 1);
        let ranges = sql.matches(">=").count();
        assert_eq!(ranges, sibling_runs(&cells), "{sql}");
        assert!(ranges < cells.len() && ranges <= MAX_COVER_RANGES, "{sql}");
        assert!(
            binds.iter().any(|b| b.len() == 5 && !b.ends_with('~')),
            "parents stay as prefixes: {binds:?}"
        );
        // a cell decodes to the region its geohash names
        let (a0, b0, a1, b1) = cell_box(&geohash(19.3001, 48.52, 7));
        assert!(a0 <= 19.3001 && 19.3001 < a1 && b0 <= 48.52 && 48.52 < b1);
        assert!(
            next_sibling("u2d0", "u2d1")
                && !next_sibling("u2d1", "u2d0")
                && !next_sibling("u2d9", "u2e0")
        );
    }

    fn geohash_parent(c: &str, n: usize) -> String {
        c.get(..n).unwrap_or(c).to_owned()
    }

    /// No cover where one cannot narrow or would have to wrap.
    #[test]
    fn regions_without_a_cover_keep_the_plain_predicate() {
        let poly = json!([[[10.0, 45.0], [10.0, 46.0], [11.0, 46.0], [10.0, 45.0]]]);
        assert!(cell_cover(&spec(Rel::Disjoint, "Polygon", &poly, "")).is_none());
        let p = json!([179.99, 0.0]);
        let near = |max| {
            spec(
                Rel::Near {
                    max,
                    min: Some(1.0),
                },
                "Point",
                &p,
                "",
            )
        };
        assert!(cell_cover(&near(None)).is_none(), "minDistance alone");
        assert!(
            cell_cover(&near(Some(5000.0))).is_none(),
            "across the antimeridian"
        );
        let pole = json!([0.0, 89.99]);
        let s = spec(
            Rel::Near {
                max: Some(5000.0),
                min: None,
            },
            "Point",
            &pole,
            "",
        );
        assert!(cell_cover(&s).is_none(), "around a pole");
        let world = json!([[
            [-170.0, -80.0],
            [-170.0, 80.0],
            [170.0, 80.0],
            [-170.0, -80.0]
        ]]);
        assert!(
            cell_cover(&spec(Rel::Within, "Polygon", &world, "")).is_none(),
            "too wide"
        );
        let c = compile_geo(&spec(Rel::Within, "Polygon", &world, ""), "location", 1).expect("sql");
        assert!(
            c.cell_binds.is_empty() && !c.sql.contains("location_cell"),
            "{}",
            c.sql
        );
    }

    #[test]
    fn refusals_leave_it_to_the_evaluator() {
        // near from an extended geometry: evaluator measures the first vertex,
        // PostGIS the nearest point — narrowing risk on minDistance
        assert!(compile_geo(
            &spec(
                Rel::Near {
                    max: Some(10.0),
                    min: None
                },
                "Polygon",
                &json!([[[0, 0], [1, 0], [1, 1], [0, 0]]]),
                ""
            ),
            "location",
            1
        )
        .is_none());
        // a non-default geoproperty has no extracted column
        assert!(compile_geo(
            &spec(
                Rel::Within,
                "Point",
                &coords(),
                "https://example.org/observationSpace"
            ),
            "location",
            1
        )
        .is_none());
        // `near` with no bound is not a filter
        assert!(compile_geo(
            &spec(
                Rel::Near {
                    max: None,
                    min: None
                },
                "Point",
                &coords(),
                ""
            ),
            "location",
            1
        )
        .is_none());
    }

    #[test]
    fn instance_variant_falls_back_to_null_and_takes_any_geoproperty() {
        let c = compile_geo_instance(
            &spec(Rel::Within, "Point", &coords(), ""),
            "gi.geo_value",
            2,
        )
        .expect("compiles");
        assert!(
            c.sql.ends_with(" OR gi.geo_value IS NULL)"),
            "an instance with an unextracted geometry must reach the evaluator: {}",
            c.sql
        );
        assert!(c.sql.contains("ST_Within(gi.geo_value,"), "{}", c.sql);
        assert!(!c.sql.contains("location_ambiguous"), "{}", c.sql);
        // the predicate core still refuses near-from-extended-geometry
        assert!(compile_geo_instance(
            &spec(
                Rel::Near {
                    max: Some(1.0),
                    min: None
                },
                "Polygon",
                &json!([[[0, 0], [1, 0], [1, 1], [0, 0]]]),
                ""
            ),
            "gi.geo_value",
            1
        )
        .is_none());
    }

    #[test]
    fn location_extraction_takes_the_single_instance_only() {
        let one = json!({
            LOCATION_IRI: [{"type": "GeoProperty",
                            "value": {"type": "Point", "coordinates": [1, 2]}}]
        });
        assert_eq!(
            extract_location(&one).expect("extracted"),
            "{\"coordinates\":[1,2],\"type\":\"Point\"}"
        );

        let multi = json!({
            LOCATION_IRI: [
                {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1, 2]}},
                {"type": "GeoProperty", "datasetId": "urn:d", "value": {"type": "Point", "coordinates": [3, 4]}}
            ]
        });
        assert!(
            extract_location(&multi).is_none(),
            "multi-instance stays NULL so the guard hands the row to the evaluator"
        );

        // a non-geometry value must not be extracted as if it were one
        let bogus = json!({ LOCATION_IRI: [{"type": "Property", "value": "somewhere"}] });
        assert!(extract_location(&bogus).is_none());
        assert!(extract_location(&json!({"id": "urn:x"})).is_none());

        // a collection is a GeoJSON geometry, but the relate predicates
        // refuse one — it stays unextracted so the row is flagged ambiguous
        let collection = json!({
            LOCATION_IRI: [{"type": "GeoProperty", "value": {
                "type": "GeometryCollection",
                "geometries": [{"type": "Point", "coordinates": [1, 2]}]
            }}]
        });
        assert!(
            extract_location(&collection).is_none(),
            "a GeometryCollection must not reach a PostGIS relate predicate"
        );
    }
}
