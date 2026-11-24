//! WKT (OGC 06-103r4 Simple Features) to and from GeoJSON geometries (RFC 7946): the
//! `parse_wkt` and `format_wkt` methods, and `geo_transform`, which reprojects a geometry
//! between the national grids cities publish in and WGS 84. Ekiben extensions; Bento has no
//! counterpart.
//!
//! Pure: no I/O. The CRS list is compiled in and names no grid file, so proj4rs reads no PROJ
//! data. GeometryCollection nesting is bounded by [`MAX_GEO_DEPTH`]; NaN, infinities
//! and malformed text are errors, never a panic. Coordinates stay as written: no CRS is read
//! or assumed (a projected WKT stays projected; `check_geometry` wants WGS 84).

use std::collections::BTreeMap;
use std::fmt::Write;

use proj4rs::proj::Proj;

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::{get_arg, get_optional_arg};
use crate::ngsild::{MAX_GEO_DEPTH, WGS84};
use crate::value::Value;

/// (WKT keyword, GeoJSON type).
const TYPES: [(&str, &str); 7] = [
    ("POINT", "Point"),
    ("LINESTRING", "LineString"),
    ("POLYGON", "Polygon"),
    ("MULTIPOINT", "MultiPoint"),
    ("MULTILINESTRING", "MultiLineString"),
    ("MULTIPOLYGON", "MultiPolygon"),
    ("GEOMETRYCOLLECTION", "GeometryCollection"),
];

pub(crate) fn eval_geo_family(
    target: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    Some(match name {
        "parse_wkt" => target.get_string().and_then(|s| parse_wkt(&s)),
        "format_wkt" => format_wkt(target).map(Value::String),
        "parse_gml" => (|| {
            let mut opt = |name, pos| {
                get_optional_arg(args, name, pos)
                    .map(|e| ctx.eval_expr(e).and_then(|v| v.get_string()))
                    .transpose()
            };
            let from = opt("from", 0)?;
            let to = opt("to", 1)?;
            crate::gml::parse_gml(target, from.as_deref(), to.as_deref())
        })(),
        "geo_transform" => (|| {
            let from = ctx.eval_expr(get_arg(args, "from", 0)?)?.get_string()?;
            let to = ctx.eval_expr(get_arg(args, "to", 1)?)?.get_string()?;
            geo_transform(target, &from, &to)
        })(),
        _ => return None,
    })
}

