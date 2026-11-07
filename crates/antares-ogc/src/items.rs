// SPDX-License-Identifier: EUPL-1.2
//! `/collections/{c}/items` and `/collections/{c}/items/{featureId}` (mapping rows C8 to C20):
//! the query parameters, the Query Entities requests they become, and the Features of the
//! answers.

use crate::crs::{bbox_geometry, corners};
use crate::filter::Lowered;
use crate::model::{Collection, Time};
use crate::terms::Terms;
use crate::Problem;
use antares_ops::residual::{self, Expr, Lit, Path, Spatial};
use antares_ops::{Caller, Pushdown, Refusal, Scanned, ViewOps};
use axum::http::StatusCode;
use serde_json::{json, Map, Value};
use std::fmt::Write as _;

/// Features per page when the request names no `limit` (Part 1 `fc-limit-definition`).
pub(crate) const DEFAULT_LIMIT: u64 = 10;
/// The largest `limit`, the `maximum` the API definition declares: a page holds up to it,
/// read from the store in pages of its `max_limit` (C10); above it the request is 400.
pub(crate) const MAX_LIMIT: u64 = 10_000;

/// The query parameters of `items`, read and checked.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Params {
    pub limit: u64,
    pub bbox: Option<[f64; 4]>,
    pub datetime: Option<(Option<String>, Option<String>)>,
    pub next: Option<String>,
    /// `sortby` (S5): Attribute IRIs, `true` descending.
    pub sortby: Vec<(String, bool)>,
    /// `properties` (S6): the Attribute IRIs to keep, all when `None`.
    pub properties: Option<Vec<String>>,
    /// `profile` (S7).
    pub profile: Profile,
    /// `crs` (R2): the CRS the answer's coordinates are in, CRS84 by default.
    pub crs: &'static str,
    /// The NGSI-LD options beside the standard's own (ADR-0028 rule 6), by name without the
    /// prefix, values verbatim.
    pub ngsild: Vec<(&'static str, String)>,
    /// `filter` and the queryables named as parameters (F1, F2), names checked.
    pub filter: Option<ogc_cql2::Expr>,
    /// The NGSI-LD `lang` a `LanguageProperty` is read in: the caller's `Accept-Language`.
    pub lang: Option<String>,
    /// Read the page without the store's whole count (T-6508), which walks every matching
    /// row: the [`Page`] states neither `matched` nor a `next` from it.
    pub uncounted: bool,
}

/// The `ngsild.` options a collection's `items` declares (ADR-0028 rule 6): each narrows.
pub(crate) const NGSILD: [&str; 8] = [
    "q",
    "scopeQ",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "pick",
    "omit",
];

/// How a Feature writes a Relationship to a collection the view shows (Part 5
/// `profiles-for-feature-references`); one to a class the view does not show stays its URN.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    /// The target's `featureId`, its URN.
    #[default]
    Key,
    /// The target Feature's URI.
    Uri,
    /// A link object: `href` the target Feature's URI, `title` its URN.
    Link,
}

/// The feature-reference profiles by id.
const PROFILES: [(&str, Profile); 3] = [
    ("rel-as-key", Profile::Key),
    ("rel-as-uri", Profile::Uri),
    ("rel-as-link", Profile::Link),
];

/// Where the profile URIs live; Part 5 writes `OGC` and `ogc` both, so it is matched in any case.
const PROFILE_BASE: &str = "http://www.opengis.net/def/profile/OGC/0/";

/// A profile as Part 5 names it: its id, a safe CURIE `[ogc-profile:id]`, or its URI.
fn profile(text: &str) -> Option<Profile> {
    let id = text
        .strip_prefix("[ogc-profile:")
        .and_then(|t| t.strip_suffix(']'))
        .or_else(|| {
            text.get(..PROFILE_BASE.len())
                .filter(|head| head.eq_ignore_ascii_case(PROFILE_BASE))
                .and_then(|_| text.get(PROFILE_BASE.len()..))
        })
        .unwrap_or(text);
    PROFILES
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, p)| *p)
}

/// The URI of a profile, as a `profile` link names it.
pub(crate) fn profile_uri(p: Profile) -> String {
    let id = PROFILES
        .iter()
        .find(|(_, q)| *q == p)
        .map_or("rel-as-key", |(id, _)| id);
    format!("{PROFILE_BASE}{id}")
}

/// `sortby` (OGC API Records and Features Part 8 drafts): `[+|-]name` by commas; a `+` the
/// form decoding turned into a space is ascending too. Only sortable properties (S5).
fn sortby(c: &Collection, text: &str) -> Option<Vec<(String, bool)>> {
    text.split(',')
        .map(|key| {
            let (name, desc) = match key.strip_prefix('-') {
                Some(name) => (name, true),
                None => (key.trim_start_matches([' ', '+']), false),
            };
            let p = c.properties.iter().find(|p| p.name == name && p.sortable)?;
            Some((p.iri.clone(), desc))
        })
        .collect()
}

/// `properties` (S6): property names by commas, none when empty.
fn properties(c: &Collection, text: &str) -> Option<Vec<String>> {
    text.split(',')
        .filter(|name| !text.is_empty() || !name.is_empty())
        .map(|name| Some(c.properties.iter().find(|p| p.name == name)?.iri.clone()))
        .collect()
}

pub(crate) fn bad(detail: &'static str) -> Problem {
    Problem(StatusCode::BAD_REQUEST, detail)
}

/// Reads the query of an `items/{featureId}` request: `properties` and `profile` only.
pub(crate) fn item_params(c: &Collection, pairs: &[(String, String)]) -> Result<Params, Problem> {
    if pairs
        .iter()
        .all(|(name, _)| matches!(name.as_str(), "properties" | "profile" | "crs"))
    {
        params(c, pairs)
    } else {
        Err(bad(
            "the request names a query parameter this resource does not define",
        ))
    }
}

/// Reads the query of an `items` request (C9 to C13, S5 to S7): each parameter once, none
/// unknown; a property the view hides reads as one the collection does not have.
pub(crate) fn params(c: &Collection, pairs: &[(String, String)]) -> Result<Params, Problem> {
    let mut p = Params {
        limit: DEFAULT_LIMIT,
        crs: crate::crs::CRS84,
        ..Params::default()
    };
    let mut seen: Vec<&str> = Vec::new();
    let mut raw = crate::filter::Raw::default();
    for (name, value) in pairs {
        if seen.contains(&name.as_str()) {
            return Err(bad("a query parameter is repeated"));
        }
        seen.push(name);
        match name.as_str() {
            "limit" => {
                let n: u64 = value.parse().map_err(|_| bad("limit is an integer from 1 to 10000"))?;
                if !(1..=MAX_LIMIT).contains(&n) {
                    return Err(bad("limit is an integer from 1 to 10000"));
                }
                p.limit = n;
            }
            "bbox" => p.bbox = Some(bbox(value).ok_or_else(|| bad("bbox is 4 or 6 numbers in CRS84: lower corner, upper corner"))?),
            "datetime" => p.datetime = Some(datetime(value).ok_or_else(|| bad("datetime is an RFC 3339 date-time or an interval start/end, '..' or empty for an open end"))?),
            "next" => p.next = Some(value.clone()),
            "crs" => p.crs = crate::crs::read(value).ok_or(bad(crate::crs::UNSUPPORTED))?,
            // R3: both CRS name the same axes, so the box reads the same.
            "bbox-crs" => {
                crate::crs::read(value).ok_or(bad(crate::crs::UNSUPPORTED))?;
            }
            "sortby" => p.sortby = sortby(c, value).ok_or_else(|| bad("sortby names properties of the collection's sortables, each [+|-]name"))?,
            "properties" => p.properties = Some(properties(c, value).ok_or_else(|| bad("properties names properties of the collection's schema"))?),
            "profile" => p.profile = profile(value).ok_or_else(|| bad("profile is one of rel-as-key, rel-as-uri and rel-as-link"))?,
            "filter" => raw.filter = Some(value.clone()),
            "filter-lang" => raw.lang = Some(value.clone()),
            "filter-crs" => raw.crs = Some(value.clone()),
            other => match other.strip_prefix("ngsild.").and_then(|o| NGSILD.iter().find(|n| **n == o)) {
                Some(option) => p.ngsild.push((option, value.clone())),
                None if crate::filter::is_parameter(c, other) => raw.equals.push((other.to_owned(), value.clone())),
                None => return Err(bad("the request names a query parameter this resource does not define")),
            },
        }
    }
    if p.option("pick").is_some() && p.option("omit").is_some() {
        return Err(bad(
            "ngsild.pick and ngsild.omit both select the properties; name one",
        ));
    }
    // `ngsild.q` is joined as `(filter);(datetime);(q)`: one that does not parse alone would
    // close its group and OR past the other conjuncts, an output filter or not (F-ANT5-012)
    if let Some(q) = p.option("q") {
        antares_ops::outputs::one_q(q).map_err(bad)?;
    }
    if let Some(o) = &c.output {
        o.filter.scope_with(p.option("scopeQ")).map_err(bad)?;
    }
    p.filter = crate::filter::read(c, &raw)?;
    if p.properties.is_some() && p.option("pick").is_some() {
        return Err(bad(
            "properties and ngsild.pick both select the properties; name one",
        ));
    }
    Ok(p)
}

