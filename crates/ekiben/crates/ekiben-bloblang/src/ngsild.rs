//! NGSI-LD helpers (E7, an ekiben extension Bento refuses): build the normalized attributes of
//! ETSI GS CIM 009 (4.5.2 Property, 4.5.3 Relationship, 4.7 GeoProperty, 4.5.18
//! LanguageProperty), convert an entity between the normalized and the simplified (keyValues,
//! 5.3.2.4) representations, and mint entity URNs.
//!
//! Pure: no I/O. The recursion over GeoJSON is bounded by [`MAX_GEO_DEPTH`]; every other walk
//! is one level deep over an entity's attributes. No input value makes these panic.

use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};

use crate::ast::ArgList;
use crate::executor::EvalContext;
use crate::methods::get_optional_arg;
use crate::value::Value;

/// The function names this module adds (an ekiben extension).
pub const EXTENSION_FUNCTIONS: &[&str] = &["ngsild_urn"];

/// The method names this module adds (an ekiben extension).
pub const EXTENSION_METHODS: &[&str] = &[
    "ngsild_geoproperty",
    "ngsild_key_values",
    "ngsild_language_property",
    "ngsild_normalized",
    "ngsild_property",
    "ngsild_relationship",
];

/// The deepest GeometryCollection nesting a GeoJSON value may have.
pub const MAX_GEO_DEPTH: usize = 8;

/// Members of an entity that are no attribute, kept as they are by both conversions.
const ENTITY_MEMBERS: &[&str] = &[
    "id",
    "type",
    "scope",
    "@context",
    "createdAt",
    "modifiedAt",
    "deletedAt",
];

/// The attribute types of the normalized representation.
const ATTRIBUTE_TYPES: &[&str] = &[
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "JsonProperty",
    "VocabProperty",
    "ListProperty",
    "ListRelationship",
];

fn obj(entries: Vec<(&str, Value)>) -> Value {
    let mut m = BTreeMap::new();
    for (k, v) in entries {
        m.insert(k.to_string(), v);
    }
    Value::Object(m.into())
}

fn is_absent(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Nothing | Value::Delete)
}

