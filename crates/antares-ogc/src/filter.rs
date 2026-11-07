// SPDX-License-Identifier: EUPL-1.2
//! Part 3 filtering (mapping rows F1, F2, F4 to F16): `filter`, `filter-lang`, `filter-crs`
//! and the queryables as query parameters, read into one `ogc-cql2` AST, then lowered for one
//! collection into what the store applies and what the residual evaluates (ADR-0028).
//!
//! The AST is lowered into one residual [`Expr`] per top-level conjunct, with the meaning CQL2
//! gives it: three-valued, a missing or hidden value unknown, arrays as sets (21-065r2
//! `/req/array-functions/array-predicates` B), the temporal functions as relations of the
//! operands' start and end with `..` an open end. A conjunct `q` says exactly (a comparison,
//! `IN`, `BETWEEN`, `LIKE`, `CASEI` equality, `IS NULL` of a scalar Attribute, `A_OVERLAPS` of a
//! `ListProperty`, the temporal functions that are such comparisons) is pushed into `q`; one
//! spatial conjunct with a `georel` into the `geoQ` when the request has no other; the rest is
//! the residual. Pushing never changes the answer, only where it is computed.
//!
//! A name that is no queryable of the collection, hidden or misspelled, is the same 400 before
//! any operation (F16); a function name is 400 too, as `/functions` lists none (F14).

use crate::crs::{bbox_geometry, corners};
use crate::model::Collection;
use crate::Problem;
use antares_ops::residual::{Cmp, Date, DateTime, Expr, Func, Lit, Path, Spatial};
use axum::http::StatusCode;
use ogc_cql2::{Expr as Cql, Geometry, Lang, Op};
use serde_json::{json, Value};

/// The broker's bound on a `q` (`antares_ql::MAX_Q_BYTES`): a conjunct that would pass it stays
/// in the residual.
const MAX_Q_BYTES: usize = 4096;

/// The largest integer an `f64` holds exactly: a number past it is a residual `Num`.
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;

/// Why a filter calls a function (F14).
const NO_FUNCTION: &str =
    "the filter calls a function this endpoint does not define; /functions lists none";

/// Why a filter names a property the collection does not offer, hidden or not (F16).
const NOT_QUERYABLE: &str =
    "the filter names a property that is not a queryable of the collection (see its queryables)";

fn bad(detail: &'static str) -> Problem {
    Problem(StatusCode::BAD_REQUEST, detail)
}

fn not_implemented(detail: &'static str) -> Problem {
    Problem(StatusCode::NOT_IMPLEMENTED, detail)
}

/// The filter parameters of one `items` request, as sent.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Raw {
    /// `filter`.
    pub filter: Option<String>,
    /// `filter-lang`.
    pub lang: Option<String>,
    /// `filter-crs`.
    pub crs: Option<String>,
    /// Queryables named as query parameters, `name=value` (`queryables-query-parameters`).
    pub equals: Vec<(String, String)>,
}

/// Whether `name` is a queryable `c` takes as a query parameter: `id` and the scalar
/// properties, whose value a query string can write.
pub(crate) fn is_parameter(c: &Collection, name: &str) -> bool {
    name == "id" || c.properties.iter().any(|p| p.name == name && p.sortable)
}

/// Reads `raw` into one boolean expression, `None` when the request filters nothing: the
/// `filter` in its language and CRS, and each queryable parameter as an equality, `AND`ed.
pub(crate) fn read(c: &Collection, raw: &Raw) -> Result<Option<Cql>, Problem> {
    let lang = match raw.lang.as_deref() {
        None => Lang::Text,
        Some(l) => Lang::from_param(l).ok_or(bad("filter-lang is cql2-text or cql2-json"))?,
    };
    if let Some(crs) = &raw.crs {
        crate::crs::read(crs).ok_or(bad(crate::crs::UNSUPPORTED))?;
    }
    let mut all = Vec::new();
    if let Some(text) = &raw.filter {
        all.push(ogc_cql2::parse(lang, text).map_err(|e| bad(e.what))?);
    }
    for (name, value) in &raw.equals {
        all.push(Cql::Op(
            Op::Eq,
            vec![Cql::Property(name.clone()), typed(c, name, value)?],
        ));
    }
    let Some(last) = all.pop() else {
        return Ok(None);
    };
    let filter = if all.is_empty() {
        last
    } else {
        all.push(last);
        Cql::Op(Op::And, all)
    };
    check(c, &filter)?;
    Ok(Some(filter))
}

/// A queryable parameter's value as its property's type reads it.
fn typed(c: &Collection, name: &str, value: &str) -> Result<Cql, Problem> {
    let kind = c
        .properties
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.schema.get("type"))
        .and_then(Value::as_str);
    let wrong = || bad("a queryable parameter's value is not of the property's type");
    Ok(match kind {
        Some("integer" | "number") => Cql::Num(
            value
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(wrong)?,
        ),
        Some("boolean") => match value {
            "true" => Cql::Bool(true),
            "false" => Cql::Bool(false),
            _ => return Err(wrong()),
        },
        _ => Cql::Str(value.to_owned()),
    })
}

/// Every property named is a queryable of `c`, and no function is called (F14, F16).
fn check(c: &Collection, e: &Cql) -> Result<(), Problem> {
    match e {
        Cql::Property(name) if queryable(c, name) => Ok(()),
        Cql::Property(_) => Err(bad(NOT_QUERYABLE)),
        Cql::Op(Op::Function(_), _) => Err(bad(NO_FUNCTION)),
        Cql::Op(_, args) | Cql::Array(args) => args.iter().try_for_each(|a| check(c, a)),
        Cql::Interval(a, b) => check(c, a).and_then(|()| check(c, b)),
        _ => Ok(()),
    }
}

/// The queryables of `c`: `id`, `geometry` when it has one, and every property it shows.
fn queryable(c: &Collection, name: &str) -> bool {
    name == "id"
        || (name == "geometry" && c.geometry.is_some())
        || c.properties.iter().any(|p| p.name == name)
}

/// One NGSI-LD `geoQ`: the `georel`, and the geometry as `GeoJSON`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GeoQ {
    pub rel: &'static str,
    pub geometry: Value,
}

/// A filter lowered for one collection: the `id` list, `q` conjuncts, at most one `geoQ`, and
/// the conjuncts the residual evaluates. Their conjunction is the filter.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Lowered {
    /// The ids `id = '…'` and `id IN (…)` conjuncts allow, intersected (T-5169); empty when
    /// no Entity can match.
    pub ids: Option<Vec<String>>,
    pub q: Vec<String>,
    pub geo: Option<GeoQ>,
    pub residual: Vec<Expr>,
}

impl Lowered {
    /// The NGSI-LD `id` parameter of [`Self::ids`], empty without one.
    pub(crate) fn id_param(&self) -> String {
        self.ids.as_ref().map_or_else(String::new, |ids| {
            let ids: Vec<String> = ids
                .iter()
                .map(|id| antares_ops::percent::encode(id))
                .collect();
            format!("&id={}", ids.join(","))
        })
    }
}

/// Lowers `filter` for `c` under `terms`; `geo_free` when the request has no other geo query.
pub(crate) fn lower(
    c: &Collection,
    terms: &crate::terms::Terms,
    filter: &Cql,
    geo_free: bool,
) -> Result<Lowered, Problem> {
    let mut l = Lower { c, terms };
    let mut conjuncts = Vec::new();
    flatten(filter, &mut conjuncts);
    let mut out = Lowered::default();
    let mut q_bytes = 0_usize;
    for conjunct in conjuncts {
        let e = fold(l.expr(conjunct)?);
        if e == Expr::Lit(Lit::Bool(true)) {
            continue;
        }
        if let Some(ids) = by_id(&e) {
            out.ids = Some(match out.ids.take() {
                None => ids,
                Some(kept) => kept.into_iter().filter(|id| ids.contains(id)).collect(),
            });
            continue;
        }
        if let Some(q) = l.q(&e) {
            let size = q_bytes.saturating_add(q.len()).saturating_add(3);
            if size <= MAX_Q_BYTES {
                q_bytes = size;
                out.q.push(q);
                // A widened time bound selects a superset: the residual decides.
                if !approximate(&e) {
                    continue;
                }
            }
        }
        if geo_free && out.geo.is_none() {
            if let Some(geo) = l.geo(&e) {
                out.geo = Some(geo);
                continue;
            }
        }
        out.residual.push(e);
    }
    Ok(out)
}

