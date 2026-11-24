//! `unarchive` format `shapefile` (an ekiben extension, T-7089): a zip holding one ESRI
//! shapefile (`.shp`, `.dbf`, `.prj`, optionally `.shx` and `.cpg`) becomes one GeoJSON Feature
//! per record, its geometry in WGS 84 (RFC 7946) and its properties from the `.dbf`.
//!
//! - The `.prj` names the CRS: its top-level `AUTHORITY["EPSG", n]`, else its PROJCS or GEOGCS
//!   name (OGC or ESRI spelling) of a CRS `geo_transform` knows. Any other `.prj`, or none, is an
//!   error naming it: coordinates are never passed on in an unknown CRS.
//! - Text fields decode by the `.cpg`, else by the `.dbf` language driver byte, else as UTF-8
//!   when valid and Windows-1252 otherwise (what ArcGIS writes without either).
//! - Rings follow RFC 7946 (outer counterclockwise, holes clockwise); a hole belongs to the
//!   outer ring holding its first vertex. A record deleted in the `.dbf` is skipped; a null shape
//!   gives a null geometry. M values are dropped, Z values kept.
//!
//! Caps: the members unpack to at most `MAX_ARCHIVE_UNPACKED_BYTES` by their declared sizes
//! (checked before any is read, and again while reading), at most `max_records` records, and at
//! most [`MAX_VERTICES`] vertices per geometry. Every length in the files is checked against the
//! bytes there: a truncated or lying file is an error, never a panic.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use ekiben_bloblang::Value;
use serde_json::{json, Value as Json};

use crate::archive::MAX_ARCHIVE_UNPACKED_BYTES;

/// Vertices one geometry may hold.
pub const MAX_VERTICES: usize = 1_000_000;

/// The CRS names a `.prj` may give for each EPSG code `geo_transform` knows, lowercased with
/// everything but letters and digits removed (`EUREF-FIN / GK25FIN` and `EUREF-FIN_GK25FIN` are
/// both `eureffingk25fin`).
const PRJ_NAMES: &[(u32, &[&str])] = &[
    (4326, &["wgs84", "gcswgs1984"]),
    (4258, &["etrs89", "gcsetrs1989"]),
    (
        3857,
        &[
            "wgs84pseudomercator",
            "wgs1984webmercatorauxiliarysphere",
            "wgs1984webmercator",
        ],
    ),
    (
        3879,
        &[
            "eureffingk25fin",
            "etrs89gk25fin",
            "etrs1989gk25fin",
            "etrsgk25",
        ],
    ),
    (
        3067,
        &[
            "eureffintm35fin",
            "eureffintm35finen",
            "etrs89tm35fin",
            "etrs1989tm35fin",
            "etrstm35fin",
        ],
    ),
    (5514, &["sjtskkrovakeastnorth"]),
    (25832, &["etrs89utmzone32n", "etrs1989utmzone32n"]),
    (25833, &["etrs89utmzone33n", "etrs1989utmzone33n"]),
    (25834, &["etrs89utmzone34n", "etrs1989utmzone34n"]),
    (2056, &["ch1903lv95"]),
    (
        27700,
        &[
            "britishnationalgrid",
            "osgb36britishnationalgrid",
            "osgb1936britishnationalgrid",
        ],
    ),
];

/// One Feature per record of the shapefile in the zip `bytes`.
pub fn features(bytes: &[u8], max_records: usize) -> Result<Vec<Value>, String> {
    let set = Members::read(bytes)?;
    let epsg = crs_of(&set.prj_name, &set.prj)?;
    let dbf = Dbf::parse(&set.dbf, set.cpg.as_deref())
        .map_err(|e| format!("shapefile {}: {e}", set.dbf_name))?;
    let shapes =
        shapes(&set.shp, max_records).map_err(|e| format!("shapefile {}: {e}", set.shp_name))?;
    if shapes.len() != dbf.records {
        return Err(format!(
            "shapefile {} has {} records but {} has {}",
            set.shp_name,
            shapes.len(),
            set.dbf_name,
            dbf.records
        ));
    }
    let mut out = Vec::with_capacity(shapes.len());
    for (i, geometry) in shapes.into_iter().enumerate() {
        let Some(properties) = dbf.record(i)? else {
            continue;
        };
        let geometry = match (geometry, epsg) {
            (Json::Null, _) => Json::Null,
            (g, 4326) => g,
            (g, code) => {
                let to = ekiben_bloblang::geo::geo_transform(
                    &Value::from_json_value(g),
                    &format!("EPSG:{code}"),
                    "EPSG:4326",
                )
                .map_err(|e| format!("shapefile {} record {}: {e}", set.shp_name, i + 1))?;
                to.to_json_value()
            }
        };
        out.push(Value::from_json_value(json!({
            "type": "Feature",
            "geometry": geometry,
            "properties": Json::Object(properties),
        })));
    }
    Ok(out)
}

/// The files of the one shapefile a zip holds.
struct Members {
    shp_name: String,
    shp: Vec<u8>,
    dbf_name: String,
    dbf: Vec<u8>,
    prj_name: String,
    prj: Vec<u8>,
    cpg: Option<Vec<u8>>,
}