impl Params {
    /// The value of the `ngsild.` option `name`.
    fn option(&self, name: &str) -> Option<&str> {
        self.ngsild
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether an `ngsild.geo*` option holds the request's one geo query.
    fn ngsild_geo(&self) -> bool {
        self.ngsild
            .iter()
            .any(|(n, _)| n.starts_with("geo") || *n == "coordinates")
    }
}

/// `minx,miny,maxx,maxy` or `minx,miny,minz,maxx,maxy,maxz` in CRS84; heights are dropped.
fn bbox(text: &str) -> Option<[f64; 4]> {
    let numbers: Vec<f64> = text
        .split(',')
        .map(|v| v.trim().parse().ok().filter(|v: &f64| v.is_finite()))
        .collect::<Option<_>>()?;
    corners(&numbers)
}

/// An instant (both ends the same) or `start/end` with `..` or empty for an open end; both
/// ends open is refused (Part 1 Req 26 F).
fn datetime(text: &str) -> Option<(Option<String>, Option<String>)> {
    let end = |s: &str| -> Option<Option<String>> {
        match s {
            "" | ".." => Some(None),
            s if is_date_time(s) => Some(Some(s.to_owned())),
            _ => None,
        }
    };
    match text.split_once('/') {
        None if is_date_time(text) => Some((Some(text.to_owned()), Some(text.to_owned()))),
        None => None,
        Some((a, b)) => match (end(a)?, end(b)?) {
            (None, None) => None,
            (from, to) => Some((from, to)),
        },
    }
}

/// RFC 3339 `date-time`: `YYYY-MM-DDThh:mm:ss[.frac](Z|±hh:mm)`, `T` and `Z` in either case.
fn is_date_time(s: &str) -> bool {
    let digits = |from: usize, to: usize| {
        s.get(from..to)
            .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
    };
    let at = |i: usize, c: u8| {
        s.as_bytes()
            .get(i)
            .is_some_and(|x| x.eq_ignore_ascii_case(&c))
    };
    let head = digits(0, 4)
        && at(4, b'-')
        && digits(5, 7)
        && at(7, b'-')
        && digits(8, 10)
        && at(10, b'T')
        && digits(11, 13)
        && at(13, b':')
        && digits(14, 16)
        && at(16, b':')
        && digits(17, 19);
    let rest = s.get(19..).unwrap_or_default();
    let zone = match rest.strip_prefix('.') {
        Some(fraction) => {
            let zone = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
            (zone.len() < fraction.len()).then_some(zone)
        }
        None => Some(rest),
    };
    let offset = |z: &str| {
        let b = z.as_bytes();
        b.len() == 6
            && matches!(b.first(), Some(b'+' | b'-'))
            && b.get(3) == Some(&b':')
            && [1, 2, 4, 5]
                .iter()
                .all(|&i| b.get(i).is_some_and(u8::is_ascii_digit))
    };
    head && zone.is_some_and(|z| z.eq_ignore_ascii_case("z") || offset(z))
        && residual::DateTime::parse(s).is_some()
}

pub(crate) use antares_ops::percent::encode;

/// What the `datetime` parameter adds (C13, C14): the temporal geometry intersects the
/// interval, or the Feature has none. The residual decides by instant, an offset read as
/// written and a value without a time (`2026-10-31`) as the whole UTC day; the `q` beside it
/// compares text, so its bounds move out by [`crate::filter::slack`] to select a superset.
fn time_q(
    time: Option<&Time>,
    terms: &Terms,
    (from, to): &(Option<String>, Option<String>),
) -> (Option<String>, Option<Expr>) {
    let at = |s: &Option<String>| s.as_deref().and_then(residual::DateTime::parse);
    let (from, to) = (at(from), at(to));
    // (`>=`, the earlier end) or (`<=`, the later end), of one Attribute.
    let bound = |iri: &str, later: bool, at: Option<residual::DateTime>| {
        let at = at?;
        let t = terms.term(iri);
        let (op, cmp) = if later {
            ("<=", residual::Cmp::Le)
        } else {
            (">=", residual::Cmp::Ge)
        };
        let exact = Expr::Cmp(
            cmp,
            Box::new(Expr::Path(Path::value(t))),
            Box::new(Expr::Lit(Lit::DateTime(at))),
        );
        let q = crate::filter::slack(at, later).map(|b| format!("{t}{op}{b}"));
        Some((q, exact))
    };
    // The bounds of one Attribute, or the Feature lacks it.
    let either = |iri: &str, bounds: Vec<(Option<String>, Expr)>| {
        if bounds.is_empty() {
            return None;
        }
        let t = terms.term(iri);
        let q: Vec<String> = bounds.iter().filter_map(|(q, _)| q.clone()).collect();
        let q = (!q.is_empty()).then(|| format!("(({})|!{t})", q.join(";")));
        let exact = bounds.into_iter().map(|(_, e)| e).collect();
        Some((
            q,
            Expr::Or(vec![
                Expr::And(exact),
                Expr::IsNull(Box::new(Expr::Path(Path::value(t)))),
            ]),
        ))
    };
    let parts: Vec<(Option<String>, Expr)> = match time {
        None => Vec::new(),
        Some(Time::Instant(t)) => either(
            t,
            bound(t, false, from)
                .into_iter()
                .chain(bound(t, true, to))
                .collect(),
        )
        .into_iter()
        .collect(),
        Some(Time::Interval(start, end)) => [
            start
                .as_ref()
                .and_then(|s| either(s, bound(s, true, to).into_iter().collect())),
            end.as_ref()
                .and_then(|e| either(e, bound(e, false, from).into_iter().collect())),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    if parts.is_empty() {
        return (None, None);
    }
    let q: Vec<String> = parts.iter().filter_map(|(q, _)| q.clone()).collect();
    let exact: Vec<Expr> = parts.into_iter().map(|(_, e)| e).collect();
    ((!q.is_empty()).then(|| q.join(";")), conjunction(exact))
}

/// The `orderBy` keys of `sortby` (S5).
fn order(terms: &Terms, p: &Params) -> Vec<String> {
    p.sortby
        .iter()
        .map(|(iri, desc)| format!("{};{}", terms.term(iri), if *desc { "desc" } else { "asc" }))
        .collect()
}

/// The `pick` of `properties` (S6) or `ngsild.pick`, and `ngsild.omit`; the `pick` of
/// `properties` keeps `id`, `type` and the geometry, which every Feature carries, less what
/// `ngsild.omit` names, as the broker takes no `omit` beside a `pick`.
fn shape(c: &Collection, terms: &Terms, p: &Params) -> String {
    let omitted: Vec<&str> = p
        .option("omit")
        .map_or_else(Vec::new, |o| o.split(',').collect());
    let pick = p.properties.as_ref().map(|keep| {
        let names: Vec<&str> = ["id", "type"]
            .into_iter()
            .chain(c.geometry.iter().map(|g| terms.term(g)))
            .chain(keep.iter().map(|iri| terms.term(iri)))
            .filter(|name| !omitted.contains(name))
            .collect();
        format!("&pick={}", encode(&names.join(",")))
    });
    let omit = if pick.is_some() { None } else { Some("omit") };
    let options = ["pick"]
        .into_iter()
        .chain(omit)
        .filter_map(|name| p.option(name).map(|v| format!("&{name}={}", encode(v))))
        .chain(scope_q(c, p).map(|v| format!("&scopeQ={}", encode(&v))));
    let lang = p.lang.iter().map(|l| format!("&lang={}", encode(l)));
    pick.into_iter().chain(options).chain(lang).collect()
}

/// The `q` of the collection's output filter (T-5216), a conjunct of every read of it.
fn output_q(c: &Collection) -> Option<String> {
    let q = c.output.as_ref()?.filter.q.as_ref()?;
    Some(format!("({q})"))
}

/// The `scopeQ` of a read: the output filter's (T-5216) joined with the request's, which
/// [`params`] has checked can be joined; the filter's alone otherwise, never wider.
fn scope_q(c: &Collection, p: &Params) -> Option<String> {
    match &c.output {
        Some(o) => o
            .filter
            .scope_with(p.option("scopeQ"))
            .unwrap_or(o.filter.scope_q.clone()),
        None => p.option("scopeQ").map(str::to_owned),
    }
}

/// One Query Entities an `items` request reads: the store's pushdown and the residual the
/// scan evaluates over its answer (ADR-0028): the filter's conjuncts NGSI-LD cannot say, and
/// the `bbox` when an `ngsild.geo*` option or the filter holds the request's one geo query.
pub(crate) struct Part {
    pub query: String,
    pub residual: Option<Expr>,
}

/// The conjunction of residual conjuncts, none when there are none.
fn conjunction(mut all: Vec<Expr>) -> Option<Expr> {
    match all.len() {
        0 => None,
        1 => all.pop(),
        _ => Some(Expr::And(all)),
    }
}

/// The Query Entities of one `items` request: one, or two when `bbox` must also keep the
/// Features without a geometry (C12). Disjoint, served in this order. `ngsild.q` and the
/// filter's `q` join the request's own `q` with `;`, the other `ngsild.` options are pushed as
/// they are. A part with a residual selects by [`attrs`], not by `pick`/`omit`, so the scan reads
/// what the residual needs and drops it again.
pub(crate) fn targets(c: &Collection, terms: &Terms, p: &Params, f: &Lowered) -> Vec<Part> {
    let bbox = p.bbox.zip(c.geometry.as_deref());
    let (time, when) = p
        .datetime
        .as_ref()
        .map_or((None, None), |d| time_q(c.time.as_ref(), terms, d));
    let residual = !f.residual.is_empty() || when.is_some() || (bbox.is_some() && p.ngsild_geo());
    let projection = if residual {
        scope_q(c, p)
            .map(|v| format!("&scopeQ={}", encode(&v)))
            .unwrap_or_default()
    } else {
        shape(c, terms, p)
    };
    let mut base = format!("type={}{}{projection}", encode(&c.type_iri), f.id_param());
    if p.ngsild_geo() {
        for name in ["georel", "geometry", "coordinates", "geoproperty"] {
            if let Some(value) = p.option(name) {
                let _ = write!(base, "&{name}={}", encode(value));
            }
        }
    }
    if let (Some(geo), Some(geometry)) = (&f.geo, c.geometry.as_deref()) {
        let _ = write!(
            base,
            "&georel={}&geometry={}&coordinates={}&geoproperty={}",
            geo.rel,
            geo.geometry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            encode(
                &geo.geometry
                    .get("coordinates")
                    .map(Value::to_string)
                    .unwrap_or_default()
            ),
            encode(geometry)
        );
    }
    let theirs = p.option("q").map(|q| format!("({q})"));
    let with_q = |extra: Option<String>| {
        let q: Vec<String> = time
            .iter()
            .cloned()
            .chain(extra)
            .chain(theirs.clone())
            .chain(output_q(c))
            .chain(f.q.iter().map(|q| format!("({q})")))
            .collect();
        if q.is_empty() {
            base.clone()
        } else {
            format!("{base}&q={}", encode(&q.join(";")))
        }
    };
    let filtered = |extra: Option<Expr>| {
        conjunction(
            extra
                .into_iter()
                .chain(when.iter().cloned())
                .chain(f.residual.iter().cloned())
                .collect(),
        )
    };
    let Some((bbox, geometry)) = bbox else {
        return vec![Part {
            query: with_q(None),
            residual: filtered(None),
        }];
    };
    let (kind, coordinates) = bbox_geometry(bbox);
    let located = if p.ngsild_geo() {
        Part {
            query: with_q(None),
            residual: filtered(Some(Expr::Spatial(
                Spatial::Intersects,
                Box::new(Expr::Path(Path::value(terms.term(geometry)))),
                Box::new(Expr::Lit(Lit::Geometry(
                    json!({ "type": kind, "coordinates": coordinates }),
                ))),
            ))),
        }
    } else {
        Part {
            query: format!(
                "{}&georel=intersects&geometry={kind}&coordinates={}&geoproperty={}",
                with_q(None),
                encode(&coordinates.to_string()),
                encode(geometry)
            ),
            residual: filtered(None),
        }
    };
    vec![
        located,
        Part {
            query: with_q(Some(format!("!{}", terms.term(geometry)))),
            residual: filtered(None),
        },
    ]
}

/// The Attributes a part with a residual selects: `properties` (with the geometry), else
/// `ngsild.pick`, else every Attribute of the collection but `ngsild.omit`; all when none.
fn attrs(c: &Collection, terms: &Terms, p: &Params) -> Option<Vec<String>> {
    let omitted: Vec<&str> = p
        .option("omit")
        .map_or_else(Vec::new, |o| o.split(',').collect());
    let names: Vec<String> = if let Some(keep) = &p.properties {
        c.geometry
            .iter()
            .chain(keep)
            .map(|iri| terms.term(iri).to_owned())
            .collect()
    } else if let Some(pick) = p.option("pick") {
        pick.split(',')
            .filter(|n| !matches!(*n, "id" | "type"))
            .map(str::to_owned)
            .collect()
    } else if omitted.is_empty() {
        return None;
    } else {
        c.geometry
            .iter()
            .chain(c.properties.iter().map(|prop| &prop.iri))
            .map(|iri| terms.term(iri).to_owned())
            .collect()
    };
    Some(
        names
            .into_iter()
            .filter(|n| !omitted.contains(&n.as_str()))
            .collect(),
    )
}

/// The token of the next page: the part and the offset in it, and a digest of the request
/// without its `next` and `limit` and of the view, so a token read under another view or query
/// does not match and a token read with another page size does (C16). ponytail: std `SipHash`; a forged token does no more than an offset, every
/// page is narrowed by the store again.
pub(crate) fn digest(view: &str, collection: &str, pairs: &[(String, String)]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (view, collection).hash(&mut hasher);
    for pair in pairs.iter().filter(|(n, _)| n != "next" && n != "limit") {
        pair.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn token(part: usize, offset: usize, digest: u64) -> String {
    format!("{part}.{offset}.{digest:016x}")
}

pub(crate) fn read_token(text: &str, digest: u64, parts: usize) -> Option<(usize, usize)> {
    let mut it = text.split('.');
    let (Some(part), Some(offset), Some(tag), None) = (it.next(), it.next(), it.next(), it.next())
    else {
        return None;
    };
    let part: usize = part.parse().ok()?;
    (tag == format!("{digest:016x}") && part < parts).then_some(())?;
    Some((part, offset.parse().ok()?))
}

/// One page of Features (C8 to C18).
pub(crate) struct Page {
    pub features: Vec<Value>,
    /// `numberMatched`: exact, or `None` when a residual's answer is larger than the budget
    /// lets one request count (ADR-0028 rule 5).
    pub matched: Option<u64>,
    pub next: Option<String>,
}

/// One page through [`ViewOps::scan`] (ADR-0028 rules 4 and 5): each part read from the
/// token's upstream offset until the page holds `limit`, the `next` token the part and the
/// upstream offset just past the last Feature served; a short page with `next` is a page.
/// Current state only: `features::page` sends a history collection to `history::page`.
pub(crate) async fn page(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    p: &Params,
    digest: u64,
    refs: Refs<'_>,
) -> Result<Page, Problem> {
    let mut cursor = Cursor::new(c, p, digest)?;
    let mut features = Vec::new();
    while let Some(more) = cursor.chunk(view, caller, c, refs).await? {
        features.extend(more);
    }
    Ok(Page {
        features,
        matched: cursor.matched,
        next: cursor.next,
    })
}

/// [`page`] read one store read at a time (T-6036): each [`Cursor::chunk`] asks the store for
/// at most [`ViewOps::page_size`] Entities, so a reader that writes each chunk out holds one
/// upstream page of Features, never the whole page. The store reads are those one [`page`]
/// makes, and a residual's [`antares_ops::RESIDUAL_SCAN`] budget is the part's, across chunks.
pub(crate) struct Cursor {
    terms: Terms,
    parts: Vec<Part>,
    order: Vec<String>,
    attrs: Option<Vec<String>>,
    /// The part and its upstream offset the page starts at, from its `next` token.
    start: (usize, usize),
    limit: usize,
    digest: u64,
    /// [`Params::uncounted`].
    uncounted: bool,
    /// The part being read and where its read stands, `None` before its first read.
    part: usize,
    reading: Option<Reading>,
    served: usize,
    /// `numberMatched` of the parts read so far, see [`Page::matched`].
    pub matched: Option<u64>,
    /// The `next` token, once a part set it.
    pub next: Option<String>,
}

/// Where the read of one part stands.
struct Reading {
    residual: Option<residual::Program>,
    pushdown: Pushdown,
    /// The upstream offset of the part's next read.
    pos: usize,
    first: bool,
    /// What is left of the part's residual budget.
    budget: usize,
    count: Option<usize>,
}

impl Cursor {
    /// The parts a page reads and where it starts.
    pub(crate) fn new(c: &Collection, p: &Params, digest: u64) -> Result<Self, Problem> {
        let terms = Terms::of(c);
        // One geoQ per request (4.10): the filter's spatial conjunct is pushed only when
        // neither `bbox` nor an `ngsild.geo*` option holds it.
        let geo_free = !p.ngsild_geo() && (p.bbox.is_none() || c.geometry.is_none());
        let lowered = match &p.filter {
            Some(f) => crate::filter::lower(c, &terms, f, geo_free)?,
            None => Lowered::default(),
        };
        // `id` conjuncts no Entity id meets: nothing to ask the store.
        let (parts, start) = if lowered.ids.as_ref().is_some_and(Vec::is_empty) {
            (Vec::new(), (0, 0))
        } else {
            let parts = targets(c, &terms, p, &lowered);
            let start = match &p.next {
                None => (0, 0),
                Some(t) => read_token(t, digest, parts.len())
                    .ok_or_else(|| bad("the next token does not belong to this request"))?,
            };
            (parts, start)
        };
        Ok(Self {
            order: order(&terms, p),
            attrs: attrs(c, &terms, p),
            terms,
            parts,
            start,
            limit: usize::try_from(p.limit).unwrap_or(usize::MAX),
            digest,
            uncounted: p.uncounted,
            part: 0,
            reading: None,
            served: 0,
            matched: Some(0),
            next: None,
        })
    }

    /// Whether every part was read.
    pub(crate) fn done(&self) -> bool {
        self.part >= self.parts.len()
    }

    /// The Features of the next store read, `None` once every part was read.
    pub(crate) async fn chunk(
        &mut self,
        view: &ViewOps,
        caller: &Caller,
        c: &Collection,
        refs: Refs<'_>,
    ) -> Result<Option<Vec<Value>>, Problem> {
        let i = self.part;
        let Some(part) = self.parts.get(i) else {
            return Ok(None);
        };
        let mut r = match self.reading.take() {
            Some(r) => r,
            None => self.enter(part)?,
        };
        let room = if i < self.start.0 {
            0
        } else {
            self.limit.saturating_sub(self.served)
        };
        let size = room.min(view.page_size());
        let page = self.store_page(&r, size);
        let (docs, next, count) = match &r.residual {
            None => scanned(view.scan(caller, &r.pushdown, None, page).await)?,
            Some(program) => scanned(
                view.scan_within(caller, &r.pushdown, program, page, &mut r.budget)
                    .await,
            )?,
        };
        let got = docs.len();
        self.served = self.served.saturating_add(got);
        r.first = false;
        r.count = r.count.or(count);
        r.pos = next.unwrap_or(r.pos.saturating_add(got));
        let simple = r.residual.is_some();
        let features = docs
            .into_iter()
            .map(|d| {
                Value::Object(feature(
                    c,
                    &self.terms,
                    &if simple { simplified(d) } else { d },
                    refs,
                ))
            })
            .collect();
        // The part goes on while its reads come back full and the page has room; a residual
        // read also ends with the store's answer.
        if got == size && size < room && (!simple || next.is_some()) {
            self.reading = Some(r);
            return Ok(Some(features));
        }
        let count = match &r.residual {
            _ if self.uncounted => None,
            None => r.count,
            // ponytail: the count reads the residual's answer again, within the budget; a
            // count kept beside the cursor when one request reads both.
            Some(program) => {
                let whole = antares_ops::Page {
                    start: 0,
                    size: 0,
                    whole: true,
                };
                match view.scan(caller, &r.pushdown, Some(program), whole).await {
                    Err(Refusal::TooManyResults { .. }) => None,
                    answer => scanned(answer)?.2,
                }
            }
        };
        self.matched = self
            .matched
            .zip(count)
            .map(|(m, n)| m.saturating_add(u64::try_from(n).unwrap_or(u64::MAX)));
        self.part = i.saturating_add(1);
        if i < self.start.0 || self.next.is_some() {
            return Ok(Some(features));
        }
        // Without a residual the whole count says whether more remains past this page.
        let next = match r.residual {
            Some(_) => next,
            None => count.filter(|n| r.pos < *n).map(|_| r.pos),
        };
        // A part after the one that filled the page answers its whole count and, holding
        // anything, starts the next page at its offset 0.
        self.next = next.map(|next| token(i, next, self.digest));
        Ok(Some(features))
    }

    /// The next store read: a part's first read also counts it, unless the page is uncounted.
    fn store_page(&self, r: &Reading, size: usize) -> antares_ops::Page {
        antares_ops::Page {
            start: r.pos,
            size,
            whole: !self.uncounted && r.residual.is_none() && r.first,
        }
    }

    /// The first read of `part`: its residual compiled and its pushdown.
    fn enter(&self, part: &Part) -> Result<Reading, Problem> {
        let residual = match &part.residual {
            None => None,
            Some(expr) => {
                let now = now().ok_or(Problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the clock reads no time",
                ))?;
                Some(residual::compile(expr, now).map_err(|e| refused(&e))?)
            }
        };
        let pushdown = Pushdown {
            query: if residual.is_some() {
                part.query.clone()
            } else {
                format!("{}&format=simplified", part.query)
            },
            order: self.order.clone(),
            attrs: residual.as_ref().and_then(|_| self.attrs.clone()),
            sys_attrs: false,
            context: self.terms.context(),
        };
        Ok(Reading {
            residual,
            pushdown,
            pos: if self.part == self.start.0 {
                self.start.1
            } else {
                0
            },
            first: true,
            budget: antares_ops::RESIDUAL_SCAN,
            count: None,
        })
    }
}

/// Why the residual refused the filter: too large (400), a construct it does not read with the
/// standard's meaning (501), or one no filter of this collection can be (400).
pub(crate) fn refused(e: &residual::Error) -> Problem {
    match e {
        residual::Error::TooComplex => bad(antares_ops::TOO_COMPLEX),
        residual::Error::Unsupported(_) => Problem(
            StatusCode::NOT_IMPLEMENTED,
            "the filter uses a construct this endpoint does not evaluate (a LIKE pattern that is no literal, or a pattern construct)",
        ),
        residual::Error::Invalid(_) => bad("the filter is not valid: a LIKE pattern ending in its escape, or a geometry that is no GeoJSON geometry in CRS84"),
    }
}

/// The time a residual's `now()` reads.
pub(crate) fn now() -> Option<residual::DateTime> {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_micros();
    residual::DateTime::from_unix_micros(i64::try_from(micros).ok()?)
}

/// The Entities of a scan, its upstream continuation and its count.
type Read = (Vec<Value>, Option<usize>, Option<usize>);

/// The Entities of a scan, its upstream continuation and its count; a refused `ngsild.`
/// option is the broker's own 400.
fn scanned(answer: Result<Scanned, Refusal>) -> Result<Read, Problem> {
    match answer {
        // no `ngsild.join` is forwarded (NGSILD), so no join walk is cut short
        Ok(Scanned::Page {
            entities,
            next,
            count,
            ..
        }) => Ok((entities, next, count)),
        // A Tenant that holds nothing yet answers 404 (5.5.10): an empty collection.
        Ok(Scanned::Answer(answer)) if answer.status() == StatusCode::NOT_FOUND => {
            Ok((Vec::new(), None, Some(0)))
        }
        Ok(Scanned::Answer(answer)) if answer.status() == StatusCode::BAD_REQUEST => Err(bad(
            "the query is not valid NGSI-LD; an ngsild. parameter takes NGSI-LD syntax",
        )),
        Ok(Scanned::Answer(answer)) => Err(Problem(answer.status(), "the view refused the query")),
        Err(refused) => Err(Problem(refused.status(), "the view refused the query")),
    }
}

/// A normalized Entity in the simplified form `feature` reads (TS 104 175 4.5.4): each
/// Attribute its value, an Attribute of several instances the `dataset` map by `datasetId`
/// (`@none` for the default one), as the broker writes it.
fn simplified(mut doc: Value) -> Value {
    let one = value_of;
    for (name, attr) in doc.as_object_mut().into_iter().flat_map(|m| m.iter_mut()) {
        if matches!(name.as_str(), "id" | "type" | "scope" | "@context") {
            continue;
        }
        let flat = match &*attr {
            Value::Array(instances) => {
                let dataset: Map<String, Value> = instances
                    .iter()
                    .map(|i| {
                        let id = i
                            .get("datasetId")
                            .and_then(Value::as_str)
                            .unwrap_or("@none");
                        (id.to_owned(), one(i))
                    })
                    .collect();
                json!({ "dataset": dataset })
            }
            single => one(single),
        };
        *attr = flat;
    }
    doc
}

/// One normalized Attribute instance's value as the simplified form writes it.
pub(crate) fn value_of(attr: &Value) -> Value {
    match attr {
        Value::Object(m) => ["value", "object", "valueList", "objectList"]
            .into_iter()
            .find_map(|k| m.get(k).cloned())
            .or_else(|| {
                ["languageMap", "json", "vocab"]
                    .into_iter()
                    .find_map(|k| m.get(k).map(|v| json!({ k: v })))
            })
            .unwrap_or_else(|| attr.clone()),
        other => other.clone(),
    }
}

/// One Entity of `c` as a `GeoJSON` Feature (M2 to M5): `id` the URN, `geometry` the primary
/// geometry or null, `properties` the other Attributes the view shows, by slot name.
pub(crate) fn feature(
    c: &Collection,
    terms: &Terms,
    doc: &Value,
    refs: Refs,
) -> Map<String, Value> {
    let geometry = c
        .geometry
        .as_ref()
        .and_then(|g| doc.get(terms.term(g)))
        .map_or(Value::Null, |g| {
            antares_ops::rewound(served("GeoProperty", g))
        });
    let properties: Map<String, Value> = c
        .properties
        .iter()
        .filter_map(|p| {
            let value = served(&p.kind, doc.get(terms.term(&p.iri))?);
            let value = match (&p.target, refs.profile) {
                (Some(target), Profile::Uri | Profile::Link) => {
                    refer(&value, &|urn| refs.render(target, urn))
                }
                _ => value,
            };
            Some((p.name.clone(), value))
        })
        .collect();
    let id = doc.get("id").cloned().unwrap_or(Value::Null);
    let links = id.as_str().map(|urn| refs.links(&c.id, urn));
    let mut feature = Map::from_iter([
        ("type".to_owned(), json!("Feature")),
        ("id".to_owned(), id),
        ("geometry".to_owned(), geometry),
        ("properties".to_owned(), Value::Object(properties)),
    ]);
    if let Some(links) = links {
        feature.insert("links".to_owned(), Value::from(links));
    }
    feature
}

/// A simplified Attribute value in the shape `/schema` declares for its kind (Part 5
/// `returnables-and-receivables`), so a Feature read and written back stores it unchanged: an
/// Attribute of several instances its default one (`@none`, as the broker's own `GeoJSON` picks;
/// none is `null`), a `VocabProperty` its term, a `JsonProperty` its JSON, a `LanguageProperty`
/// its language map. ponytail: a Property whose own value is an object of the one member
/// `dataset` reads as instances, as the simplified form cannot tell them apart.
pub(crate) fn served(kind: &str, value: &Value) -> Value {
    let value = match value {
        Value::Object(m) if m.len() == 1 => match m.get("dataset") {
            Some(Value::Object(instances)) => {
                instances.get("@none").cloned().unwrap_or(Value::Null)
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    };
    let member = match kind {
        "VocabProperty" => "vocab",
        "JsonProperty" => "json",
        "LanguageProperty" => "languageMap",
        _ => return value,
    };
    match value {
        Value::Object(mut m) if m.len() == 1 && m.contains_key(member) => {
            m.remove(member).unwrap_or(Value::Null)
        }
        other => other,
    }
}

/// How one request writes references (S7): the profile and the endpoint's URI.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Refs<'a> {
    pub profile: Profile,
    pub base: &'a str,
    /// Whether the view offers Retrieve Entity, so a Feature's canonical Entity answers (EP-38).
    pub alternate: bool,
}

impl Refs<'_> {
    /// EP-38: a Feature's `alternate` link to its Entity under the same view's NGSI-LD root,
    /// when the view offers Retrieve Entity (never the Tenant's API, never a link to a 403), and
    /// its `describedby` link to the collection's schema.
    pub(crate) fn links(self, collection: &str, urn: &str) -> Vec<Value> {
        let entity = self
            .base
            .strip_suffix("/ogc/features")
            .filter(|_| self.alternate)
            .map(|root| {
                json!({
                    "rel": "alternate",
                    "type": "application/ld+json",
                    "title": urn,
                    "href": format!("{root}/ngsi-ld/v1/entities/{}", encode(urn)),
                })
            });
        let schema = crate::link(
            "describedby",
            crate::SCHEMA,
            &format!("{}/collections/{collection}/schema", self.base),
        );
        entity.into_iter().chain([schema]).collect()
    }

    /// A reference to `urn` in the collection `target` the view shows, as `rel-as-uri` or
    /// `rel-as-link` write it (`rel-as-key` keeps the URN and never calls this).
    fn render(self, target: &str, urn: &str) -> Value {
        let href = format!("{}/collections/{target}/items/{}", self.base, encode(urn));
        if self.profile == Profile::Link {
            json!({"href": href, "title": urn})
        } else {
            Value::from(href)
        }
    }
}

/// A Relationship's object (a URN, or an array of them) with each URN rendered.
fn refer(value: &Value, render: &dyn Fn(&str) -> Value) -> Value {
    match value {
        Value::String(urn) => render(urn),
        Value::Array(objects) => Value::Array(objects.iter().map(|o| refer(o, render)).collect()),
        other => other.clone(),
    }
}

/// One Feature by its id, if the view shows it and it is of the collection's type (C19): a
/// Query Entities by id and type, so the view and its engine narrow it like a page. An id that
/// is no URI names no Feature either. Current state only, as [`page`].
pub(crate) async fn one(
    view: &ViewOps,
    caller: &Caller,
    c: &Collection,
    id: &str,
    p: &Params,
    refs: Refs<'_>,
) -> Result<Map<String, Value>, Problem> {
    let terms = Terms::of(c);
    let pushdown = Pushdown {
        query: format!(
            "id={}&type={}&format=simplified{}{}",
            encode(id),
            encode(&c.type_iri),
            shape(c, &terms, p),
            output_q(c)
                .map(|q| format!("&q={}", encode(&q)))
                .unwrap_or_default()
        ),
        context: terms.context(),
        ..Pushdown::default()
    };
    let page = antares_ops::Page {
        start: 0,
        size: 1,
        whole: false,
    };
    match scanned(view.scan(caller, &pushdown, None, page).await) {
        Ok((docs, _, _)) => docs.first().map(|d| feature(c, &terms, d, refs)),
        Err(Problem(StatusCode::BAD_REQUEST, _)) => None,
        Err(refused) => return Err(refused),
    }
    .ok_or(Problem(
        StatusCode::NOT_FOUND,
        "no such Feature in this collection",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6508: an uncounted page asks the store for no whole count; `items` still does.
    #[test]
    fn an_uncounted_page_asks_the_store_for_no_count() -> Result<(), String> {
        let c = car();
        let whole = |p: Params| -> Result<bool, String> {
            let cursor = Cursor::new(&c, &p, 0).map_err(|e| e.1.to_owned())?;
            let part = cursor.parts.first().ok_or("one part")?;
            let reading = cursor.enter(part).map_err(|e| e.1.to_owned())?;
            Ok(cursor.store_page(&reading, 10).whole)
        };
        assert!(whole(params(&c, &[]).map_err(|e| e.1.to_owned())?)?);
        let uncounted = Params {
            uncounted: true,
            ..params(&c, &[]).map_err(|e| e.1.to_owned())?
        };
        assert!(!whole(uncounted)?);
        Ok(())
    }

    fn prop(name: &str, sortable: bool) -> crate::model::Prop {
        crate::model::Prop {
            name: name.into(),
            iri: format!("https://e.org/{name}"),
            schema: json!({"type": "number"}),
            role: None,
            unit: None,
            description: None,
            sortable,
            kind: "Property".into(),
            reference: false,
            target: None,
        }
    }

    fn car() -> Collection {
        Collection {
            history: false,
            output: None,
            id: "Car".into(),
            description: None,
            type_iri: "https://e.org/Car".into(),
            geometry: Some(crate::model::LOCATION.into()),
            properties: vec![prop("speed", true), prop("tags", false)],
            time: None,
        }
    }

    fn pairs(q: &[(&str, &str)]) -> Vec<(String, String)> {
        q.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn a_q_that_closes_its_group_is_refused_with_or_without_an_output_filter() {
        let output = serde_json::from_value(json!({"name": "fast", "filter": {"q": "speed>9"},
                                                   "sources": [{"type": "Car"}]}));
        let filtered = Collection {
            output: output.ok(),
            ..car()
        };
        let escape = pairs(&[("ngsild.q", "speed<1)|(speed>0")]);
        assert!(params(&filtered, &escape).is_err());
        assert!(params(&filtered, &pairs(&[("ngsild.q", "speed<1")])).is_ok());
        // F-ANT5-012: without one too, it would OR past the filter's conjunct
        assert!(params(&car(), &escape).is_err());
    }

    #[test]
    fn parameters_are_read_and_checked() {
        assert_eq!(
            params(&car(), &[]).map(|p| p.limit).ok(),
            Some(DEFAULT_LIMIT)
        );
        assert_eq!(
            params(&car(), &pairs(&[("limit", "10000")]))
                .map(|p| p.limit)
                .ok(),
            Some(MAX_LIMIT),
            "the declared maximum is served whole"
        );
        for bad in [
            vec![("limit", "0")],
            vec![("limit", "10001")],
            vec![("limit", "x")],
            vec![("limit", "1"), ("limit", "2")],
            vec![("crs", "x")],
            vec![("bbox", "1,2,3")],
            vec![("bbox", "1,2,3,x")],
            vec![("bbox", "0,1,1,0")],
            vec![("bbox", "0,0,181,1")],
            vec![("bbox", "0,0,5,1,1,4")],
            vec![("bbox", "-181,0,1,1")],
            vec![("bbox", "0,-91,1,0")],
            vec![("bbox", "0,0,1,91")],
            vec![("datetime", "yesterday")],
            vec![("datetime", "../..")],
            vec![("datetime", "/")],
            vec![("datetime", "2026-01-01")],
            vec![("datetime", "2026-01-01T00:00:00Z/tomorrow")],
        ] {
            assert!(params(&car(), &pairs(&bad)).is_err(), "{bad:?}");
        }
        let p = params(
            &car(),
            &pairs(&[
                ("bbox", "170,-10,-170,10"),
                ("datetime", "2026-01-01T00:00:00Z/.."),
                ("next", "t"),
            ]),
        )
        .unwrap_or_default();
        assert_eq!(p.bbox, Some([170.0, -10.0, -170.0, 10.0]));
        assert_eq!(
            p.datetime,
            Some((Some("2026-01-01T00:00:00Z".into()), None))
        );
        assert_eq!(p.next.as_deref(), Some("t"));
        assert_eq!(
            params(&car(), &pairs(&[("bbox", "0,0,-5,1,1,5")]))
                .map(|p| p.bbox)
                .ok(),
            Some(Some([0.0, 0.0, 1.0, 1.0]))
        );
        assert_eq!(
            params(
                &car(),
                &pairs(&[("datetime", "/2026-01-01T00:00:00.5+01:00")])
            )
            .map(|p| p.datetime)
            .ok(),
            Some(Some((None, Some("2026-01-01T00:00:00.5+01:00".into()))))
        );
    }

    #[test]
    fn date_times_follow_rfc_3339() {
        for ok in [
            "2026-01-01T00:00:00Z",
            "2026-01-01t00:00:00z",
            "2026-01-01T00:00:00.123Z",
            "2026-01-01T00:00:00-05:30",
        ] {
            assert!(is_date_time(ok), "{ok}");
        }
        for no in [
            "2026-01-01",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00+0100",
            "2026-01-01T00:00:00+01:0x",
            "2026-01-01T00:00:00+0x:00",
            "2026-1-01T00:00:00Z",
            "x026-01-01T00:00:00Z",
            "2026x01-01T00:00:00Z",
            "2026-01-0xT00:00:00Z",
            "2026-01-01T0x:00:00Z",
            "2026-01-01T00:0x:00Z",
            "2026-01x01T00:00:00Z",
            "2026-01-01x00:00:00Z",
            "2026-01-01T00x00:00Z",
            "2026-01-01T00:00x00Z",
            "2026-01-01T00:00:0xZ",
            "2026-01-01T00:00:00*01:00",
            "2026-01-01T00:00:00+01x00",
            "2026-01-01T00:00:00+01:000",
        ] {
            assert!(!is_date_time(no), "{no}");
        }
    }

    #[test]
    fn a_box_across_the_antimeridian_is_two_polygons() {
        assert_eq!(bbox_geometry([0.0, 0.0, 1.0, 1.0]).0, "Polygon");
        let (kind, coordinates) = bbox_geometry([170.0, -1.0, -170.0, 1.0]);
        assert_eq!(kind, "MultiPolygon");
        assert!(
            coordinates
                .to_string()
                .starts_with("[[[[170.0,-1.0],[180.0,-1.0]"),
            "{coordinates}"
        );
    }

    #[test]
    fn a_token_belongs_to_its_digest_and_names_a_part() {
        let t = token(1, 20, 7);
        assert_eq!(read_token(&t, 7, 2), Some((1, 20)));
        assert_eq!(read_token(&t, 8, 2), None, "another request");
        assert_eq!(read_token(&t, 7, 1), None, "no such part");
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "x.2.0000000000000007",
            "1.x.0000000000000007",
        ] {
            assert_eq!(read_token(bad, 7, 2), None, "{bad}");
        }
        assert_ne!(digest("a", "Car", &[]), digest("b", "Car", &[]));
        assert_eq!(
            digest("a", "Car", &pairs(&[("limit", "1")])),
            digest("a", "Car", &pairs(&[("limit", "2"), ("next", "x")]))
        );
    }

    /// Which of `events` (start, end) `datetime` keeps on an interval collection, and its `q`.
    fn kept(datetime: &str, events: &[(&str, Option<&str>)]) -> (Option<String>, Vec<usize>) {
        let terms = Terms {
            context: Map::new(),
            by_iri: vec![("s".into(), "s".into()), ("e".into(), "e".into())],
        };
        let range = super::datetime(datetime).unwrap_or_default();
        let time = Time::Interval(Some("s".into()), Some("e".into()));
        let (q, exact) = time_q(Some(&time), &terms, &range);
        let program = exact.and_then(|e| residual::compile(&e, residual::DateTime::MIN).ok());
        let hits = events
            .iter()
            .enumerate()
            .filter(|(_, (start, end))| {
                let mut entity = json!({"id": "urn:ngsi-ld:Event:1", "type": "Event",
                    "s": {"type": "Property", "value": start}});
                if let (Some(end), Some(m)) = (end, entity.as_object_mut()) {
                    m.insert("e".into(), json!({"type": "Property", "value": end}));
                }
                program
                    .as_ref()
                    .is_some_and(|p| p.eval(&entity).ok() == Some(residual::Truth::True))
            })
            .map(|(i, _)| i)
            .collect();
        (q, hits)
    }

    /// T-5204: the live Event runs 08:00+01:00 to 15:00+01:00, 07:00Z to 14:00Z; the
    /// whole-day one ends 2026-10-31, inclusive.
    #[test]
    fn datetime_compares_instants_across_offsets_and_whole_days() {
        let events = [
            (
                "2026-11-11T08:00:00+01:00",
                Some("2026-11-11T15:00:00+01:00"),
            ),
            ("2026-10-30", Some("2026-10-31")),
            ("2026-11-11T09:00:00.123456789+02:00", None),
        ];
        assert_eq!(kept("2026-11-11T07:30:00Z", &events).1, vec![0, 2]);
        assert_eq!(kept("2026-11-11T14:30:00Z", &events).1, vec![2]);
        assert_eq!(kept("../2026-11-11T07:30:00Z", &events).1, vec![0, 1, 2]);
        assert_eq!(
            kept("2026-11-11T07:00:00.000000001Z/..", &events).1,
            vec![0, 2]
        );
        assert_eq!(kept("2026-11-11T14:00:00.000001Z/..", &events).1, vec![2]);
        assert_eq!(kept("2026-10-31T12:00:00Z", &events).1, vec![1]);
        assert_eq!(kept("2026-11-01T00:00:00+01:00", &events).1, vec![1]);
        assert_eq!(kept("2026-11-01T00:00:00Z", &events).1, Vec::<usize>::new());
        // The third starts 123 ms after 07:00Z, past this window.
        assert_eq!(
            kept("2026-11-11T06:59:59Z/2026-11-11T07:00:00Z", &events).1,
            vec![0]
        );
        // The store's `q` reads text: its bounds sit 48 h out, a superset.
        assert_eq!(
            kept("2026-11-11T07:30:00+01:00/2026-11-11T08:00:00.5Z", &events)
                .0
                .as_deref(),
            Some("((s<=2026-11-13T08:00:00.5Z)|!s);((e>=2026-11-09T06:30:00Z)|!e)")
        );
    }

    #[test]
    fn the_time_query_keeps_features_without_time() {
        let terms = Terms {
            context: Map::new(),
            by_iri: vec![("s".into(), "s".into()), ("e".into(), "e".into())],
        };
        let at = "2026-01-02T00:00:00Z".to_owned();
        let range = (Some(at.clone()), Some(at));
        assert_eq!(
            time_q(Some(&Time::Instant("s".into())), &terms, &range)
                .0
                .as_deref(),
            Some("((s>=2025-12-31T00:00:00Z;s<=2026-01-04T00:00:00Z)|!s)")
        );
        let missing = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car"});
        let (_, exact) = time_q(Some(&Time::Instant("s".into())), &terms, &range);
        let program = exact.and_then(|e| residual::compile(&e, residual::DateTime::MIN).ok());
        assert_eq!(
            program.and_then(|p| p.eval(&missing).ok()),
            Some(residual::Truth::True),
            "a Feature without the time is kept"
        );
        assert_eq!(
            time_q(
                Some(&Time::Interval(Some("s".into()), None)),
                &terms,
                &(Some("2026-01-02T00:00:00Z".into()), None)
            ),
            (None, None),
            "an open end constrains nothing"
        );
        assert_eq!(time_q(None, &terms, &range), (None, None));
    }

    /// T-5204: a Feature holds one `GeoJSON` geometry and each property in its `/schema` shape,
    /// whatever instances or kind the Attribute has.
    #[test]
    fn a_feature_serves_the_default_instance_and_the_schema_shape() {
        let kinded = |name: &str, kind: &str| crate::model::Prop {
            kind: kind.into(),
            ..prop(name, false)
        };
        let c = Collection {
            properties: vec![
                kinded("speed", "Property"),
                kinded("state", "VocabProperty"),
                kinded("raw", "JsonProperty"),
                kinded("label", "LanguageProperty"),
            ],
            ..car()
        };
        let terms = Terms::of(&c);
        let here = json!({"type": "Point", "coordinates": [17.1, 48.1]});
        let doc = simplified(json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car",
            "location": [
                {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [0, 0]}, "datasetId": "urn:ngsi-ld:Dataset:gps2"},
                {"type": "GeoProperty", "value": here},
            ],
            "speed": [{"type": "Property", "value": 9, "datasetId": "urn:ngsi-ld:Dataset:a"}],
            "state": {"type": "VocabProperty", "vocab": "parked"},
            "raw": {"type": "JsonProperty", "json": {"a": [1]}},
            "label": {"type": "LanguageProperty", "languageMap": {"sk": "auto", "en": "car"}},
        }));
        let refs = Refs {
            profile: Profile::Key,
            base: "http://h/x/views/city/ogc/features",
            alternate: false,
        };
        let f = Value::Object(feature(&c, &terms, &doc, refs));
        // A value already in its shape, or an object of other members, is served as it is.
        assert_eq!(served("VocabProperty", &json!("x")), json!("x"));
        let two = json!({"json": 1, "b": 2});
        assert_eq!(served("JsonProperty", &two), two);
        assert_eq!(served("JsonProperty", &json!({"b": 2})), json!({"b": 2}));
        assert_eq!(f.get("geometry"), Some(&here));
        assert_eq!(
            f.get("properties"),
            Some(&json!({"speed": null, "state": "parked", "raw": {"a": [1]},
                         "label": {"sk": "auto", "en": "car"}}))
        );
    }

    #[test]
    fn a_feature_links_its_entity_only_when_the_view_retrieves_it() {
        let refs = |alternate| Refs {
            profile: Profile::Key,
            base: "http://h/x/views/city/ogc/features",
            alternate,
        };
        let urn = "urn:ngsi-ld:Car:a#1";
        assert_eq!(
            refs(true).links("Car", urn),
            [
                json!({"rel": "alternate", "type": "application/ld+json", "title": urn,
                       "href": "http://h/x/views/city/ngsi-ld/v1/entities/urn%3Angsi-ld%3ACar%3Aa%231"}),
                json!({"rel": "describedby", "type": "application/schema+json",
                       "href": "http://h/x/views/city/ogc/features/collections/Car/schema"}),
            ]
        );
        let without = refs(false).links("Car", urn);
        assert_eq!(without.len(), 1, "no Retrieve Entity, no alternate link");
        assert_eq!(
            without.first().and_then(|l| l.get("rel")),
            Some(&json!("describedby"))
        );
        let terms = Terms::of(&car());
        let nameless = feature(&car(), &terms, &json!({"type": "Car"}), refs(true));
        assert_eq!(nameless.get("links"), None, "no id, nothing to link");
        let elsewhere = Refs {
            base: "http://h/other",
            ..refs(true)
        };
        assert_eq!(
            elsewhere.links("Car", urn).len(),
            1,
            "a base off the view has no entity root"
        );
    }

    #[test]
    fn references_render_each_urn_of_an_object() {
        let refs = Refs {
            profile: Profile::Uri,
            base: "http://b",
            alternate: false,
        };
        let render = |urn: &str| refs.render("Stop", urn);
        assert_eq!(
            refer(&json!(["urn:a", "urn:b"]), &render),
            json!([
                "http://b/collections/Stop/items/urn%3Aa",
                "http://b/collections/Stop/items/urn%3Ab"
            ])
        );
        assert_eq!(refer(&json!(7), &render), json!(7), "no URN, kept");
        assert_eq!(profile("rel-as-nothing"), None);
        assert_eq!(profile("[ogc-profile:rel-as-link"), None);
        assert_eq!(
            profile_uri(Profile::Uri),
            "http://www.opengis.net/def/profile/OGC/0/rel-as-uri"
        );
    }

    #[test]
    fn core_attributes_keep_their_name_and_others_get_a_context_term() {
        let c = Collection {
            history: false,
            output: None,
            id: "Car".into(),
            description: None,
            type_iri: "https://e.org/Car".into(),
            geometry: Some(crate::model::LOCATION.into()),
            // A name the core context defines, one the query grammar cannot read, and a name
            // another property's fallback term would take.
            properties: vec![
                prop("speed", true),
                prop("value", false),
                prop("max speed", false),
                prop("p1", false),
            ],
            time: None,
        };
        let t = Terms::of(&c);
        assert_eq!(t.term(crate::model::LOCATION), "location");
        assert_eq!(t.term("https://e.org/speed"), "speed");
        assert_eq!(t.term("https://e.org/value"), "p1");
        assert_eq!(t.term("https://e.org/max speed"), "p2");
        assert_eq!(t.term("https://e.org/p1"), "p3", "p1 is taken");
        assert_eq!(t.term("https://e.org/other"), "https://e.org/other");
        assert_eq!(t.context.get("speed"), Some(&json!("https://e.org/speed")));
        assert_eq!(t.context.len(), 4);
        let long = Terms::of(&Collection {
            history: false,
            output: None,
            properties: vec![prop(&"x".repeat(257), false), prop("1st", false)],
            ..c.clone()
        });
        assert_eq!(
            long.term(&format!("https://e.org/{}", "x".repeat(257))),
            "p0"
        );
        assert_eq!(long.term("https://e.org/1st"), "p1");
        let only_core = Terms::of(&Collection {
            history: false,
            output: None,
            properties: Vec::new(),
            ..c
        });
        assert!(only_core.context.is_empty());
    }

    #[test]
    fn a_normalized_entity_reads_as_the_simplified_one() {
        let doc = json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car", "scope": ["/a"], "createdAt": "2026-01-01T00:00:00Z",
            "speed": {"type": "Property", "value": 1},
            "at": {"type": "Relationship", "object": "urn:ngsi-ld:Stop:1"},
            "route": {"type": "ListProperty", "valueList": [1, 2]},
            "stops": {"type": "ListRelationship", "objectList": [{"object": "urn:ngsi-ld:Stop:1"}]},
            "name": {"type": "LanguageProperty", "languageMap": {"sk": "auto"}},
            "raw": {"type": "JsonProperty", "json": {"a": 1}},
            "kind": {"type": "VocabProperty", "vocab": "car"},
            "odd": {"type": "Property"},
            "seen": [
                {"type": "Property", "value": "now"},
                {"type": "Property", "value": "then", "datasetId": "urn:ngsi-ld:Dataset:old"}
            ]
        });
        assert_eq!(
            simplified(doc),
            json!({
                "id": "urn:ngsi-ld:Car:1", "type": "Car", "scope": ["/a"], "createdAt": "2026-01-01T00:00:00Z",
                "speed": 1,
                "at": "urn:ngsi-ld:Stop:1",
                "route": [1, 2],
                "stops": [{"object": "urn:ngsi-ld:Stop:1"}],
                "name": {"languageMap": {"sk": "auto"}},
                "raw": {"json": {"a": 1}},
                "kind": {"vocab": "car"},
                "odd": {"type": "Property"},
                "seen": {"dataset": {"@none": "now", "urn:ngsi-ld:Dataset:old": "then"}}
            })
        );
    }

    #[test]
    fn a_refused_residual_names_its_status() {
        let status = |e| refused(&e).0;
        assert_eq!(status(residual::Error::TooComplex), StatusCode::BAD_REQUEST);
        assert_eq!(
            status(residual::Error::Unsupported("x".into())),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            status(residual::Error::Invalid("x".into())),
            StatusCode::BAD_REQUEST
        );
    }
}