/// The ids `e` allows when it is `id = '…'` or `id IN (…)` of strings, in the order written and
/// without the strings no Entity id can be (no URI). `None` when a string holds a `,`, which
/// the NGSI-LD `id` list cannot carry, or the list passes the broker's `q` bound: the residual
/// decides those.
fn by_id(e: &Expr) -> Option<Vec<String>> {
    let lits: &[Lit] = match e {
        Expr::Cmp(Cmp::Eq, left, right) => match (left.as_ref(), right.as_ref()) {
            (Expr::Path(Path::Id), Expr::Lit(lit)) | (Expr::Lit(lit), Expr::Path(Path::Id)) => {
                std::slice::from_ref(lit)
            }
            _ => return None,
        },
        Expr::In(value, lits) if **value == Expr::Path(Path::Id) => lits,
        _ => return None,
    };
    let mut ids: Vec<String> = Vec::new();
    for lit in lits {
        let Lit::Str(id) = lit else { return None };
        if id.contains(',') {
            return None;
        }
        if antares_ops::EntityId::new(id).is_ok() && !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let bytes = ids
        .iter()
        .map(|id| id.len().saturating_add(1))
        .sum::<usize>();
    (bytes <= MAX_Q_BYTES).then_some(ids)
}

/// Lowers `filter` for `c` under `terms` to residual conjuncts alone, for a collection the
/// store cannot query: a history (T-5100) evaluates the whole filter in process.
pub(crate) fn residual(
    c: &Collection,
    terms: &crate::terms::Terms,
    filter: &Cql,
) -> Result<Vec<Expr>, Problem> {
    let mut l = Lower { c, terms };
    let mut conjuncts = Vec::new();
    flatten(filter, &mut conjuncts);
    conjuncts.into_iter().map(|e| l.expr(e).map(fold)).collect()
}

/// `e` with its constant and repeated operands of `AND`/`OR` folded, which three-valued logic
/// allows: `TRUE` is the identity of `AND` and absorbs `OR`, `FALSE` the reverse, and `a AND a`
/// is `a` (an instant is both ends of its period, so `T_EQUALS` of two instants is one `=`).
fn fold(e: Expr) -> Expr {
    let (and, es) = match e {
        Expr::And(es) => (true, es),
        Expr::Or(es) => (false, es),
        Expr::Not(inner) => return Expr::Not(Box::new(fold(*inner))),
        other => return other,
    };
    let mut kept: Vec<Expr> = Vec::with_capacity(es.len());
    for e in es.into_iter().map(fold) {
        match e {
            Expr::Lit(Lit::Bool(b)) if b == and => {}
            Expr::Lit(Lit::Bool(b)) => return Expr::Lit(Lit::Bool(b)),
            e if kept.contains(&e) => {}
            e => kept.push(e),
        }
    }
    match kept.len() {
        0 => Expr::Lit(Lit::Bool(and)),
        1 => kept.pop().unwrap_or(Expr::Lit(Lit::Bool(and))),
        _ if and => Expr::And(kept),
        _ => Expr::Or(kept),
    }
}

/// The top-level conjuncts of `e`.
fn flatten<'a>(e: &'a Cql, out: &mut Vec<&'a Cql>) {
    match e {
        Cql::Op(Op::And, args) => args.iter().for_each(|a| flatten(a, out)),
        other => out.push(other),
    }
}

/// One end of a temporal operand.
#[derive(Clone)]
enum Bound {
    Before,
    At(Expr),
    After,
}

impl Bound {
    /// Where an open end sorts against a value: before every one, or after.
    fn rank(&self) -> u8 {
        match self {
            Self::Before => 0,
            Self::At(_) => 1,
            Self::After => 2,
        }
    }
}

struct Lower<'a> {
    c: &'a Collection,
    terms: &'a crate::terms::Terms,
}