/// The CRSs `geo_transform` knows, as PROJ.4 definitions (EPSG's, with the WGS 84 shift PROJ
/// uses where no grid is installed). None names a `+nadgrids` file, so nothing is read from disk.
const CRS: &[(u32, &str)] = &[
    (4326, "+proj=longlat +datum=WGS84 +no_defs"),
    // ETRS89, geographic: within a metre of WGS 84 (EPSG transformation 1149, a null shift).
    (4258, "+proj=longlat +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +no_defs"),
    (3857, "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +nadgrids=@null +no_defs"),
    // Helsinki (GK25 / EUREF-FIN) and Finland (TM35FIN): EUREF-FIN to WGS 84 is a null shift.
    (3879, "+proj=tmerc +lat_0=0 +lon_0=25 +k=1 +x_0=25500000 +y_0=0 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (3067, "+proj=utm +zone=35 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    // S-JTSK / Krovak East North (Czechia, Slovakia), EPSG transformation 1623.
    (5514, "+proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +towgs84=570.8,85.7,462.8,4.998,1.587,5.261,3.56 +units=m +no_defs"),
    // ETRS89 / UTM 29N..31N: Spain and Portugal (Barcelona's open data is 25831).
    (25829, "+proj=utm +zone=29 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (25830, "+proj=utm +zone=30 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (25831, "+proj=utm +zone=31 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (25832, "+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (25833, "+proj=utm +zone=33 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    (25834, "+proj=utm +zone=34 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"),
    // CH1903+ / LV95, EPSG transformation 1676.
    (2056, "+proj=somerc +lat_0=46.9524055555556 +lon_0=7.43958333333333 +k_0=1 +x_0=2600000 +y_0=1200000 +ellps=bessel +towgs84=674.374,15.056,405.346,0,0,0,0 +units=m +no_defs"),
    // OSGB 1936 / British National Grid, EPSG transformation 1314 (about 2 m; OSTN15 is a grid).
    (27700, "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m +no_defs"),
];

/// The EPSG code a CRS name gives: `EPSG:n`, its OGC URN or URL forms, or a WGS 84 alias.
pub(crate) fn epsg_code(name: &str) -> Option<u32> {
    if WGS84.contains(&name) || name.ends_with(":CRS84") || name.ends_with("/CRS84") {
        return Some(4326);
    }
    // A URN may carry a version: urn:ogc:def:crs:EPSG:6.6:4326.
    let urn = ["urn:ogc:def:crs:EPSG:", "urn:x-ogc:def:crs:EPSG:"]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .and_then(|rest| rest.rsplit(':').next());
    urn.or_else(|| {
        [
            "EPSG:",
            "http://www.opengis.net/def/crs/EPSG/0/",
            "http://www.opengis.net/gml/srs/epsg.xml#",
        ]
        .iter()
        .find_map(|p| name.strip_prefix(p))
    })
    .and_then(|c| c.parse::<u32>().ok())
}

/// Whether `geo_transform` knows EPSG:`code`.
pub(crate) fn is_known_crs(code: u32) -> bool {
    CRS.iter().any(|(c, _)| *c == code)
}

/// The PROJ definition of a CRS named `EPSG:n`, by its OGC URN or URL, or a WGS 84 alias.
fn crs(param: &str, name: &str) -> Result<Proj, String> {
    crs_in(CRS, param, name)
}

/// [`crs`] over a given table, so a test can give it a definition PROJ refuses.
fn crs_in(table: &[(u32, &str)], param: &str, name: &str) -> Result<Proj, String> {
    let code = epsg_code(name);
    let known = || {
        table
            .iter()
            .map(|(c, _)| format!("EPSG:{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let def = code
        .and_then(|code| table.iter().find(|(c, _)| *c == code))
        .ok_or_else(|| {
            format!(
                "parameter {param}: {name:?} is not a known CRS; use one of {}",
                known()
            )
        })?
        .1;
    Proj::from_proj_string(def).map_err(|e| format!("parameter {param}: {name}: {e}"))
}

/// Every position of a GeoJSON geometry, or one `[x, y]` / `[x, y, z]` position, reprojected
/// from one CRS to another. Geographic positions are `[longitude, latitude]` in degrees; a z is
/// kept as given.
pub fn geo_transform(target: &Value, from: &str, to: &str) -> Result<Value, String> {
    let (src, dst) = (crs("from", from)?, crs("to", to)?);
    let mut point = |x: f64, y: f64| -> Result<(f64, f64), String> {
        if src.is_latlong() && !(-90.0..=90.0).contains(&y) {
            return Err("geo_transform: latitude is out of range -90..90".to_string());
        }
        let mut p = if src.is_latlong() {
            (x.to_radians(), y.to_radians(), 0.0)
        } else {
            (x, y, 0.0)
        };
        proj4rs::transform::transform(&src, &dst, &mut p)
            .map_err(|e| format!("geo_transform from {from} to {to}: {e}"))?;
        let (x, y) = if dst.is_latlong() {
            (p.0.to_degrees(), p.1.to_degrees())
        } else {
            (p.0, p.1)
        };
        if x.is_finite() && y.is_finite() {
            Ok((x, y))
        } else {
            Err(format!(
                "geo_transform from {from} to {to}: the position is outside the projection"
            ))
        }
    };
    reproject(target, &mut point, 0)
}

fn reproject(
    v: &Value,
    point: &mut dyn FnMut(f64, f64) -> Result<(f64, f64), String>,
    depth: usize,
) -> Result<Value, String> {
    // A collection nests MAX_GEO_DEPTH deep, its MultiPolygon three arrays more.
    if depth > MAX_GEO_DEPTH + 4 {
        return Err(format!("the geometry nests deeper than {MAX_GEO_DEPTH}"));
    }
    match v {
        Value::Array(items) if items.first().is_some_and(|i| i.as_f64().is_some()) => {
            let n: Vec<f64> = items
                .iter()
                .map(|i| i.as_f64().filter(|f| f.is_finite()))
                .collect::<Option<_>>()
                .filter(|n: &Vec<f64>| (2..=3).contains(&n.len()))
                .ok_or("a position holds 2 or 3 finite numbers")?;
            let (x, y) = point(n[0], n[1])?;
            let mut out = vec![Value::Float(x), Value::Float(y)];
            out.extend(items.get(2).cloned());
            Ok(Value::Array(out.into()))
        }
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|i| reproject(i, point, depth + 1))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        )),
        Value::Object(m) => {
            let key = match m.get("type") {
                Some(Value::String(t)) if t == "GeometryCollection" => "geometries",
                _ if m.contains_key("coordinates") => "coordinates",
                _ => return Err("geo_transform wants a GeoJSON geometry or a position, got an object without coordinates".to_string()),
            };
            let inner = m
                .get(key)
                .ok_or_else(|| format!("a GeometryCollection needs {key}"))?;
            let mut out = (**m).clone();
            out.insert(key.to_string(), reproject(inner, point, depth + 1)?);
            Ok(Value::Object(out.into()))
        }
        other => Err(format!(
            "geo_transform wants a GeoJSON geometry or a position, got {}",
            other.type_name()
        )),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dim {
    /// No tag: 2 numbers, or 3 read as Z.
    Plain,
    Z,
    /// The measure is dropped: GeoJSON has none.
    M,
    Zm,
}

fn split_dim(word: &str) -> Option<(&'static str, Dim)> {
    for (wkt, geojson) in TYPES {
        if let Some(rest) = word.strip_prefix(wkt) {
            let dim = match rest {
                "" => Dim::Plain,
                "Z" => Dim::Z,
                "M" => Dim::M,
                "ZM" => Dim::Zm,
                _ => continue,
            };
            return Some((geojson, dim));
        }
    }
    None
}

fn object(kind: &str, key: &str, v: Value) -> Value {
    let mut m = BTreeMap::new();
    m.insert("type".to_string(), Value::String(kind.to_string()));
    m.insert(key.to_string(), v);
    Value::Object(m.into())
}

/// A WKT geometry as a GeoJSON geometry object.
pub fn parse_wkt(text: &str) -> Result<Value, String> {
    let mut r = Reader {
        s: text,
        at: 0,
        width: None,
    };
    let g = r.geometry(0)?;
    r.ws();
    if r.at != text.len() {
        return Err(r.err("unexpected text after the geometry"));
    }
    Ok(g)
}

struct Reader<'a> {
    s: &'a str,
    /// Only ever advanced over ASCII, so always a char boundary.
    at: usize,
    /// Numbers per position kept so far in the current geometry.
    width: Option<usize>,
}

impl Reader<'_> {
    fn err(&self, why: &str) -> String {
        format!("failed to parse WKT at byte {}: {why}", self.at)
    }

    fn byte(&self) -> Option<u8> {
        self.s.as_bytes().get(self.at).copied()
    }

    fn ws(&mut self) {
        while self.byte().is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.byte()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.eat(b) {
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", b as char)))
        }
    }

    fn word(&mut self) -> String {
        self.ws();
        let start = self.at;
        while self.byte().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.at += 1;
        }
        self.s[start..self.at].to_ascii_uppercase()
    }

    fn number(&mut self) -> Result<f64, String> {
        self.ws();
        let start = self.at;
        while self
            .byte()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
        {
            self.at += 1;
        }
        let t = &self.s[start..self.at];
        let f: f64 = t
            .parse()
            .map_err(|_| self.err(&format!("{t:?} is not a number")))?;
        if !f.is_finite() {
            return Err(self.err(&format!("{t} is not a finite number")));
        }
        Ok(f)
    }

    fn position(&mut self, dim: Dim) -> Result<Value, String> {
        let mut nums = vec![self.number()?];
        while nums.len() < 4
            && self
                .peek()
                .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
        {
            nums.push(self.number()?);
        }
        let keep = match (dim, nums.len()) {
            (Dim::Plain, 2 | 3) | (Dim::Z, 3) => nums.len(),
            (Dim::M, 3) => 2,
            (Dim::Zm, 4) => 3,
            (_, n) => {
                return Err(self.err(&format!(
                    "a position of {n} numbers does not match the geometry's dimensions"
                )))
            }
        };
        if self.width.is_some_and(|w| w != keep) {
            return Err(self.err("positions mix 2 and 3 dimensions"));
        }
        self.width = Some(keep);
        Ok(Value::Array(
            nums[..keep].iter().map(|f| Value::Float(*f)).collect(),
        ))
    }

    fn list(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<Value, String>,
    ) -> Result<Value, String> {
        self.expect(b'(')?;
        let mut out = vec![item(self)?];
        while self.eat(b',') {
            out.push(item(self)?);
        }
        self.expect(b')')?;
        Ok(Value::Array(out.into()))
    }

    fn geometry(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_GEO_DEPTH {
            return Err(self.err(&format!(
                "the GeometryCollection nests deeper than {MAX_GEO_DEPTH}"
            )));
        }
        self.width = None;
        let word = self.word();
        let (kind, mut dim) = split_dim(&word)
            .ok_or_else(|| self.err(&format!("{word:?} is not a WKT geometry type")))?;
        if dim == Dim::Plain {
            let save = self.at;
            dim = match self.word().as_str() {
                "Z" => Dim::Z,
                "M" => Dim::M,
                "ZM" => Dim::Zm,
                _ => {
                    self.at = save;
                    Dim::Plain
                }
            };
        }
        let save = self.at;
        if self.word() == "EMPTY" {
            return match kind {
                "Point" => Err(self.err("POINT EMPTY has no GeoJSON form")),
                "GeometryCollection" => {
                    Ok(object(kind, "geometries", Value::Array(Vec::new().into())))
                }
                _ => Ok(object(kind, "coordinates", Value::Array(Vec::new().into()))),
            };
        }
        self.at = save;
        let coords = match kind {
            "Point" => {
                self.expect(b'(')?;
                let p = self.position(dim)?;
                self.expect(b')')?;
                p
            }
            "LineString" => self.list(|r| r.position(dim))?,
            "Polygon" | "MultiLineString" => self.list(|r| r.list(|r| r.position(dim)))?,
            // Both `MULTIPOINT (1 2, 3 4)` and the standard `MULTIPOINT ((1 2), (3 4))`.
            "MultiPoint" => self.list(|r| {
                if r.eat(b'(') {
                    let p = r.position(dim)?;
                    r.expect(b')')?;
                    Ok(p)
                } else {
                    r.position(dim)
                }
            })?,
            "MultiPolygon" => self.list(|r| r.list(|r| r.list(|r| r.position(dim))))?,
            _ => {
                let geoms = self.list(|r| r.geometry(depth + 1))?;
                return Ok(object(kind, "geometries", geoms));
            }
        };
        Ok(object(kind, "coordinates", coords))
    }
}

/// A GeoJSON geometry object as WKT (` Z` when its positions carry an altitude).
pub fn format_wkt(v: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_geometry(&mut out, v, 0)?;
    Ok(out)
}

fn array<'v>(v: Option<&'v Value>, what: &str) -> Result<&'v [Value], String> {
    match v {
        Some(Value::Array(a)) => Ok(a.as_slice()),
        Some(other) => Err(format!(
            "GeoJSON {what} must be an array, got {}",
            other.type_name()
        )),
        None => Err(format!("a GeoJSON geometry needs {what}")),
    }
}

fn write_geometry(out: &mut String, v: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_GEO_DEPTH {
        return Err(format!(
            "the GeometryCollection nests deeper than {MAX_GEO_DEPTH}"
        ));
    }
    let Value::Object(m) = v else {
        return Err(format!(
            "a GeoJSON geometry is an object, got {}",
            v.type_name()
        ));
    };
    let kind = match m.get("type") {
        Some(Value::String(s)) => s.as_str(),
        _ => return Err("a GeoJSON geometry needs a type".to_string()),
    };
    let word = TYPES
        .iter()
        .find(|(_, g)| *g == kind)
        .map(|(w, _)| *w)
        .ok_or_else(|| format!("{kind:?} is not a GeoJSON geometry type"))?;
    out.push_str(word);
    if kind == "GeometryCollection" {
        let geoms = array(m.get("geometries"), "geometries")?;
        if geoms.is_empty() {
            out.push_str(" EMPTY");
            return Ok(());
        }
        out.push_str(" (");
        for (i, g) in geoms.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_geometry(out, g, depth + 1)?;
        }
        out.push(')');
        return Ok(());
    }
    let coords = m
        .get("coordinates")
        .ok_or_else(|| format!("a GeoJSON {kind} needs coordinates"))?;
    if kind != "Point" && matches!(coords, Value::Array(a) if a.is_empty()) {
        out.push_str(" EMPTY");
        return Ok(());
    }
    let mut body = String::new();
    let mut width = None;
    match kind {
        "Point" => {
            body.push('(');
            position(&mut body, coords, &mut width)?;
            body.push(')');
        }
        "MultiPoint" => {
            body.push('(');
            for (i, p) in array(Some(coords), "coordinates")?.iter().enumerate() {
                if i > 0 {
                    body.push_str(", ");
                }
                body.push('(');
                position(&mut body, p, &mut width)?;
                body.push(')');
            }
            body.push(')');
        }
        "LineString" => nested(&mut body, coords, 1, &mut width)?,
        "Polygon" | "MultiLineString" => nested(&mut body, coords, 2, &mut width)?,
        _ => nested(&mut body, coords, 3, &mut width)?,
    }
    if width == Some(3) {
        out.push_str(" Z");
    }
    out.push(' ');
    out.push_str(&body);
    Ok(())
}