/// An optional argument: `None` when not given, null, or the empty string (the default).
fn opt_string(
    args: &ArgList,
    name: &str,
    pos: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Option<String>, String> {
    let Some(expr) = get_optional_arg(args, name, pos) else {
        return Ok(None);
    };
    match ctx.eval_expr(expr)? {
        v if is_absent(&v) => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => Ok(Some(s)),
        other => Err(format!(
            "parameter {name}: expected string value, got {}",
            other.type_name()
        )),
    }
}

/// `observed_at`: an RFC 3339 string or a timestamp, written as UTC RFC 3339.
fn opt_time(
    args: &ArgList,
    name: &str,
    pos: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Option<Value>, String> {
    let Some(expr) = get_optional_arg(args, name, pos) else {
        return Ok(None);
    };
    match ctx.eval_expr(expr)? {
        v if is_absent(&v) => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => match chrono::DateTime::parse_from_rfc3339(&s) {
            Ok(_) => Ok(Some(Value::String(s))),
            Err(_) => Err(format!(
                "parameter {name}: {s:?} is not an RFC 3339 date-time"
            )),
        },
        Value::Timestamp(ts) => Ok(Some(Value::String(
            ts.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::AutoSi, true),
        ))),
        other => Err(format!(
            "parameter {name}: expected timestamp value, got {}",
            other.type_name()
        )),
    }
}

/// `dataset_id`: a URI (it holds a scheme separator).
fn opt_dataset(
    args: &ArgList,
    pos: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Option<Value>, String> {
    match opt_string(args, "dataset_id", pos, ctx)? {
        None => Ok(None),
        Some(s) if is_uri(&s) => Ok(Some(Value::String(s))),
        Some(s) => Err(format!("parameter dataset_id: {s:?} is not a URI")),
    }
}

/// A URI as NGSI-LD takes one: a scheme (a letter, then letters, digits, `+`, `-`, `.`), a
/// colon and a non-empty rest without whitespace.
fn is_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !s.chars().any(char::is_whitespace)
}

fn is_ngsild_urn(v: &Value) -> bool {
    match v {
        Value::String(s) => s.starts_with("urn:ngsi-ld:") && is_uri(s),
        Value::Array(items) => !items.is_empty() && items.iter().all(is_ngsild_urn_str),
        _ => false,
    }
}

fn is_ngsild_urn_str(v: &Value) -> bool {
    matches!(v, Value::String(s) if s.starts_with("urn:ngsi-ld:") && is_uri(s))
}

/// The method family of this module, or `None` when `name` is none of its methods.
pub(crate) fn eval_ngsild_method(
    target: &Value,
    name: &str,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Option<Result<Value, String>> {
    Some(match name {
        "ngsild_property" => property(target, args, ctx),
        "ngsild_relationship" => relationship(target, args, ctx),
        "ngsild_geoproperty" => geoproperty(target, args, ctx),
        "ngsild_language_property" => language_property(target, args, ctx),
        "ngsild_key_values" => key_values(target),
        "ngsild_normalized" => normalized(target),
        _ => return None,
    })
}

fn property(
    target: &Value,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    if is_absent(target) {
        return Err("an NGSI-LD Property needs a value: expected a value, got null".to_string());
    }
    let mut entries = vec![
        ("type", Value::String("Property".into())),
        ("value", target.clone()),
    ];
    if let Some(u) = opt_string(args, "unit_code", 0, ctx)? {
        entries.push(("unitCode", Value::String(u)));
    }
    if let Some(t) = opt_time(args, "observed_at", 1, ctx)? {
        entries.push(("observedAt", t));
    }
    if let Some(d) = opt_dataset(args, 2, ctx)? {
        entries.push(("datasetId", d));
    }
    Ok(obj(entries))
}

fn relationship(
    target: &Value,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let ok = match target {
        Value::String(s) => is_uri(s),
        Value::Array(items) => {
            !items.is_empty()
                && items
                    .iter()
                    .all(|i| matches!(i, Value::String(s) if is_uri(s)))
        }
        _ => false,
    };
    if !ok {
        return Err(match target {
            Value::String(s) if s.is_empty() => {
                "an NGSI-LD Relationship needs an entity id: got an empty string".to_string()
            }
            Value::String(_) | Value::Array(_) => {
                "an NGSI-LD Relationship needs an entity id that is a URI (or a non-empty array of them)"
                    .to_string()
            }
            other => format!("expected string value, got {}", other.type_name()),
        });
    }
    let mut entries = vec![
        ("type", Value::String("Relationship".into())),
        ("object", target.clone()),
    ];
    if let Some(t) = opt_time(args, "observed_at", 0, ctx)? {
        entries.push(("observedAt", t));
    }
    if let Some(d) = opt_dataset(args, 1, ctx)? {
        entries.push(("datasetId", d));
    }
    Ok(obj(entries))
}

fn geoproperty(
    target: &Value,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    if !matches!(target, Value::Object(_)) {
        return Err(format!("expected object value, got {}", target.type_name()));
    }
    check_geometry(target, 0)?;
    let mut entries = vec![
        ("type", Value::String("GeoProperty".into())),
        ("value", target.clone()),
    ];
    if let Some(t) = opt_time(args, "observed_at", 0, ctx)? {
        entries.push(("observedAt", t));
    }
    Ok(obj(entries))
}

fn language_property(
    target: &Value,
    args: &ArgList,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    let lang = opt_string(args, "lang", 0, ctx)?;
    let map = match (target, lang) {
        (Value::String(s), Some(lang)) => {
            if !is_lang_tag(&lang) {
                return Err(format!("parameter lang: {lang:?} is not a language tag"));
            }
            obj(vec![(lang.as_str(), Value::String(s.clone()))])
        }
        (Value::String(_), None) => {
            return Err(
                "a string becomes a LanguageProperty only with lang (or give a language map)"
                    .to_string(),
            )
        }
        (Value::Object(m), None) => {
            if m.is_empty() {
                return Err("the language map is empty".to_string());
            }
            for (k, v) in m.iter() {
                let strings = match v {
                    Value::String(_) => true,
                    Value::Array(items) => items.iter().all(|i| matches!(i, Value::String(_))),
                    _ => false,
                };
                if !(is_lang_tag(k) || k == "@none") || !strings {
                    return Err(format!(
                        "the language map entry {k:?} must be a language tag holding a string"
                    ));
                }
            }
            target.clone()
        }
        (Value::Object(_), Some(_)) => {
            return Err(
                "lang is for a string target; a language map names its languages".to_string(),
            )
        }
        (other, _) => return Err(format!("expected string value, got {}", other.type_name())),
    };
    Ok(obj(vec![
        ("type", Value::String("LanguageProperty".into())),
        ("languageMap", map),
    ]))
}

/// A BCP 47 shaped tag: letters, digits and `-`, starting with 2 or 3 letters.
fn is_lang_tag(s: &str) -> bool {
    let first = s.split('-').next().unwrap_or("");
    (2..=8).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && s.len() <= 35
        && s.split('-')
            .all(|p| !p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn number(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Int(i) => *i as f64,
        Value::Uint(u) => *u as f64,
        Value::Float(f) => *f,
        Value::Number(s) => s.parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn items_of<'a>(v: &'a Value, what: &str) -> Result<&'a [Value], String> {
    match v {
        Value::Array(items) => Ok(items.as_slice()),
        other => Err(format!(
            "GeoJSON {what} must be an array, got {}",
            other.type_name()
        )),
    }
}

/// A position: longitude and latitude (WGS 84), with an optional altitude.
fn position(v: &Value) -> Result<Vec<f64>, String> {
    let items = items_of(v, "position")?;
    let nums: Option<Vec<f64>> = items.iter().map(number).collect();
    let nums = nums.ok_or_else(|| "a GeoJSON position holds numbers only".to_string())?;
    if !(2..=3).contains(&nums.len()) {
        return Err(format!(
            "a GeoJSON position holds 2 or 3 numbers, got {}",
            nums.len()
        ));
    }
    let (lon, lat) = match nums.as_slice() {
        [lon, lat, ..] => (*lon, *lat),
        _ => return Err("a GeoJSON position holds 2 or 3 numbers".to_string()),
    };
    if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err(format!(
            "the GeoJSON position [{lon}, {lat}] is outside longitude -180..180, latitude -90..90"
        ));
    }
    Ok(nums)
}

fn positions(v: &Value, min: usize, what: &str) -> Result<Vec<Vec<f64>>, String> {
    let items = items_of(v, what)?;
    if items.len() < min {
        return Err(format!(
            "a GeoJSON {what} needs at least {min} positions, got {}",
            items.len()
        ));
    }
    items.iter().map(position).collect()
}

fn ring(v: &Value) -> Result<(), String> {
    let ps = positions(v, 4, "linear ring")?;
    if ps.first() != ps.last() {
        return Err("a GeoJSON linear ring must end where it starts".to_string());
    }
    Ok(())
}

fn polygon(v: &Value) -> Result<(), String> {
    let rings = items_of(v, "polygon")?;
    if rings.is_empty() {
        return Err("a GeoJSON Polygon needs at least one ring".to_string());
    }
    rings.iter().try_for_each(ring)
}

/// Whether `v` is a GeoJSON geometry (RFC 7946 3.1) with sound coordinates.
pub fn check_geometry(v: &Value, depth: usize) -> Result<(), String> {
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
    if kind == "GeometryCollection" {
        let geoms = m
            .get("geometries")
            .ok_or_else(|| "a GeometryCollection needs geometries".to_string())?;
        return items_of(geoms, "geometries")?
            .iter()
            .try_for_each(|g| check_geometry(g, depth + 1));
    }
    let coords = m
        .get("coordinates")
        .ok_or_else(|| format!("a GeoJSON {kind} needs coordinates"))?;
    match kind {
        "Point" => position(coords).map(|_| ()),
        "MultiPoint" => positions(coords, 1, "MultiPoint").map(|_| ()),
        "LineString" => positions(coords, 2, "LineString").map(|_| ()),
        "MultiLineString" => items_of(coords, "MultiLineString")?
            .iter()
            .try_for_each(|l| positions(l, 2, "LineString").map(|_| ())),
        "Polygon" => polygon(coords),
        "MultiPolygon" => items_of(coords, "MultiPolygon")?
            .iter()
            .try_for_each(polygon),
        other => Err(format!("{other:?} is not a GeoJSON geometry type")),
    }
}

fn is_geometry(v: &Value) -> bool {
    matches!(v, Value::Object(m) if matches!(m.get("type"), Some(Value::String(_))))
        && check_geometry(v, 0).is_ok()
}

fn entity_map(target: &Value) -> Result<&BTreeMap<String, Value>, String> {
    match target {
        Value::Object(m) => Ok(&**m),
        other => Err(format!("expected object value, got {}", other.type_name())),
    }
}

/// One attribute in the simplified representation (5.3.2.4).
fn simplify(v: &Value) -> Value {
    match v {
        Value::Array(items)
            if !items.is_empty()
                && items.iter().all(|i| {
                    matches!(i, Value::Object(m) if matches!(m.get("type"), Some(Value::String(t)) if ATTRIBUTE_TYPES.contains(&t.as_str())))
                }) =>
        {
            // A multi-attribute: one simplified value per datasetId.
            let mut dataset = BTreeMap::new();
            for item in items.iter() {
                let key = match item {
                    Value::Object(m) => match m.get("datasetId") {
                        Some(Value::String(d)) => d.clone(),
                        _ => "@none".to_string(),
                    },
                    _ => "@none".to_string(),
                };
                dataset.insert(key, simplify(item));
            }
            obj(vec![("dataset", Value::Object(dataset.into()))])
        }
        Value::Object(m) => {
            let get = |k: &str| m.get(k).cloned().unwrap_or(Value::Null);
            match m.get("type") {
                Some(Value::String(t)) => match t.as_str() {
                    "Property" | "GeoProperty" => get("value"),
                    "Relationship" => get("object"),
                    "LanguageProperty" => obj(vec![("languageMap", get("languageMap"))]),
                    "JsonProperty" => obj(vec![("json", get("json"))]),
                    "VocabProperty" => obj(vec![("vocab", get("vocab"))]),
                    "ListProperty" => get("valueList"),
                    "ListRelationship" => get("objectList"),
                    _ => v.clone(),
                },
                _ => v.clone(),
            }
        }
        other => other.clone(),
    }
}

fn key_values(target: &Value) -> Result<Value, String> {
    let m = entity_map(target)?;
    let mut out = BTreeMap::new();
    for (k, v) in m.iter() {
        if ENTITY_MEMBERS.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        } else {
            out.insert(k.clone(), simplify(v));
        }
    }
    Ok(Value::Object(out.into()))
}

fn is_attribute(v: &Value) -> bool {
    matches!(v, Value::Object(m) if matches!(m.get("type"), Some(Value::String(t)) if ATTRIBUTE_TYPES.contains(&t.as_str())))
}

/// One attribute in the normalized representation, from its simplified value.
fn normalize(key: &str, v: &Value, in_dataset: bool) -> Result<Value, String> {
    if is_absent(v) {
        return Err(format!(
            "attribute {key:?} is null: NGSI-LD has no null attribute value"
        ));
    }
    if is_attribute(v) {
        return Ok(v.clone());
    }
    if let Value::Object(m) = v {
        if m.len() == 1 {
            if let Some(map) = m.get("languageMap") {
                return Ok(obj(vec![
                    ("type", Value::String("LanguageProperty".into())),
                    ("languageMap", map.clone()),
                ]));
            }
            if let Some(json) = m.get("json") {
                return Ok(obj(vec![
                    ("type", Value::String("JsonProperty".into())),
                    ("json", json.clone()),
                ]));
            }
            if let Some(vocab) = m.get("vocab") {
                return Ok(obj(vec![
                    ("type", Value::String("VocabProperty".into())),
                    ("vocab", vocab.clone()),
                ]));
            }
            if let (Some(Value::Object(ds)), false) = (m.get("dataset"), in_dataset) {
                let mut items = Vec::with_capacity(ds.len());
                for (dataset_id, inner) in ds.iter() {
                    let mut attr = normalize(key, inner, true)?;
                    if dataset_id != "@none" {
                        if let Value::Object(am) = &mut attr {
                            let mut copy: BTreeMap<String, Value> = (**am).clone();
                            copy.insert("datasetId".to_string(), Value::String(dataset_id.clone()));
                            attr = Value::Object(copy.into());
                        }
                    }
                    items.push(attr);
                }
                return Ok(Value::Array(items.into()));
            }
        }
    }
    if is_geometry(v) {
        return Ok(obj(vec![
            ("type", Value::String("GeoProperty".into())),
            ("value", v.clone()),
        ]));
    }
    if key.starts_with("ref") && is_ngsild_urn(v) {
        return Ok(obj(vec![
            ("type", Value::String("Relationship".into())),
            ("object", v.clone()),
        ]));
    }
    Ok(obj(vec![
        ("type", Value::String("Property".into())),
        ("value", v.clone()),
    ]))
}

fn normalized(target: &Value) -> Result<Value, String> {
    let m = entity_map(target)?;
    let mut out = BTreeMap::new();
    for (k, v) in m.iter() {
        if ENTITY_MEMBERS.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        } else {
            out.insert(k.clone(), normalize(k, v, false)?);
        }
    }
    Ok(Value::Object(out.into()))
}