impl Lower<'_> {
    /// The path of a queryable (checked by [`read`]).
    fn path(&self, name: &str) -> Result<Path, Problem> {
        if name == "id" {
            return Ok(Path::Id);
        }
        let iri = if name == "geometry" {
            self.c.geometry.as_deref()
        } else {
            self.c
                .properties
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.iri.as_str())
        };
        iri.map(|iri| Path::value(self.terms.term(iri)))
            .ok_or(bad(NOT_QUERYABLE))
    }

    fn boxed(&mut self, e: &Cql) -> Result<Box<Expr>, Problem> {
        Ok(Box::new(self.expr(e)?))
    }

    fn all(&mut self, es: &[Cql]) -> Result<Vec<Expr>, Problem> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    /// One CQL2 expression as a residual expression.
    fn expr(&mut self, e: &Cql) -> Result<Expr, Problem> {
        Ok(match e {
            Cql::Property(name) => Expr::Path(self.path(name)?),
            Cql::Str(s) => Expr::Lit(Lit::Str(s.clone())),
            Cql::Num(n) => Expr::Lit(number(*n)?),
            Cql::Bool(b) => Expr::Lit(Lit::Bool(*b)),
            Cql::Timestamp(t) => Expr::Lit(instant(DateTime::parse(t).map(Lit::DateTime))?),
            Cql::Date(d) => Expr::Lit(instant(Date::parse(d).map(Lit::Date))?),
            Cql::Geometry(g) => Expr::Lit(Lit::Geometry(geojson(g))),
            Cql::BBox(numbers) => Expr::Lit(Lit::Geometry(bbox(numbers)?)),
            Cql::Interval(..) | Cql::Unbounded | Cql::Array(_) => {
                return Err(bad(
                    "an interval or a list is an operand of a temporal, IN or array function only",
                ))
            }
            Cql::Op(op, args) => self.op(op, args)?,
        })
    }

    fn op(&mut self, op: &Op, args: &[Cql]) -> Result<Expr, Problem> {
        Ok(match op {
            Op::And => Expr::And(self.all(args)?),
            Op::Or => Expr::Or(self.all(args)?),
            Op::Not => {
                let [a] = operands(args)?;
                Expr::Not(self.boxed(a)?)
            }
            Op::IsNull => {
                let [a] = operands(args)?;
                Expr::IsNull(self.boxed(a)?)
            }
            Op::Casei | Op::Accenti => {
                let [a] = operands(args)?;
                let f = if *op == Op::Casei {
                    Func::CaseI
                } else {
                    Func::AccentI
                };
                Expr::Call(f, vec![self.expr(a)?])
            }
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let [a, b] = operands(args)?;
                Expr::Cmp(cmp(op), self.boxed(a)?, self.boxed(b)?)
            }
            Op::Like => {
                let [a, b] = operands(args)?;
                Expr::Call(Func::Like, vec![self.expr(a)?, self.expr(b)?])
            }
            Op::Between => {
                let [v, low, high] = operands(args)?;
                let v = self.expr(v)?;
                Expr::And(vec![
                    Expr::Cmp(Cmp::Ge, Box::new(v.clone()), self.boxed(low)?),
                    Expr::Cmp(Cmp::Le, Box::new(v), self.boxed(high)?),
                ])
            }
            Op::In => self.in_list(args)?,
            Op::SIntersects
            | Op::SEquals
            | Op::SDisjoint
            | Op::STouches
            | Op::SWithin
            | Op::SOverlaps
            | Op::SCrosses
            | Op::SContains => {
                let [a, b] = operands(args)?;
                Expr::Spatial(spatial(op), self.boxed(a)?, self.boxed(b)?)
            }
            Op::TAfter
            | Op::TBefore
            | Op::TContains
            | Op::TDisjoint
            | Op::TDuring
            | Op::TEquals
            | Op::TFinishedBy
            | Op::TFinishes
            | Op::TIntersects
            | Op::TMeets
            | Op::TMetBy
            | Op::TOverlappedBy
            | Op::TOverlaps
            | Op::TStartedBy
            | Op::TStarts => {
                let [a, b] = operands(args)?;
                let a = self.period(a)?;
                let b = self.period(b)?;
                temporal(op, &a, &b)
            }
            Op::AEquals | Op::AContains | Op::AContainedBy | Op::AOverlaps => {
                self.arrays(op, args)?
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::IntDiv | Op::Pow => {
                self.arithmetic(op, args)?
            }
            Op::Function(_) => return Err(bad(NO_FUNCTION)),
        })
    }

    /// `IN`: a list of literals is one membership test, any other list an `OR` of `=`; the
    /// empty list holds for no known value (`v <> v`: false when known, unknown when not).
    fn in_list(&mut self, args: &[Cql]) -> Result<Expr, Problem> {
        let [a, list] = operands(args)?;
        let Cql::Array(items) = list else {
            return Err(bad("IN takes a list"));
        };
        let a = self.expr(a)?;
        let items = self.all(items)?;
        if items.is_empty() {
            return Ok(Expr::Cmp(Cmp::Ne, Box::new(a.clone()), Box::new(a)));
        }
        let lits: Option<Vec<Lit>> = items
            .iter()
            .map(|i| match i {
                Expr::Lit(l) => Some(l.clone()),
                _ => None,
            })
            .collect();
        Ok(match lits {
            Some(lits) => Expr::In(Box::new(a), lits),
            None => Expr::Or(
                items
                    .into_iter()
                    .map(|i| Expr::Cmp(Cmp::Eq, Box::new(a.clone()), Box::new(i)))
                    .collect(),
            ),
        })
    }

    /// An array function: both operands as sets (21-065r2 `/req/array-functions/array-predicates` B).
    fn arrays(&mut self, op: &Op, args: &[Cql]) -> Result<Expr, Problem> {
        let [a, b] = operands(args)?;
        let a = self.array(a)?;
        let b = self.array(b)?;
        Ok(match op {
            Op::AEquals => Expr::And(vec![subset(&a, &b), subset(&b, &a)]),
            Op::AContains => subset(&b, &a),
            Op::AContainedBy => subset(&a, &b),
            _ => overlaps(&a, &b),
        })
    }

    /// An arithmetic operator: `/` always a number, `div` truncating for two integers.
    fn arithmetic(&mut self, op: &Op, args: &[Cql]) -> Result<Expr, Problem> {
        let [a, b] = operands(args)?;
        let f = match op {
            Op::Add => Func::Add,
            Op::Sub => Func::Sub,
            Op::Mul => Func::Mul,
            Op::Div => Func::DivBy,
            Op::Mod => Func::Mod,
            Op::IntDiv => Func::Div,
            _ => Func::Pow,
        };
        Ok(Expr::Call(f, vec![self.expr(a)?, self.expr(b)?]))
    }

    /// A temporal operand as its start and end: an instant is both.
    fn period(&mut self, e: &Cql) -> Result<(Bound, Bound), Problem> {
        match e {
            Cql::Interval(start, end) => Ok((
                self.bound(start, Bound::Before)?,
                self.bound(end, Bound::After)?,
            )),
            Cql::Timestamp(_) | Cql::Date(_) | Cql::Property(_) | Cql::Op(..) => {
                let v = self.expr(e)?;
                Ok((Bound::At(v.clone()), Bound::At(v)))
            }
            _ => Err(bad("a temporal function takes instants and intervals")),
        }
    }

    fn bound(&mut self, e: &Cql, open: Bound) -> Result<Bound, Problem> {
        match e {
            Cql::Unbounded => Ok(open),
            other => Ok(Bound::At(self.expr(other)?)),
        }
    }

    /// An array operand: a property's list value, or a list of literals.
    fn array(&mut self, e: &Cql) -> Result<Set, Problem> {
        match e {
            Cql::Property(_) => Ok(Set::Value(self.expr(e)?)),
            Cql::Array(items) => {
                let mut lits = Vec::with_capacity(items.len());
                for item in items {
                    match self.expr(item)? {
                        Expr::Lit(l) if !matches!(l, Lit::Geometry(_)) => lits.push(l),
                        _ => {
                            return Err(not_implemented(
                                "an array element other than a string, number, boolean, date or timestamp literal",
                            ))
                        }
                    }
                }
                Ok(Set::Lits(lits))
            }
            _ => Err(bad("an array function takes properties and lists")),
        }
    }

    /// `e` as one `q` term, when `q` says exactly what it means.
    fn q(&self, expr: &Expr) -> Option<String> {
        match expr {
            // `fold` leaves no empty AND or OR.
            Expr::And(es) => {
                let parts: Option<Vec<String>> = es.iter().map(|part| self.q(part)).collect();
                Some(format!("({})", parts?.join(";")))
            }
            Expr::Or(es) => {
                let parts: Option<Vec<String>> = es.iter().map(|part| self.q(part)).collect();
                Some(format!("({})", parts?.join("|")))
            }
            Expr::Cmp(op, left, right) => match (left.as_ref(), right.as_ref()) {
                (Expr::Path(path), Expr::Lit(lit)) => self.compare(path, *op, lit),
                (Expr::Lit(lit), Expr::Path(path)) => self.compare(path, mirrored(*op), lit),
                (Expr::Call(Func::CaseI, value), Expr::Call(Func::CaseI, pattern))
                    if *op == Cmp::Eq =>
                {
                    match (value.as_slice(), pattern.as_slice()) {
                        ([Expr::Path(path)], [Expr::Lit(Lit::Str(text))]) => {
                            let term = self.scalar(path)?;
                            Some(format!("{term}~=\"(?i)^{}$\"", escape(text)))
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            Expr::In(value, lits) => {
                let Expr::Path(path) = value.as_ref() else {
                    return None;
                };
                let term = self.scalar(path)?;
                lits.first()?;
                let values: Option<Vec<String>> = lits.iter().map(Self::value).collect();
                Some(format!("{term}=={}", values?.join(",")))
            }
            Expr::IsNull(value) => Some(format!("!{}", attribute(value)?)),
            Expr::Not(inner) => match inner.as_ref() {
                Expr::IsNull(value) => attribute(value).map(str::to_owned),
                _ => None,
            },
            Expr::Call(Func::Like, args) => {
                let (term, pattern, flags) = match args.as_slice() {
                    [Expr::Path(path), Expr::Lit(Lit::Str(text))] => {
                        (self.scalar(path)?, text, "(?s)")
                    }
                    [Expr::Call(Func::CaseI, value), Expr::Call(Func::CaseI, folded)] => {
                        match (value.as_slice(), folded.as_slice()) {
                            ([Expr::Path(path)], [Expr::Lit(Lit::Str(text))]) => {
                                (self.scalar(path)?, text, "(?is)")
                            }
                            _ => return None,
                        }
                    }
                    _ => return None,
                };
                Some(format!("{term}~=\"{flags}^{}$\"", like(pattern)?))
            }
            // A_OVERLAPS of a ListProperty: `==` matches any member (TS 104 175 4.9).
            Expr::Any(list, Some(body)) => {
                let (Expr::Path(path), Expr::In(var, lits)) = (list.as_ref(), body.as_ref()) else {
                    return None;
                };
                let term = self.list(path).filter(|_| **var == member(0))?;
                lits.first()?;
                let values: Option<Vec<String>> = lits.iter().map(Self::value).collect();
                Some(format!("{term}=={}", values?.join(",")))
            }
            _ => None,
        }
    }

    fn compare(&self, p: &Path, op: Cmp, l: &Lit) -> Option<String> {
        let term = self.scalar(p)?;
        if let Some(at) = moment(l) {
            // `q` orders text: a value written with an offset or as a whole day is in order
            // only to within 48 h, so the bound moves out by that much and `=`/`<>` stay residual.
            let (op, later) = match op {
                Cmp::Lt | Cmp::Le => ("<=", true),
                Cmp::Gt | Cmp::Ge => (">=", false),
                Cmp::Eq | Cmp::Ne => return None,
            };
            return Some(format!("{term}{op}{}", slack(at, later)?));
        }
        let op = match op {
            Cmp::Eq => "==",
            Cmp::Ne => "!=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        };
        Some(format!("{term}{op}{}", Self::value(l)?))
    }

    /// The term of a scalar Attribute's value, which `q` compares as CQL2 does.
    fn scalar<'p>(&self, p: &'p Path) -> Option<&'p str> {
        let term = attribute_path(p)?;
        self.c
            .properties
            .iter()
            .find(|prop| prop.sortable && self.terms.term(&prop.iri) == term)
            .map(|_| term)
    }

    /// The term of a `ListProperty`'s value.
    fn list<'p>(&self, p: &'p Path) -> Option<&'p str> {
        let term = attribute_path(p)?;
        self.c
            .properties
            .iter()
            .find(|prop| prop.kind == "ListProperty" && self.terms.term(&prop.iri) == term)
            .map(|_| term)
    }

    /// A literal as a `q` value.
    fn value(l: &Lit) -> Option<String> {
        match l {
            Lit::Bool(b) => Some(b.to_string()),
            Lit::Int(n) => Some(n.to_string()),
            Lit::Num(n) => n.is_finite().then(|| format!("{n}")),
            Lit::Str(s) => Some(Value::from(s.as_str()).to_string()),
            Lit::DateTime(_)
            | Lit::Date(_)
            | Lit::Time(_)
            | Lit::Duration(_)
            | Lit::Binary(_)
            | Lit::Geometry(_) => None,
        }
    }

    /// `e` as the `geoQ`: a relation NGSI-LD names, of the collection's geometry and a literal
    /// geometry NGSI-LD takes.
    fn geo(&self, e: &Expr) -> Option<GeoQ> {
        let Expr::Spatial(relation, left, right) = e else {
            return None;
        };
        let (rel, geometry) = match (left.as_ref(), right.as_ref()) {
            (Expr::Path(path), Expr::Lit(Lit::Geometry(shape))) => (*relation, (path, shape)),
            (Expr::Lit(Lit::Geometry(shape)), Expr::Path(path)) => (
                match relation {
                    Spatial::Within => Spatial::Contains,
                    Spatial::Contains => Spatial::Within,
                    other => *other,
                },
                (path, shape),
            ),
            _ => return None,
        };
        let (path, g) = geometry;
        let own = self.terms.term(self.c.geometry.as_deref()?);
        if attribute_path(path) != Some(own) {
            return None;
        }
        let rel = match rel {
            Spatial::Intersects => "intersects",
            Spatial::Within => "within",
            Spatial::Contains => "contains",
            Spatial::Equals => "equals",
            Spatial::Disjoint => "disjoint",
            Spatial::Overlaps => "overlaps",
            Spatial::Touches | Spatial::Crosses => return None,
        };
        let pushable = matches!(
            g.get("type").and_then(Value::as_str),
            Some(
                "Point"
                    | "LineString"
                    | "Polygon"
                    | "MultiPoint"
                    | "MultiLineString"
                    | "MultiPolygon"
            )
        );
        pushable.then(|| GeoQ {
            rel,
            geometry: g.clone(),
        })
    }
}