fn nested(
    out: &mut String,
    v: &Value,
    levels: usize,
    width: &mut Option<usize>,
) -> Result<(), String> {
    if levels == 0 {
        return position(out, v, width);
    }
    let items = array(Some(v), "coordinates")?;
    if items.is_empty() {
        return Err("a nested GeoJSON coordinate array is empty".to_string());
    }
    out.push('(');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        nested(out, item, levels - 1, width)?;
    }
    out.push(')');
    Ok(())
}

fn position(out: &mut String, v: &Value, width: &mut Option<usize>) -> Result<(), String> {
    let items = array(Some(v), "position")?;
    if !(2..=3).contains(&items.len()) {
        return Err(format!(
            "a GeoJSON position holds 2 or 3 numbers, got {}",
            items.len()
        ));
    }
    if width.is_some_and(|w| w != items.len()) {
        return Err("GeoJSON positions mix 2 and 3 dimensions".to_string());
    }
    *width = Some(items.len());
    for (i, n) in items.iter().enumerate() {
        let f = n
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| "a GeoJSON position holds finite numbers only".to_string())?;
        if i > 0 {
            out.push(' ');
        }
        // Rust's f64 Display: shortest round-trip digits, never an exponent.
        let _ = write!(out, "{f}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ArgList, Assignment, Expr, Statement};

    #[test]
    fn a_method_no_family_implements_is_refused_at_run_time() {
        // The parser refuses an unknown name; a tree built by hand reaches the end of the
        // dispatch, where geo, the last family, answers None.
        let query = Expr::MethodCall {
            target: Box::new(Expr::Literal(Value::String("x".into()))),
            name: "no_such_method".into(),
            args: ArgList::Positional(Vec::new()),
        };
        let stmt = Statement::Single {
            line: 1,
            assignment: Assignment::Json(Default::default()),
            query,
        };
        let exec = crate::Executor::new(
            "test".into(),
            Default::default(),
            vec![stmt],
            crate::Environment::new(),
        );
        let err = exec.exec_on_value(Value::Null).map(|_| ()).unwrap_err();
        assert!(
            err.to_string()
                .contains("unrecognised method 'no_such_method'"),
            "{err}"
        );
    }

    #[test]
    fn a_wkt_fault_names_its_byte_and_reason() {
        assert_eq!(
            parse_wkt("POINT(1 x)"),
            Err("failed to parse WKT at byte 8: a position of 1 numbers does not match the geometry's dimensions".to_string())
        );
    }

    #[test]
    fn crs84_names_end_in_a_colon_or_a_slash() {
        assert_eq!(epsg_code("urn:ogc:def:crs:OGC:2:CRS84"), Some(4326));
        assert_eq!(
            epsg_code("http://www.opengis.net/def/crs/OGC/0/CRS84"),
            Some(4326)
        );
        assert_eq!(epsg_code("CRS84"), None);
    }

    fn arr(items: Vec<Value>) -> Value {
        Value::Array(items.into())
    }

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into(),
        )
    }

    #[test]
    fn nesting_is_counted_through_arrays_and_collections_to_the_limit() {
        let point = || arr(vec![Value::Float(24.9), Value::Float(60.1)]);
        let wrap = |levels: usize| (0..levels).fold(point(), |v, _| arr(vec![v]));
        let limit = MAX_GEO_DEPTH + 4;
        assert!(geo_transform(&wrap(limit), "EPSG:4326", "EPSG:3857").is_ok());
        let deep = geo_transform(&wrap(limit + 1), "EPSG:4326", "EPSG:3857");
        assert_eq!(
            deep,
            Err(format!("the geometry nests deeper than {MAX_GEO_DEPTH}"))
        );
        // Each collection is two levels (the object and its geometries array), its Point's
        // position one more: five collections fit, six do not.
        let collections = |n: usize| {
            (0..n).fold(
                obj(vec![
                    ("type", Value::String("Point".into())),
                    ("coordinates", point()),
                ]),
                |g, _| {
                    obj(vec![
                        ("type", Value::String("GeometryCollection".into())),
                        ("geometries", arr(vec![g])),
                    ])
                },
            )
        };
        assert!(geo_transform(&collections(5), "EPSG:4326", "EPSG:3857").is_ok());
        assert!(geo_transform(&collections(6), "EPSG:4326", "EPSG:3857").is_err());
    }

    #[test]
    fn an_object_without_coordinates_or_geometries_is_named() {
        let point = obj(vec![("type", Value::String("Point".into()))]);
        assert_eq!(
            geo_transform(&point, "EPSG:4326", "EPSG:3857"),
            Err("geo_transform wants a GeoJSON geometry or a position, got an object without coordinates".to_string())
        );
        let collection = obj(vec![("type", Value::String("GeometryCollection".into()))]);
        assert_eq!(
            geo_transform(&collection, "EPSG:4326", "EPSG:3857"),
            Err("a GeometryCollection needs geometries".to_string())
        );
    }

    #[test]
    fn one_coordinate_past_the_projection_is_refused() {
        // LV95 maps this position to a finite x and an infinite y: one is enough to refuse it.
        let far_south = arr(vec![Value::Float(2.6e6), Value::Float(-1e200)]);
        assert_eq!(
            geo_transform(&far_south, "EPSG:2056", "EPSG:2056"),
            Err(
                "geo_transform from EPSG:2056 to EPSG:2056: the position is outside the projection"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_crs_definition_proj_refuses_names_the_parameter_and_crs() {
        let table = [(4326, "+proj=no_such_projection")];
        let err = crs_in(&table, "from", "EPSG:4326").map(|_| ()).unwrap_err();
        assert!(err.starts_with("parameter from: EPSG:4326: "), "{err}");
        let err = crs_in(&table, "to", "EPSG:3857").map(|_| ()).unwrap_err();
        assert!(
            err.contains("EPSG:3857\" is not a known CRS; use one of EPSG:4326"),
            "{err}"
        );
    }

    #[test]
    fn a_pole_has_no_web_mercator_position() {
        let pole = Value::Array(vec![Value::Float(0.0), Value::Float(90.0)].into());
        let err = geo_transform(&pole, "EPSG:4326", "EPSG:3857")
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.starts_with("geo_transform from EPSG:4326 to EPSG:3857"),
            "{err}"
        );
    }
}