/// RFC 8141 NSS: unreserved, sub-delims, `:` and `@` (and `/`) stay; every other byte,
/// `%` included, is percent-encoded.
fn nss_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
            );
        if keep {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn urn_part(
    args: &ArgList,
    name: &str,
    pos: usize,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<String, String> {
    let Some(expr) = get_optional_arg(args, name, pos) else {
        return Err(format!("missing parameter: {name}"));
    };
    match ctx.eval_expr(expr)? {
        Value::String(s) if s.is_empty() => Err(format!("parameter {name} must not be empty")),
        Value::String(s) => Ok(s),
        v @ (Value::Int(_) | Value::Uint(_) | Value::Number(_)) if name == "id" => {
            Ok(v.to_string_coerced())
        }
        other => Err(format!(
            "parameter {name}: expected string value, got {}",
            other.type_name()
        )),
    }
}

/// `ngsild_urn(type, id, prefix: "")`: `urn:ngsi-ld:{type}:{prefix}{id}`. A URN names an entity
/// and nothing else (ADR-N-041): no space, organization or right is read from its segments.
pub(crate) fn eval_urn(args: &ArgList, ctx: &mut EvalContext<'_, '_>) -> Result<Value, String> {
    let kind = urn_part(args, "type", 0, ctx)?;
    if !kind
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "parameter type: {kind:?} must hold only letters, digits, '-', '_' and '.'"
        ));
    }
    let id = urn_part(args, "id", 1, ctx)?;
    let prefix = opt_string(args, "prefix", 2, ctx)?.unwrap_or_default();
    Ok(Value::String(format!(
        "urn:ngsi-ld:{kind}:{}{}",
        nss_encode(&prefix),
        nss_encode(&id)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nss() {
        assert_eq!(nss_encode("a:b@c/d"), "a:b@c/d");
        assert_eq!(nss_encode("100%"), "100%25");
        assert_eq!(nss_encode("a b#c?d"), "a%20b%23c%3Fd");
        assert_eq!(nss_encode("é"), "%C3%A9");
    }

    #[test]
    fn uris_and_tags() {
        assert!(is_uri("urn:ngsi-ld:T:1"));
        assert!(is_uri("https://x"));
        assert!(!is_uri("no-scheme"));
        assert!(!is_uri("1x:y"));
        assert!(!is_uri("urn:a b"));
        assert!(is_lang_tag("en"));
        assert!(is_lang_tag("zh-Hant-TW"));
        assert!(!is_lang_tag("e"));
        assert!(!is_lang_tag("en_US"));
    }

    #[test]
    fn deep_collections_stop() {
        let mut g = obj(vec![
            ("type", Value::String("Point".into())),
            (
                "coordinates",
                Value::Array(vec![Value::Int(1), Value::Int(2)].into()),
            ),
        ]);
        for _ in 0..(MAX_GEO_DEPTH + 2) {
            g = obj(vec![
                ("type", Value::String("GeometryCollection".into())),
                ("geometries", Value::Array(vec![g].into())),
            ]);
        }
        assert!(check_geometry(&g, 0).is_err());
    }
}