/// A date or date-time literal of the filter, which the filter refuses unless it is a calendar
/// instant.
fn instant(lit: Option<Lit>) -> Result<Lit, Problem> {
    lit.ok_or(bad(
        "a date or timestamp of the filter is no calendar instant",
    ))
}

/// How far a `q` bound on a date or date-time moves out: past any UTC offset (at most 14 h)
/// and a whole day, so the store's text order selects a superset of the instants.
const SLACK_MICROS: i64 = 48 * 3_600 * 1_000_000;

/// `at` moved [`SLACK_MICROS`] later or earlier, as `q` text in UTC; `None` past the years
/// 0000 to 9999, where the bound stays in the residual alone.
pub(crate) fn slack(at: DateTime, later: bool) -> Option<String> {
    let micros = if later {
        at.unix_micros().saturating_add(SLACK_MICROS)
    } else {
        at.unix_micros().saturating_sub(SLACK_MICROS)
    };
    DateTime::from_unix_micros(micros).map(|d| d.to_string())
}

/// A date literal as the instant its day starts in UTC, a date-time as itself.
fn moment(l: &Lit) -> Option<DateTime> {
    match l {
        Lit::DateTime(at) => Some(*at),
        Lit::Date(d) => DateTime::parse(&format!("{d}T00:00:00Z")),
        _ => None,
    }
}

/// Whether the `q` of `e` holds a widened time bound ([`slack`]), so it selects a superset the
/// residual must still decide.
fn approximate(e: &Expr) -> bool {
    match e {
        Expr::And(es) | Expr::Or(es) => es.iter().any(approximate),
        Expr::Cmp(_, a, b) => [a, b]
            .iter()
            .any(|x| matches!(x.as_ref(), Expr::Lit(Lit::DateTime(_) | Lit::Date(_)))),
        _ => false,
    }
}

/// The term of any queryable Attribute's value (`IS NULL` is whether it is there).
fn attribute(e: &Expr) -> Option<&str> {
    match e {
        Expr::Path(p) => attribute_path(p),
        _ => None,
    }
}

/// The term of a top-level Attribute's value.
/// The term of a top-level Attribute's value (the only paths [`Lower::path`] builds).
fn attribute_path(p: &Path) -> Option<&str> {
    match p {
        Path::Attribute { attrs, .. } => attrs.first().map(String::as_str),
        _ => None,
    }
}

/// The `N` operands of an operator; `ogc-cql2` already checked how many each takes.
fn operands<const N: usize>(args: &[Cql]) -> Result<&[Cql; N], Problem> {
    args.try_into()
        .map_err(|_| bad("wrong number of arguments for the operator"))
}

/// A CQL2 number: an integer when it is one an `f64` holds exactly.
fn number(n: f64) -> Result<Lit, Problem> {
    if !n.is_finite() {
        return Err(bad("a number of the filter is not finite"));
    }
    let int = (n.fract() == 0.0 && n.abs() <= MAX_EXACT)
        .then(|| format!("{n}").parse::<i64>().ok())
        .flatten();
    Ok(int.map_or(Lit::Num(n), Lit::Int))
}

fn cmp(op: &Op) -> Cmp {
    match op {
        Op::Ne => Cmp::Ne,
        Op::Lt => Cmp::Lt,
        Op::Le => Cmp::Le,
        Op::Gt => Cmp::Gt,
        Op::Ge => Cmp::Ge,
        _ => Cmp::Eq,
    }
}

/// `v op p` read as `p op' v`.
fn mirrored(op: Cmp) -> Cmp {
    match op {
        Cmp::Gt => Cmp::Lt,
        Cmp::Ge => Cmp::Le,
        Cmp::Lt => Cmp::Gt,
        Cmp::Le => Cmp::Ge,
        same => same,
    }
}

fn spatial(op: &Op) -> Spatial {
    match op {
        Op::SEquals => Spatial::Equals,
        Op::SDisjoint => Spatial::Disjoint,
        Op::STouches => Spatial::Touches,
        Op::SWithin => Spatial::Within,
        Op::SOverlaps => Spatial::Overlaps,
        Op::SCrosses => Spatial::Crosses,
        Op::SContains => Spatial::Contains,
        _ => Spatial::Intersects,
    }
}

/// `a op b` of two temporal bounds. An open end compares by where it sorts; against a value,
/// the comparison is still unknown when the value is (`v = v` is true exactly when `v` is
/// known), so a missing time never holds a relation, even under `NOT`.
fn at(op: Cmp, a: &Bound, b: &Bound) -> Expr {
    if let (Bound::At(x), Bound::At(y)) = (a, b) {
        return Expr::Cmp(op, Box::new(x.clone()), Box::new(y.clone()));
    }
    let order = a.rank().cmp(&b.rank());
    let holds = match op {
        Cmp::Eq => order.is_eq(),
        Cmp::Ne => order.is_ne(),
        Cmp::Lt => order.is_lt(),
        Cmp::Le => order.is_le(),
        Cmp::Gt => order.is_gt(),
        Cmp::Ge => order.is_ge(),
    };
    let known = match (a, b) {
        (Bound::At(v), _) | (_, Bound::At(v)) => Some(v),
        _ => None,
    };
    match known {
        Some(v) => Expr::Cmp(
            if holds { Cmp::Eq } else { Cmp::Ne },
            Box::new(v.clone()),
            Box::new(v.clone()),
        ),
        None => Expr::Lit(Lit::Bool(holds)),
    }
}