impl Members {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        let zip_err = |e: zip::result::ZipError| format!("failed to read shapefile zip: {e}");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_err)?;
        if zip.len() > crate::archive::MAX_ARCHIVE_MEMBERS {
            return Err(format!(
                "shapefile zip has more than {} members",
                crate::archive::MAX_ARCHIVE_MEMBERS
            ));
        }
        // By extension: (name, index, declared size).
        let mut found: BTreeMap<&'static str, Vec<(String, usize, u64)>> = BTreeMap::new();
        let mut total: u64 = 0;
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(zip_err)?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().map_err(zip_err)?.into_owned();
            let lower = name.to_ascii_lowercase();
            let Some(ext) = ["shp", "dbf", "prj", "cpg"]
                .into_iter()
                .find(|e| lower.ends_with(&format!(".{e}")))
            else {
                continue;
            };
            total = total.saturating_add(f.size());
            if total > MAX_ARCHIVE_UNPACKED_BYTES {
                return Err(format!(
                    "shapefile unpacks to more than {MAX_ARCHIVE_UNPACKED_BYTES} bytes"
                ));
            }
            found.entry(ext).or_default().push((name, i, f.size()));
        }
        let mut take = |ext: &str, required: bool| -> Result<Option<(String, Vec<u8>)>, String> {
            match found.get(ext).map(Vec::as_slice).unwrap_or_default() {
                [] if required => Err(format!(
                    "shapefile zip holds no .{ext} file; zip the .shp, .shx, .dbf and .prj of one layer"
                )),
                [] => Ok(None),
                [(name, i, size)] => {
                    let f = zip.by_index(*i).map_err(zip_err)?;
                    let mut buf = Vec::new();
                    f.take(size.saturating_add(1))
                        .read_to_end(&mut buf)
                        .map_err(|e| format!("failed to read {name}: {e}"))?;
                    if buf.len() as u64 > *size {
                        return Err(format!("{name} unpacks to more than its declared size"));
                    }
                    Ok(Some((name.clone(), buf)))
                }
                many => Err(format!(
                    "shapefile zip holds {} .{ext} files ({}); zip one layer per data source",
                    many.len(),
                    many.iter()
                        .map(|(n, _, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        };
        let need = |v: Option<(String, Vec<u8>)>| v.ok_or_else(String::new);
        let (shp_name, shp) = need(take("shp", true)?)?;
        let (dbf_name, dbf) = need(take("dbf", true)?)?;
        let (prj_name, prj) = need(take("prj", true)?)?;
        let cpg = take("cpg", false)?.map(|(_, b)| b);
        Ok(Self {
            shp_name,
            shp,
            dbf_name,
            dbf,
            prj_name,
            prj,
            cpg,
        })
    }
}

/// The EPSG code of a `.prj` (WKT 1, OGC or ESRI), when it is one `geo_transform` knows.
fn crs_of(prj_name: &str, prj: &[u8]) -> Result<u32, String> {
    let text = String::from_utf8_lossy(prj);
    let text = text.trim();
    let name = wkt_name(text);
    let code = top_authority(text).or_else(|| {
        let key: String = name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        PRJ_NAMES
            .iter()
            .find(|(_, names)| names.contains(&key.as_str()))
            .map(|(c, _)| *c)
    });
    let known = || {
        PRJ_NAMES
            .iter()
            .map(|(c, _)| format!("EPSG:{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match code {
        Some(c) if PRJ_NAMES.iter().any(|(k, _)| *k == c) => Ok(c),
        Some(c) => Err(format!(
            "{prj_name} names EPSG:{c}, which ekiben cannot reproject to WGS 84; export the layer in one of {}",
            known()
        )),
        None => Err(format!(
            "{prj_name} names the CRS {name:?}, which ekiben does not know; export the layer in one of {}",
            known()
        )),
    }
}

/// The name of a WKT's top-level node: `PROJCS["<name>", ...]`.
fn wkt_name(text: &str) -> String {
    text.split_once('"')
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or_else(|| text.chars().take(80).collect(), |(n, _)| n.to_string())
}

/// The code of an `AUTHORITY["EPSG", "n"]` (or WKT 2 `ID["EPSG", n]`) directly inside the
/// top-level node, not one of a nested datum or GEOGCS.
fn top_authority(text: &str) -> Option<u32> {
    let mut depth = 0usize;
    let mut in_str = false;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'[' | b'(' if !in_str => depth += 1,
            b']' | b')' if !in_str => depth = depth.saturating_sub(1),
            _ if !in_str && depth == 1 && text.is_char_boundary(i) => {
                let rest = &text[i..];
                let upper = rest.get(..10).unwrap_or(rest).to_ascii_uppercase();
                let skip = if upper.starts_with("AUTHORITY[") {
                    10
                } else if upper.starts_with("ID[") {
                    3
                } else {
                    0
                };
                if skip > 0 {
                    let inner = rest[skip..].split(']').next().unwrap_or("");
                    let mut parts = inner.split(',').map(|p| p.trim().trim_matches('"'));
                    if parts.next().is_some_and(|a| a.eq_ignore_ascii_case("EPSG")) {
                        return parts.next().and_then(|c| c.parse().ok());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Little- and big-endian reads that fail on a short buffer instead of panicking.
struct Bytes<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or("the file ends inside a record")?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    fn i32_be(&mut self) -> Result<i32, String> {
        self.arr().map(i32::from_be_bytes)
    }
    fn i32_le(&mut self) -> Result<i32, String> {
        self.arr().map(i32::from_le_bytes)
    }
    fn f64_le(&mut self) -> Result<f64, String> {
        self.arr().map(f64::from_le_bytes)
    }
    fn count(&mut self, what: &str, max: usize) -> Result<usize, String> {
        let n = self.i32_le()?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= max)
            .ok_or_else(|| format!("a record has {n} {what}, over the cap of {max}"))
    }
}

/// The geometry of every `.shp` record, in the file's CRS.
fn shapes(shp: &[u8], max_records: usize) -> Result<Vec<Json>, String> {
    let mut b = Bytes { buf: shp, at: 0 };
    if b.i32_be()? != 9994 {
        return Err("is not a shapefile (file code is not 9994)".to_string());
    }
    b.take(20)?;
    let words = b.i32_be()?;
    let len = usize::try_from(words)
        .ok()
        .and_then(|w| w.checked_mul(2))
        .filter(|l| *l >= 100 && *l <= shp.len())
        .ok_or("the header's file length does not match the file")?;
    b.take(4)?; // version
    let file_type = b.i32_le()?;
    b.take(64)?; // bounding box
    let mut out = Vec::new();
    while b.at < len {
        if out.len() >= max_records {
            return Err(format!("has more than {max_records} records"));
        }
        b.i32_be()?; // record number
        let words = b.i32_be()?;
        let content = usize::try_from(words)
            .ok()
            .and_then(|w| w.checked_mul(2))
            .ok_or("a record's content length is negative")?;
        let mut r = Bytes {
            buf: b.take(content)?,
            at: 0,
        };
        let kind = r.i32_le()?;
        if kind != 0 && kind != file_type {
            return Err(format!(
                "record {} is shape type {kind} in a file of shape type {file_type}",
                out.len() + 1
            ));
        }
        out.push(shape(kind, &mut r).map_err(|e| format!("record {}: {e}", out.len() + 1))?);
    }
    Ok(out)
}

/// A position: `[x, y]` or `[x, y, z]`.
fn pos(x: f64, y: f64, z: Option<f64>) -> Result<Json, String> {
    let finite = |v: f64| -> Result<Json, String> {
        serde_json::Number::from_f64(v)
            .map(Json::Number)
            .ok_or_else(|| "a coordinate is not a finite number".to_string())
    };
    let mut p = vec![finite(x)?, finite(y)?];
    if let Some(z) = z {
        p.push(finite(z)?);
    }
    Ok(Json::Array(p))
}

/// One shape's GeoJSON geometry (shape types per the ESRI Shapefile Technical Description).
fn shape(kind: i32, r: &mut Bytes) -> Result<Json, String> {
    let has_z = matches!(kind, 11 | 13 | 15 | 18);
    match kind {
        0 => Ok(Json::Null),
        1 | 11 | 21 => {
            let (x, y) = (r.f64_le()?, r.f64_le()?);
            let z = if has_z { Some(r.f64_le()?) } else { None };
            Ok(json!({"type": "Point", "coordinates": pos(x, y, z)?}))
        }
        8 | 18 | 28 => {
            r.take(32)?;
            let n = r.count("points", MAX_VERTICES)?;
            let pts = points(r, n, has_z)?;
            Ok(json!({"type": "MultiPoint", "coordinates": pts}))
        }
        3 | 13 | 23 | 5 | 15 | 25 => {
            r.take(32)?;
            let parts = r.count("parts", MAX_VERTICES)?;
            let n = r.count("points", MAX_VERTICES)?;
            let mut starts = Vec::with_capacity(parts);
            for _ in 0..parts {
                let s = r.i32_le()?;
                let s = usize::try_from(s)
                    .ok()
                    .filter(|s| *s <= n && starts.last().is_none_or(|p| s >= p))
                    .ok_or("a part starts outside the record's points")?;
                starts.push(s);
            }
            let pts = points(r, n, has_z)?;
            let mut lines: Vec<Vec<Json>> = Vec::with_capacity(parts);
            for (i, s) in starts.iter().enumerate() {
                let e = starts.get(i + 1).copied().unwrap_or(n);
                lines.push(pts[*s..e].to_vec());
            }
            if matches!(kind, 5 | 15 | 25) {
                polygon(lines)
            } else if lines.len() == 1 {
                Ok(json!({"type": "LineString", "coordinates": lines.remove(0)}))
            } else {
                Ok(json!({"type": "MultiLineString", "coordinates": lines}))
            }
        }
        31 => Err("MultiPatch shapes are not supported; export the layer as polygons".to_string()),
        other => Err(format!("unknown shape type {other}")),
    }
}

/// `n` points, then their Z range and values when `has_z`.
fn points(r: &mut Bytes, n: usize, has_z: bool) -> Result<Vec<Json>, String> {
    let mut xy = Vec::with_capacity(n);
    for _ in 0..n {
        xy.push((r.f64_le()?, r.f64_le()?));
    }
    let mut zs = Vec::new();
    if has_z {
        r.take(16)?;
        for _ in 0..n {
            zs.push(r.f64_le()?);
        }
    }
    xy.into_iter()
        .enumerate()
        .map(|(i, (x, y))| pos(x, y, zs.get(i).copied()))
        .collect()
}

fn xy(p: &Json) -> (f64, f64) {
    let c = |i: usize| p.get(i).and_then(Json::as_f64).unwrap_or(0.0);
    (c(0), c(1))
}

/// Twice the ring's signed area: positive when counterclockwise.
fn signed_area(ring: &[Json]) -> f64 {
    ring.windows(2)
        .map(|w| {
            let (a, b) = (xy(&w[0]), xy(&w[1]));
            a.0 * b.1 - b.0 * a.1
        })
        .sum()
}

/// Whether `p` lies inside `ring` (even-odd rule).
fn contains(ring: &[Json], p: (f64, f64)) -> bool {
    let mut inside = false;
    for w in ring.windows(2) {
        let (a, b) = (xy(&w[0]), xy(&w[1]));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Shapefile rings (outer clockwise, holes counterclockwise) as a Polygon or MultiPolygon with
/// RFC 7946 winding.
fn polygon(rings: Vec<Vec<Json>>) -> Result<Json, String> {
    let mut polys: Vec<Vec<Vec<Json>>> = Vec::new();
    let mut holes = Vec::new();
    for mut ring in rings {
        if ring.len() < 4 {
            return Err("a polygon ring has fewer than 4 points".to_string());
        }
        let outer = signed_area(&ring) <= 0.0;
        ring.reverse();
        if outer {
            polys.push(vec![ring]);
        } else {
            holes.push(ring);
        }
    }
    for hole in holes {
        let first = xy(&hole[0]);
        match polys.iter_mut().find(|p| contains(&p[0], first)) {
            Some(p) => p.push(hole),
            // A hole outside every outer ring is read as an outer ring of its own (GDAL does
            // the same), wound counterclockwise.
            None => {
                let mut ring = hole;
                ring.reverse();
                polys.push(vec![ring]);
            }
        }
    }
    Ok(if polys.len() == 1 {
        json!({"type": "Polygon", "coordinates": polys.remove(0)})
    } else {
        json!({"type": "MultiPolygon", "coordinates": polys})
    })
}

struct Field {
    name: String,
    kind: u8,
    len: usize,
    decimals: u8,
}

/// A dBASE table: its fields and the text encoding of its records.
struct Dbf<'a> {
    buf: &'a [u8],
    records: usize,
    header: usize,
    record_len: usize,
    fields: Vec<Field>,
    encoding: Encoding,
}

#[derive(Clone, Copy)]
enum Encoding {
    Known(&'static encoding_rs::Encoding),
    /// No `.cpg` and no language driver: UTF-8 when valid, else Windows-1252.
    Guess,
    /// A language driver no decoder here reads (a DOS code page): ASCII only.
    Unsupported(u8),
}

impl<'a> Dbf<'a> {
    fn parse(buf: &'a [u8], cpg: Option<&[u8]>) -> Result<Self, String> {
        let short = || "the table header is truncated".to_string();
        let head = buf.get(..32).ok_or_else(short)?;
        let records = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let header = u16::from_le_bytes([head[8], head[9]]) as usize;
        let record_len = u16::from_le_bytes([head[10], head[11]]) as usize;
        let encoding = match cpg {
            Some(c) => Encoding::Known(cpg_encoding(c)?),
            None => ldid_encoding(head[29]),
        };
        let mut fields = Vec::new();
        let mut at = 32;
        let mut width = 1; // the deletion flag
        while buf.get(at).is_some_and(|b| *b != 0x0D) && at + 32 <= header {
            let d = buf.get(at..at + 32).ok_or_else(short)?;
            let name_end = d[..11].iter().position(|b| *b == 0).unwrap_or(11);
            let name = String::from_utf8_lossy(&d[..name_end]).trim().to_string();
            let len = usize::from(d[16]);
            width += len;
            fields.push(Field {
                name,
                kind: d[11],
                len,
                decimals: d[17],
            });
            at += 32;
        }
        if width > record_len {
            return Err("its fields are wider than its records".to_string());
        }
        let end = records
            .checked_mul(record_len)
            .and_then(|n| n.checked_add(header))
            .ok_or_else(short)?;
        if end > buf.len() {
            return Err(format!(
                "declares {records} records but the file ends before them"
            ));
        }
        Ok(Self {
            buf,
            records,
            header,
            record_len,
            fields,
            encoding,
        })
    }

    /// Record `i`'s fields, `None` when it is marked deleted.
    fn record(&self, i: usize) -> Result<Option<serde_json::Map<String, Json>>, String> {
        let start = self.header + i * self.record_len;
        let rec = &self.buf[start..start + self.record_len];
        if rec[0] == b'*' {
            return Ok(None);
        }
        let mut at = 1;
        let mut out = serde_json::Map::new();
        for f in &self.fields {
            let raw = &rec[at..at + f.len];
            at += f.len;
            let v = self
                .value(f, raw)
                .map_err(|e| format!("record {} field {}: {e}", i + 1, f.name))?;
            out.insert(f.name.clone(), v);
        }
        Ok(Some(out))
    }

    fn text(&self, raw: &[u8]) -> Result<String, String> {
        let s = match self.encoding {
            Encoding::Known(e) => e.decode_without_bom_handling(raw).0.into_owned(),
            Encoding::Guess => match std::str::from_utf8(raw) {
                Ok(s) => s.to_string(),
                Err(_) => encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(raw)
                    .0
                    .into_owned(),
            },
            Encoding::Unsupported(ldid) => {
                if !raw.is_ascii() {
                    return Err(format!(
                        "the table's language driver 0x{ldid:02X} is a DOS code page ekiben cannot decode; add a .cpg naming the encoding, for example UTF-8"
                    ));
                }
                String::from_utf8_lossy(raw).into_owned()
            }
        };
        Ok(s.trim_end_matches(['\0', ' ']).trim_start().to_string())
    }

    fn value(&self, f: &Field, raw: &[u8]) -> Result<Json, String> {
        let t = self.text(raw)?;
        if f.kind != b'C' && (t.is_empty() || t.bytes().all(|b| b == b'*')) {
            return Ok(Json::Null);
        }
        Ok(match f.kind {
            b'C' => Json::String(t),
            b'N' | b'F' => {
                let int = (f.decimals == 0).then(|| t.parse::<i64>().ok()).flatten();
                match int {
                    Some(n) => json!(n),
                    None => t
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64)
                        .map(Json::Number)
                        .ok_or_else(|| format!("{t:?} is not a number"))?,
                }
            }
            b'L' => match t.as_bytes().first() {
                Some(b'T' | b't' | b'Y' | b'y') => Json::Bool(true),
                Some(b'F' | b'f' | b'N' | b'n') => Json::Bool(false),
                _ => Json::Null,
            },
            b'D' if t.len() == 8 && t.bytes().all(|b| b.is_ascii_digit()) => {
                Json::String(format!("{}-{}-{}", &t[..4], &t[4..6], &t[6..]))
            }
            b'D' => Json::Null,
            _ => Json::String(t),
        })
    }
}

/// The encoding a `.cpg` names: an encoding label (`UTF-8`, `ISO-8859-2`) or a Windows code
/// page number (`1250`, `CP1250`, `ANSI 1250`).
fn cpg_encoding(cpg: &[u8]) -> Result<&'static encoding_rs::Encoding, String> {
    let label = String::from_utf8_lossy(cpg).trim().to_string();
    let upper = label.to_ascii_uppercase();
    let digits = upper
        .trim_start_matches("ANSI")
        .trim_start_matches("CP")
        .trim();
    let by_number = digits.parse::<u16>().ok().and_then(|n| match n {
        65001 => Some("utf-8".to_string()),
        874 | 1250..=1258 => Some(format!("windows-{n}")),
        932 => Some("shift_jis".to_string()),
        936 => Some("gbk".to_string()),
        949 => Some("euc-kr".to_string()),
        950 => Some("big5".to_string()),
        866 => Some("ibm866".to_string()),
        28591..=28606 => Some(format!("iso-8859-{}", n - 28590)),
        _ => None,
    });
    by_number
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .or_else(|| encoding_rs::Encoding::for_label(label.as_bytes()))
        .ok_or_else(|| {
            format!("the .cpg names the encoding {label:?}, which ekiben cannot decode; use UTF-8 or a Windows code page such as 1252")
        })
}

/// The encoding of a dBASE language driver id (byte 29 of the header).
fn ldid_encoding(ldid: u8) -> Encoding {
    let label: &[u8] = match ldid {
        0x00 => return Encoding::Guess,
        0x03 | 0x57 | 0x58 | 0x59 => b"windows-1252",
        0xC8 => b"windows-1250",
        0xC9 => b"windows-1251",
        0xCA => b"windows-1254",
        0xCB => b"windows-1253",
        0x7D => b"windows-1255",
        0x7E => b"windows-1256",
        0x7C => b"windows-874",
        0x13 => b"shift_jis",
        0x4D => b"gbk",
        0x4E => b"euc-kr",
        0x4F => b"big5",
        0x26 => b"ibm866",
        other => return Encoding::Unsupported(other),
    };
    encoding_rs::Encoding::for_label(label).map_or(Encoding::Unsupported(ldid), Encoding::Known)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/shapefile/helsinki-3879-alueet.zip");

    /// A hostile .prj with a non-ASCII character outside quotes at the top level is read, not
    /// sliced mid-character.
    #[test]
    fn a_prj_with_a_non_ascii_character_at_the_top_level_is_read_without_a_panic() {
        assert_eq!(
            top_authority("PROJCS[\"x\",ä,AUTHORITY[\"EPSG\",\"3879\"]]"),
            Some(3879)
        );
        assert_eq!(top_authority("PROJCS[ää]"), None);
    }

    /// The fixture's members, `edit` applied, zipped again.
    fn rezip(edit: impl Fn(&mut Vec<(String, Vec<u8>)>)) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
        let mut members = Vec::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            members.push((f.name().unwrap().into_owned(), buf));
        }
        edit(&mut members);
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in members {
            out.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(&data).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn member<'a>(m: &'a mut [(String, Vec<u8>)], ext: &str) -> &'a mut Vec<u8> {
        &mut m.iter_mut().find(|(n, _)| n.ends_with(ext)).unwrap().1
    }

    /// The fixture with a 4-byte field (`at` bytes into the central directory header) of the
    /// member ending in `ext` set to `value`: 16 is its CRC-32, 24 its declared size.
    fn patch_central(ext: &str, at: usize, value: u32) -> Vec<u8> {
        let mut zip = rezip(|_| {});
        let h = (0..zip.len() - 46)
            .find(|&h| {
                let name_len = usize::from(u16::from_le_bytes([zip[h + 28], zip[h + 29]]));
                zip[h..h + 4] == *b"PK\x01\x02"
                    && zip[h + 46..h + 46 + name_len].ends_with(ext.as_bytes())
            })
            .expect("a central directory header for the member");
        zip[h + at..h + at + 4].copy_from_slice(&value.to_le_bytes());
        zip
    }

    #[test]
    fn a_member_whose_central_directory_lies_is_refused() {
        let cap = u32::try_from(MAX_ARCHIVE_UNPACKED_BYTES).unwrap();
        let err = features(&patch_central(".dbf", 24, cap + 1), 100).unwrap_err();
        assert_eq!(
            err,
            format!("shapefile unpacks to more than {MAX_ARCHIVE_UNPACKED_BYTES} bytes")
        );
        // A size declared too small: the zip reader refuses past it.
        let err = features(&patch_central(".shp", 24, 10), 100).unwrap_err();
        assert!(
            err.starts_with("failed to read ")
                && err.ends_with("larger than its declared uncompressed size"),
            "{err}"
        );
        let err = features(&patch_central(".prj", 16, 0xDEAD_BEEF), 100).unwrap_err();
        assert!(
            err.starts_with("failed to read ") && err.contains(".prj: "),
            "{err}"
        );
    }

    #[test]
    fn the_prj_names_its_crs_by_authority_or_by_name() {
        let esri = br#"PROJCS["EUREF-FIN_GK25FIN",GEOGCS["GCS_EUREF_FIN",DATUM["D_ETRS_1989",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],UNIT["Meter",1.0]]"#;
        assert_eq!(crs_of("a.prj", esri), Ok(3879));
        let ogc = br#"PROJCS["ETRS89 / TM35FIN(E,N)",GEOGCS["ETRS89",AUTHORITY["EPSG","4258"]],UNIT["metre",1],AUTHORITY["EPSG","3067"]]"#;
        assert_eq!(
            crs_of("a.prj", ogc),
            Ok(3067),
            "the top-level authority, not the GEOGCS's"
        );
        assert_eq!(
            crs_of("a.prj", br#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984"]]"#),
            Ok(4326)
        );
        let err = crs_of(
            "roads.prj",
            br#"PROJCS["KKJ_Finland_Uniform_Coordinate_System"]"#,
        )
        .unwrap_err();
        assert!(
            err.contains("roads.prj") && err.contains("KKJ_Finland_Uniform_Coordinate_System"),
            "{err}"
        );
        // A .prj with no quoted name is named by its first 80 characters.
        let err = crs_of("a.prj", "LOCAL_CS[x]".repeat(10).as_bytes()).unwrap_err();
        assert!(
            err.contains(&format!("the CRS {:?}", &"LOCAL_CS[x]".repeat(10)[..80])),
            "{err}"
        );
        let err = crs_of("a.prj", br#"PROJCS["x",AUTHORITY["EPSG","2393"]]"#).unwrap_err();
        assert!(
            err.contains("EPSG:2393") && err.contains("EPSG:3879"),
            "{err}"
        );
    }

    #[test]
    fn cpg_labels_and_language_drivers_name_their_encodings() {
        let name = |c: &[u8]| cpg_encoding(c).map(|e| e.name());
        assert_eq!(name(b"UTF-8\r\n"), Ok("UTF-8"));
        assert_eq!(name(b"1252"), Ok("windows-1252"));
        assert_eq!(name(b"ANSI 1250"), Ok("windows-1250"));
        assert_eq!(name(b"CP1257"), Ok("windows-1257"));
        assert_eq!(name(b"ISO-8859-2"), Ok("ISO-8859-2"));
        assert!(name(b"klingon").unwrap_err().contains("\"klingon\""));
        // A number that is no code page ekiben maps, and no encoding label either.
        assert!(name(b"437").unwrap_err().contains("\"437\""));
        assert!(matches!(ldid_encoding(0x57), Encoding::Known(e) if e.name() == "windows-1252"));
        assert!(matches!(ldid_encoding(0x00), Encoding::Guess));
        assert!(matches!(ldid_encoding(0x02), Encoding::Unsupported(0x02)));
    }

    #[test]
    fn text_decodes_by_the_language_driver_without_a_cpg() {
        // The fixture's .dbf in Windows-1252 with language driver 0x57 and no .cpg.
        let zip = rezip(|m| {
            m.retain(|(n, _)| !n.ends_with(".cpg"));
            let dbf = member(m, ".dbf");
            dbf[29] = 0x57;
            // "Kamppi–Etu-Töölö" in UTF-8 has an en dash (3 bytes) and three 2-byte umlauts;
            // Windows-1252 spells it in 16 bytes, padded back to the field.
            let utf8 = "Kamppi–Etu-Töölö".len();
            let cp1252 = b"Kamppi\x96Etu-T\xf6\xf6l\xf6";
            let mut field = cp1252.to_vec();
            field.resize(utf8, b' ');
            let byte_at = dbf.windows(6).position(|w| w == b"Kamppi").unwrap();
            dbf[byte_at..byte_at + utf8].copy_from_slice(&field);
        });
        let got = features(&zip, 100).unwrap();
        assert_eq!(
            got[0].to_json_value()["properties"]["NIMI"],
            "Kamppi–Etu-Töölö"
        );
        // A DOS code page this crate cannot decode refuses non-ASCII text by name.
        let zip = rezip(|m| {
            m.retain(|(n, _)| !n.ends_with(".cpg"));
            let dbf = member(m, ".dbf");
            dbf[29] = 0x02;
        });
        let err = features(&zip, 100).unwrap_err();
        assert!(err.contains("0x02") && err.contains(".cpg"), "{err}");
    }

    #[test]
    fn a_zip_without_one_layer_is_refused_by_name() {
        let err = features(&rezip(|m| m.retain(|(n, _)| !n.ends_with(".prj"))), 100).unwrap_err();
        assert!(err.contains("no .prj"), "{err}");
        let err = features(&rezip(|m| m.retain(|(n, _)| !n.ends_with(".dbf"))), 100).unwrap_err();
        assert!(err.contains("no .dbf"), "{err}");
        let err = features(
            &rezip(|m| {
                let shp = member(m, ".shp").clone();
                m.push(("other.shp".into(), shp));
            }),
            100,
        )
        .unwrap_err();
        assert!(
            err.contains("2 .shp files") && err.contains("other.shp"),
            "{err}"
        );
        let err = features(b"PK\x03\x04 not really", 100).unwrap_err();
        assert!(err.contains("shapefile zip"), "{err}");
    }

    #[test]
    fn caps_and_damaged_files_are_errors_not_panics() {
        let err = features(FIXTURE, 2).unwrap_err();
        assert!(err.contains("more than 2 records"), "{err}");
        // Every truncation of the .shp and the .dbf is an error, never a panic.
        for ext in [".shp", ".dbf"] {
            let len = std::cell::Cell::new(0);
            rezip(|m| len.set(member(m, ext).len()));
            let len = len.get();
            for cut in (0..len).step_by(7) {
                let zip = rezip(|m| member(m, ext).truncate(cut));
                let _ = features(&zip, 100);
            }
        }
        // A part count past the record's points, and a vertex count over the cap.
        let zip = rezip(|m| {
            let shp = member(m, ".shp");
            // The first record's content starts at 108; numParts at +36, numPoints at +40.
            shp[108 + 40..108 + 44].copy_from_slice(&(MAX_VERTICES as i32 + 1).to_le_bytes());
        });
        let err = features(&zip, 100).unwrap_err();
        assert!(err.contains("over the cap"), "{err}");
        // The record count of the .dbf and the .shp disagree.
        let zip = rezip(|m| member(m, ".dbf")[4] = 2);
        let err = features(&zip, 100).unwrap_err();
        assert!(err.contains("records but"), "{err}");
    }

    #[test]
    fn a_hole_outside_every_ring_reads_as_a_polygon_of_its_own() {
        let ring =
            |pts: &[(f64, f64)]| -> Vec<Json> { pts.iter().map(|(x, y)| json!([x, y])).collect() };
        // A counterclockwise ring alone: GDAL's reading, a polygon wound counterclockwise.
        let ccw = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]);
        let got = polygon(vec![ccw.clone()]).unwrap();
        assert_eq!(got["type"], "Polygon");
        assert_eq!(got["coordinates"][0], Json::Array(ccw));
        assert!(polygon(vec![ring(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0)])]).is_err());
    }

    /// Little-endian record content: the shape type, then `f64`s and `i32`s as given.
    fn content(kind: i32, f: &[f64], ints: &[i32], tail: &[f64]) -> Vec<u8> {
        let mut v = kind.to_le_bytes().to_vec();
        f.iter().for_each(|x| v.extend(x.to_le_bytes()));
        ints.iter().for_each(|x| v.extend(x.to_le_bytes()));
        tail.iter().for_each(|x| v.extend(x.to_le_bytes()));
        v
    }

    fn read_shape(buf: &[u8]) -> Result<Json, String> {
        let mut r = Bytes { buf, at: 0 };
        let kind = r.i32_le()?;
        shape(kind, &mut r)
    }

    #[test]
    fn every_shape_kind_reads_as_its_geojson_geometry() {
        assert_eq!(read_shape(&content(0, &[], &[], &[])), Ok(Json::Null));
        assert_eq!(
            read_shape(&content(11, &[1.0, 2.0, 3.0], &[], &[])),
            Ok(json!({"type": "Point", "coordinates": [1.0, 2.0, 3.0]}))
        );
        let bbox = [0.0; 4];
        // MultiPointZ: bbox, 2 points, xy, z range, z values.
        let mut mp = content(18, &bbox, &[2], &[1.0, 2.0, 3.0, 4.0]);
        [0.0f64, 9.0, 5.0, 6.0]
            .iter()
            .for_each(|x| mp.extend(x.to_le_bytes()));
        assert_eq!(
            read_shape(&mp),
            Ok(json!({"type": "MultiPoint", "coordinates": [[1.0, 2.0, 5.0], [3.0, 4.0, 6.0]]}))
        );
        // A PolyLine of one part is a LineString, of two a MultiLineString.
        let line = content(3, &bbox, &[1, 2, 0], &[0.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            read_shape(&line),
            Ok(json!({"type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0]]}))
        );
        let multi = content(
            3,
            &bbox,
            &[2, 4, 0, 2],
            &[0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0],
        );
        assert_eq!(
            read_shape(&multi),
            Ok(json!({"type": "MultiLineString",
                "coordinates": [[[0.0, 0.0], [1.0, 1.0]], [[2.0, 2.0], [3.0, 3.0]]]}))
        );
        for (buf, want) in [
            (
                content(3, &bbox, &[1, 2, 3], &[0.0; 4]),
                "a part starts outside the record's points",
            ),
            (
                content(3, &bbox, &[2, 2, 1, 0], &[0.0; 4]),
                "a part starts outside the record's points",
            ),
            (
                content(1, &[f64::NAN, 0.0], &[], &[]),
                "a coordinate is not a finite number",
            ),
            (
                content(11, &[0.0, 0.0, f64::INFINITY], &[], &[]),
                "a coordinate is not a finite number",
            ),
            (
                content(31, &[], &[], &[]),
                "MultiPatch shapes are not supported",
            ),
            (content(7, &[], &[], &[]), "unknown shape type 7"),
        ] {
            let err = read_shape(&buf).unwrap_err();
            assert!(err.contains(want), "{want}: {err}");
        }
    }

    /// A .shp of `kind` holding `records` (each the content of one record).
    fn shp(kind: i32, records: &[Vec<u8>]) -> Vec<u8> {
        let body: usize = records.iter().map(|r| 8 + r.len()).sum();
        let mut v = 9994i32.to_be_bytes().to_vec();
        v.extend([0u8; 20]);
        v.extend((((100 + body) / 2) as i32).to_be_bytes());
        v.extend(1000i32.to_le_bytes());
        v.extend(kind.to_le_bytes());
        v.extend([0u8; 64]);
        for (i, r) in records.iter().enumerate() {
            v.extend((i as i32 + 1).to_be_bytes());
            v.extend(((r.len() / 2) as i32).to_be_bytes());
            v.extend(r);
        }
        v
    }

    #[test]
    fn a_shp_header_and_its_record_types_are_checked() {
        let point = content(1, &[1.0, 2.0], &[], &[]);
        assert_eq!(
            shapes(&shp(1, &[point.clone(), content(0, &[], &[], &[])]), 10),
            Ok(vec![
                json!({"type": "Point", "coordinates": [1.0, 2.0]}),
                Json::Null
            ])
        );
        let mut wrong = shp(1, std::slice::from_ref(&point));
        wrong[3] = 0;
        assert_eq!(
            shapes(&wrong, 10),
            Err("is not a shapefile (file code is not 9994)".to_string())
        );
        assert_eq!(
            shapes(&shp(3, &[point]), 10),
            Err("record 1 is shape type 1 in a file of shape type 3".to_string())
        );
    }

    fn dbf(encoding: Encoding) -> Dbf<'static> {
        Dbf {
            buf: &[],
            records: 0,
            header: 0,
            record_len: 0,
            fields: Vec::new(),
            encoding,
        }
    }

    #[test]
    fn dbase_values_read_by_field_type() {
        let t = dbf(Encoding::Guess);
        let field = |kind: u8, decimals: u8| Field {
            name: "f".into(),
            kind,
            len: 10,
            decimals,
        };
        for (kind, decimals, raw, want) in [
            (b'N', 0, &b" 42 "[..], json!(42)),
            (b'N', 2, b"4.50", json!(4.5)),
            (b'F', 0, b"1e3", json!(1000.0)),
            (b'N', 0, b"****", Json::Null),
            (b'N', 0, b"   ", Json::Null),
            (b'L', 0, b"y", json!(true)),
            (b'L', 0, b"N", json!(false)),
            (b'L', 0, b"?", Json::Null),
            (b'D', 0, b"20261123", json!("2026-11-23")),
            (b'D', 0, b"2026-11", Json::Null),
            (b'M', 0, b"memo", json!("memo")),
            (b'C', 0, b"  a b  \0", json!("a b")),
        ] {
            assert_eq!(t.value(&field(kind, decimals), raw), Ok(want), "{raw:?}");
        }
        assert_eq!(
            t.value(&field(b'N', 0), b"12abc"),
            Err("\"12abc\" is not a number".to_string())
        );
        // Without a driver or .cpg, text that is not UTF-8 reads as Windows-1252.
        assert_eq!(t.text(b"T\xf6\xf6l\xf6"), Ok("Töölö".to_string()));
        // An undecodable DOS code page still reads ASCII.
        assert_eq!(
            dbf(Encoding::Unsupported(0x02)).text(b"abc"),
            Ok("abc".to_string())
        );
    }

    #[test]
    fn a_dbf_whose_fields_outgrow_its_records_is_refused() {
        let mut buf = vec![0u8; 32];
        buf[8..10].copy_from_slice(&65u16.to_le_bytes());
        buf[10..12].copy_from_slice(&5u16.to_le_bytes());
        let mut field = [0u8; 32];
        field[..1].copy_from_slice(b"A");
        field[11] = b'C';
        field[16] = 10;
        buf.extend(field);
        buf.push(0x0D);
        assert_eq!(
            Dbf::parse(&buf, None).err(),
            Some("its fields are wider than its records".to_string())
        );
    }

    #[test]
    fn code_page_numbers_and_every_language_driver_name_an_encoding() {
        let name = |c: &[u8]| cpg_encoding(c).map(|e| e.name());
        for (cpg, want) in [
            (&b"65001"[..], "UTF-8"),
            (b"874", "windows-874"),
            (b"932", "Shift_JIS"),
            (b"936", "GBK"),
            (b"949", "EUC-KR"),
            (b"950", "Big5"),
            (b"866", "IBM866"),
            (b"28592", "ISO-8859-2"),
        ] {
            assert_eq!(name(cpg), Ok(want), "{cpg:?}");
        }
        for (ldid, want) in [
            (0x03, "windows-1252"),
            (0xC8, "windows-1250"),
            (0xC9, "windows-1251"),
            (0xCA, "windows-1254"),
            (0xCB, "windows-1253"),
            (0x7D, "windows-1255"),
            (0x7E, "windows-1256"),
            (0x7C, "windows-874"),
            (0x13, "Shift_JIS"),
            (0x4D, "GBK"),
            (0x4E, "EUC-KR"),
            (0x4F, "Big5"),
            (0x26, "IBM866"),
        ] {
            assert!(
                matches!(ldid_encoding(ldid), Encoding::Known(e) if e.name() == want),
                "{ldid:#x}"
            );
        }
    }

    #[test]
    fn a_prj_names_its_crs_by_an_id_and_a_zip_holds_one_layer_within_the_caps() {
        assert_eq!(
            top_authority("PROJCRS[\"x\",ID[\"EPSG\",3067]]"),
            Some(3067)
        );
        assert_eq!(top_authority("PROJCRS[\"x\",ID[\"OGC\",1]]"), None);
        // A layer already in WGS 84 passes its geometry as it is, and a directory is skipped.
        let wgs84 = rezip(|m| {
            *member(m, ".prj") = br#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984"]]"#.to_vec();
            m.push(("sub/".into(), Vec::new()));
        });
        let raw = features(&wgs84, 100).unwrap();
        let shp = rezip(|_| {});
        let projected = features(&shp, 100).unwrap();
        assert_eq!(raw.len(), projected.len());
        assert_ne!(
            raw[0].to_json_value()["geometry"],
            projected[0].to_json_value()["geometry"]
        );
        // A geographic CRS read over projected metres puts the points off the globe.
        let etrs89 = rezip(|m| {
            *member(m, ".prj") = br#"GEOGCS["ETRS89",AUTHORITY["EPSG","4258"]]"#.to_vec();
        });
        let err = features(&etrs89, 100).unwrap_err();
        assert!(
            err.contains("record 1:") && err.contains("latitude is out of range"),
            "{err}"
        );
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..=crate::archive::MAX_ARCHIVE_MEMBERS {
            zw.start_file(format!("m{i}"), zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        let many = zw.finish().unwrap().into_inner();
        assert_eq!(
            features(&many, 100).err(),
            Some("shapefile zip has more than 10000 members".to_string())
        );
    }
}