/// The temporal functions as relations of start and end (OWL-Time, which 21-065r2 7.8 cites);
/// intervals are closed at both ends.
fn temporal(op: &Op, (a0, a1): &(Bound, Bound), (b0, b1): &(Bound, Bound)) -> Expr {
    use Cmp::{Eq, Ge, Gt, Le, Lt};
    let all = |cs: Vec<Expr>| Expr::And(cs);
    match op {
        Op::TAfter => at(Gt, a0, b1),
        Op::TBefore => at(Lt, a1, b0),
        Op::TContains => all(vec![at(Lt, a0, b0), at(Gt, a1, b1)]),
        Op::TDisjoint => Expr::Or(vec![at(Lt, a1, b0), at(Gt, a0, b1)]),
        Op::TDuring => all(vec![at(Gt, a0, b0), at(Lt, a1, b1)]),
        Op::TEquals => all(vec![at(Eq, a0, b0), at(Eq, a1, b1)]),
        Op::TFinishedBy => all(vec![at(Lt, a0, b0), at(Eq, a1, b1)]),
        Op::TFinishes => all(vec![at(Gt, a0, b0), at(Eq, a1, b1)]),
        Op::TMeets => at(Eq, a1, b0),
        Op::TMetBy => at(Eq, a0, b1),
        Op::TOverlappedBy => all(vec![at(Gt, a0, b0), at(Lt, a0, b1), at(Gt, a1, b1)]),
        Op::TOverlaps => all(vec![at(Lt, a0, b0), at(Gt, a1, b0), at(Lt, a1, b1)]),
        Op::TStartedBy => all(vec![at(Eq, a0, b0), at(Gt, a1, b1)]),
        Op::TStarts => all(vec![at(Eq, a0, b0), at(Lt, a1, b1)]),
        // T_INTERSECTS
        _ => all(vec![at(Le, a0, b1), at(Ge, a1, b0)]),
    }
}

/// An array operand: a list value of the Entity, or literals.
enum Set {
    Value(Expr),
    Lits(Vec<Lit>),
}

/// The member of the innermost list.
fn member(up: usize) -> Expr {
    Expr::Var {
        up,
        keys: Vec::new(),
    }
}

/// `a ⊆ b` as sets.
fn subset(a: &Set, b: &Set) -> Expr {
    match (a, b) {
        (Set::Value(a), Set::Lits(b)) => Expr::All(
            Box::new(a.clone()),
            Box::new(Expr::In(Box::new(member(0)), b.clone())),
        ),
        (Set::Lits(a), Set::Value(b)) => Expr::And(
            a.iter()
                .map(|l| {
                    Expr::Any(
                        Box::new(b.clone()),
                        Some(Box::new(Expr::Cmp(
                            Cmp::Eq,
                            Box::new(member(0)),
                            Box::new(Expr::Lit(l.clone())),
                        ))),
                    )
                })
                .collect(),
        ),
        (Set::Value(a), Set::Value(b)) => Expr::All(
            Box::new(a.clone()),
            Box::new(Expr::Any(
                Box::new(b.clone()),
                Some(Box::new(Expr::Cmp(
                    Cmp::Eq,
                    Box::new(member(1)),
                    Box::new(member(0)),
                ))),
            )),
        ),
        (Set::Lits(a), Set::Lits(b)) => Expr::Lit(Lit::Bool(a.iter().all(|l| b.contains(l)))),
    }
}

/// `a ∩ b ≠ ∅`.
fn overlaps(a: &Set, b: &Set) -> Expr {
    match (a, b) {
        (Set::Value(v), Set::Lits(lits)) | (Set::Lits(lits), Set::Value(v)) => Expr::Any(
            Box::new(v.clone()),
            Some(Box::new(Expr::In(Box::new(member(0)), lits.clone()))),
        ),
        (Set::Value(a), Set::Value(b)) => Expr::Any(
            Box::new(a.clone()),
            Some(Box::new(Expr::Any(
                Box::new(b.clone()),
                Some(Box::new(Expr::Cmp(
                    Cmp::Eq,
                    Box::new(member(1)),
                    Box::new(member(0)),
                ))),
            ))),
        ),
        (Set::Lits(a), Set::Lits(b)) => Expr::Lit(Lit::Bool(a.iter().any(|l| b.contains(l)))),
    }
}

/// A `BBOX` as its polygon, two across the antimeridian.
fn bbox(numbers: &[f64]) -> Result<Value, Problem> {
    let corners = corners(numbers).ok_or(bad("a BBOX is 4 or 6 numbers in CRS84"))?;
    let (kind, coordinates) = bbox_geometry(corners);
    Ok(json!({"type": kind, "coordinates": coordinates}))
}

fn geojson(g: &Geometry) -> Value {
    let (kind, coordinates) = match g {
        Geometry::Point(p) => ("Point", json!(p)),
        Geometry::LineString(l) => ("LineString", json!(l)),
        Geometry::Polygon(p) => ("Polygon", json!(p)),
        Geometry::MultiPoint(m) => ("MultiPoint", json!(m)),
        Geometry::MultiLineString(m) => ("MultiLineString", json!(m)),
        Geometry::MultiPolygon(m) => ("MultiPolygon", json!(m)),
        Geometry::Collection(gs) => {
            let members: Vec<Value> = gs.iter().map(geojson).collect();
            return json!({"type": "GeometryCollection", "geometries": members});
        }
    };
    json!({"type": kind, "coordinates": coordinates})
}

/// The characters of `s` with every regular-expression metacharacter escaped, and `"` as
/// `\x22`: the quoted `RegExp` of `q` is taken as written up to the next quote.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_escaped(&mut out, c);
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    if c == '"' {
        out.push_str(r"\x22");
        return;
    }
    if r"\.^$|?*+()[]{}#&-~".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// A CQL2 `LIKE` pattern as the body of a regular expression: `%` any run, `_` one character,
/// `\` the next character as written; `None` for a pattern ending in an escape (the residual
/// refuses it with its reason).
fn like(pattern: &str) -> Option<String> {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            '\\' => push_escaped(&mut out, chars.next()?),
            other => push_escaped(&mut out, other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Prop;
    use crate::terms::Terms;

    fn prop(name: &str, ty: &str, kind: &str, sortable: bool) -> Prop {
        Prop {
            name: name.into(),
            iri: format!("https://uri.etsi.org/ngsi-ld/default-context/{name}"),
            schema: json!({ "type": ty }),
            role: None,
            unit: None,
            description: None,
            sortable,
            kind: kind.into(),
            reference: false,
            target: None,
        }
    }

    fn station() -> Collection {
        Collection {
            history: false,
            output: None,
            id: "AirQualityObserved".into(),
            description: None,
            type_iri: "https://uri.etsi.org/ngsi-ld/default-context/AirQualityObserved".into(),
            geometry: Some("https://uri.etsi.org/ngsi-ld/location".into()),
            time: None,
            properties: vec![
                prop("pm10", "number", "Property", true),
                prop("pm25", "number", "Property", true),
                prop("label", "string", "Property", true),
                prop("dateObserved", "string", "Property", true),
                prop("tags", "array", "ListProperty", false),
                prop("labels", "array", "Property", false),
                prop("quiet", "boolean", "Property", true),
            ],
        }
    }

    fn raw(filter: &str) -> Raw {
        Raw {
            filter: Some(filter.into()),
            ..Raw::default()
        }
    }

    fn lowered(filter: &str) -> Result<Lowered, Problem> {
        let c = station();
        let terms = Terms::of(&c);
        let f = read(&c, &raw(filter))?.ok_or(bad("no filter"))?;
        lower(&c, &terms, &f, true)
    }

    /// What `filter` lowers to: its `q`, its `geoQ` and how many residual conjuncts.
    fn q(filter: &str) -> String {
        format!(
            "{:?}",
            lowered(filter).map(|l| (l.q.join(";"), l.geo, l.residual.len()))
        )
    }

    /// [`q`] of a filter wholly pushed into `expected`.
    fn pushed(expected: &str) -> String {
        format!(
            "{:?}",
            Ok::<_, Problem>((expected.to_owned(), None::<GeoQ>, 0_usize))
        )
    }

    /// [`q`] of a filter pushed widened into `expected` and decided by one residual conjunct.
    fn widened(expected: &str) -> String {
        format!(
            "{:?}",
            Ok::<_, Problem>((expected.to_owned(), None::<GeoQ>, 1_usize))
        )
    }

    /// Whether `filter` is one residual conjunct and nothing pushed.
    fn residual(filter: &str) -> bool {
        lowered(filter).map(|l| (l.q.len(), l.geo.is_some(), l.residual.len())) == Ok((0, false, 1))
    }

    /// The gateway's `every_supported_cql2_operator_reaches_the_broker` and
    /// `edge_translators_cql2_compile_tests` inputs, as the `q` this surface pushes.
    #[test]
    fn basic_cql2_pushed_to_q() {
        for (filter, expected) in [
            ("pm10 > 50", "pm10>50"),
            ("pm10>50", "pm10>50"),
            ("pm10 = 50", "pm10==50"),
            ("pm10 <> 50", "pm10!=50"),
            ("50 < pm10", "pm10>50"),
            ("pm10 >= 2.5", "pm10>=2.5"),
            ("pm10 IS NULL", "!pm10"),
            ("pm10 IS NOT NULL", "pm10"),
            ("tags IS NULL", "!tags"),
            ("quiet = TRUE", "quiet==true"),
            ("pm10 > 50 AND pm25 < 20", "pm10>50;pm25<20"),
            ("pm10 > 50 OR pm25 < 20", "(pm10>50|pm25<20)"),
            (
                "(pm10 > 50 AND pm25 < 20) OR label = 'x'",
                "((pm10>50;pm25<20)|label==\"x\")",
            ),
            ("\"pm10\" > 5", "pm10>5"),
            ("pm10 = 'a);(b'", r#"pm10=="a);(b""#),
            ("pm10 = 'a|b'", r#"pm10=="a|b""#),
            ("label = 'say \"hi\"'", r#"label=="say \"hi\"""#),
            ("TRUE AND pm10 > 1", "pm10>1"),
        ] {
            assert_eq!(q(filter), pushed(expected), "{filter}");
        }
    }

    #[test]
    fn like_between_in_pushed() {
        for (filter, expected) in [
            ("pm10 BETWEEN 10 AND 20", "(pm10>=10;pm10<=20)"),
            ("pm10 IN (10, 20)", "pm10==10,20"),
            ("label IN ('a', 'b')", r#"label=="a","b""#),
            ("label LIKE 'Kal%'", r#"label~="(?s)^Kal.*$""#),
            ("label LIKE 'a_b'", r#"label~="(?s)^a.b$""#),
            ("label LIKE 'a.b%'", r#"label~="(?s)^a\.b.*$""#),
            ("label LIKE '\"x'", r#"label~="(?s)^\x22x$""#),
        ] {
            assert_eq!(q(filter), pushed(expected), "{filter}");
        }
        assert_eq!(like(r"100\%"), Some("100%".into()));
        // A pattern ending in its escape is the residual's to refuse.
        assert_eq!(like(r"a\"), None);
        // `IN` with a property in the list compares each.
        assert!(residual("pm10 IN (pm25, 3)"));
    }

    #[test]
    fn casei_pushed_as_regex_flag() {
        assert_eq!(
            q("CASEI(label) = CASEI('Ab.c')"),
            pushed(r#"label~="(?i)^Ab\.c$""#)
        );
        assert_eq!(
            q("CASEI(label) LIKE CASEI('ab%')"),
            pushed(r#"label~="(?is)^ab.*$""#)
        );
        assert!(residual("CASEI(label) <> CASEI('x')"));
        assert!(residual("CASEI(label) = 'x'"));
    }

    #[test]
    fn accenti_is_residual() {
        assert!(residual("ACCENTI(label) = ACCENTI('Hlavná')"));
        assert!(residual("ACCENTI(label) LIKE ACCENTI('hlav%')"));
    }

    #[test]
    fn id_conjuncts_are_the_id_list() {
        let ids = |filter: &str| lowered(filter).map(|l| (l.ids, l.q.len(), l.residual.len()));
        let list = |ids: &[&str]| Some(ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>());
        assert_eq!(ids("id = 'urn:a'"), Ok((list(&["urn:a"]), 0, 0)));
        assert_eq!(ids("'urn:a' = id"), Ok((list(&["urn:a"]), 0, 0)));
        assert_eq!(
            ids("id IN ('urn:a', 'urn:b', 'urn:a')"),
            Ok((list(&["urn:a", "urn:b"]), 0, 0))
        );
        // ANDed lists intersect; beside another conjunct the id list still holds.
        assert_eq!(
            ids("id IN ('urn:a', 'urn:b') AND id = 'urn:b'"),
            Ok((list(&["urn:b"]), 0, 0))
        );
        assert_eq!(ids("id = 'urn:a' AND id = 'urn:b'"), Ok((list(&[]), 0, 0)));
        assert_eq!(
            ids("id = 'urn:a' AND pm10 > 3"),
            Ok((list(&["urn:a"]), 1, 0))
        );
        // A string no Entity id can be matches nothing.
        assert_eq!(ids("id = 'not an id'"), Ok((list(&[]), 0, 0)));
        assert_eq!(ids("id IN ('urn:a', 'x y')"), Ok((list(&["urn:a"]), 0, 0)));
        // What the id list cannot say stays the residual's.
        assert_eq!(ids("id = 'urn:a,b'"), Ok((None, 0, 1)));
        assert_eq!(ids("id IN ('urn:a', 3)"), Ok((None, 0, 1)));
        assert_eq!(ids("id <> 'urn:a'"), Ok((None, 0, 1)));
        assert_eq!(ids("id = 'urn:a' OR pm10 > 3"), Ok((None, 0, 1)));
        assert_eq!(ids("NOT id IN ('urn:a')"), Ok((None, 0, 1)));
        let long = (0..600)
            .map(|i| format!("'urn:x:{i}'"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(ids(&format!("id IN ({long})")), Ok((None, 0, 1)));
    }

    #[test]
    fn not_is_residual() {
        assert!(residual("NOT pm10 > 50"));
        assert!(residual("NOT (pm10 > 50 OR pm25 < 1)"));
        // One branch NGSI-LD cannot say keeps the whole OR in the residual.
        assert!(residual("pm10 > 50 OR NOT pm25 < 1"));
        let l = lowered("pm10 > 50 AND NOT pm25 < 1").map(|l| (l.q, l.residual.len()));
        assert_eq!(l, Ok((vec!["pm10>50".to_owned()], 1)));
    }

    #[test]
    fn non_scalar_and_relationship_comparisons_are_residual() {
        assert!(residual("labels = 'x'"));
        assert!(residual("id <> 'urn:ngsi-ld:AirQualityObserved:1'"));
        assert!(residual("pm10 = pm25"));
        assert!(residual("pm10 + 1 > 5"));
    }

    #[test]
    fn temporal_functions_pushed_or_residual() {
        let t = "TIMESTAMP('2026-09-01T00:00:00Z')";
        let i = "INTERVAL('2026-09-01T00:00:00Z','2026-09-02T00:00:00Z')";
        // `q` reads text, so a time bound is pushed 48 h wider and the residual decides.
        for (filter, expected) in [
            (
                format!("T_AFTER(dateObserved, {t})"),
                "dateObserved>=2026-08-30T00:00:00Z",
            ),
            (
                format!("T_BEFORE(dateObserved, {t})"),
                "dateObserved<=2026-09-03T00:00:00Z",
            ),
            (
                format!("T_DURING(dateObserved, {i})"),
                "(dateObserved>=2026-08-30T00:00:00Z;dateObserved<=2026-09-04T00:00:00Z)",
            ),
            (
                format!("T_INTERSECTS(dateObserved, {i})"),
                "(dateObserved<=2026-09-04T00:00:00Z;dateObserved>=2026-08-30T00:00:00Z)",
            ),
            (
                "T_AFTER(dateObserved, DATE('2026-09-18'))".to_owned(),
                "dateObserved>=2026-09-16T00:00:00Z",
            ),
        ] {
            assert_eq!(q(&filter), widened(expected), "{filter}");
        }
        // Equality of instants written in two offsets is no text equality.
        assert!(residual(&format!("T_EQUALS(dateObserved, {t})")));
        assert!(residual(&format!("dateObserved <> {t}")));
        // A bound the widening would push past 9999 stays residual alone.
        assert!(residual("dateObserved < TIMESTAMP('9999-12-31T00:00:00Z')"));
        // An open end against a value is the value's own `v = v`: residual, unknown when missing.
        assert!(residual(
            "T_DURING(dateObserved, INTERVAL('..', '2026-09-02T00:00:00Z'))"
        ));
        for f in [
            "T_CONTAINS",
            "T_DISJOINT",
            "T_FINISHEDBY",
            "T_FINISHES",
            "T_MEETS",
            "T_METBY",
            "T_OVERLAPPEDBY",
            "T_OVERLAPS",
            "T_STARTEDBY",
            "T_STARTS",
        ] {
            let l = lowered(&format!("{f}(INTERVAL(dateObserved, dateObserved), {i})"));
            assert!(l.map_or(0, |l| l.residual.len() + l.q.len()) > 0, "{f}");
        }
        assert_eq!(
            lowered("T_AFTER(pm10, BBOX(0,0,1,1))").err().map(|p| p.0),
            Some(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn open_ends_sort_outside_every_value() {
        let open = (Bound::Before, Bound::After);
        let v = Expr::Path(Path::value("t"));
        let instant = (Bound::At(v.clone()), Bound::At(v.clone()));
        let known = |op| Expr::Cmp(op, Box::new(v.clone()), Box::new(v.clone()));
        assert_eq!(
            temporal(&Op::TIntersects, &instant, &open),
            Expr::And(vec![known(Cmp::Eq), known(Cmp::Eq)])
        );
        assert_eq!(temporal(&Op::TAfter, &instant, &open), known(Cmp::Ne));
        assert_eq!(
            temporal(&Op::TEquals, &open, &open),
            Expr::And(vec![Expr::Lit(Lit::Bool(true)), Expr::Lit(Lit::Bool(true))])
        );
        assert_eq!(
            temporal(&Op::TBefore, &open, &open),
            Expr::Lit(Lit::Bool(false))
        );
    }

    #[test]
    fn array_functions() {
        assert_eq!(
            q("A_OVERLAPS(tags, ('a', 'b'))"),
            pushed(r#"tags=="a","b""#)
        );
        assert_eq!(
            q("A_OVERLAPS(('a', 'c'), tags)"),
            pushed(r#"tags=="a","c""#)
        );
        // A plain multi-valued Property is no ListProperty: the residual reads it.
        assert!(residual("A_OVERLAPS(labels, ('a'))"));
        for f in ["A_EQUALS", "A_CONTAINS", "A_CONTAINEDBY"] {
            assert!(residual(&format!("{f}(tags, ('a', 'b'))")), "{f}");
            assert!(residual(&format!("{f}(tags, labels)")), "{f}");
        }
        // Two literal sets fold to a constant: TRUE filters nothing, FALSE everything.
        assert_eq!(
            lowered("A_CONTAINS(('a', 'b', 'c'), ('a', 'b'))").map(|l| l.residual),
            Ok(vec![])
        );
        assert_eq!(
            lowered("A_EQUALS(('a', 'b'), ('b', 'a', 'a'))").map(|l| l.residual),
            Ok(vec![])
        );
        assert_eq!(
            lowered("A_EQUALS(('a', 'b'), ('b', 'c'))").map(|l| l.residual),
            Ok(vec![Expr::Lit(Lit::Bool(false))])
        );
        assert_eq!(
            lowered("A_OVERLAPS(tags, (pm10, 'a'))").err(),
            Some(not_implemented(
                "an array element other than a string, number, boolean, date or timestamp literal"
            ))
        );
    }

    #[test]
    fn s_intersects_pushed_to_geoq() {
        let l = lowered("S_INTERSECTS(geometry, POINT(19.1 48.7))");
        assert_eq!(
            l.map(|l| l.geo),
            Ok(Some(GeoQ {
                rel: "intersects",
                geometry: json!({"type": "Point", "coordinates": [19.1, 48.7]})
            }))
        );
        let bbox = lowered("S_INTERSECTS(geometry, BBOX(170, -10, -170, 10))")
            .map(|l| l.geo.and_then(|g| g.geometry.get("type").cloned()));
        assert_eq!(bbox, Ok(Some(json!("MultiPolygon"))));
    }

    #[test]
    fn spatial_functions_pushed_or_residual() {
        let poly = "POLYGON((0 0, 1 0, 1 1, 0 0))";
        for (f, rel) in [
            ("S_WITHIN", "within"),
            ("S_CONTAINS", "contains"),
            ("S_EQUALS", "equals"),
            ("S_DISJOINT", "disjoint"),
            ("S_OVERLAPS", "overlaps"),
        ] {
            let l = lowered(&format!("{f}(geometry, {poly})")).map(|l| l.geo.map(|g| g.rel));
            assert_eq!(l, Ok(Some(rel)), "{f}");
        }
        // Mirrored: the literal first.
        let l = lowered(&format!("S_WITHIN({poly}, geometry)")).map(|l| l.geo.map(|g| g.rel));
        assert_eq!(l, Ok(Some("contains")));
        for f in ["S_TOUCHES", "S_CROSSES"] {
            assert!(residual(&format!("{f}(geometry, {poly})")), "{f}");
        }
        // One geoQ: the second spatial conjunct is residual, and so is any under OR.
        let two = lowered(&format!(
            "S_INTERSECTS(geometry, {poly}) AND S_WITHIN(geometry, {poly})"
        ))
        .map(|l| (l.geo.is_some(), l.residual.len()));
        assert_eq!(two, Ok((true, 1)));
        assert!(residual(&format!(
            "S_INTERSECTS(geometry, {poly}) OR pm10 > 1"
        )));
        assert!(residual(
            "S_INTERSECTS(geometry, GEOMETRYCOLLECTION(POINT(1 1)))"
        ));
        // Another geo query in the request keeps the filter's in the residual.
        let c = station();
        let terms = Terms::of(&c);
        let f = read(&c, &raw(&format!("S_WITHIN(geometry, {poly})")));
        let l = f.and_then(|f| lower(&c, &terms, &f.ok_or(bad("none"))?, false));
        assert_eq!(l.map(|l| (l.geo, l.residual.len())), Ok((None, 1)));
    }

    #[test]
    fn property_property_is_residual() {
        assert!(residual("pm10 > pm25"));
        assert!(residual("S_INTERSECTS(geometry, geometry)"));
        assert!(residual("'x' LIKE label"));
    }

    #[test]
    fn arithmetic_is_residual() {
        for op in ["+", "-", "*", "/", "%", "div", "^"] {
            assert!(residual(&format!("pm10 {op} 2 > 3")), "{op}");
        }
    }

    #[test]
    fn hidden_and_unknown_property_are_the_same_400() {
        for filter in [
            "secret = 'x'",
            "pm1O > 5",
            "geometry2 IS NULL",
            "x = 1 OR pm10 > 1",
        ] {
            assert_eq!(lowered(filter).err(), Some(bad(NOT_QUERYABLE)), "{filter}");
        }
    }

    #[test]
    fn functions_list_is_empty_and_unknown_is_400() {
        assert_eq!(
            lowered("Buffer(geometry, 10) IS NULL").err().map(|p| p.0),
            Some(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn languages_crs_and_parameters_are_read() {
        let c = station();
        let json = Raw {
            filter: Some(r#"{"op": ">", "args": [{"property": "pm10"}, 50]}"#.into()),
            lang: Some("cql2-json".into()),
            ..Raw::default()
        };
        assert!(read(&c, &json).is_ok_and(|f| f.is_some()));
        for bad_raw in [
            Raw {
                lang: Some("ecql".into()),
                ..raw("pm10 > 1")
            },
            Raw {
                crs: Some("EPSG:4326".into()),
                ..raw("pm10 > 1")
            },
            raw("pm10 >"),
            Raw {
                equals: vec![("pm10".into(), "high".into())],
                ..Raw::default()
            },
            Raw {
                equals: vec![("quiet".into(), "yes".into())],
                ..Raw::default()
            },
        ] {
            assert!(read(&c, &bad_raw).is_err(), "{bad_raw:?}");
        }
        let params = Raw {
            equals: vec![
                ("pm10".into(), "5".into()),
                ("quiet".into(), "false".into()),
                ("quiet".into(), "true".into()),
                ("label".into(), "x".into()),
            ],
            ..raw("pm25 < 1")
        };
        let terms = Terms::of(&c);
        let l = read(&c, &params).and_then(|f| lower(&c, &terms, &f.ok_or(bad("none"))?, true));
        assert_eq!(
            l.map(|l| l.q),
            Ok(vec![
                "pm25<1".into(),
                "pm10==5".into(),
                "quiet==false".into(),
                "quiet==true".into(),
                "label==\"x\"".into()
            ])
        );
        assert!(read(&c, &Raw::default()).is_ok_and(|f| f.is_none()));
        assert!(is_parameter(&c, "pm10"));
        assert!(is_parameter(&c, "id"));
        assert!(!is_parameter(&c, "tags"));
        assert!(!is_parameter(&c, "geometry"));
    }

    #[test]
    fn a_q_past_the_bound_leaves_the_rest_to_the_residual() {
        let long = "x".repeat(1500);
        let filter = format!("label = '{long}' AND pm10 = '{long}' AND pm25 = '{long}'");
        let l = lowered(&filter).map(|l| (l.q.len(), l.residual.len()));
        assert_eq!(l, Ok((2, 1)));
    }

    #[test]
    fn numbers_are_exact_or_residual_numbers() {
        assert_eq!(number(5.0), Ok(Lit::Int(5)));
        assert_eq!(number(-0.5), Ok(Lit::Num(-0.5)));
        assert_eq!(number(1e300), Ok(Lit::Num(1e300)));
        assert!(number(f64::INFINITY).is_err());
    }

    #[test]
    fn mirrored_and_every_comparison_pushed() {
        for (filter, expected) in [
            ("pm10 <= 5", "pm10<=5"),
            ("5 > pm10", "pm10<5"),
            ("5 >= pm10", "pm10<=5"),
            ("5 <= pm10", "pm10>=5"),
            ("5 = pm10", "pm10==5"),
        ] {
            assert_eq!(q(filter), pushed(expected), "{filter}");
        }
    }

    #[test]
    fn shapes_q_cannot_say_stay_residual() {
        for filter in [
            "CASEI(label) = CASEI(pm10)",
            "pm10 + 1 IN (1, 2)",
            "CASEI(label) LIKE CASEI(pm10)",
            "label = POINT(1 1)",
            "id IS NULL",
            "S_INTERSECTS(pm10, POINT(1 1))",
            "A_OVERLAPS(tags, labels)",
            "pm10 + 1 IS NULL",
        ] {
            assert!(residual(filter), "{filter}");
        }
        assert_eq!(
            lowered("S_INTERSECTS(POINT(19.1 48.7), geometry)").map(|l| l.geo.map(|g| g.rel)),
            Ok(Some("intersects"))
        );
        assert_eq!(
            lowered("A_OVERLAPS(('a', 'b'), ('b', 'c'))").map(|l| (l.q.len(), l.residual.len())),
            Ok((0, 0))
        );
        assert_eq!(
            lowered("DATE('2026-02-31') = pm10").err(),
            Some(bad(
                "a date or timestamp of the filter is no calendar instant"
            ))
        );
    }

    #[test]
    fn a_collection_without_geometry_has_no_geometry_queryable() {
        let mut c = station();
        c.geometry = None;
        assert_eq!(
            read(&c, &raw("geometry IS NULL")).err(),
            Some(bad(NOT_QUERYABLE))
        );
    }

    /// What `ogc-cql2` never hands over (it checks arity, lists and names) still answers 400.
    #[test]
    fn malformed_trees_are_refused_not_trusted() {
        let c = station();
        let terms = Terms::of(&c);
        let mut l = Lower {
            c: &c,
            terms: &terms,
        };
        let n = || Cql::Num(1.0);
        assert!(operands::<2>(&[n()]).is_err());
        assert_eq!(
            l.op(&Op::In, &[n(), n()]).err(),
            Some(bad("IN takes a list"))
        );
        assert_eq!(
            l.op(&Op::Function("buffer".into()), &[]).err(),
            Some(bad(NO_FUNCTION))
        );
        assert!(l.expr(&Cql::Array(vec![n()])).is_err());
        assert!(l.expr(&Cql::Unbounded).is_err());
        assert!(l.array(&n()).is_err());
        let point = Cql::Geometry(Geometry::Point(vec![1.0, 2.0]));
        assert!(l.array(&Cql::Array(vec![point])).is_err());
        assert!(l.period(&Cql::Str("x".into())).is_err());
        assert!(l.expr(&Cql::BBox(vec![1.0])).is_err());
        assert!(number(f64::NAN).is_err());
        assert!(l.path("nothing").is_err());
        // The empty list: false for a known value, unknown for a missing one.
        let empty = l.op(
            &Op::In,
            &[Cql::Property("pm10".into()), Cql::Array(Vec::new())],
        );
        let pm10 = l.path("pm10").map(|p| Box::new(Expr::Path(p)));
        assert_eq!(empty, pm10.map(|p| Expr::Cmp(Cmp::Ne, p.clone(), p)));
    }

    #[test]
    fn every_geometry_becomes_geojson() {
        let p = || vec![1.0, 2.0];
        for (g, kind) in [
            (Geometry::MultiPoint(vec![p()]), "MultiPoint"),
            (
                Geometry::MultiLineString(vec![vec![p(), p()]]),
                "MultiLineString",
            ),
            (
                Geometry::MultiPolygon(vec![vec![vec![p()]]]),
                "MultiPolygon",
            ),
            (
                Geometry::Collection(vec![Geometry::Point(p())]),
                "GeometryCollection",
            ),
        ] {
            assert_eq!(geojson(&g).get("type"), Some(&json!(kind)), "{kind}");
        }
    }

    #[test]
    fn an_open_end_against_another_reads_every_operator() {
        let v = Expr::Path(Path::Id);
        assert_eq!(
            at(Cmp::Ne, &Bound::Before, &Bound::After),
            Expr::Lit(Lit::Bool(true))
        );
        assert_eq!(
            at(Cmp::Ne, &Bound::At(v.clone()), &Bound::After),
            Expr::Cmp(Cmp::Eq, Box::new(v.clone()), Box::new(v))
        );
    }

    #[test]
    fn fold_keeps_three_valued_meaning() {
        let p = || Expr::Path(Path::Id);
        let t = || Expr::Lit(Lit::Bool(true));
        let f = || Expr::Lit(Lit::Bool(false));
        assert_eq!(fold(Expr::Or(vec![f(), p(), p()])), p());
        assert_eq!(fold(Expr::Or(vec![p(), t()])), t());
        assert_eq!(fold(Expr::And(vec![p(), f()])), f());
        assert_eq!(fold(Expr::Or(Vec::new())), f());
        assert_eq!(
            fold(Expr::Not(Box::new(Expr::And(vec![t(), p()])))),
            Expr::Not(Box::new(p()))
        );
    }
}
